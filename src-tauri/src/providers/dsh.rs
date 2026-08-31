use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;
use crate::model::{local_date, local_hour, now_ms, UsageRecord};
use crate::paths;
use crate::providers::jsonl_util::{f64f, u64f};
use crate::providers::{AgentProvider, ScanCtx};

pub struct DshProvider;

const AGENT: &str = "dsh";
pub const PARSER_VERSION: u64 = 2;

impl AgentProvider for DshProvider {
    fn id(&self) -> &'static str {
        AGENT
    }

    fn display_name(&self) -> &'static str {
        "DSH"
    }

    fn detect(&self) -> bool {
        paths::dsh_storages().is_dir()
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        vec![paths::dsh_storages(), paths::dsh_home().join("sessions")]
    }

    fn scan(&self, ctx: &mut ScanCtx) -> Result<Vec<UsageRecord>> {
        let mut st: DshState =
            serde_json::from_value(std::mem::take(ctx.state)).unwrap_or_default();
        let mut records = Vec::new();

        // 按天表数据源:cost-meter/ledger.json(自带按天×按模型聚合与成本)
        if let Err(e) = scan_ledger(&mut st, &mut records) {
            eprintln!("[dsh] ledger: {}", e);
        }
        // 按小时表数据源:会话日志中的 usage 事件(按真实事件时间分桶)
        let logs_available = match scan_session_logs(&mut st, &mut records) {
            Ok(available) => available,
            Err(e) => {
                eprintln!("[dsh] session logs: {}", e);
                false
            }
        };
        // 某些旧设备只有台账没有会话日志,至少保留会话 at 的降级数据。
        if !logs_available {
            if let Err(e) = scan_ledger_sessions(&mut st, &mut records) {
                eprintln!("[dsh] ledger sessions: {}", e);
            }
        }
        // 会话表数据源:session_projcache.json(按会话×模型;skip_daily 避免与 ledger 双计)
        if let Err(e) = scan_projcache(&mut st, &mut records) {
            eprintln!("[dsh] projcache: {}", e);
        }

        *ctx.state = serde_json::to_value(&st).unwrap_or(Value::Null);
        Ok(records)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct DshEntry {
    #[serde(default)]
    input: u64,
    #[serde(default)]
    output: u64,
    #[serde(default)]
    cache_read: u64,
    #[serde(default)]
    cache_write: u64,
    #[serde(default)]
    reasoning: u64,
    #[serde(default)]
    calls: u64,
    #[serde(default)]
    cost: f64,
}

impl DshEntry {
    fn from_json(v: &Value) -> Self {
        DshEntry {
            input: u64f(v, "input"),
            output: u64f(v, "output"),
            cache_read: u64f(v, "cacheRead"),
            cache_write: u64f(v, "cacheWrite"),
            reasoning: u64f(v, "reasoning"),
            calls: u64f(v, "calls"),
            cost: f64f(v, "cost"),
        }
    }

    fn delta_from(&self, prev: &DshEntry) -> DshEntry {
        DshEntry {
            input: self.input.saturating_sub(prev.input),
            output: self.output.saturating_sub(prev.output),
            cache_read: self.cache_read.saturating_sub(prev.cache_read),
            cache_write: self.cache_write.saturating_sub(prev.cache_write),
            reasoning: self.reasoning.saturating_sub(prev.reasoning),
            calls: self.calls.saturating_sub(prev.calls),
            cost: self.cost - prev.cost,
        }
    }

    fn is_zero(&self) -> bool {
        self.input + self.output + self.cache_read + self.cache_write == 0
            && self.calls == 0
            && self.cost.abs() < f64::EPSILON
    }
}

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct DshState {
    /// key: "日期|provider:model"
    #[serde(default)]
    ledger: HashMap<String, DshEntry>,
    /// key: "日期|会话 id|provider:model",用于无日志设备的小时降级分桶
    #[serde(default)]
    ledger_sessions: HashMap<String, DshEntry>,
    /// 从 DSH 会话日志重建的绝对小时聚合,key: "日期|小时|provider:model"
    #[serde(default)]
    hourly: HashMap<String, DshEntry>,
    /// key: "session_id|model"
    #[serde(default)]
    sessions: HashMap<String, DshEntry>,
}

/// projcache 行可能是 {val: ...} 包装,也可能是裸对象
fn row_val<'a>(row: &'a Value) -> &'a Value {
    row.get("val").unwrap_or(row)
}

