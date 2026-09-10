use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{AppError, Result};
use crate::model::{now_ms, UsageRecord};
use crate::paths;
use crate::providers::{AgentProvider, ScanCtx};

pub struct CursorProvider;

const AGENT: &str = "cursor";
pub const PARSER_VERSION: u64 = 1;

const USAGE_URL: &str = "https://cursor.com/api/dashboard/get-filtered-usage-events";
const PAGE_SIZE: u32 = 500;
const MAX_PAGES: u32 = 60;
const THROTTLE_MS: i64 = 60_000;
/// 分页被 MAX_PAGES 截断后的重试节流:别每 60 秒就去翻 60 页
const TRUNCATED_THROTTLE_MS: i64 = 10 * 60_000;
/// 水位回退窗口。以前只用 ts 比水位、更小的直接丢,晚到的事件会永久丢失;
/// 现在每次都回退这么久重新拉,靠 key 去重保证不重复计入。
const OVERLAP_MS: i64 = 10 * 60_000;
/// 去重窗口里保留的最大 key 数(key 以 ts 开头,超出按时间裁掉最老的)
const RECENT_KEYS_MAX: usize = 4096;
const HTTP_TIMEOUT_SECS: u64 = 20;

impl AgentProvider for CursorProvider {
    fn id(&self) -> &str {
        AGENT
    }

    fn display_name(&self) -> &str {
        "Cursor"
    }

    fn detect(&self) -> bool {
        paths::cursor_state_db().is_file()
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        let db = paths::cursor_state_db();
        vec![db.clone(), PathBuf::from(format!("{}-wal", db.display()))]
    }

    fn parser_version(&self) -> u64 {
        PARSER_VERSION
    }

