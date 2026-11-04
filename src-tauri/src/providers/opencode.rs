use std::collections::HashMap;
use std::path::PathBuf;

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;
use crate::model::{now_ms, plausible_gen_ms, UsageRecord};
use crate::paths;
use crate::providers::{AgentProvider, ScanCtx};

pub struct OpencodeProvider;

const AGENT: &str = "opencode";

/// 解析语义版本;变更时启动会全量重建。
/// v1: session 表绝对值 diff,基线随重建一起重算。
/// v2: reasoning 不在 output 里,合计时要加进去,否则官方总额会少这一栏。
/// v3: 估算速度来自 message.data 的 time.created→completed,不使用会话的 time_updated。
pub const PARSER_VERSION: u64 = 3;

impl AgentProvider for OpencodeProvider {
    fn id(&self) -> &str {
        AGENT
    }

    fn display_name(&self) -> &str {
        "OpenCode"
    }

    fn detect(&self) -> bool {
        paths::opencode_db().is_file()
    }

    /// 监听具体文件而非目录:该目录下有 snapshot/log 等高频写入的子目录,递归监听太吵;
    /// WAL 模式下写都落在 -wal 文件上,所以 db 和 -wal 都要监
    fn watch_paths(&self) -> Vec<PathBuf> {
        let db = paths::opencode_db();
        vec![db.clone(), db.with_extension("db-wal")]
    }

    fn parser_version(&self) -> u64 {
        PARSER_VERSION
    }

    /// session.cost 由 OpenCode 按 models.dev 的美元单价算出
    fn native_cost_currency(&self) -> Option<&'static str> {
        Some("USD")
    }

    fn scan(&self, ctx: &mut ScanCtx) -> Result<Vec<UsageRecord>> {
        let mut st: OcState = serde_json::from_value(std::mem::take(ctx.state)).unwrap_or_default();
        let records = scan_sessions(&mut st)?;
        *ctx.state = serde_json::to_value(&st).unwrap_or(Value::Null);
        Ok(records)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct OcEntry {
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
    cost: f64,
}

impl OcEntry {
    fn delta_from(&self, prev: &OcEntry) -> OcEntry {
        OcEntry {
            input: self.input.saturating_sub(prev.input),
            output: self.output.saturating_sub(prev.output),
            cache_read: self.cache_read.saturating_sub(prev.cache_read),
            cache_write: self.cache_write.saturating_sub(prev.cache_write),
            reasoning: self.reasoning.saturating_sub(prev.reasoning),
            cost: self.cost - prev.cost,
        }
    }

    fn is_zero(&self) -> bool {
        self.input + self.output + self.cache_read + self.cache_write + self.reasoning == 0
            && self.cost.abs() < f64::EPSILON
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct OcSpeed {
    #[serde(default)]
    output: u64,
    #[serde(default)]
    ms: u64,
    #[serde(default)]
    samples: u64,
}

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct OcState {
    /// key: session id;会话表是绝对值,存上次快照做差
    sessions: HashMap<String, OcEntry>,
    /// key: session\x01model\x01provider。消息耗时的绝对值,只把增长写成估算速度。
    #[serde(default)]
    speed: HashMap<String, OcSpeed>,
}

/// session 表直接给出按会话聚合的 token 列;模型字段是 JSON 字符串 {"id","providerID","variant"}
fn scan_sessions(st: &mut OcState) -> Result<Vec<UsageRecord>> {
    let db = paths::opencode_db();
    if !db.is_file() {
        return Ok(vec![]);
    }
    // 优先只读打开;WAL 库在无 -shm 时只读会失败,此时退回普通打开(SQLite 自身加锁,查询瞬时完成)
    let conn = match Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(c) => c,
        Err(_) => Connection::open(&db)?,
    };
    let mut stmt = match conn.prepare(
        "SELECT id, title, directory, cost, tokens_input, tokens_output, tokens_reasoning,
                tokens_cache_read, tokens_cache_write, model, time_created, time_updated
         FROM session",
    ) {
        Ok(s) => s,
        // 表结构可能随版本变化,宽容降级
        Err(e) => {
            eprintln!("[opencode] query: {e}");
            return Ok(vec![]);
        }
    };
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, Option<f64>>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, Option<i64>>(5)?,
            row.get::<_, Option<i64>>(6)?,
            row.get::<_, Option<i64>>(7)?,
            row.get::<_, Option<i64>>(8)?,
            row.get::<_, Option<String>>(9)?,
            row.get::<_, Option<i64>>(10)?,
            row.get::<_, Option<i64>>(11)?,
        ))
    })?;

    let mut out = Vec::new();
    let mut alive: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut session_ts: HashMap<String, i64> = HashMap::new();
    for row in rows {
        let (sid, title, directory, cost, tin, tout, tre, tcr, tcw, model, created, updated) =
            match row {
                Ok(r) => r,
                Err(_) => continue,
            };
        alive.insert(sid.clone());
        let cur = OcEntry {
            input: tin.unwrap_or(0).max(0) as u64,
            output: tout.unwrap_or(0).max(0) as u64,
            cache_read: tcr.unwrap_or(0).max(0) as u64,
            cache_write: tcw.unwrap_or(0).max(0) as u64,
            reasoning: tre.unwrap_or(0).max(0) as u64,
            cost: cost.unwrap_or(0.0),
        };
        let prev = st.sessions.get(&sid).cloned().unwrap_or_default();
        let delta = cur.delta_from(&prev);
        // 该会话的绝对值快照按会话起始日计入按天表(OpenCode 没有更细的按天粒度)
        let ts = created.filter(|t| *t > 0).unwrap_or_else(now_ms);
        session_ts.insert(sid.clone(), ts);
        if delta.is_zero() {
            continue;
        }
        let (model_id, provider_id) = parse_model(model.as_deref());
        out.push(UsageRecord {
            agent: AGENT.into(),
            session_id: Some(sid.clone()),
            project: directory,
            title,
            model: model_id,
            provider: provider_id,
            ts,
            touch_ts: updated.filter(|t| *t > 0),
            input_tokens: delta.input,
            // OpenCode 官方合计是 input + output + reasoning + 两种缓存。
            // 本应用的合计不另加 reasoning,所以把它并进 output,单列仍保留。
            output_tokens: delta.output.saturating_add(delta.reasoning),
            cache_read_tokens: delta.cache_read,
            cache_write_tokens: delta.cache_write,
            reasoning_tokens: delta.reasoning,
            cost: delta.cost,
            ..Default::default()
        });
        st.sessions.insert(sid, cur);
    }
    // 会话被删除时同步清掉基准
    st.sessions.retain(|k, _| alive.contains(k));
    drop(stmt);
    match load_message_speeds(&conn) {
        Ok(snaps) => out.extend(speed_deltas(&mut st.speed, snaps, &session_ts)),
        Err(e) => eprintln!("[opencode] message speed: {e}"),
    }
    Ok(out)
}