fn read_json_file(path: &Path) -> Result<Value> {
    let mut last_error = None;
    for attempt in 0..3 {
        match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<Value>(&text) {
                Ok(value) => return Ok(value),
                Err(error) => last_error = Some(error),
            },
            Err(error) => return Err(error.into()),
        }
        if attempt < 2 {
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }
    Err(last_error
        .expect("JSON parse must fail before retry exhaustion")
        .into())
}

fn date_start_ms(date: &str) -> i64 {
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .and_then(|nd| nd.and_local_timezone(chrono::Local).single())
        .map(|dt| dt.timestamp_millis())
        .unwrap_or_else(now_ms)
}

fn split_model_key(mkey: &str) -> (Option<String>, Option<String>) {
    match mkey.split_once(':') {
        Some((provider, model)) => (Some(provider.to_string()), Some(model.to_string())),
        None => (None, Some(mkey.to_string())),
    }
}

fn record_from_entry(
    entry: &DshEntry,
    model_key: &str,
    ts: i64,
    bucket_date: Option<String>,
    bucket_hour: Option<i64>,
) -> UsageRecord {
    let (provider, model) = split_model_key(model_key);
    UsageRecord {
        agent: AGENT.into(),
        model,
        provider,
        ts,
        input_tokens: entry.input,
        output_tokens: entry.output,
        cache_read_tokens: entry.cache_read,
        cache_write_tokens: entry.cache_write,
        reasoning_tokens: entry.reasoning,
        calls: entry.calls,
        cost: entry.cost,
        bucket_date,
        bucket_hour,
        ..Default::default()
    }
}

fn scan_ledger(st: &mut DshState, out: &mut Vec<UsageRecord>) -> Result<()> {
    let path = paths::dsh_storages().join("cost-meter").join("ledger.json");
    if !path.is_file() {
        return Ok(());
    }
    let v = read_json_file(&path)?;
    let Some(days) = v.get("days").and_then(|d| d.as_object()) else {
        return Ok(());
    };
    let mut alive: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (date, day) in days {
        if let Some(models) = day.get("byProviderModel").and_then(|m| m.as_object()) {
            let ts = date_start_ms(date);
            for (mkey, m) in models {
                let key = format!("{}|{}", date, mkey);
                alive.insert(key.clone());
                let cur = DshEntry::from_json(m);
                let prev = st.ledger.get(&key).cloned().unwrap_or_default();
                let delta = cur.delta_from(&prev);
                if !delta.is_zero() {
                    let mut record = record_from_entry(&delta, mkey, ts, None, None);
                    record.skip_hourly = true;
                    out.push(record);
                }
                st.ledger.insert(key, cur);
            }
        }
    }
    // 台账里消失的日期不再保留基准(避免状态无限膨胀)
    st.ledger.retain(|k, _| alive.contains(k));
    Ok(())
}

fn scan_ledger_sessions(st: &mut DshState, out: &mut Vec<UsageRecord>) -> Result<()> {
    let path = paths::dsh_storages().join("cost-meter").join("ledger.json");
    if !path.is_file() {
        return Ok(());
    }
    let v = read_json_file(&path)?;
    let Some(days) = v.get("days").and_then(|d| d.as_object()) else {
        return Ok(());
    };
    let mut alive = std::collections::HashSet::new();
    for (date, day) in days {
        let Some(sessions) = day.get("sessions").and_then(|s| s.as_array()) else {
            continue;
        };
        for session in sessions {
            let Some(sid) = session.get("id").and_then(|s| s.as_str()) else {
                continue;
            };
            let candidate_ts = session
                .get("at")
                .and_then(|at| at.as_i64())
                .filter(|at| *at > 0);
            let ts = candidate_ts
                .filter(|at| local_date(*at) == *date)
                .unwrap_or_else(|| date_start_ms(date));
            let hour = local_hour(ts);
            let Some(models) = session.get("byProviderModel").and_then(|m| m.as_object()) else {
                continue;
            };
            for (mkey, value) in models {
                let key = format!("{}|{}|{}", date, sid, mkey);
                alive.insert(key.clone());
                let cur = DshEntry::from_json(value);
                let prev = st.ledger_sessions.get(&key).cloned().unwrap_or_default();
                let delta = cur.delta_from(&prev);
                if !delta.is_zero() {
                    let mut record =
                        record_from_entry(&delta, mkey, ts, Some(date.clone()), Some(hour));
                    record.skip_daily = true;
                    out.push(record);
                }
                st.ledger_sessions.insert(key, cur);
            }
        }
    }
    st.ledger_sessions.retain(|key, _| alive.contains(key));
    Ok(())
}

