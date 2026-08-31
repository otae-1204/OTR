use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::Result;
use crate::model::{iso_to_ms, now_ms, UsageRecord};
use crate::paths;
use crate::providers::jsonl_util::{self, u64f};
use crate::providers::{AgentProvider, ScanCtx};

pub struct CodexProvider;

const AGENT: &str = "codex";
pub const PARSER_VERSION: u64 = 1;

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
    // codex resume 会在新 rollout 文件里重放整个会话历史,同一 thread 的旧副本
    // 若一起统计就会双计(CC-Switch 同款处理):按文件名尾部的 thread uuid 分组,
    // 每组只保留文件名最新(时间戳即字典序最大)的一份。
    let mut groups: std::collections::HashMap<String, PathBuf> = std::collections::HashMap::new();
    for f in files {
        match thread_id_from_filename(&f) {
            Some(tid) => {
                let e = groups.entry(tid).or_insert_with(|| f.clone());
                if f > *e {
                    *e = f;
                }
            }
            // 文件名不符合 rollout 命名规则的,原样保留单独处理
            None => {
                groups.insert(f.to_string_lossy().to_string(), f);
            }
        }
    }
    let selected: Vec<PathBuf> = groups.into_values().collect();
    let mut records = Vec::new();
    for file in selected {
        // 子代理/派生会话(session_meta 带 parent_thread_id)的 rollout 会重放
        // 父线程的全部 token_count 历史,官方 /status 与 CC-Switch 均不计入,跳过
        if let Some((_, Some(parent))) = session_meta_thread_info(&file) {
            eprintln!("[{}] 跳过派生会话(父线程 {})", agent, parent);
            continue;
        }
        if let Err(e) = scan_file(&file, agent, ctx, &mut records) {
            eprintln!("[{}] {}: {}", agent, file.display(), e);
        }
    }
    Ok(records)
}

/// 读 session_meta(文件头部),返回 (自身 thread_id, parent_thread_id)
fn session_meta_thread_info(path: &Path) -> Option<(Option<String>, Option<String>)> {
    use std::fs::File;
    use std::io::{BufRead, BufReader};
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);
    for line in reader.lines() {
        let Ok(line) = line else { continue };
        if !line.contains("\"session_meta\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let payload = v.get("payload").unwrap_or(&Value::Null);
        let thread_id = payload
            .get("id")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        let parent = payload
            .get("parent_thread_id")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        return Some((thread_id, parent));
    }
    None
}