/// message.data 里的 time.created → time.completed 才是单次请求耗时。
/// 会话行的 time_updated 含着用户离开后的空档,不能拿来算速度。
fn load_message_speeds(conn: &Connection) -> rusqlite::Result<HashMap<String, OcSpeed>> {
    let mut stmt = conn.prepare("SELECT session_id, data FROM message")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut map: HashMap<String, OcSpeed> = HashMap::new();
    for row in rows {
        let (sid, data) = match row {
            Ok(row) => row,
            Err(_) => continue,
        };
        let Ok(v) = serde_json::from_str::<Value>(&data) else {
            continue;
        };
        let Some((key, snap)) = speed_from_data(&sid, &v) else {
            continue;
        };
        let dst = map.entry(key).or_default();
        dst.output = dst.output.saturating_add(snap.output);
        dst.ms = dst.ms.saturating_add(snap.ms);
        dst.samples = dst.samples.saturating_add(snap.samples);
    }
    Ok(map)
}

fn speed_from_data(session_id: &str, data: &Value) -> Option<(String, OcSpeed)> {
    if data.get("role").and_then(|v| v.as_str()) != Some("assistant") {
        return None;
    }
    let model = data
        .get("modelID")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| data.pointer("/model/id").and_then(|v| v.as_str()))
        .filter(|s| !s.is_empty())?;
    let provider = data
        .get("providerID")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .or_else(|| data.pointer("/model/providerID").and_then(|v| v.as_str()))
        .unwrap_or("");
    let output = json_u64(data.pointer("/tokens/output"));
    if output == 0 {
        return None;
    }
    let created = data.pointer("/time/created").and_then(Value::as_f64)?;
    let completed = data.pointer("/time/completed").and_then(Value::as_f64)?;
    let duration = completed - created;
    if !duration.is_finite() || duration <= 0.0 {
        return None;
    }
    let ms = plausible_gen_ms(duration as i64)?;
    Some((
        speed_key(session_id, model, provider),
        OcSpeed {
            output,
            ms,
            samples: 1,
        },
    ))
}

fn speed_key(session_id: &str, model: &str, provider: &str) -> String {
    format!("{session_id}\u{1}{model}\u{1}{provider}")
}

fn split_speed_key(key: &str) -> Option<(&str, &str, &str)> {
    let mut parts = key.split('\u{1}');
    Some((parts.next()?, parts.next()?, parts.next()?))
}