    /// dashboard 的 tokenUsage.totalCents 是美元
    fn native_cost_currency(&self) -> Option<&'static str> {
        Some("USD")
    }

    /// 让 UI 能看到"登录态失效 / 分页被截断"这类问题,
    /// 而不是只在 stderr 里打一行、用户完全不知情
    fn health(&self, state: &Value) -> Option<String> {
        let st: CursorState = serde_json::from_value(state.clone()).ok()?;
        if st.last_error_ms <= 0 {
            return None;
        }
        st.last_error
    }

    fn scan(&self, ctx: &mut ScanCtx) -> Result<Vec<UsageRecord>> {
        let prev = ctx.state.clone();
        let mut st: CursorState =
            serde_json::from_value(std::mem::take(ctx.state)).unwrap_or_default();
        match scan_usage(ctx.full, &mut st) {
            Ok(records) => {
                *ctx.state = serde_json::to_value(&st).unwrap_or(Value::Null);
                Ok(records)
            }
            Err(e) => {
                *ctx.state = prev;
                Err(e)
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct CursorState {
    /// 水位:已计入的最大事件时间戳
    #[serde(default)]
    last_ts: i64,
    /// 水位窗口内已经计过的 key(形如 "{ts}|{model}|{input}|..."),用于重叠窗口去重
    #[serde(default)]
    recent_keys: Vec<String>,
    #[serde(default)]
    last_fetch_ms: i64,
    /// 是否已经启用重叠窗口。老状态里没有这个字段,第一次升级上来必须退化成
    /// "严格按水位拉",否则会把水位之前已经计过的事件重新算一遍。
    #[serde(default)]
    overlap: bool,
    /// 上一次分页是否撞到 MAX_PAGES 上限(截断时不推进水位)
    #[serde(default)]
    truncated: bool,
    /// 最近一次异常原因 + 时间,给 AgentCard 显示健康状态
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    last_error_ms: i64,
}

#[derive(Debug, Clone)]
struct UsageEvent {
    key: String,
    ts: i64,
    model: String,
    kind: Option<String>,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    cost: f64,
    headless: bool,
}

/// dedup key 形如 "{ts}|{model}|{input}|...",取开头的 ts
fn key_ts(key: &str) -> Option<i64> {
    key.split('|').next()?.parse::<i64>().ok()
}

/// 从拉到的事件里挑出真正的新事件,并维护去重窗口。
/// 判定标准只有"ts 是否落在窗口内"和"key 是否见过",不再要求 ts >= 水位 ——
/// 晚到的事件(服务端补数据、时钟回拨)因此不会再被永久丢掉。
fn select_new_events(
    st: &mut CursorState,
    events: &[UsageEvent],
    window_cutoff: i64,
) -> Vec<UsageRecord> {
    // 老 key 先按时间裁一遍,窗口之外的不可能再出现
    if window_cutoff > i64::MIN {
        st.recent_keys
            .retain(|k| key_ts(k).map_or(true, |ts| ts >= window_cutoff));
    }
    let known: HashSet<String> = st.recent_keys.iter().cloned().collect();

    let mut records = Vec::new();
    let mut fresh = Vec::new();
    for ev in events {
        if ev.ts < window_cutoff || known.contains(ev.key.as_str()) {
            continue;
        }
        records.push(event_record(ev));
        fresh.push(ev.key.clone());
    }
    st.recent_keys.extend(fresh);
    if st.recent_keys.len() > RECENT_KEYS_MAX {
        st.recent_keys.sort_by_key(|k| key_ts(k).unwrap_or(0));
        let drop = st.recent_keys.len() - RECENT_KEYS_MAX;
        st.recent_keys.drain(..drop);
    }
    records
}

fn scan_usage(full: bool, st: &mut CursorState) -> Result<Vec<UsageRecord>> {
    let now = now_ms();
    if !full && st.last_fetch_ms > 0 {
        let throttle = if st.truncated {
            TRUNCATED_THROTTLE_MS
        } else {
            THROTTLE_MS
        };
        if now - st.last_fetch_ms < throttle {
            return Ok(vec![]);
        }
    }

    let jwt = match read_access_token() {
        Some(t) if !t.is_empty() => t,
        _ => {
            st.last_fetch_ms = now;
            st.last_error = Some("未找到本机登录态,请打开 Cursor 并登录".into());
            st.last_error_ms = now;
            return Ok(vec![]);
        }
    };

    let prev_ts = st.last_ts;
    let end_ms = now + 60_000;
    // 全量不带 startDate:带日期过滤时部分账号会只返回极少事件。
    // 增量则在老状态(没有 recent_keys)下严格从水位拉,之后回退一个重叠窗口。
    let start_ms = if full || prev_ts <= 0 {
        None
    } else if st.overlap {
        Some(prev_ts.saturating_sub(OVERLAP_MS))
    } else {
        Some(prev_ts)
    };
    let window_cutoff = start_ms.unwrap_or(i64::MIN);

    let outcome = match fetch_events(&jwt, start_ms, end_ms) {
        Ok(o) => o,
        Err(e) => {
            // 失败也要记节流 + 健康状态:以前 Err 会让 scan_provider 回滚 state,
            // last_fetch_ms 不更新 → 每个 watcher tick 都拿失效 token 再打一次 cursor.com,
            // 用户还看不到任何提示。
            st.last_fetch_ms = now;
            st.last_error_ms = now;
            let text = e.to_string();
            st.last_error = Some(if text.contains("not_authenticated") {
                "Cursor 登录态已失效,请在 Cursor 里重新登录".into()
            } else {
                format!("拉取用量失败:{text}")
            });
            return Ok(vec![]);
        }
    };
    st.last_fetch_ms = now;
    st.last_error = None;
    st.last_error_ms = 0;
    st.truncated = outcome.truncated;
    st.overlap = !full || st.overlap;

    let records = select_new_events(st, &outcome.events, window_cutoff);

    if outcome.truncated {
        // 撞到分页上限:本次拿到的事件照常入库(已记进去重窗口),但**不推进水位**,
        // 下次会重新拉同一段并靠 key 去重跳过已计部分,把后面的页补回来。
        st.last_error = Some(format!(
            "用量事件超过 {MAX_PAGES} 页上限(本次取到 {} 条),稍后自动重试补齐",
            outcome.events.len()
        ));
        st.last_error_ms = now;
        return Ok(records);
    }

    if let Some(max_ts) = outcome.events.iter().map(|e| e.ts).max() {
        st.last_ts = st.last_ts.max(max_ts);
    }
    Ok(records)
}

fn event_record(ev: &UsageEvent) -> UsageRecord {
    let title = if ev.headless {
        Some(format!("后台 · {}", ev.model))
    } else if ev.model.is_empty() {
        Some("Cursor".into())
    } else {
        Some(ev.model.clone())
    };
    UsageRecord {
        agent: AGENT.into(),
        session_id: Some(ev.key.clone()),
        model: (!ev.model.is_empty()).then(|| ev.model.clone()),
        provider: ev.kind.clone(),
        title,
        ts: ev.ts,
        input_tokens: ev.input,
        output_tokens: ev.output,
        cache_read_tokens: ev.cache_read,
        cache_write_tokens: ev.cache_write,
        calls: 1,
        cost: ev.cost,
        ..Default::default()
    }
}

struct FetchOutcome {
    events: Vec<UsageEvent>,
    /// 循环跑满 MAX_PAGES 仍未取完:静默截断会让数据无声丢失,必须标出来
    truncated: bool,
}

fn fetch_events(jwt: &str, start_ms: Option<i64>, end_ms: i64) -> Result<FetchOutcome> {
    let cookies = cookie_candidates(jwt);
    if cookies.is_empty() {
        return Err(AppError::Msg("登录态无法解析".into()));
    }

    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build();

    let mut last_err: Option<String> = None;
    for cookie in &cookies {
        match fetch_with_cookie(&agent, cookie, start_ms, end_ms) {
            Ok(outcome) => return Ok(outcome),
            Err(e) => last_err = Some(e),
        }
    }
    Err(AppError::Msg(format!(
        "dashboard API: {}",
        last_err.unwrap_or_else(|| "unknown".into())
    )))
}

fn fetch_with_cookie(
    agent: &ureq::Agent,
    cookie: &str,
    start_ms: Option<i64>,
    end_ms: i64,
) -> std::result::Result<FetchOutcome, String> {
    let mut all = Vec::new();
    let mut total: Option<u64> = None;
    let mut truncated = false;
    for page in 1..=MAX_PAGES {
        let mut body = serde_json::json!({
            "endDate": end_ms.to_string(),
            "page": page,
            "pageSize": PAGE_SIZE,
        });
        if let Some(start) = start_ms {
            body["startDate"] = serde_json::json!(start.to_string());
        }
        let resp = agent
            .post(USAGE_URL)
            .set("Cookie", &format!("WorkosCursorSessionToken={cookie}"))
            .set("Origin", "https://cursor.com")
            .set("Referer", "https://cursor.com/dashboard/usage")
            .set("Content-Type", "application/json")
            .send_json(&body)
            .map_err(|e| http_err(&e))?;
        let status = resp.status();
        if status == 401 || status == 403 {
            return Err("not_authenticated".into());
        }
        if status >= 400 {
            return Err(format!("http {status}"));
        }
        let value: Value = resp.into_json().map_err(|e| e.to_string())?;
        if let Some(n) = value.get("totalUsageEventsCount").and_then(json_u64_opt) {
            total = Some(n);
        }
        let page_events = parse_events(&value);
        let page_len = page_events.len();
        all.extend(page_events);
        if page_len < PAGE_SIZE as usize {
            break;
        }
        if let Some(n) = total {
            if all.len() as u64 >= n {
                break;
            }
        }
        // 最后一个允许的页仍然取满 → 后面还有数据没拿到
        if page == MAX_PAGES {
            truncated = true;
        }
    }
    Ok(FetchOutcome {
        events: all,
        truncated,
    })
}

fn http_err(err: &ureq::Error) -> String {
    match err {
        ureq::Error::Status(code, _) => {
            if *code == 401 || *code == 403 {
                "not_authenticated".into()
            } else {
                format!("http {code}")
            }
        }
        ureq::Error::Transport(t) => format!("transport: {t}"),
    }
}

fn parse_events(root: &Value) -> Vec<UsageEvent> {
    let Some(arr) = root
        .get("usageEventsDisplay")
        .or_else(|| root.get("usageEvents"))
        .and_then(|v| v.as_array())
    else {
        return Vec::new();
    };
    arr.iter().filter_map(parse_event).collect()
}

fn parse_event(v: &Value) -> Option<UsageEvent> {
    let ts = json_ts(v.get("timestamp")?);
    if ts <= 0 {
        return None;
    }
    let usage = v.get("tokenUsage").cloned().unwrap_or(Value::Null);
    let input = json_u64(&usage, "inputTokens");
    let output = json_u64(&usage, "outputTokens");
    let cache_read = json_u64(&usage, "cacheReadTokens");
    let cache_write = json_u64(&usage, "cacheWriteTokens");
    let cents = usage
        .get("totalCents")
        .and_then(json_f64)
        .or_else(|| v.get("chargedCents").and_then(json_f64))
        .unwrap_or(0.0);
    let cost = if cents.abs() > f64::EPSILON {
        cents / 100.0
    } else {
        0.0
    };
    if input + output + cache_read + cache_write == 0 && cost.abs() < f64::EPSILON {
        // 仍计入请求次数(套餐内 0 成本调用)
        if v.get("model")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .is_empty()
        {
            return None;
        }
    }
    let model = v
        .get("model")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let kind = v.get("kind").and_then(|x| x.as_str()).map(short_kind);
    let key = format!(
        "{ts}|{model}|{input}|{output}|{cache_read}|{cache_write}|{:.4}",
        cost
    );
    Some(UsageEvent {
        key,
        ts,
        model,
        kind,
        input,
        output,
        cache_read,
        cache_write,
        cost,
        headless: v
            .get("isHeadless")
            .and_then(|x| x.as_bool())
            .unwrap_or(false),
    })
}

fn short_kind(kind: &str) -> String {
    kind.strip_prefix("USAGE_EVENT_KIND_")
        .unwrap_or(kind)
        .to_ascii_lowercase()
}

fn cookie_candidates(jwt: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(sub) = jwt_sub(jwt) {
        let stripped = sub.rsplit('|').next().unwrap_or(sub.as_str());
        if stripped != sub {
            out.push(format!("{stripped}%3A%3A{jwt}"));
        }
        out.push(format!("{sub}%3A%3A{jwt}"));
    }
    out
}

fn jwt_sub(token: &str) -> Option<String> {
    let payload = token.split('.').nth(1)?;
    let bytes = b64url_decode(payload)?;
    let v: Value = serde_json::from_slice(&bytes).ok()?;
    v.get("sub")?.as_str().map(ToOwned::to_owned)
}

fn b64url_decode(input: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let mut s = input.replace('-', "+").replace('_', "/");
    while s.len() % 4 != 0 {
        s.push('=');
    }
    base64::engine::general_purpose::STANDARD.decode(s).ok()
}

fn read_access_token() -> Option<String> {
    let db = paths::cursor_state_db();
    if !db.is_file() {
        return None;
    }
    let conn = open_ro(&db)?;
    let raw: String = conn
        .query_row(
            "SELECT value FROM ItemTable WHERE key = 'cursorAuth/accessToken'",
            [],
            |row| row.get(0),
        )
        .ok()?;
    Some(unquote_json_string(&raw))
}

fn open_ro(path: &std::path::Path) -> Option<Connection> {
    match Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(c) => Some(c),
        Err(_) => {
            let uri = format!(
                "file:{}?mode=ro&immutable=1",
                path.to_string_lossy().replace('\\', "/")
            );
            Connection::open_with_flags(
                &uri,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
            )
            .ok()
        }
    }
}

fn unquote_json_string(raw: &str) -> String {
    serde_json::from_str::<String>(raw).unwrap_or_else(|_| raw.trim_matches('"').to_string())
}

fn json_u64(v: &Value, key: &str) -> u64 {
    v.get(key).and_then(json_u64_opt).unwrap_or(0)
}

fn json_u64_opt(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_i64().map(|n| n.max(0) as u64))
        .or_else(|| v.as_f64().map(|n| n.max(0.0).round() as u64))
        .or_else(|| {
            v.as_str()?
                .parse::<f64>()
                .ok()
                .map(|n| n.max(0.0).round() as u64)
        })
}

fn json_f64(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| v.as_i64().map(|n| n as f64))
        .or_else(|| v.as_u64().map(|n| n as f64))
        .or_else(|| v.as_str()?.parse().ok())
}

