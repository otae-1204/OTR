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

/// 解析语义版本;变更时启动会全量重建。
/// v1: (message.id, requestId) 去重窗口跨扫描持久化 + 截断转全量重建。
pub const PARSER_VERSION: u64 = 1;

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

    fn parser_version(&self) -> u64 {
        PARSER_VERSION
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
    if update.truncated {
        // 文件被截断/重写,已入库的贡献无法精确扣减 → 让编排层整 Agent 重建
        ctx.force_full = true;
        return Ok(());
    }
    let session_id = path
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string());
    let project = decode_project(project_dir);
    // 去重窗口跨扫描持久化:重复行常分落在两次扫描之间
    let mut order = jsonl_util::load_seen(&cursor);
    let mut seen: HashSet<String> = order.iter().cloned().collect();
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
        let dedup_key = format!("{msg_id}|{req_id}");
        if !msg_id.is_empty() && seen.contains(&dedup_key) {
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
        // 只在真正落记录时占用去重 key,避免被过滤的行把 key 吃掉
        if !msg_id.is_empty() {
            seen.insert(dedup_key.clone());
            order.push(dedup_key);
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

/// 目录名是路径的编码形式("C--Users-otae" -> "C:\Users\otae");含连字符的真实路径无法无损还原,尽力而为
fn decode_project(encoded: &str) -> String {
    let b = encoded.as_bytes();
    if b.len() > 2 && b[1] == b'-' && b[2] == b'-' {
        format!("{}:\\{}", &encoded[..1], encoded[3..].replace('-', "\\"))
    } else {
        encoded.replace('-', "\\")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{FileCursor, ScanCtx};
    use std::collections::HashMap;
    use std::io::Write;

    fn temp_root(name: &str) -> std::path::PathBuf {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("otr-claude-{name}-{suffix}"));
        std::fs::create_dir_all(dir.join("proj")).unwrap();
        dir
    }

    fn line(msg_id: &str, req: &str, input: u64) -> String {
        format!(
            r#"{{"type":"assistant","requestId":"{req}","timestamp":"2026-09-01T10:00:00.000Z","message":{{"id":"{msg_id}","model":"claude-opus-5","usage":{{"input_tokens":{input},"output_tokens":1,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}"#
        )
    }

    fn ctx<'a>(
        cursors: &'a mut HashMap<String, FileCursor>,
        state: &'a mut serde_json::Value,
    ) -> ScanCtx<'a> {
        ScanCtx {
            full: false,
            force_full: false,
            cursors,
            state,
        }
    }

    #[test]
    fn duplicate_message_across_scans_is_counted_once() {
        let root = temp_root("dup");
        let file = root.join("proj").join("s1.jsonl");
        std::fs::write(&file, format!("{}\n", line("m1", "r1", 10))).unwrap();

        let mut cursors: HashMap<String, FileCursor> = HashMap::new();
        let mut state = serde_json::Value::Null;
        let first = {
            let mut c = ctx(&mut cursors, &mut state);
            scan_root(&root, AGENT, &mut c).unwrap()
        };
        assert_eq!(first.len(), 1);

        // 第二次扫描前追加:同一条消息的重复行 + 一条新消息
        let mut f = std::fs::OpenOptions::new().append(true).open(&file).unwrap();
        writeln!(f, "{}", line("m1", "r1", 10)).unwrap();
        writeln!(f, "{}", line("m2", "r2", 20)).unwrap();
        drop(f);

        let mut state2 = serde_json::Value::Null;
        let second = {
            let mut c = ctx(&mut cursors, &mut state2);
            scan_root(&root, AGENT, &mut c).unwrap()
        };
        assert_eq!(second.len(), 1, "重复行必须跨扫描去重,只剩新消息 m2");
        assert_eq!(second[0].input_tokens, 20);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn truncated_file_requests_full_rebuild() {
        let root = temp_root("trunc");
        let file = root.join("proj").join("s1.jsonl");
        std::fs::write(
            &file,
            format!("{}\n{}\n", line("m1", "r1", 10), line("m2", "r2", 20)),
        )
        .unwrap();

        let mut cursors: HashMap<String, FileCursor> = HashMap::new();
        let mut state = serde_json::Value::Null;
        {
            let mut c = ctx(&mut cursors, &mut state);
            assert_eq!(scan_root(&root, AGENT, &mut c).unwrap().len(), 2);
            assert!(!c.force_full);
        }

        // 截断重写为一行
        std::fs::write(&file, format!("{}\n", line("m1", "r1", 10))).unwrap();
        let mut state2 = serde_json::Value::Null;
        let mut c = ctx(&mut cursors, &mut state2);
        let recs = scan_root(&root, AGENT, &mut c).unwrap();
        assert!(c.force_full, "截断必须请求全量重建");
        assert!(recs.is_empty(), "截断时不得输出增量记录");
        let _ = std::fs::remove_dir_all(root);
    }
}
