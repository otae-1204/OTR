use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;
use crate::model::{now_ms, UsageRecord};
use crate::paths;
use crate::providers::jsonl_util::{f64f, u64f};
use crate::providers::{ScanCtx, AgentProvider};

pub struct DshProvider;

const AGENT: &str = "dsh";

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
        vec![paths::dsh_storages()]
    }

    fn scan(&self, ctx: &mut ScanCtx) -> Result<Vec<UsageRecord>> {
        let mut st: DshState =
            serde_json::from_value(std::mem::take(ctx.state)).unwrap_or_default();
        let mut records = Vec::new();

        // 按天表数据源:cost-meter/ledger.json(自带按天×按模型聚合与成本)
        if let Err(e) = scan_ledger(&mut st, &mut records) {
            eprintln!("[dsh] ledger: {}", e);
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
    ledger: HashMap<String, DshEntry>,
    /// key: "session_id|model"
    sessions: HashMap<String, DshEntry>,
}

/// projcache 行可能是 {val: ...} 包装,也可能是裸对象
fn row_val<'a>(row: &'a Value) -> &'a Value {
    row.get("val").unwrap_or(row)
}

fn date_noon_ms(date: &str) -> i64 {
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(12, 0, 0))
        .and_then(|nd| nd.and_local_timezone(chrono::Local).single())
        .map(|dt| dt.timestamp_millis())
        .unwrap_or_else(now_ms)
}

fn scan_ledger(st: &mut DshState, out: &mut Vec<UsageRecord>) -> Result<()> {
    let path = paths::dsh_storages().join("cost-meter").join("ledger.json");
    if !path.is_file() {
        return Ok(());
    }
    let text = std::fs::read_to_string(&path)?;
    let v: Value = serde_json::from_str(&text)?;
    let Some(days) = v.get("days").and_then(|d| d.as_object()) else {
        return Ok(());
    };
    let mut alive: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (date, day) in days {
        let Some(models) = day.get("byProviderModel").and_then(|m| m.as_object()) else {
            continue;
        };
        let ts = date_noon_ms(date);
        for (mkey, m) in models {
            let key = format!("{}|{}", date, mkey);
            alive.insert(key.clone());
            let cur = DshEntry::from_json(m);
            let prev = st.ledger.get(&key).cloned().unwrap_or_default();
            let delta = cur.delta_from(&prev);
            if delta.is_zero() {
                continue;
            }
            let (provider, model) = match mkey.split_once(':') {
                Some((p, m)) => (Some(p.to_string()), Some(m.to_string())),
                None => (None, Some(mkey.clone())),
            };
            out.push(UsageRecord {
                agent: AGENT.into(),
                model,
                provider,
                ts,
                input_tokens: delta.input,
                output_tokens: delta.output,
                cache_read_tokens: delta.cache_read,
                cache_write_tokens: delta.cache_write,
                reasoning_tokens: delta.reasoning,
                calls: delta.calls,
                cost: delta.cost,
                ..Default::default()
            });
            st.ledger.insert(key, cur);
        }
    }
    // 台账里消失的日期不再保留基准(避免状态无限膨胀)
    st.ledger.retain(|k, _| alive.contains(k));
    Ok(())
}

fn scan_projcache(st: &mut DshState, out: &mut Vec<UsageRecord>) -> Result<()> {
    let path = paths::dsh_storages().join("session_projcache.json");
    if !path.is_file() {
        return Ok(());
    }
    let text = std::fs::read_to_string(&path)?;
    let v: Value = serde_json::from_str(&text)?;
    let Some(sessions) = v
        .pointer("/tables/sessions")
        .and_then(|s| s.as_object())
    else {
        return Ok(());
    };
    for (sid, entry) in sessions {
        let identity = entry.get("identity").cloned().unwrap_or(Value::Null);
        let created_at = identity.get("createdAt").and_then(|x| x.as_i64()).unwrap_or(0);
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
                    ..Default::default()
                });
                st.sessions.insert(key, cur);
            }
            }
        }
    }
    Ok(())
}