fn collect_session_logs(root: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let projects = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for project in projects {
        let project = project?;
        if !project.file_type()?.is_dir() {
            continue;
        }
        for session in std::fs::read_dir(project.path())? {
            let session = session?;
            if !session.file_type()?.is_dir() {
                continue;
            }
            for name in ["session.jsonl.zstd", "session.jsonl"] {
                let path = session.path().join(name);
                if path.is_file() {
                    out.push(path);
                    break;
                }
            }
        }
    }
    Ok(())
}

fn read_session_text(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    let decoded = if path.extension().and_then(|s| s.to_str()) == Some("zstd") {
        zstd::stream::decode_all(bytes.as_slice())?
    } else {
        bytes
    };
    Ok(String::from_utf8_lossy(&decoded).into_owned())
}

fn scan_session_file(path: &Path, aggregate: &mut HashMap<String, DshEntry>) -> bool {
    let Ok(text) = read_session_text(path) else {
        return false;
    };
    let mut provider = "deepseek".to_string();
    let mut model = "default".to_string();
    let mut created_at = 0i64;
    let mut samples: HashMap<String, (i64, String, DshEntry)> = HashMap::new();
    for line in text.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match event.get("type").and_then(|v| v.as_str()) {
            Some("session") => {
                created_at = event.get("createdAt").and_then(|v| v.as_i64()).unwrap_or(0);
                continue;
            }
            Some("request/header") => {
                if let Some(config) = event.pointer("/data/header/config") {
                    if let Some(value) = config.get("provider").and_then(|v| v.as_str()) {
                        if !value.is_empty() {
                            provider = value.to_string();
                        }
                    }
                    if let Some(value) = config.get("model").and_then(|v| v.as_str()) {
                        if !value.is_empty() {
                            model = value.to_string();
                        }
                    }
                }
                continue;
            }
            _ => {}
        }
        let event_time = event.get("time").and_then(|v| v.as_i64()).unwrap_or(0);
        if event_time <= 0 || (created_at > 0 && event_time < created_at) {
            continue;
        }
        let (usage, turn, step) =
            if event.get("type").and_then(|v| v.as_str()) == Some("assistant/message") {
                (
                    event.pointer("/data/usage"),
                    event
                        .pointer("/data/turn")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    event
                        .pointer("/data/step")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                )
            } else if event.get("type").and_then(|v| v.as_str()) == Some("assistant/chunk")
                && event.pointer("/data/chunk/type").and_then(|v| v.as_str()) == Some("usage")
            {
                (
                    event.pointer("/data/chunk/usage"),
                    event
                        .pointer("/data/turn")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    event
                        .pointer("/data/step")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                )
            } else {
                continue;
            };
        let Some(usage) = usage else { continue };
        let entry = DshEntry {
            input: u64f(usage, "inputTokens"),
            output: u64f(usage, "outputTokens"),
            cache_read: u64f(usage, "cacheReadTokens"),
            cache_write: u64f(usage, "cacheWriteTokens"),
            reasoning: u64f(usage, "reasoningTokens"),
            calls: 1,
            cost: 0.0,
        };
        let key = format!("{}:{}", turn, step);
        samples.insert(key, (event_time, format!("{}:{}", provider, model), entry));
    }
    for (_sample_key, (ts, model_key, entry)) in samples {
        let key = format!("{}|{}|{}", local_date(ts), local_hour(ts), model_key);
        let bucket = aggregate.entry(key).or_default();
        bucket.input += entry.input;
        bucket.output += entry.output;
        bucket.cache_read += entry.cache_read;
        bucket.cache_write += entry.cache_write;
        bucket.reasoning += entry.reasoning;
        bucket.calls += entry.calls;
    }
    true
}