fn json_u64(v: Option<&Value>) -> u64 {
    let Some(v) = v else {
        return 0;
    };
    if let Some(n) = v.as_u64() {
        return n;
    }
    if let Some(n) = v.as_i64() {
        return n.max(0) as u64;
    }
    v.as_f64()
        .filter(|n| n.is_finite() && *n > 0.0)
        .map(|n| n as u64)
        .unwrap_or(0)
}

fn speed_deltas(
    speed: &mut HashMap<String, OcSpeed>,
    snaps: HashMap<String, OcSpeed>,
    session_ts: &HashMap<String, i64>,
) -> Vec<UsageRecord> {
    let mut out = Vec::new();
    for (key, snap) in snaps {
        let prev = speed.get(&key).cloned().unwrap_or_default();
        let d_out = snap.output.saturating_sub(prev.output);
        let d_ms = snap.ms.saturating_sub(prev.ms);
        let d_samples = snap.samples.saturating_sub(prev.samples);
        if d_out == 0 && d_ms == 0 && d_samples == 0 {
            continue;
        }
        let Some((sid, model, provider)) = split_speed_key(&key) else {
            continue;
        };
        if model.is_empty() {
            continue;
        }
        let ts = session_ts
            .get(sid)
            .copied()
            .filter(|t| *t > 0)
            .unwrap_or_else(now_ms);
        out.push(UsageRecord {
            agent: AGENT.into(),
            model: Some(model.to_string()),
            provider: if provider.is_empty() {
                None
            } else {
                Some(provider.to_string())
            },
            ts,
            est_output: d_out,
            est_ms: d_ms,
            est_samples: d_samples,
            skip_daily: true,
            skip_hourly: true,
            ..Default::default()
        });
        speed.insert(key, snap);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasoning_counts_once_in_the_official_total() {
        let delta = OcEntry {
            input: 10,
            output: 4,
            reasoning: 6,
            cache_read: 2,
            cache_write: 1,
            ..Default::default()
        }
        .delta_from(&OcEntry::default());
        assert!(!delta.is_zero());
        let output = delta.output.saturating_add(delta.reasoning);
        let total = delta.input + output + delta.cache_read + delta.cache_write;
        assert_eq!(total, 23, "官方合计含 reasoning,且不能再加一次");

        let only_reasoning = OcEntry {
            reasoning: 3,
            ..Default::default()
        };
        assert!(
            !only_reasoning.delta_from(&OcEntry::default()).is_zero(),
            "只有 reasoning 的增量不能被当成空快照丢掉"
        );
    }

    #[test]
    fn message_time_is_estimated_speed_and_does_not_repeat() {
        let payload = serde_json::json!({
            "role": "assistant",
            "modelID": "claude-sonnet-4",
            "providerID": "anthropic",
            "tokens": { "output": 4 },
            "time": { "created": 1783882279705i64, "completed": 1783882279943i64 }
        });
        let (key, snap) = speed_from_data("ses_1", &payload).unwrap();
        assert_eq!(snap.output, 4);
        assert_eq!(snap.ms, 238);
        assert_eq!(snap.samples, 1);

        let user = serde_json::json!({
            "role": "user",
            "time": { "created": 1783882279705i64, "completed": 1783882279943i64 }
        });
        assert!(speed_from_data("ses_1", &user).is_none());
        let unfinished = serde_json::json!({
            "role": "assistant",
            "modelID": "claude-sonnet-4",
            "tokens": { "output": 4 },
            "time": { "created": 1783882279705i64 }
        });
        assert!(speed_from_data("ses_1", &unfinished).is_none());

        let mut speed = HashMap::new();
        let mut snaps = HashMap::new();
        snaps.insert(key, snap);
        let session_ts = HashMap::from([("ses_1".into(), 50i64)]);
        let first = speed_deltas(&mut speed, snaps.clone(), &session_ts);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].est_output, 4);
        assert_eq!(first[0].est_ms, 238);
        assert_eq!(first[0].output_tokens, 0);
        assert!(first[0].skip_daily && first[0].skip_hourly);
        assert_eq!(first[0].ts, 50);
        assert!(speed_deltas(&mut speed, snaps, &session_ts).is_empty());
    }
}

fn parse_model(model: Option<&str>) -> (Option<String>, Option<String>) {
    let Some(s) = model else {
        return (None, None);
    };
    let Ok(v) = serde_json::from_str::<Value>(s) else {
        return (None, None);
    };
    (
        v.get("id").and_then(|x| x.as_str()).map(|s| s.to_string()),
        v.get("providerID")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
    )
}
