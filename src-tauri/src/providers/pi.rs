use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::Result;
use crate::model::{iso_to_ms, UsageRecord};
use crate::paths;
use crate::providers::jsonl_util::{self, u64f};
use crate::providers::{AgentProvider, ScanCtx};

pub struct PiProvider;

const AGENT: &str = "pi";

impl AgentProvider for PiProvider {
    fn id(&self) -> &str {
        AGENT
    }

    fn display_name(&self) -> &str {
        "Pi"
    }

    fn detect(&self) -> bool {
        paths::pi_sessions().is_dir()
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        vec![paths::pi_sessions()]
    }

    fn scan(&self, ctx: &mut ScanCtx) -> Result<Vec<UsageRecord>> {
        let root = paths::pi_sessions();
        let mut files = Vec::new();
        jsonl_util::collect_jsonl(&root, 2, &mut files);
        let mut records = Vec::new();
        for file in files {
            if let Err(e) = scan_file(&file, ctx, &mut records) {
                eprintln!("[{}] {}: {}", AGENT, file.display(), e);
            }
        }
        Ok(records)
    }
}

/// Pi(badlogic/pi-mono)会话:<编码cwd>/<时间戳>_<uuid>.jsonl,
/// 每行 {type:"message", id, timestamp, message:{model, provider, usage:{input, output,
/// cacheRead, cacheWrite, cost:{total}}}};usage.input 为未缓存输入,自带美元成本
fn scan_file(path: &Path, ctx: &mut ScanCtx, out: &mut Vec<UsageRecord>) -> Result<()> {
    let key = path.to_string_lossy().to_string();
    let mut cursor = ctx.cursors.get(&key).cloned().unwrap_or_default();
    let Some(update) = jsonl_util::read_appended(path, cursor.offset)? else {
        return Ok(());
    };
    let file_session = path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string());
    // 目录名形如 "--C--Code-qqbot-bot-entari--":首段是盘符,其余按 -- 分隔尽力反解
    let project = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(|dir| {
            let trimmed = dir.trim_matches('-');
            let parts: Vec<&str> = trimmed.split("--").collect();
            match parts.as_slice() {
                [drive, rest @ ..] => format!("{}:\\{}", drive, rest.join("\\")),
                _ => trimmed.to_string(),
            }
        });
    let mut seen: HashSet<String> = HashSet::new();

    for line in &update.lines {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(usage) = v.pointer("/message/usage") else {
            continue;
        };
        let msg_id = v
            .get("id")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        let dedup = format!("{}|{}", key, msg_id);
        if !msg_id.is_empty() && !seen.insert(dedup) {
            continue;
        }
        let input = u64f(usage, "input");
        let output = u64f(usage, "output");
        let cache_read = u64f(usage, "cacheRead");
        let cache_write = u64f(usage, "cacheWrite");
        if input + output + cache_read + cache_write == 0 {
            continue;
        }
        let ts = v
            .get("timestamp")
            .and_then(|x| x.as_str())
            .and_then(iso_to_ms)
            .unwrap_or(0);
        let r = UsageRecord {
            agent: AGENT.into(),
            session_id: file_session.clone(),
            project: project.clone(),
            model: v
                .pointer("/message/model")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string()),
            provider: v
                .pointer("/message/provider")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string()),
            title: None,
            ts,
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cache_read,
            cache_write_tokens: cache_write,
            calls: 1,
            cost: usage
                .pointer("/cost/total")
                .and_then(|x| x.as_f64())
                .unwrap_or(0.0),
            ..Default::default()
        };
        out.push(r);
    }
    cursor.offset = update.new_offset;
    cursor.size = update.size;
    cursor.mtime_ms = jsonl_util::file_mtime_ms(path);
    ctx.cursors.insert(key, cursor);
    Ok(())
}