fn scan_session_logs(st: &mut DshState, out: &mut Vec<UsageRecord>) -> Result<bool> {
    let root = paths::dsh_home().join("sessions");
    let mut paths = Vec::new();
    collect_session_logs(&root, &mut paths)?;
    if paths.is_empty() {
        return Ok(false);
    }
    let mut current = HashMap::new();
    let mut usable = false;
    let mut failed = false;
    for path in paths {
        if scan_session_file(&path, &mut current) {
            usable = true;
        } else {
            failed = true;
        }
    }
    if failed {
        // 任一日志仍在写入或损坏时保留旧快照,避免下一次恢复时整份日志
        // 被当成新增长量再次写入小时表。
        return Ok(true);
    }
    if !usable {
        // 日志文件存在但当前仍在写入/损坏时,不要切换到台账降级路径,
        // 否则日志恢复后同一增量可能被重复记入小时表。
        return Ok(true);
    }
    let mut alive = std::collections::HashSet::new();
    for (key, cur) in &current {
        alive.insert(key.clone());
        let prev = st.hourly.get(key).cloned().unwrap_or_default();
        let delta = cur.delta_from(&prev);
        if delta.is_zero() {
            continue;
        }
        let mut parts = key.splitn(3, '|');
        let date = parts.next().unwrap_or_default().to_string();
        let hour = parts
            .next()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        let model_key = parts.next().unwrap_or_default();
        let mut record = record_from_entry(
            &delta,
            model_key,
            date_start_ms(&date),
            Some(date),
            Some(hour),
        );
        record.skip_daily = true;
        out.push(record);
    }
    st.hourly = current;
    st.hourly.retain(|key, _| alive.contains(key));
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::{local_date, local_hour, scan_session_file, DshEntry};
    use std::collections::HashMap;
    use std::fs;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(ext: &str) -> std::path::PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("otr-dsh-{suffix}.{ext}"))
    }

    #[test]
    fn session_log_uses_event_hour_and_replaces_streaming_sample() {
        let path = temp_path("jsonl");
        let first_ts = 1_780_000_000_000i64;
        let second_ts = first_ts + 3_600_000;
        let mut file = fs::File::create(&path).unwrap();
        let events = [
            serde_json::json!({
                "type": "session", "id": "s-1", "createdAt": first_ts - 1
            }),
            serde_json::json!({
                "type": "request/header", "time": first_ts,
                "data": {"header": {"config": {"provider": "p", "model": "m"}}}
            }),
            serde_json::json!({
                "type": "assistant/message", "time": first_ts,
                "data": {"turn": 1, "step": 1, "usage": {"inputTokens": 10, "outputTokens": 2}}
            }),
            serde_json::json!({
                "type": "assistant/message", "time": second_ts,
                "data": {"turn": 1, "step": 1, "usage": {"inputTokens": 20, "outputTokens": 3}}
            }),
            serde_json::json!({
                "type": "assistant/message", "time": second_ts,
                "data": {"turn": 1, "step": 2, "usage": {"inputTokens": 7, "outputTokens": 1}}
            }),
        ];
        for event in events {
            writeln!(file, "{event}").unwrap();
        }
        let mut aggregate = HashMap::new();
        assert!(scan_session_file(&path, &mut aggregate));
        let first_key = format!("{}|{}|p:m", local_date(first_ts), local_hour(first_ts));
        let second_key = format!("{}|{}|p:m", local_date(second_ts), local_hour(second_ts));
        assert_eq!(aggregate.get(&first_key).map(|v| v.input), None);
        assert_eq!(aggregate.get(&second_key).map(|v| v.input), Some(27));
        assert_eq!(aggregate.get(&second_key).map(|v| v.output), Some(4));
        assert_eq!(aggregate.get(&second_key).map(|v| v.calls), Some(2));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn old_hourly_state_delta_is_zero_for_unchanged_snapshot() {
        let current = DshEntry {
            input: 10,
            calls: 1,
            ..Default::default()
        };
        assert!(current.delta_from(&current).is_zero());
    }
}