fn json_ts(v: &Value) -> i64 {
    v.as_i64()
        .or_else(|| v.as_u64().map(|n| n as i64))
        .or_else(|| v.as_f64().map(|n| n as i64))
        .or_else(|| v.as_str()?.parse::<f64>().ok().map(|n| n as i64))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_body() -> Value {
        serde_json::json!({
            "totalUsageEventsCount": 2,
            "usageEventsDisplay": [
                {
                    "timestamp": "1756700000000",
                    "model": "composer-2",
                    "kind": "USAGE_EVENT_KIND_INCLUDED_IN_ULTRA",
                    "isHeadless": false,
                    "tokenUsage": {
                        "inputTokens": 100,
                        "outputTokens": 20,
                        "cacheReadTokens": 50,
                        "cacheWriteTokens": 10,
                        "totalCents": 12.5
                    },
                    "chargedCents": 12.5
                },
                {
                    "timestamp": 1756700001000_i64,
                    "model": "claude-4.6-sonnet",
                    "kind": "USAGE_EVENT_KIND_USAGE_BASED",
                    "isHeadless": true,
                    "tokenUsage": {
                        "inputTokens": "3",
                        "outputTokens": 200,
                        "cacheWriteTokens": 8
                    },
                    "chargedCents": 0
                }
            ]
        })
    }

    #[test]
    fn parse_dashboard_events() {
        let events = parse_events(&sample_body());
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].model, "composer-2");
        assert_eq!(events[0].input, 100);
        assert_eq!(events[0].cache_read, 50);
        assert_eq!(events[0].cache_write, 10);
        assert!((events[0].cost - 0.125).abs() < 1e-9);
        assert_eq!(events[0].kind.as_deref(), Some("included_in_ultra"));
        assert!(!events[0].headless);

        assert_eq!(events[1].input, 3);
        assert_eq!(events[1].output, 200);
        assert_eq!(events[1].cache_write, 8);
        assert!(events[1].headless);
        assert_eq!(events[1].kind.as_deref(), Some("usage_based"));
        let rec = event_record(&events[1]);
        assert_eq!(rec.title.as_deref(), Some("后台 · claude-4.6-sonnet"));
        assert_eq!(rec.calls, 1);
        assert_eq!(rec.agent, "cursor");
    }

    #[test]
    fn jwt_sub_strips_provider_prefix_via_cookie_order() {
        use base64::Engine;
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"sub":"github|user_01ABC"}"#);
        let jwt = format!("aaa.{payload}.sig");
        assert_eq!(jwt_sub(&jwt).as_deref(), Some("github|user_01ABC"));
        let cookies = cookie_candidates(&jwt);
        assert!(cookies[0].starts_with("user_01ABC%3A%3A"));
        assert!(cookies[1].starts_with("github|user_01ABC%3A%3A"));
    }

    fn ev(ts: i64, model: &str, input: u64) -> UsageEvent {
        UsageEvent {
            key: format!("{ts}|{model}|{input}|0|0|0|0.0000"),
            ts,
            model: model.into(),
            kind: None,
            input,
            output: 0,
            cache_read: 0,
            cache_write: 0,
            cost: 0.0,
            headless: false,
        }
    }

    /// 晚到的事件(ts 比水位早)必须能补进来 —— 旧口径是 ev.ts < prev_ts 直接丢,
    /// 服务端补数据 / 时钟回拨造成的事件会永久丢失。
    #[test]
    fn late_arriving_event_is_not_dropped() {
        let watermark = 1_756_700_000_000i64;
        let mut st = CursorState {
            last_ts: watermark,
            overlap: true,
            ..Default::default()
        };
        let cutoff = watermark - OVERLAP_MS;
        let late = ev(watermark - 60_000, "composer-2", 100);

        let records = select_new_events(&mut st, std::slice::from_ref(&late), cutoff);
        assert_eq!(records.len(), 1, "晚到的事件不该被丢掉");

        // 重叠窗口下一次会重复返回同一条 → key 去重后不能再计一遍
        let again = select_new_events(&mut st, std::slice::from_ref(&late), cutoff);
        assert!(again.is_empty(), "同一个 key 不能重复计入");

        // 真正在窗口之外的仍然要丢掉(否则每轮都会把历史重算一遍)
        let ancient = ev(cutoff - 1, "composer-2", 999);
        assert!(select_new_events(&mut st, &[ancient], cutoff).is_empty());
    }

    /// 去重窗口必须有界,且裁剪掉的是最老的 key
    #[test]
    fn dedup_window_is_bounded_and_keeps_the_newest() {
        let base = 1_756_700_000_000i64;
        let events: Vec<UsageEvent> = (0..RECENT_KEYS_MAX + 50)
            .map(|i| ev(base + i as i64 * 1000, "m", i as u64 + 1))
            .collect();
        let mut st = CursorState::default();
        let records = select_new_events(&mut st, &events, i64::MIN);
        assert_eq!(records.len(), events.len());
        assert_eq!(st.recent_keys.len(), RECENT_KEYS_MAX, "去重窗口必须封顶");
        let newest = base + (events.len() as i64 - 1) * 1000;
        assert!(
            st.recent_keys.iter().any(|k| key_ts(k) == Some(newest)),
            "最新的 key 必须还在"
        );
        assert!(
            !st.recent_keys.iter().any(|k| key_ts(k) == Some(base)),
            "最老的 key 应该被裁掉"
        );
    }

    /// 老状态没有 overlap 标记:第一次升级上来必须严格按水位拉,不能重算历史
    #[test]
    fn legacy_state_does_not_rewind_the_watermark() {
        let st: CursorState = serde_json::from_value(serde_json::json!({
            "lastTs": 1_756_700_000_000i64,
            "lastTsKeys": ["x"],
            "lastFetchMs": 1_756_700_100_000i64
        }))
        .unwrap();
        assert!(!st.overlap, "老状态默认不开重叠窗口");
        assert!(st.recent_keys.is_empty());
        assert_eq!(st.last_ts, 1_756_700_000_000);
    }
}
