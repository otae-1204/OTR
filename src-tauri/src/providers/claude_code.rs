use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::Result;
use crate::model::{iso_to_ms, UsageRecord};
use crate::paths;
use crate::providers::jsonl_util::{self, u64f};
use crate::providers::{AgentProvider, ScanCtx};

pub struct ClaudeCodeProvider;

const AGENT: &str = "claude-code";

impl AgentProvider for ClaudeCodeProvider {
    fn id(&self) -> &str {
        AGENT
    }

    fn display_name(&self) -> &str {
        "Claude Code"
    }

    fn detect(&self) -> bool {
        paths::claude_projects().is_dir()
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        vec![paths::claude_projects()]
    }

    fn scan(&self, ctx: &mut ScanCtx) -> Result<Vec<UsageRecord>> {
        scan_root(&paths::claude_projects(), AGENT, ctx)
    }
}

/// 扫描 "projects/<编码路径>/<session>.jsonl" 布局的目录(自定义 Agent 可复用)
pub fn scan_root(root: &Path, agent: &str, ctx: &mut ScanCtx) -> Result<Vec<UsageRecord>> {
    if !root.is_dir() {
        return Ok(vec![]);
    }
    let mut records = Vec::new();
    for entry in fs::read_dir(root)? {
        let proj_dir = entry?.path();
        if !proj_dir.is_dir() {
            continue;
        }
        let proj_name = proj_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        for file in fs::read_dir(&proj_dir)? {
            let file = file?.path();
            if file.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            if let Err(e) = scan_file(&file, &proj_name, agent, ctx, &mut records) {
                eprintln!("[{}] {}: {}", agent, file.display(), e);
            }
        }
    }
    Ok(records)
}

/// 流式写盘会对同一条 assistant 消息追加重复行,按 (message.id, requestId) 去重
fn scan_file(
    path: &Path,
    project_dir: &str,
    agent: &str,
    ctx: &mut ScanCtx,
    out: &mut Vec<UsageRecord>,
) -> Result<()> {
    let key = path.to_string_lossy().to_string();
    let mut cursor = ctx.cursors.get(&key).cloned().unwrap_or_default();
    let Some(update) = jsonl_util::read_appended(path, cursor.offset)? else {
        return Ok(());
    };
    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string());
    let project = decode_project(project_dir);
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for line in &update.lines {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let Some(msg) = v.get("message") else {
            continue;
        };
        let Some(usage) = msg.get("usage") else {
            continue;
        };
        let msg_id = msg
            .get("id")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let req_id = v
            .get("requestId")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        if !msg_id.is_empty() && !seen.insert((msg_id, req_id)) {
            continue;
        }
        let model = msg
            .get("model")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        if model.is_empty() || model.starts_with('<') {
            continue;
        }
        let ts = v
            .get("timestamp")
            .and_then(|x| x.as_str())
            .and_then(iso_to_ms)
            .unwrap_or(0);
        let r = UsageRecord {
            agent: agent.into(),
            session_id: session_id.clone(),
            project: Some(project.clone()),
            model: Some(model),
            ts,
            input_tokens: u64f(usage, "input_tokens"),
            output_tokens: u64f(usage, "output_tokens"),
            cache_read_tokens: u64f(usage, "cache_read_input_tokens"),
            cache_write_tokens: u64f(usage, "cache_creation_input_tokens"),
            calls: 1,
            ..Default::default()
        };
        if r.total_tokens() == 0 {
            continue;
        }
        out.push(r);
    }
    cursor.offset = update.new_offset;
    cursor.size = update.size;
    cursor.mtime_ms = jsonl_util::file_mtime_ms(path);
    ctx.cursors.insert(key, cursor);
    Ok(())
}

/// 目录名是路径的编码形式("C--Users-otae" -> "C:\Users\otae");含连字符的真实路径无法无损还原,尽力而为
fn decode_project(encoded: &str) -> String {
    let b = encoded.as_bytes();
    if b.len() > 2 && b[1] == b'-' && b[2] == b'-' {
        format!("{}:\\{}", &encoded[..1], encoded[3..].replace('-', "\\"))
    } else {
        encoded.replace('-', "\\")
    }
}
