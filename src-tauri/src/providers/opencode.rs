use std::path::PathBuf;

use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;
use crate::model::{now_ms, UsageRecord};
use crate::paths;
use crate::providers::{ScanCtx, AgentProvider};

pub struct OpencodeProvider;

const AGENT: &str = "opencode";

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
        vec![
            db.clone(),
            db.with_extension("db-wal"),
        ]
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
        self.input + self.output + self.cache_read + self.cache_write == 0
            && self.cost.abs() < f64::EPSILON
    }
}

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct OcState {
    /// key: session id;会话表是绝对值,存上次快照做差
    sessions: std::collections::HashMap<String, OcEntry>,
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
        if delta.is_zero() {
            continue;
        }
        let (model_id, provider_id) = parse_model(model.as_deref());
        // 该会话的绝对值快照按会话起始日计入按天表(OpenCode 没有更细的按天粒度)
        let ts = created.filter(|t| *t > 0).unwrap_or_else(now_ms);
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
            output_tokens: delta.output,
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
    Ok(out)
}

fn parse_model(model: Option<&str>) -> (Option<String>, Option<String>) {
    let Some(s) = model else {
        return (None, None);
    };
    let Ok(v) = serde_json::from_str::<Value>(s) else {
        return (None, None);
    };
    (
        v.get("id")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
        v.get("providerID")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
    )
}