/// rollout-<时间戳>-<uuid>.jsonl → uuid(末 5 段);与 CC-Switch 的 thread_id 提取一致
fn thread_id_from_filename(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let stem = stem.strip_prefix("rollout-")?;
    let parts: Vec<&str> = stem.split('-').collect();
    if parts.len() < 5 {
        return None;
    }
    Some(parts[parts.len() - 5..].join("-"))
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
    let previous_offset = cursor.offset;
    let Some(update) = jsonl_util::read_appended(path, cursor.offset)? else {
        return Ok(());
    };

    if update.new_offset < previous_offset {
        cursor.extra = serde_json::Value::Null;
    }

    let mut session_id = cursor_string(&cursor, "session_id");
    let mut project = cursor_string(&cursor, "project");
    let mut model = cursor_string(&cursor, "model");
    let calls_prev = cursor
        .extra
        .get("calls")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let mut new_calls = 0u64;
    let mut last_ts: i64 = cursor.extra.get("ts").and_then(|v| v.as_i64()).unwrap_or(0);

    if cursor.offset > 0 && (session_id.is_none() || project.is_none() || model.is_none()) {
        recover_context(
            path,
            cursor.offset,
            &mut session_id,
            &mut project,
            &mut model,
        );
    }

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
        update_model(&payload, &mut model);

        match ptype {
            "session_meta" => {
                if session_id.is_none() {
                    session_id = payload
                        .get("session_id")
                        .or_else(|| payload.get("id"))
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
            _ => {}
        }
    }

    cursor.offset = update.new_offset;
    cursor.size = update.size;
    cursor.mtime_ms = jsonl_util::file_mtime_ms(path);
    cursor.extra = serde_json::json!({
        "version": PARSER_VERSION,
        "calls": calls_prev + new_calls,
        "ts": last_ts,
        "session_id": session_id,
        "project": project,
        "model": model,
    });
    ctx.cursors.insert(key, cursor);
    Ok(())
}

fn cursor_string(cursor: &crate::providers::FileCursor, key: &str) -> Option<String> {
    cursor
        .extra
        .get(key)
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn update_model(payload: &Value, model: &mut Option<String>) {
    if let Some(value) = payload.get("model").and_then(|value| value.as_str()) {
        if !value.is_empty() {
            *model = Some(value.to_owned());
        }
    }
}

fn recover_context(
    path: &Path,
    offset: u64,
    session_id: &mut Option<String>,
    project: &mut Option<String>,
    model: &mut Option<String>,
) {
    let Ok(file) = File::open(path) else {
        return;
    };
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut read = 0u64;
    while reader
        .read_until(b'\n', &mut line)
        .ok()
        .filter(|size| *size > 0)
        .is_some()
    {
        let line_size = line.len() as u64;
        if read.saturating_add(line_size) > offset {
            break;
        }
        read = read.saturating_add(line_size);
        let text = String::from_utf8_lossy(&line);
        let Ok(value) = serde_json::from_str::<Value>(text.trim()) else {
            line.clear();
            continue;
        };
        let top_type = value
            .get("type")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let payload = value.get("payload").cloned().unwrap_or(Value::Null);
        let payload_type = payload
            .get("type")
            .and_then(|value| value.as_str())
            .unwrap_or(top_type);
        if payload_type == "session_meta" {
            if session_id.is_none() {
                *session_id = payload
                    .get("session_id")
                    .or_else(|| payload.get("id"))
                    .and_then(|value| value.as_str())
                    .map(ToOwned::to_owned);
            }
            if project.is_none() {
                *project = payload
                    .get("cwd")
                    .and_then(|value| value.as_str())
                    .map(ToOwned::to_owned);
            }
        }
        update_model(&payload, model);
        line.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::scan_roots;
    use crate::providers::{FileCursor, ScanCtx};
    use std::collections::HashMap;
    use std::fs;
    use std::io::Write;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture_root() -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("otr-codex-{suffix}"));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_fixture(file: &mut std::fs::File, model: &str, input: u64) {
        write_event(
            file,
            serde_json::json!({
                "timestamp": "2026-08-31T07:53:18.000Z",
                "type": "session_meta",
                "payload": {
                    "type": "session_meta",
                    "id": "session-1",
                    "cwd": r"C:\Code\Token-Show"
                }
            }),
        );
        write_event(
            file,
            serde_json::json!({
                "timestamp": "2026-08-31T07:53:19.000Z",
                "type": "response_item",
                "payload": {"type": "turn_context", "model": model}
            }),
        );
        write_token_count(file, "2026-08-31T07:53:20.000Z", input, 1);
    }

    fn write_event(file: &mut std::fs::File, value: serde_json::Value) {
        writeln!(file, "{value}").unwrap();
    }

    fn write_token_count(file: &mut std::fs::File, timestamp: &str, input: u64, output: u64) {
        write_event(
            file,
            serde_json::json!({
                "timestamp": timestamp,
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "last_token_usage": {
                            "input_tokens": input,
                            "cached_input_tokens": 0,
                            "output_tokens": output
                        }
                    }
                }
            }),
        );
    }

    fn scan(
        root: &PathBuf,
        cursors: &mut HashMap<String, FileCursor>,
        full: bool,
    ) -> Vec<crate::model::UsageRecord> {
        let mut state = serde_json::Value::Null;
        let mut ctx = ScanCtx {
            full,
            cursors,
            state: &mut state,
        };
        scan_roots(std::slice::from_ref(root), "codex", &mut ctx).unwrap()
    }

    #[test]
    fn incremental_scan_restores_model_from_cursor() {
        let root = fixture_root();
        let path =
            root.join("rollout-2026-08-31T07-53-17-11111111-2222-3333-4444-555555555555.jsonl");
        let mut file = fs::File::create(&path).unwrap();
        write_fixture(&mut file, "gpt-5.6-sol", 10);
        let mut cursors = HashMap::new();
        let first = scan(&root, &mut cursors, true);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(first[0].session_id.as_deref(), Some("session-1"));
        assert_eq!(first[0].project.as_deref(), Some(r"C:\Code\Token-Show"));

        write_token_count(&mut file, "2026-08-31T07:53:21.000Z", 20, 2);
        file.flush().unwrap();
        let second = scan(&root, &mut cursors, false);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(second[0].session_id.as_deref(), Some("session-1"));
        assert_eq!(second[0].project.as_deref(), Some(r"C:\Code\Token-Show"));
        assert_eq!(
            cursors.values().next().unwrap().extra["model"],
            "gpt-5.6-sol"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_cursor_recovers_context_from_file_prefix() {
        let root = fixture_root();
        let path =
            root.join("rollout-2026-08-31T07-53-17-11111111-2222-3333-4444-555555555555.jsonl");
        let mut file = fs::File::create(&path).unwrap();
        write_fixture(&mut file, "gpt-5.6-sol", 10);
        let offset = file.metadata().unwrap().len();
        write_token_count(&mut file, "2026-08-31T07:53:21.000Z", 20, 2);
        file.flush().unwrap();
        let mut cursors = HashMap::from([(
            path.to_string_lossy().to_string(),
            FileCursor {
                offset,
                size: offset,
                ..Default::default()
            },
        )]);
        let records = scan(&root, &mut cursors, false);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(records[0].session_id.as_deref(), Some("session-1"));
        assert_eq!(records[0].project.as_deref(), Some(r"C:\Code\Token-Show"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn incremental_scan_tracks_model_switches() {
        let root = fixture_root();
        let path =
            root.join("rollout-2026-08-31T07-53-17-11111111-2222-3333-4444-555555555555.jsonl");
        let mut file = fs::File::create(&path).unwrap();
        write_fixture(&mut file, "gpt-5.6-sol", 10);
        let mut cursors = HashMap::new();
        let first = scan(&root, &mut cursors, true);
        assert_eq!(first[0].model.as_deref(), Some("gpt-5.6-sol"));
        write_event(
            &mut file,
            serde_json::json!({
                "timestamp": "2026-08-31T07:53:21.000Z",
                "type": "response_item",
                "payload": {"type": "turn_context", "model": "gpt-5.6-terra"}
            }),
        );
        write_token_count(&mut file, "2026-08-31T07:53:22.000Z", 30, 3);
        file.flush().unwrap();
        let second = scan(&root, &mut cursors, false);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].model.as_deref(), Some("gpt-5.6-terra"));
        fs::remove_dir_all(root).unwrap();
    }
}
