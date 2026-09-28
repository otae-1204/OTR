use std::collections::HashMap;
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
/// 2: 事件身份是「时间戳 + 模型 + 会话」。Cursor 会在同一条事件上原地把
/// tokenUsage 改大,旧版把 token 数写进去重键,每次改写都当成新事件累加,
/// 长会话的本地总量就会远高于官网最终值。版本变更触发一次全量重建。
pub const PARSER_VERSION: u64 = 2;

const USAGE_URL: &str = "https://cursor.com/api/dashboard/get-filtered-usage-events";
const PAGE_SIZE: u32 = 500;
const MAX_PAGES: u32 = 60;
const THROTTLE_MS: i64 = 60_000;
/// 分页被 MAX_PAGES 截断后的重试节流:别每 60 秒就去翻 60 页
const TRUNCATED_THROTTLE_MS: i64 = 10 * 60_000;
/// 水位回退窗口。同一条用量事件的时间戳不变,token 数会随长会话变大;
/// 增量每次回拉这么久,已经计过的只补差额,也能接住晚到的修订。
/// 24 小时能盖住一次长会话结束前的最后一次改写。
const OVERLAP_MS: i64 = 24 * 60 * 60 * 1_000;
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
            Ok(pull) => {
                // 已入库的快照比这次读到的更大,补差会留下偏高的残值。
                // 丢掉本批,让编排层清掉该 Agent 再按官网当前值重建。
                if pull.revise_down && !ctx.full {
                    ctx.force_full = true;
                }
                *ctx.state = serde_json::to_value(&st).unwrap_or(Value::Null);
                Ok(pull.records)
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
    /// 重叠窗口里已经按「当前值」计过的事件。同一条事件 token 变长时只补差额。
    #[serde(default)]
    applied: Vec<AppliedUsage>,
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

/// 一条已经写入本地库的用量事件的当前值。
/// 库本身只做累加,所以后续修订必须记成差额,不能再写一遍绝对值。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppliedUsage {
    id: String,
    ts: i64,
    #[serde(default)]
    model: String,
    /// 第一次见到时的 kind。按天/会话表的主键含 provider,修订必须沿用它,
    /// 否则差额会写进另一行,原来的那行就停在中间快照上。
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    headless: bool,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    cost: f64,
}

