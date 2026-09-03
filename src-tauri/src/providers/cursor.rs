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
    #[serde(default)]
    last_ts: i64,
    #[serde(default)]
    last_ts_keys: Vec<String>,
    #[serde(default)]
    last_fetch_ms: i64,
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

fn scan_usage(full: bool, st: &mut CursorState) -> Result<Vec<UsageRecord>> {
    let now = now_ms();
    if !full && st.last_fetch_ms > 0 && now - st.last_fetch_ms < THROTTLE_MS {
        return Ok(vec![]);
    }

    let jwt = match read_access_token() {
        Some(t) if !t.is_empty() => t,
        _ => {
            eprintln!("[{AGENT}] 未找到本机登录态,打开 Cursor 并登录后再刷新");
            return Ok(vec![]);
        }
    };

    let end_ms = now + 60_000;
    // 全量不带 startDate:带日期过滤时部分账号会只返回极少事件
    let start_ms = if full || st.last_ts <= 0 {
        None
    } else {
        Some(st.last_ts)
    };

    let events = fetch_events(&jwt, start_ms, end_ms)?;
    st.last_fetch_ms = now;
    if events.is_empty() {
        return Ok(vec![]);
    }

    let prev_ts = st.last_ts;
    let prev_keys: HashSet<&str> = st.last_ts_keys.iter().map(String::as_str).collect();
    let mut records = Vec::new();
    for ev in &events {
        let seen = ev.ts < prev_ts || (ev.ts == prev_ts && prev_keys.contains(ev.key.as_str()));
        if seen {
            continue;
        }
        records.push(event_record(ev));
    }

    if let Some(max_ts) = events.iter().map(|e| e.ts).max() {
        st.last_ts = st.last_ts.max(max_ts);
        st.last_ts_keys = events
            .iter()
            .filter(|e| e.ts == st.last_ts)
            .map(|e| e.key.clone())
            .collect();
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

fn fetch_events(jwt: &str, start_ms: Option<i64>, end_ms: i64) -> Result<Vec<UsageEvent>> {
    let cookies = cookie_candidates(jwt);
    if cookies.is_empty() {
        eprintln!("[{AGENT}] 登录态无法解析");
        return Ok(vec![]);
    }

    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
        .build();

    let mut last_err: Option<String> = None;
    for cookie in &cookies {
        match fetch_with_cookie(&agent, cookie, start_ms, end_ms) {
            Ok(events) => return Ok(events),
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
) -> std::result::Result<Vec<UsageEvent>, String> {
    let mut all = Vec::new();
    let mut total: Option<u64> = None;
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
        if page == 1 {
            eprintln!("[{AGENT}] page1 events={page_len} api_total={total:?}");
        }
        if page_len < PAGE_SIZE as usize {
            break;
        }
        if let Some(n) = total {
            if all.len() as u64 >= n {
                break;
            }
        }
    }
    Ok(all)
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
    use std::collections::HashSet;

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

    #[test]
    fn watermark_skips_already_ingested_event() {
        let events = parse_events(&sample_body());
        let prev_ts = events[0].ts;
        let prev_keys: HashSet<&str> = [events[0].key.as_str()].into_iter().collect();
        let new: Vec<_> = events
            .iter()
            .filter(|ev| {
                !(ev.ts < prev_ts || (ev.ts == prev_ts && prev_keys.contains(ev.key.as_str())))
            })
            .collect();
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].model, "claude-4.6-sonnet");
    }
}
