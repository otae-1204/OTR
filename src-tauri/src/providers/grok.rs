//! Grok Build 的 Token 消耗:读本地会话,不走网络。
//!
//! 数据在 `$GROK_HOME/sessions/<编码 cwd>/<session-id>/updates.jsonl`
//! (`GROK_HOME` 缺省 `~/.grok`)。一轮对话结束时追加一条
//! `sessionUpdate = "turn_completed"`,里面的 `usage` 是这一轮的增量,不是累计。
//!
//! 口径对齐 ccusage / ccstats 对 `updates.jsonl` 的读法,以及 Grok 自己的字段说明:
//! - ACP 的 `inputTokens` **包含** `cachedReadTokens` 和 `cacheCreationTokens`,
//!   未缓存输入要减掉。若 `totalTokens` 等于「input + 缓存 + output」,
//!   或缓存比 input 还大,说明这几栏已经分开,再减就会把未缓存输入减成 0;
//! - `reasoningTokens` 已经含在 `outputTokens` 里。本应用的合计不另加 reasoning,
//!   所以输出保持原值,只把 reasoning 单列;
//! - `modelUsage` 是同一笔账的拆分,不和父级再加一次。拆分之和小于父级时补差额
//!   (否则漏掉没写进模型行的调用);大于父级时说明模型行把整笔又抄了一遍,只留父级;
//! - 同一 `prompt_id` 是这一轮的快照,不是又一次调用。fork 拷贝或重复行不累加;
//!   后写的一份更大时,只补多出来的部分;
//! - `costUsdTicks` 是美元的 1e10 分之一(Grok 文档:1 USD = 10^10 ticks)。
//!
//! 不读 `logs/unified.jsonl`,也不读 `signals.json`。前者和 `turn_completed`
//! 是同一笔账的两种粒度,一起算会重计;后者是上下文占用,不是消耗,拿来当账单会漏一大截。
//!
//! 子会话(`session_kind` 以 `subagent` 开头)的用量已经折进协调者那一轮,
//! 再计就是双计,整段跳过。fork / worktree 按 `prompt_id` 并到父会话,父会话先扫。

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::Result;
use crate::model::{iso_to_ms, UsageRecord};
use crate::paths;
use crate::providers::jsonl_util;
use crate::providers::{AgentProvider, ScanCtx};

pub struct GrokProvider;

const AGENT: &str = "grok";

/// 解析语义版本;变更时启动会全量重建。
/// v2: 模型拆分补差额 / 重复快照只记增量,避免重计和漏计。
pub const PARSER_VERSION: u64 = 2;

/// `costUsdTicks` 的刻度:1 USD = 10_000_000_000 ticks。
const TICKS_PER_USD: f64 = 10_000_000_000.0;

impl AgentProvider for GrokProvider {
    fn id(&self) -> &str {
        AGENT
    }

    fn display_name(&self) -> &str {
        "Grok Build"
    }

    fn detect(&self) -> bool {
        paths::grok_sessions().is_dir()
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        vec![paths::grok_sessions()]
    }

    fn parser_version(&self) -> u64 {
        PARSER_VERSION
    }

    fn native_cost_currency(&self) -> Option<&'static str> {
        Some("USD")
    }

    fn scan(&self, ctx: &mut ScanCtx) -> Result<Vec<UsageRecord>> {
        scan_root(&paths::grok_sessions(), ctx)
    }
}

struct SessionMeta {
    kind: Option<String>,
    project: Option<String>,
    title: Option<String>,
    model: Option<String>,
    /// 行上没有时间戳时的兜底,避免整轮消耗被丢掉
    when: Option<i64>,
}

enum SummaryRead {
    Ready(SessionMeta),
    /// 文件坏了。可能正是子会话标记读不出来,不能当成普通会话放行。
    Broken,
}

