use std::path::PathBuf;

use serde_json::Value;

use crate::error::Result;
use crate::model::{iso_to_ms, now_ms, UsageRecord};
use crate::paths;
use crate::providers::jsonl_util::{self, u64f};
use crate::providers::{ScanCtx, AgentProvider};

pub struct CodexProvider;

const AGENT: &str = "codex";

impl AgentProvider for CodexProvider {
    fn id(&self) -> &str {
        AGENT
    }

    fn display_name(&self) -> &str {
        "Codex CLI"
    }

    fn detect(&self) -> bool {
        paths::codex_sessions().is_dir()
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        vec![paths::codex_sessions(), paths::codex_archived_sessions()]
    }

    fn scan(&self, ctx: &mut ScanCtx) -> Result<Vec<UsageRecord>> {
        scan_roots(
            &[paths::codex_sessions(), paths::codex_archived_sessions()],
            AGENT,
            ctx,
        )
    }
}

/// 扫描 "sessions/YYYY/MM/DD/rollout-*.jsonl" 布局的目录(自定义 Agent 可复用)
pub fn scan_roots(roots: &[PathBuf], agent: &str, ctx: &mut ScanCtx) -> Result<Vec<UsageRecord>> {
    let mut files = Vec::new();
    for root in roots {
        jsonl_util::collect_jsonl(root, 4, &mut files);
    }
    let mut records = Vec::new();
    for file in files {
        if let Err(e) = scan_file(&file, agent, ctx, &mut records) {
            eprintln!("[{}] {}: {}", agent, file.display(), e);
        }
    }
    Ok(records)
}

/// 逐事件解析:token_count 事件里的 `last_token_usage` 是该次请求的真实用量,
/// `total_token_usage` 是会话累计(且其 input_tokens 已包含 cached,不能当未缓存输入用,
/// 也不能拿累计差做日切分——跨天会话会把全部历史记到最后活跃日)。
/// 每个事件产出一条增量记录,时间取事件时间戳,按天分布即精确。
fn scan_file(
    path: &PathBuf,
    agent: &str,
    ctx: &mut ScanCtx,
    out: &mut Vec<UsageRecord>,
) -> Result<()> {
    let key = path.to_string_lossy().to_string();
    let mut cursor = ctx.cursors.get(&key).cloned().unwrap_or_default();
    let Some(update) = jsonl_util::read_appended(path, cursor.offset)? else {
        return Ok(());
    };

    let mut session_id: Option<String> = None;
    let mut project: Option<String> = None;
    let mut model: Option<String> = None;
    let calls_prev = cursor
        .extra
        .get("calls")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let mut new_calls = 0u64;
    let mut last_ts: i64 = cursor
        .extra
        .get("ts")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);

    for line in &update.lines {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let top_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let payload = v.get("payload").cloned().unwrap_or(Value::Null);
        let ptype = payload
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or(top_type);

        match ptype {
            "session_meta" => {
                if session_id.is_none() {
                    session_id = payload
                        .get("session_id")
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string());
                }
                if project.is_none() {
                    project = payload
                        .get("cwd")
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string());
                }
                if let Some(ts) = payload.get("timestamp").and_then(|x| x.as_str()) {
                    if let Some(ms) = iso_to_ms(ts) {
                        if last_ts == 0 {
                            last_ts = ms;
                        }
                    }
                }
                if let Some(m) = payload.get("model").and_then(|x| x.as_str()) {
                    model = Some(m.to_string());
                }
            }
            "token_count" => {
                let info = payload.get("info").unwrap_or(&payload);
                // 会话结束时 codex 会补一个 info 为空的 token_count 事件,跳过
                let Some(lu) = info.get("last_token_usage") else {
                    continue;
                };
                if !lu.is_object() {
                    continue;
                }
                let in_total = u64f(lu, "input_tokens");
                let cached = u64f(lu, "cached_input_tokens");
                let r = UsageRecord {
                    agent: agent.into(),
                    session_id: session_id.clone(),
                    project: project.clone(),
                    model: model.clone(),
                    ts: v
                        .get("timestamp")
                        .and_then(|x| x.as_str())
                        .and_then(iso_to_ms)
                        .unwrap_or_else(now_ms),
                    input_tokens: in_total.saturating_sub(cached),
                    output_tokens: u64f(lu, "output_tokens"),
                    cache_read_tokens: cached,
                    cache_write_tokens: u64f(lu, "cache_write_input_tokens"),
                    reasoning_tokens: u64f(lu, "reasoning_output_tokens"),
                    calls: 1,
                    ..Default::default()
                };
                if r.total_tokens() > 0 {
                    out.push(r);
                    new_calls += 1;
                }
            }
            _ => {
                // turn_context 等事件里可能带模型名,尽力抓取
                if model.is_none() {
                    if let Some(m) = payload.get("model").and_then(|x| x.as_str()) {
                        if !m.is_empty() {
                            model = Some(m.to_string());
                        }
                    }
                }
            }
        }
    }

    cursor.offset = update.new_offset;
    cursor.size = update.size;
    cursor.mtime_ms = jsonl_util::file_mtime_ms(path);
    cursor.extra = serde_json::json!({ "calls": calls_prev + new_calls, "ts": last_ts });
    ctx.cursors.insert(key, cursor);
    Ok(())
}
