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

/// 解析语义版本;变更时启动会全量重建。
/// v1: 去重窗口跨扫描持久化 + 截断转全量重建。
pub const PARSER_VERSION: u64 = 1;

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

    fn parser_version(&self) -> u64 {
        PARSER_VERSION
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
    if update.truncated {
        // 文件被截断/重写,已入库的贡献无法精确扣减 → 让编排层整 Agent 重建
        ctx.force_full = true;
        return Ok(());
    }
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
    // 去重窗口跨扫描持久化:重复行常分落在两次扫描之间
    let mut order = jsonl_util::load_seen(&cursor);
    let mut seen: HashSet<String> = order.iter().cloned().collect();

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
        if !msg_id.is_empty() && seen.contains(&dedup) {
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
        // 只在真正落记录时占用去重 key
        if !msg_id.is_empty() {
            seen.insert(dedup.clone());
            order.push(dedup);
        }
        out.push(r);
    }
    cursor.offset = update.new_offset;
    cursor.size = update.size;
    cursor.mtime_ms = jsonl_util::file_mtime_ms(path);
    jsonl_util::save_seen(&mut cursor, &order);
    ctx.cursors.insert(key, cursor);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{FileCursor, ScanCtx};
    use std::collections::HashMap;
    use std::io::Write;

    fn temp_file(name: &str) -> std::path::PathBuf {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("otr-pi-{name}-{suffix}.jsonl"))
    }

    fn line(id: &str, input: u64) -> String {
        format!(
            r#"{{"type":"message","id":"{id}","timestamp":"2026-09-01T10:00:00.000Z","message":{{"model":"m","provider":"p","usage":{{"input":{input},"output":1,"cacheRead":0,"cacheWrite":0,"cost":{{"total":0.01}}}}}}}}"#
        )
    }

    #[test]
    fn duplicate_message_across_scans_is_counted_once() {
        let file = temp_file("dup");
        std::fs::write(&file, format!("{}\n", line("m1", 10))).unwrap();

        let mut cursors: HashMap<String, FileCursor> = HashMap::new();
        let mut out = Vec::new();
        {
            let mut state = serde_json::Value::Null;
            let mut c = ScanCtx {
                full: false,
                force_full: false,
                cursors: &mut cursors,
                state: &mut state,
            };
            scan_file(&file, &mut c, &mut out).unwrap();
        }
        assert_eq!(out.len(), 1);

        let mut f = std::fs::OpenOptions::new().append(true).open(&file).unwrap();
        writeln!(f, "{}", line("m1", 10)).unwrap();
        writeln!(f, "{}", line("m2", 20)).unwrap();
        drop(f);

        let mut out2 = Vec::new();
        {
            let mut state = serde_json::Value::Null;
            let mut c = ScanCtx {
                full: false,
                force_full: false,
                cursors: &mut cursors,
                state: &mut state,
            };
            scan_file(&file, &mut c, &mut out2).unwrap();
        }
        assert_eq!(out2.len(), 1, "重复行必须跨扫描去重,只剩新消息 m2");
        assert_eq!(out2[0].input_tokens, 20);
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn truncated_file_requests_full_rebuild() {
        let file = temp_file("trunc");
        std::fs::write(&file, format!("{}\n{}\n", line("m1", 10), line("m2", 20))).unwrap();

        let mut cursors: HashMap<String, FileCursor> = HashMap::new();
        let mut out = Vec::new();
        {
            let mut state = serde_json::Value::Null;
            let mut c = ScanCtx {
                full: false,
                force_full: false,
                cursors: &mut cursors,
                state: &mut state,
            };
            scan_file(&file, &mut c, &mut out).unwrap();
            assert!(!c.force_full);
        }

        std::fs::write(&file, format!("{}\n", line("m1", 10))).unwrap();
        let mut out2 = Vec::new();
        let mut state = serde_json::Value::Null;
        let mut c = ScanCtx {
            full: false,
            force_full: false,
            cursors: &mut cursors,
            state: &mut state,
        };
        scan_file(&file, &mut c, &mut out2).unwrap();
        assert!(c.force_full, "截断必须请求全量重建");
        assert!(out2.is_empty(), "截断时不得输出增量记录");
        let _ = std::fs::remove_file(file);
    }
}