fn scan_projcache(st: &mut DshState, out: &mut Vec<UsageRecord>) -> Result<()> {
    let path = paths::dsh_storages().join("session_projcache.json");
    if !path.is_file() {
        return Ok(());
    }
    let v = read_json_file(&path)?;
    let Some(sessions) = v.pointer("/tables/sessions").and_then(|s| s.as_object()) else {
        return Ok(());
    };
    for (sid, entry) in sessions {
        let identity = entry.get("identity").cloned().unwrap_or(Value::Null);
        let created_at = identity
            .get("createdAt")
            .and_then(|x| x.as_i64())
            .unwrap_or(0);
        let cwd = identity
            .get("cwd")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        let rows = entry.get("rows").cloned().unwrap_or(Value::Null);
        // 标题尽力而为
        let title = rows
            .get("title")
            .map(row_val)
            .and_then(|t| {
                t.get("title")
                    .and_then(|x| x.as_str())
                    .or_else(|| t.as_str())
            })
            .map(|s| s.to_string());
        // 优先 costUsage(带 provider/model/成本),缺失则退回 tokenUsage.totals
        let cost_usage = rows.get("costUsage").map(row_val);
        let by_model = cost_usage.and_then(|c| c.get("byModel")).cloned();
        let mut used_by_model = false;
        if let Some(models) = by_model.as_ref().and_then(|v| v.as_object()) {
            used_by_model = true;
            for (model, m) in models {
                let key = format!("{}|{}", sid, model);
                let cur = DshEntry::from_json(m);
                let prev = st.sessions.get(&key).cloned().unwrap_or_default();
                let delta = cur.delta_from(&prev);
                if delta.is_zero() {
                    continue;
                }
                let provider = cost_usage
                    .and_then(|c| c.get("provider"))
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string());
                out.push(UsageRecord {
                    agent: AGENT.into(),
                    session_id: Some(sid.clone()),
                    project: cwd.clone(),
                    title: title.clone(),
                    model: Some(model.clone()),
                    provider,
                    ts: created_at,
                    touch_ts: Some(now_ms()),
                    input_tokens: delta.input,
                    output_tokens: delta.output,
                    cache_read_tokens: delta.cache_read,
                    cache_write_tokens: delta.cache_write,
                    reasoning_tokens: delta.reasoning,
                    calls: delta.calls,
                    cost: delta.cost,
                    skip_daily: true,
                    skip_hourly: true,
                    ..Default::default()
                });
                st.sessions.insert(key, cur);
            }
        }
        if !used_by_model {
            if let Some(tok) = rows.get("tokenUsage").map(row_val) {
                let totals = tok.get("totals").cloned().unwrap_or(Value::Null);
                let cur = DshEntry {
                    input: u64f(&totals, "uncachedInputTokens"),
                    output: u64f(&totals, "outputTokens"),
                    cache_read: u64f(&totals, "cacheReadTokens"),
                    cache_write: u64f(&totals, "cacheWriteTokens"),
                    ..Default::default()
                };
                let key = format!("{}|", sid);
                let prev = st.sessions.get(&key).cloned().unwrap_or_default();
                let delta = cur.delta_from(&prev);
                if !delta.is_zero() {
                    out.push(UsageRecord {
                        agent: AGENT.into(),
                        session_id: Some(sid.clone()),
                        project: cwd.clone(),
                        title: title.clone(),
                        ts: created_at,
                        touch_ts: Some(now_ms()),
                        input_tokens: delta.input,
                        output_tokens: delta.output,
                        cache_read_tokens: delta.cache_read,
                        cache_write_tokens: delta.cache_write,
                        skip_daily: true,
                        skip_hourly: true,
                        ..Default::default()
                    });
                    st.sessions.insert(key, cur);
                }
            }
        }
    }
    Ok(())
}