/// 扫描一次会话根目录。测试直接喂临时目录,不碰用户的 `~/.grok`。
pub fn scan_root(root: &Path, ctx: &mut ScanCtx) -> Result<Vec<UsageRecord>> {
    if !root.is_dir() {
        return Ok(vec![]);
    }
    let mut sessions = Vec::new();
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) => {
            eprintln!("[{AGENT}] {}: {e}", root.display());
            return Ok(vec![]);
        }
    };
    for entry in entries.flatten() {
        let cwd_dir = entry.path();
        if !cwd_dir.is_dir() {
            continue;
        }
        let Ok(children) = fs::read_dir(&cwd_dir) else {
            continue;
        };
        for child in children.flatten() {
            let dir = child.path();
            if !dir.is_dir() {
                continue;
            }
            let updates = dir.join("updates.jsonl");
            if !updates.is_file() {
                continue;
            }
            let Some(session_id) = dir.file_name().and_then(|n| n.to_str()).map(str::to_string) else {
                continue;
            };
            match read_summary(&dir) {
                SummaryRead::Broken => {
                    eprintln!(
                        "[{AGENT}] {} 的 summary.json 读不出来,跳过(避免把子会话再计一遍)",
                        dir.display()
                    );
                    continue;
                }
                SummaryRead::Ready(meta) => {
                    if meta
                        .kind
                        .as_deref()
                        .is_some_and(|kind| kind.starts_with("subagent"))
                    {
                        continue;
                    }
                    sessions.push(SessionFile {
                        updates,
                        session_id,
                        meta,
                    });
                }
            }
        }
    }

    // 普通会话在前,fork / worktree 在后:同一 prompt_id 留下父会话的项目和标题。
    sessions.sort_by(|a, b| {
        let rank = |s: &SessionFile| u8::from(s.meta.kind.is_some());
        rank(a)
            .cmp(&rank(b))
            .then_with(|| a.updates.cmp(&b.updates))
    });

    let mut accounted = load_accounted(ctx.state);
    let mut records = Vec::new();
    for session in &sessions {
        if let Err(e) = scan_file(session, ctx, &mut accounted, &mut records) {
            eprintln!("[{AGENT}] {}: {e}", session.updates.display());
        }
    }
    store_accounted(ctx.state, &accounted);
    Ok(records)
}

struct SessionFile {
    updates: PathBuf,
    session_id: String,
    meta: SessionMeta,
}

fn read_summary(session_dir: &Path) -> SummaryRead {
    let path = session_dir.join("summary.json");
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return SummaryRead::Ready(SessionMeta {
                kind: None,
                project: None,
                title: None,
                model: None,
                when: None,
            });
        }
        Err(_) => return SummaryRead::Broken,
    };
    let Ok(v) = serde_json::from_str::<Value>(&raw) else {
        return SummaryRead::Broken;
    };
    SummaryRead::Ready(SessionMeta {
        kind: text_at(&v, "session_kind"),
        project: v
            .pointer("/info/cwd")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        title: ["generated_title", "title", "session_summary"]
            .iter()
            .find_map(|key| text_at(&v, key)),
        model: text_at(&v, "current_model_id")
            .or_else(|| v.pointer("/info/model_id").and_then(Value::as_str).map(str::to_string)),
        when: ["updated_at", "last_active_at", "created_at"]
            .iter()
            .find_map(|key| text_at(&v, key).as_deref().and_then(iso_to_ms)),
    })
}