#[derive(Debug, Clone)]
struct UsageEvent {
    /// 稳定身份:`{timestamp}|{model}|{conversationId}`。不含 token 数和金额。
    id: String,
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

struct UsagePull {
    records: Vec<UsageRecord>,
    /// 本次读到的 token 比已应用快照小。增量补不回这个差额。
    revise_down: bool,
}

fn token_sum(ev: &UsageEvent) -> u64 {
    ev.input
        .saturating_add(ev.output)
        .saturating_add(ev.cache_read)
        .saturating_add(ev.cache_write)
}

/// 同一次响应里如果出现同一身份的多条,只留 token 总量更大的那条。
/// 官网对一条请求只给最终值;这里防的是分页或重试把中间值又带回来。
fn collapse_events(events: &[UsageEvent]) -> Vec<UsageEvent> {
    let mut order = Vec::new();
    let mut best: HashMap<String, UsageEvent> = HashMap::new();
    for ev in events {
        match best.get(&ev.id) {
            None => {
                order.push(ev.id.clone());
                best.insert(ev.id.clone(), ev.clone());
            }
            Some(prev) => {
                let replace = token_sum(ev) > token_sum(prev)
                    || (token_sum(ev) == token_sum(prev) && ev.cost >= prev.cost);
                if replace {
                    best.insert(ev.id.clone(), ev.clone());
                }
            }
        }
    }
    order
        .into_iter()
        .map(|id| best.remove(&id).expect("id inserted above"))
        .collect()
}

/// 把本次拉到的事件折成「相对已应用快照的差额」。
///
/// - 没见过:整笔记入,算 1 次请求
/// - 见过且 token 变大(或金额变化):只记差额,请求数不再加
/// - 见过且某个 token 字段变小:增量语义补不出负数。`revert_on_down` 时
///   回滚本批对快照的修改并要求上层全量重建;全量扫描本身没有旧快照,忽略变小
fn reconcile_events(
    st: &mut CursorState,
    events: &[UsageEvent],
    window_cutoff: i64,
    revert_on_down: bool,
) -> UsagePull {
    let backup = st.applied.clone();
    let mut index: HashMap<String, usize> = HashMap::new();
    for (i, applied) in st.applied.iter().enumerate() {
        index.insert(applied.id.clone(), i);
    }

    let mut records = Vec::new();
    let mut revise_down = false;
    for ev in events {
        if window_cutoff > i64::MIN && ev.ts < window_cutoff {
            continue;
        }
        if let Some(&idx) = index.get(&ev.id) {
            let prev = st.applied[idx].clone();
            if ev.input < prev.input
                || ev.output < prev.output
                || ev.cache_read < prev.cache_read
                || ev.cache_write < prev.cache_write
            {
                revise_down = true;
                if revert_on_down {
                    break;
                }
                continue;
            }
            let d_in = ev.input - prev.input;
            let d_out = ev.output - prev.output;
            let d_cr = ev.cache_read - prev.cache_read;
            let d_cw = ev.cache_write - prev.cache_write;
            let d_cost = ev.cost - prev.cost;
            if d_in + d_out + d_cr + d_cw > 0 || d_cost.abs() > 1e-9 {
                records.push(usage_record(
                    &ev.id,
                    &prev.model,
                    prev.kind.clone(),
                    prev.headless,
                    ev.ts,
                    d_in,
                    d_out,
                    d_cr,
                    d_cw,
                    0,
                    d_cost,
                ));
            }
            let slot = &mut st.applied[idx];
            slot.input = ev.input;
            slot.output = ev.output;
            slot.cache_read = ev.cache_read;
            slot.cache_write = ev.cache_write;
            slot.cost = ev.cost;
        } else {
            records.push(usage_record(
                &ev.id,
                &ev.model,
                ev.kind.clone(),
                ev.headless,
                ev.ts,
                ev.input,
                ev.output,
                ev.cache_read,
                ev.cache_write,
                1,
                ev.cost,
            ));
            index.insert(ev.id.clone(), st.applied.len());
            st.applied.push(AppliedUsage {
                id: ev.id.clone(),
                ts: ev.ts,
                model: ev.model.clone(),
                kind: ev.kind.clone(),
                headless: ev.headless,
                input: ev.input,
                output: ev.output,
                cache_read: ev.cache_read,
                cache_write: ev.cache_write,
                cost: ev.cost,
            });
        }
    }

    if revise_down && revert_on_down {
        st.applied = backup;
        return UsagePull {
            records: Vec::new(),
            revise_down: true,
        };
    }
    UsagePull {
        records,
        revise_down: false,
    }
}

fn trim_applied(st: &mut CursorState, floor: i64) {
    st.applied.retain(|a| a.ts >= floor);
}

fn scan_usage(full: bool, st: &mut CursorState) -> Result<UsagePull> {
    let now = now_ms();
    if !full && st.last_fetch_ms > 0 {
        let throttle = if st.truncated {
            TRUNCATED_THROTTLE_MS
        } else {
            THROTTLE_MS
        };
        if now - st.last_fetch_ms < throttle {
            return Ok(UsagePull {
                records: vec![],
                revise_down: false,
            });
        }
    }

    let jwt = match read_access_token() {
        Some(t) if !t.is_empty() => t,
        _ => {
            st.last_fetch_ms = now;
            st.last_error = Some("未找到本机登录态,请打开 Cursor 并登录".into());
            st.last_error_ms = now;
            return Ok(UsagePull {
                records: vec![],
                revise_down: false,
            });
        }
    };

    let prev_ts = st.last_ts;
    let end_ms = now + 60_000;
    // 全量不带 startDate:带日期过滤时部分账号会只返回极少事件。
    // 增量在重叠窗口开启前回退 0(老状态第一次升级,避免把已入库的事件再加一遍);
    // 全量重建会清掉状态并重写行,成功后打开重叠窗口,之后的回拉只补差额。
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
            return Ok(UsagePull {
                records: vec![],
                revise_down: false,
            });
        }
    };
    st.last_fetch_ms = now;
    st.last_error = None;
    st.last_error_ms = 0;
    st.truncated = outcome.truncated;
    // 全量重建从空状态重写全部行,之后的增量必须回拉重叠窗口才能接住原地修订。
    // 老状态(overlap 仍为 false)的下一次增量仍严格从水位拉,避免升级当口双计。
    if full {
        st.overlap = true;
    }

    let pull = reconcile_events(
        st,
        &collapse_events(&outcome.events),
        window_cutoff,
        !full,
    );
    if pull.revise_down {
        return Ok(pull);
    }

    if outcome.truncated {
        // 撞到分页上限:本次拿到的事件照常入库(快照已记下,重试不会再加一遍),
        // 但**不推进水位**,下次仍从同一段接着补。
        st.last_error = Some(format!(
            "用量事件超过 {MAX_PAGES} 页上限(本次取到 {} 条),稍后自动重试补齐",
            outcome.events.len()
        ));
        st.last_error_ms = now;
        return Ok(pull);
    }

    if let Some(max_ts) = outcome.events.iter().map(|e| e.ts).max() {
        st.last_ts = st.last_ts.max(max_ts);
    }
    // 只留下一轮回拉还会再见到的快照。更老的不会再出现,留着只会让状态膨胀。
    if st.overlap {
        trim_applied(st, st.last_ts.saturating_sub(OVERLAP_MS));
    }
    Ok(pull)
}