fn text_at(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn scan_file(
    session: &SessionFile,
    ctx: &mut ScanCtx,
    accounted: &mut BTreeMap<String, Bucket>,
    out: &mut Vec<UsageRecord>,
) -> Result<()> {
    let key = session.updates.to_string_lossy().to_string();
    let mut cursor = ctx.cursors.get(&key).cloned().unwrap_or_default();
    let Some(update) = jsonl_util::read_appended(&session.updates, cursor.offset)? else {
        return Ok(());
    };
    if update.truncated {
        ctx.force_full = true;
        return Ok(());
    }

    for line in &update.lines {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(turn) = v.pointer("/params/update") else {
            continue;
        };
        if turn.get("sessionUpdate").and_then(Value::as_str) != Some("turn_completed") {
            continue;
        }
        let Some(usage) = turn.get("usage") else {
            continue;
        };
        let Some(ts) = event_ms(&v).or(session.meta.when) else {
            continue;
        };
        let parts = split_usage(usage, session.meta.model.as_deref());
        let snapshot = parts.iter().fold(Bucket::default(), |mut sum, part| {
            sum.add(Bucket::from_part(part));
            sum
        });
        if snapshot.is_blank() {
            continue;
        }
        let key = turn_key(turn, usage);
        let previous = accounted.get(&key).copied().unwrap_or_default();
        let delta = snapshot.saturating_delta(previous);
        if delta.is_blank() {
            continue;
        }
        if previous.is_blank() {
            for part in parts {
                out.push(record_from_part(session, ts, &part));
            }
        } else {
            out.push(record_from_part(
                session,
                ts,
                &TurnPart::from_bucket(delta, session.meta.model.clone()),
            ));
        }
        accounted.insert(key, snapshot);
    }

    cursor.offset = update.new_offset;
    cursor.size = update.size;
    cursor.mtime_ms = jsonl_util::file_mtime_ms(&session.updates);
    ctx.cursors.insert(key, cursor);
    Ok(())
}

struct TurnPart {
    model: Option<String>,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    reasoning: u64,
    calls: u64,
    cost: f64,
}

impl TurnPart {
    fn from_bucket(bucket: Bucket, model: Option<String>) -> Self {
        Self {
            model,
            input: bucket.input,
            output: bucket.output,
            cache_read: bucket.cache_read,
            cache_write: bucket.cache_write,
            reasoning: bucket.reasoning,
            calls: bucket.calls,
            cost: bucket.cost,
        }
    }

    fn add_bucket(&mut self, bucket: Bucket) {
        self.input = self.input.saturating_add(bucket.input);
        self.output = self.output.saturating_add(bucket.output);
        self.cache_read = self.cache_read.saturating_add(bucket.cache_read);
        self.cache_write = self.cache_write.saturating_add(bucket.cache_write);
        self.reasoning = self.reasoning.saturating_add(bucket.reasoning);
        self.calls = self.calls.saturating_add(bucket.calls);
        self.cost += bucket.cost;
    }
}

/// 已经入账的一轮快照。后来的同 `prompt_id` 只补差额。
#[derive(Clone, Copy, Default)]
struct Bucket {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    reasoning: u64,
    calls: u64,
    cost: f64,
}

impl Bucket {
    fn from_part(part: &TurnPart) -> Self {
        Self {
            input: part.input,
            output: part.output,
            cache_read: part.cache_read,
            cache_write: part.cache_write,
            reasoning: part.reasoning,
            calls: part.calls,
            cost: part.cost,
        }
    }

    fn add(&mut self, other: Self) {
        self.input = self.input.saturating_add(other.input);
        self.output = self.output.saturating_add(other.output);
        self.cache_read = self.cache_read.saturating_add(other.cache_read);
        self.cache_write = self.cache_write.saturating_add(other.cache_write);
        self.reasoning = self.reasoning.saturating_add(other.reasoning);
        self.calls = self.calls.saturating_add(other.calls);
        self.cost += other.cost;
    }

    fn saturating_delta(self, older: Self) -> Self {
        Self {
            input: self.input.saturating_sub(older.input),
            output: self.output.saturating_sub(older.output),
            cache_read: self.cache_read.saturating_sub(older.cache_read),
            cache_write: self.cache_write.saturating_sub(older.cache_write),
            reasoning: self.reasoning.saturating_sub(older.reasoning),
            calls: self.calls.saturating_sub(older.calls),
            cost: (self.cost - older.cost).max(0.0),
        }
    }

    fn total_tokens(self) -> u64 {
        self.input
            .saturating_add(self.output)
            .saturating_add(self.cache_read)
            .saturating_add(self.cache_write)
    }

    fn is_blank(self) -> bool {
        self.total_tokens() == 0 && self.cost <= 1e-9
    }
}

fn record_from_part(session: &SessionFile, ts: i64, part: &TurnPart) -> UsageRecord {
    UsageRecord {
        agent: AGENT.into(),
        session_id: Some(session.session_id.clone()),
        project: session.meta.project.clone(),
        model: part.model.clone(),
        provider: Some("xai".into()),
        title: session.meta.title.clone(),
        ts,
        input_tokens: part.input,
        output_tokens: part.output,
        cache_read_tokens: part.cache_read,
        cache_write_tokens: part.cache_write,
        reasoning_tokens: part.reasoning,
        calls: part.calls,
        cost: part.cost,
        ..Default::default()
    }
}

fn turn_key(turn: &Value, usage: &Value) -> String {
    turn.get("prompt_id")
        .or_else(|| turn.get("promptId"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("usage:{}", usage))
}

/// 一轮的账单。`modelUsage` 是拆分不是另一笔;拆分对不上父级时补或丢,避免漏计和重计。
fn split_usage(usage: &Value, fallback_model: Option<&str>) -> Vec<TurnPart> {
    let Some(parent) = part_from(usage, fallback_model.map(str::to_string)) else {
        return Vec::new();
    };
    let Some(models) = usage
        .get("modelUsage")
        .and_then(Value::as_object)
        .filter(|models| !models.is_empty())
    else {
        return vec![parent];
    };

    let mut names: Vec<&String> = models.keys().collect();
    names.sort();
    let mut parts = Vec::new();
    for name in names {
        if let Some(part) = part_from(&models[name], Some(name.clone())) {
            parts.push(part);
        }
    }
    if parts.is_empty() {
        return vec![parent];
    }

    let parent_bucket = Bucket::from_part(&parent);
    let attributed = parts.iter().fold(Bucket::default(), |mut sum, part| {
        sum.add(Bucket::from_part(part));
        sum
    });
    // 模型行把父级整笔又抄了一遍:再留下就会翻倍,只信父级这一份。
    if attributed.total_tokens() > parent_bucket.total_tokens() {
        return vec![parent];
    }
    if parent.cost > 0.0 {
        let child_cost: f64 = parts.iter().map(|part| part.cost).sum();
        if child_cost > parent.cost + 1e-6 {
            for part in &mut parts {
                part.cost = 0.0;
            }
            if let Some(first) = parts.first_mut() {
                first.cost = parent.cost;
            }
        }
    }

    let residual = parent_bucket.saturating_delta(parts.iter().fold(Bucket::default(), |mut sum, part| {
        sum.add(Bucket::from_part(part));
        sum
    }));
    if !residual.is_blank() {
        let model = fallback_model
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(str::to_string);
        if let Some(slot) = parts.iter_mut().find(|part| part.model == model) {
            slot.add_bucket(residual);
        } else if parts.len() == 1 {
            parts[0].add_bucket(residual);
        } else {
            parts.push(TurnPart::from_bucket(residual, model));
        }
    }
    parts
}

fn part_from(entry: &Value, model: Option<String>) -> Option<TurnPart> {
    let raw_input = first_u64(entry, &["inputTokens", "input_tokens"]);
    let output = first_u64(entry, &["outputTokens", "output_tokens"]);
    let (cache_read, cache_write, input_is_uncached) = cache_buckets(entry, raw_input, output);
    let input = if input_is_uncached {
        raw_input
    } else {
        raw_input
            .saturating_sub(cache_read)
            .saturating_sub(cache_write)
    };
    let cost = cost_usd(entry).unwrap_or(0.0);
    if input + output + cache_read + cache_write == 0 && cost == 0.0 {
        return None;
    }
    let calls = first_u64(entry, &["modelCalls", "model_calls"]);
    Some(TurnPart {
        model: model.filter(|name| !name.is_empty()),
        input,
        output,
        cache_read,
        cache_write,
        reasoning: first_u64(entry, &["reasoningTokens", "reasoning_tokens"]),
        calls: if calls > 0 { calls } else { 1 },
        cost,
    })
}

/// 缓存已经含在 input 里,还是单独一栏。
///
/// `updates.jsonl` 的 ACP 字段是前者。headless 投影和带 `totalTokens` 的分开栏是后者。
/// 判错的后果刚好相反:该减不减会把缓存算两遍,不该减却减会把未缓存输入减没。
fn cache_buckets(entry: &Value, raw_input: u64, raw_output: u64) -> (u64, u64, bool) {
    let subset_read = u64_field(entry, "cachedReadTokens");
    let subset_write = if entry.get("cacheCreationTokens").is_some() {
        u64_field(entry, "cacheCreationTokens")
    } else {
        u64_field(entry, "cacheWriteTokens")
    };
    let disjoint_read = optional_u64(entry, &["cacheReadInputTokens", "cache_read_input_tokens"]);
    let disjoint_write = optional_u64(
        entry,
        &["cacheCreationInputTokens", "cache_creation_input_tokens"],
    );
    if (disjoint_read.is_some() || disjoint_write.is_some())
        && entry.get("cachedReadTokens").is_none()
        && entry.get("cacheCreationTokens").is_none()
    {
        return (
            disjoint_read.unwrap_or(0),
            disjoint_write.unwrap_or(0),
            true,
        );
    }

    let cache_read = subset_read;
    let cache_write = subset_write;
    let cache = cache_read.saturating_add(cache_write);
    if cache > raw_input && cache > 0 {
        return (cache_read, cache_write, true);
    }
    let Some(total) = optional_u64(entry, &["totalTokens", "total_tokens"]) else {
        return (cache_read, cache_write, false);
    };
    let included = raw_input.saturating_add(raw_output);
    let split = included.saturating_add(cache);
    if split == included {
        return (cache_read, cache_write, false);
    }
    let closer_to_split = abs_diff(total, split) < abs_diff(total, included);
    (cache_read, cache_write, closer_to_split)
}

fn u64_field(v: &Value, key: &str) -> u64 {
    v.get(key).map(as_u64).unwrap_or(0)
}

fn as_u64(v: &Value) -> u64 {
    match v {
        Value::Number(n) => n.as_u64().unwrap_or_else(|| {
            n.as_f64()
                .filter(|f| f.is_finite() && *f > 0.0)
                .map(|f| f as u64)
                .unwrap_or(0)
        }),
        Value::String(s) => s
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|f| f.is_finite() && *f > 0.0)
            .map(|f| f as u64)
            .unwrap_or(0),
        _ => 0,
    }
}

fn cost_usd(v: &Value) -> Option<f64> {
    let ticks = match v.get("costUsdTicks")? {
        Value::Number(n) => n
            .as_f64()
            .or_else(|| n.as_u64().map(|u| u as f64))
            .or_else(|| n.as_i64().map(|i| i as f64))?,
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        _ => return None,
    };
    if !ticks.is_finite() || ticks < 0.0 {
        return None;
    }
    Some(ticks / TICKS_PER_USD)
}

/// 信封上的 `timestamp`。上游写的是 unix 秒,也认毫秒和 RFC3339。
fn epoch_ms(v: &Value) -> Option<i64> {
    let n = match v {
        Value::Number(n) => n.as_f64().or_else(|| n.as_i64().map(|i| i as f64))?,
        Value::String(s) => {
            let s = s.trim();
            if let Some(ms) = iso_to_ms(s) {
                return Some(ms);
            }
            s.parse::<f64>().ok()?
        }
        _ => return None,
    };
    if !n.is_finite() || n <= 0.0 {
        return None;
    }
    if n > 1e12 {
        Some(n as i64)
    } else {
        Some((n * 1000.0) as i64)
    }
}

fn first_u64(v: &Value, keys: &[&str]) -> u64 {
    optional_u64(v, keys).unwrap_or(0)
}

fn optional_u64(v: &Value, keys: &[&str]) -> Option<u64> {
    keys.iter().find_map(|key| v.get(*key).map(as_u64).filter(|_| v.get(*key).is_some()))
}

fn abs_diff(left: u64, right: u64) -> u64 {
    left.abs_diff(right)
}

fn event_ms(v: &Value) -> Option<i64> {
    let meta = v
        .pointer("/params/_meta/agentTimestampMs")
        .or_else(|| v.pointer("/params/_meta/agent_timestamp_ms"));
    if let Some(ms) = meta.and_then(epoch_ms) {
        return Some(ms);
    }
    v.get("timestamp").and_then(epoch_ms)
}

fn load_accounted(state: &Value) -> BTreeMap<String, Bucket> {
    let Some(obj) = state.get("accounted").and_then(Value::as_object) else {
        return BTreeMap::new();
    };
    obj.iter()
        .map(|(key, value)| (key.clone(), bucket_from_json(value)))
        .collect()
}

fn store_accounted(state: &mut Value, accounted: &BTreeMap<String, Bucket>) {
    if !state.is_object() {
        *state = serde_json::json!({});
    }
    let mut obj = serde_json::Map::new();
    for (key, bucket) in accounted {
        obj.insert(
            key.clone(),
            serde_json::json!({
                "input": bucket.input,
                "output": bucket.output,
                "cacheRead": bucket.cache_read,
                "cacheWrite": bucket.cache_write,
                "reasoning": bucket.reasoning,
                "calls": bucket.calls,
                "cost": bucket.cost,
            }),
        );
    }
    state["accounted"] = Value::Object(obj);
}

fn bucket_from_json(v: &Value) -> Bucket {
    Bucket {
        input: u64_field(v, "input"),
        output: u64_field(v, "output"),
        cache_read: u64_field(v, "cacheRead"),
        cache_write: u64_field(v, "cacheWrite"),
        reasoning: u64_field(v, "reasoning"),
        calls: u64_field(v, "calls"),
        cost: v.get("cost").and_then(Value::as_f64).unwrap_or(0.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::FileCursor;
    use std::collections::HashMap;
    use std::io::Write;

    fn temp_root(name: &str) -> PathBuf {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("otr-grok-{name}-{suffix}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_session(root: &Path, id: &str, summary: Option<&str>, updates: &str) {
        let dir = root.join("proj").join(id);
        fs::create_dir_all(&dir).unwrap();
        if let Some(summary) = summary {
            fs::write(dir.join("summary.json"), summary).unwrap();
        }
        let mut file = fs::File::create(dir.join("updates.jsonl")).unwrap();
        file.write_all(updates.as_bytes()).unwrap();
    }

    fn ctx<'a>(
        cursors: &'a mut HashMap<String, FileCursor>,
        state: &'a mut Value,
    ) -> ScanCtx<'a> {
        ScanCtx {
            full: true,
            force_full: false,
            cursors,
            state,
        }
    }

    fn turn(prompt: &str, usage: &str) -> String {
        format!(
            r#"{{"timestamp":1710000000,"method":"_x.ai/session/update","params":{{"update":{{"sessionUpdate":"turn_completed","prompt_id":"{prompt}","usage":{usage}}}}}}}"#
        )
    }

    #[test]
    fn completed_turn_splits_cache_out_of_input_and_prices_ticks() {
        let root = temp_root("split");
        let usage = r#"{"inputTokens":1000,"outputTokens":50,"cachedReadTokens":400,"cacheCreationTokens":100,"reasoningTokens":20,"modelCalls":3,"costUsdTicks":25000000000}"#;
        write_session(
            &root,
            "s1",
            Some(r#"{"generated_title":"修登录","current_model_id":"grok-4.5","info":{"cwd":"C:\\Code\\app"}}"#),
            &format!("{}\n", turn("p1", usage)),
        );
        let mut cursors = HashMap::new();
        let mut state = Value::Null;
        let recs = scan_root(&root, &mut ctx(&mut cursors, &mut state)).unwrap();
        assert_eq!(recs.len(), 1);
        let r = &recs[0];
        assert_eq!(r.input_tokens, 500, "未缓存输入要扣掉两种缓存");
        assert_eq!(r.output_tokens, 50);
        assert_eq!(r.cache_read_tokens, 400);
        assert_eq!(r.cache_write_tokens, 100);
        assert_eq!(r.reasoning_tokens, 20, "推理只单列,不能再加进输出");
        assert_eq!(r.total_tokens(), 1050);
        assert_eq!(r.calls, 3);
        assert!((r.cost - 2.5).abs() < 1e-9, "25000000000 ticks = $2.5, got {}", r.cost);
        assert_eq!(r.model.as_deref(), Some("grok-4.5"));
        assert_eq!(r.project.as_deref(), Some(r"C:\Code\app"));
        assert_eq!(r.title.as_deref(), Some("修登录"));
        assert_eq!(r.provider.as_deref(), Some("xai"));
        assert_eq!(r.ts, 1_710_000_000_000);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn model_usage_replaces_the_parent_total() {
        let root = temp_root("models");
        let usage = r#"{"inputTokens":1000,"outputTokens":50,"cachedReadTokens":400,"cacheCreationTokens":100,"costUsdTicks":25000000000,"modelUsage":{"grok-4.5":{"inputTokens":800,"outputTokens":40,"cachedReadTokens":300,"cacheCreationTokens":100,"reasoningTokens":15,"modelCalls":2,"costUsdTicks":20000000000},"grok-4":{"inputTokens":200,"outputTokens":10,"cachedReadTokens":100,"reasoningTokens":5,"modelCalls":1,"costUsdTicks":5000000000}}}"#;
        write_session(&root, "s1", None, &format!("{}\n", turn("p1", usage)));
        let mut cursors = HashMap::new();
        let mut state = Value::Null;
        let recs = scan_root(&root, &mut ctx(&mut cursors, &mut state)).unwrap();
        assert_eq!(recs.len(), 2, "父级总计不能再记一笔");
        let total: u64 = recs.iter().map(|r| r.total_tokens()).sum();
        assert_eq!(total, 1050);
        let cost: f64 = recs.iter().map(|r| r.cost).sum();
        assert!((cost - 2.5).abs() < 1e-9, "{cost}");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn subagent_and_fork_copies_are_not_counted_twice() {
        let root = temp_root("dedup");
        let usage = r#"{"inputTokens":10,"outputTokens":2,"modelCalls":1}"#;
        write_session(
            &root,
            "parent",
            Some(r#"{"generated_title":"父会话","info":{"cwd":"D:\\work"}}"#),
            &format!("{}\n", turn("p1", usage)),
        );
        write_session(
            &root,
            "child",
            Some(r#"{"session_kind":"subagent","info":{"cwd":"D:\\work"}}"#),
            &format!("{}\n", turn("p-sub", r#"{"inputTokens":999,"outputTokens":999}"#)),
        );
        write_session(
            &root,
            "fork",
            Some(r#"{"session_kind":"fork","parent_session_id":"parent","info":{"cwd":"D:\\elsewhere"}}"#),
            &format!(
                "{}\n{}\n",
                turn("p1", usage),
                turn("p2", r#"{"inputTokens":4,"outputTokens":1}"#)
            ),
        );
        // 同目录里的聊天记录不是用量来源
        let noise = root.join("proj").join("parent").join("chat_history.jsonl");
        fs::write(
            noise,
            format!("{}\n", turn("p-noise", r#"{"inputTokens":5000,"outputTokens":5000}"#)),
        )
        .unwrap();

        let mut cursors = HashMap::new();
        let mut state = Value::Null;
        let recs = scan_root(&root, &mut ctx(&mut cursors, &mut state)).unwrap();
        assert_eq!(recs.len(), 2, "子会话跳过,fork 里的旧 prompt 不重复,新 prompt 要留下");
        assert!(recs.iter().all(|r| r.session_id.as_deref() != Some("child")));
        let copied = recs.iter().find(|r| r.input_tokens == 10).unwrap();
        assert_eq!(copied.session_id.as_deref(), Some("parent"));
        assert_eq!(copied.project.as_deref(), Some(r"D:\work"));
        assert!(recs.iter().any(|r| r.input_tokens == 4 && r.session_id.as_deref() == Some("fork")));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn append_is_incremental_and_truncation_asks_for_rebuild() {
        let root = temp_root("append");
        let usage = r#"{"inputTokens":8,"outputTokens":1}"#;
        write_session(&root, "s1", None, &format!("{}\n", turn("p1", usage)));
        let mut cursors = HashMap::new();
        let mut state = Value::Null;
        let first = scan_root(&root, &mut ctx(&mut cursors, &mut state)).unwrap();
        assert_eq!(first.len(), 1);

        let path = root.join("proj").join("s1").join("updates.jsonl");
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file, "{}", turn("p2", r#"{"inputTokens":3,"outputTokens":1}"#)).unwrap();
        let second = scan_root(&root, &mut ctx(&mut cursors, &mut state)).unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].input_tokens, 3);

        fs::write(&path, format!("{}\n", turn("p9", usage))).unwrap();
        let mut again = ctx(&mut cursors, &mut state);
        let _ = scan_root(&root, &mut again).unwrap();
        assert!(again.force_full, "文件变短必须要求全量重建");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn partial_model_usage_keeps_the_parent_remainder_once() {
        let usage = serde_json::json!({
            "inputTokens": 1000,
            "outputTokens": 50,
            "cachedReadTokens": 400,
            "cacheCreationTokens": 100,
            "costUsdTicks": 25000000000u64,
            "modelUsage": {
                "grok-4.5": { "inputTokens": 200, "outputTokens": 10, "modelCalls": 1 }
            }
        });
        let parts = split_usage(&usage, Some("grok-4.5"));
        let total: u64 = parts.iter().map(|part| part.input + part.output + part.cache_read + part.cache_write).sum();
        let cost: f64 = parts.iter().map(|part| part.cost).sum();
        assert_eq!(total, 1050, "没写进模型行的部分要补上,且只能补一次");
        assert!((cost - 2.5).abs() < 1e-9, "{cost}");
    }

    #[test]
    fn copied_model_rows_do_not_multiply_the_parent() {
        let row = serde_json::json!({
            "inputTokens": 1000,
            "outputTokens": 50,
            "cachedReadTokens": 400,
            "cacheCreationTokens": 100,
            "costUsdTicks": 25000000000u64
        });
        let usage = serde_json::json!({
            "inputTokens": 1000,
            "outputTokens": 50,
            "cachedReadTokens": 400,
            "cacheCreationTokens": 100,
            "costUsdTicks": 25000000000u64,
            "modelUsage": { "grok-4.5": row.clone(), "grok-4": row }
        });
        let parts = split_usage(&usage, None);
        assert_eq!(parts.len(), 1, "模型行大于父级时只留父级");
        assert_eq!(parts[0].input + parts[0].output + parts[0].cache_read + parts[0].cache_write, 1050);
        assert!((parts[0].cost - 2.5).abs() < 1e-9);
    }

    #[test]
    fn cache_larger_than_input_is_a_separate_bucket() {
        let usage = serde_json::json!({
            "inputTokens": 7210,
            "outputTokens": 1893,
            "cachedReadTokens": 41000
        });
        let parts = split_usage(&usage, Some("grok-4.6"));
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].input, 7210, "缓存比 input 大时,input 已经是未缓存部分");
        assert_eq!(parts[0].cache_read, 41000);
        assert_eq!(parts[0].output, 1893);
    }

    #[test]
    fn repeated_snapshot_counts_the_increase_only() {
        let root = temp_root("snapshot");
        write_session(
            &root,
            "s1",
            None,
            &format!(
                "{}\n{}\n",
                turn("p1", r#"{"inputTokens":10,"outputTokens":1}"#),
                turn("p1", r#"{"inputTokens":10,"outputTokens":1}"#)
            ),
        );
        let mut cursors = HashMap::new();
        let mut state = Value::Null;
        let same = scan_root(&root, &mut ctx(&mut cursors, &mut state)).unwrap();
        assert_eq!(same.len(), 1);
        assert_eq!(same[0].input_tokens, 10);

        let path = root.join("proj").join("s1").join("updates.jsonl");
        let mut file = fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file, "{}", turn("p1", r#"{"inputTokens":40,"outputTokens":5}"#)).unwrap();
        let grown = scan_root(&root, &mut ctx(&mut cursors, &mut state)).unwrap();
        assert_eq!(grown.len(), 1, "后写的快照只补多出来的部分");
        assert_eq!(grown[0].input_tokens, 30);
        assert_eq!(grown[0].output_tokens, 4);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn broken_summary_is_skipped() {
        let root = temp_root("broken");
        write_session(
            &root,
            "s1",
            Some("not-json"),
            &format!("{}\n", turn("p1", r#"{"inputTokens":10,"outputTokens":1}"#)),
        );
        let mut cursors = HashMap::new();
        let mut state = Value::Null;
        let recs = scan_root(&root, &mut ctx(&mut cursors, &mut state)).unwrap();
        assert!(recs.is_empty());
        let _ = fs::remove_dir_all(root);
    }
}