fn usage_record(
    id: &str,
    model: &str,
    kind: Option<String>,
    headless: bool,
    ts: i64,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    calls: u64,
    cost: f64,
) -> UsageRecord {
    let title = if headless {
        Some(format!("后台 · {model}"))
    } else if model.is_empty() {
        Some("Cursor".into())
    } else {
        Some(model.to_string())
    };
    UsageRecord {
        agent: AGENT.into(),
        session_id: Some(id.to_string()),
        model: (!model.is_empty()).then(|| model.to_string()),
        provider: kind,
        title,
        ts,
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        cache_write_tokens: cache_write,
        calls,
        cost,
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
    // 身份不含 token / 金额:这两个字段会在同一条事件上被原地改写。
    let conversation = v
        .get("conversationId")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim();
    let id = format!("{ts}|{model}|{conversation}");
    Some(UsageEvent {
        id,
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

/// 用量事件的 session_id 是 `{timestamp}|{model}|{conversationId}`。
/// 第三段才是 Cursor 会话本身;前面两段只是为了把同一次请求拆开。
pub fn conversation_id(session_id: &str) -> Option<&str> {
    let mut parts = session_id.splitn(3, '|');
    let ts = parts.next()?;
    if ts.is_empty() || !ts.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let model = parts.next()?;
    if model.is_empty() {
        return None;
    }
    let conv = parts.next()?.trim();
    if conv.is_empty() { None } else { Some(conv) }
}

/// composerHeaders.value 里的 `name` 才是侧边栏上的会话名。
fn composer_display_name(value: &str) -> Option<String> {
    let v: Value = serde_json::from_str(value).ok()?;
    let name = v.get("name")?.as_str()?.trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// conversationId → 会话名。读不到(库被锁、没有 Cursor)时返回空表,调用方保持原标题。
pub fn conversation_names() -> HashMap<String, String> {
    let db = paths::cursor_state_db();
    if !db.is_file() {
        return HashMap::new();
    }
    let Some(conn) = open_ro(&db) else {
        return HashMap::new();
    };
    let mut stmt = match conn.prepare("SELECT composerId, value FROM composerHeaders") {
        Ok(stmt) => stmt,
        Err(e) => {
            eprintln!("[cursor] composerHeaders: {e}");
            return HashMap::new();
        }
    };
    let rows = match stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    }) {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!("[cursor] composerHeaders: {e}");
            return HashMap::new();
        }
    };
    let mut out = HashMap::new();
    for row in rows.flatten() {
        let (id, value) = row;
        if let Some(name) = composer_display_name(&value) {
            out.insert(id, name);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_id_is_the_third_segment() {
        assert_eq!(
            conversation_id("1790589579808|grok-4.7-xhigh|f3b23c8c-2688-43e9-9054-117d0b624c16"),
            Some("f3b23c8c-2688-43e9-9054-117d0b624c16")
        );
        assert_eq!(conversation_id("session-abc"), None);
        assert_eq!(conversation_id("123|model|"), None);
    }

    #[test]
    fn composer_display_name_reads_name_not_id() {
        let raw = r#"{"composerId":"abc","name":"Data update flicker issue","subtitle":"Edited a file"}"#;
        assert_eq!(
            composer_display_name(raw).as_deref(),
            Some("Data update flicker issue")
        );
        assert_eq!(composer_display_name(r#"{"name":"  "}"#), None);
        assert_eq!(composer_display_name(r#"{"composerId":"only-an-id"}"#), None);
    }

    fn sample_body() -> Value {
        serde_json::json!({
            "totalUsageEventsCount": 2,
            "usageEventsDisplay": [
                {
                    "timestamp": "1756700000000",
                    "model": "composer-2",
                    "conversationId": "conv-a",
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
        assert_eq!(events[0].id, "1756700000000|composer-2|conv-a");
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
        let rec = usage_record(
            &events[1].id,
            &events[1].model,
            events[1].kind.clone(),
            events[1].headless,
            events[1].ts,
            events[1].input,
            events[1].output,
            events[1].cache_read,
            events[1].cache_write,
            1,
            events[1].cost,
        );
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
            id: format!("{ts}|{model}|conv"),
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

    /// 晚到的事件(ts 比水位早、仍在重叠窗口内)必须能补进来。
    /// 同一条再出现时不能按绝对值再加一遍。
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

        let records = reconcile_events(&mut st, std::slice::from_ref(&late), cutoff, true);
        assert_eq!(records.records.len(), 1, "晚到的事件不该被丢掉");
        assert_eq!(records.records[0].input_tokens, 100);
        assert_eq!(records.records[0].calls, 1);

        let again = reconcile_events(&mut st, std::slice::from_ref(&late), cutoff, true);
        assert!(again.records.is_empty(), "同一个事件不能按绝对值再计一遍");
        assert!(!again.revise_down);

        let ancient = ev(cutoff - 1, "composer-2", 999);
        assert!(reconcile_events(&mut st, &[ancient], cutoff, true)
            .records
            .is_empty());
    }

    /// 同一条事件 token 变长时,累加差额必须等于最终值,请求数只算 1 次。
    /// 这是 Opus 5.5 本地远高于官网的原因:旧键把 token 数算进去,18 个中间快照被加总。
    #[test]
    fn in_place_revision_sums_to_the_final_snapshot() {
        let mut st = CursorState::default();
        let mut cache = 0u64;
        let mut output = 0u64;
        let mut calls = 0u64;
        for step in 1..=18 {
            let mut event = ev(1_790_412_335_535, "claude-opus-5-5-high", 4);
            event.cache_read = step * 1_000;
            event.output = step * 10;
            event.cost = step as f64;
            let pull = reconcile_events(&mut st, &[event], i64::MIN, true);
            assert!(!pull.revise_down);
            for record in &pull.records {
                cache += record.cache_read_tokens;
                output += record.output_tokens;
                calls += record.calls;
            }
        }
        assert_eq!(cache, 18_000);
        assert_eq!(output, 180);
        assert_eq!(calls, 1);
        assert_eq!(st.applied.len(), 1);
        assert_eq!(st.applied[0].cache_read, 18_000);
    }

    /// 修订必须沿用第一次的 kind,差额才能加回同一行。
    #[test]
    fn revision_keeps_the_original_kind() {
        let mut st = CursorState::default();
        let mut event = ev(50, "claude-opus-5-5-high", 10);
        event.kind = Some("usage_based".into());
        let first = reconcile_events(&mut st, &[event.clone()], i64::MIN, true);
        assert_eq!(first.records[0].provider.as_deref(), Some("usage_based"));

        event.kind = Some("included".into());
        event.input = 25;
        let second = reconcile_events(&mut st, &[event], i64::MIN, true);
        assert_eq!(second.records.len(), 1);
        assert_eq!(second.records[0].input_tokens, 15);
        assert_eq!(second.records[0].calls, 0);
        assert_eq!(second.records[0].provider.as_deref(), Some("usage_based"));
    }

    /// 同一毫秒、同一模型、不同会话是两条用量,不能并成一条。
    #[test]
    fn same_timestamp_different_conversations_both_count() {
        let mut st = CursorState::default();
        let mut a = ev(5, "m", 10);
        a.id = "5|m|a".into();
        let mut b = ev(5, "m", 7);
        b.id = "5|m|b".into();
        let pull = reconcile_events(&mut st, &[a, b], i64::MIN, true);
        assert_eq!(pull.records.len(), 2);
        let input: u64 = pull.records.iter().map(|r| r.input_tokens).sum();
        assert_eq!(input, 17);
    }

    /// token 变小无法用累加补回来,必须整批作废并要求重建,快照保持重建前的值。
    #[test]
    fn downward_revision_reverts_and_asks_for_rebuild() {
        let mut st = CursorState::default();
        let event = ev(5, "m", 100);
        reconcile_events(&mut st, &[event.clone()], i64::MIN, true);
        let mut smaller = event;
        smaller.input = 40;
        let pull = reconcile_events(&mut st, &[smaller], i64::MIN, true);
        assert!(pull.revise_down);
        assert!(pull.records.is_empty());
        assert_eq!(st.applied[0].input, 100);
    }

    /// 回拉窗口之外的快照丢掉;窗口内的全部保留,不能为了封顶把还会再见到的事件裁掉。
    #[test]
    fn trim_drops_only_snapshots_outside_the_overlap() {
        let base = 1_756_700_000_000i64;
        let mut st = CursorState::default();
        let old = ev(base, "m", 1);
        let recent = ev(base + OVERLAP_MS, "m", 2);
        reconcile_events(&mut st, &[old, recent], i64::MIN, true);
        assert_eq!(st.applied.len(), 2);
        trim_applied(&mut st, base + OVERLAP_MS);
        assert_eq!(st.applied.len(), 1);
        assert_eq!(st.applied[0].ts, base + OVERLAP_MS);
    }

    /// 老状态没有 overlap 标记:反序列化后仍是关闭,避免升级当口按新窗口重算历史。
    #[test]
    fn legacy_state_does_not_rewind_the_watermark() {
        let st: CursorState = serde_json::from_value(serde_json::json!({
            "lastTs": 1_756_700_000_000i64,
            "lastTsKeys": ["x"],
            "recentKeys": ["1|m|1|0|0|0|0.0000"],
            "lastFetchMs": 1_756_700_100_000i64
        }))
        .unwrap();
        assert!(!st.overlap, "老状态默认不开重叠窗口");
        assert!(st.applied.is_empty());
        assert_eq!(st.last_ts, 1_756_700_000_000);
    }
}
