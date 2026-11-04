use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::Result;
use crate::model::{iso_to_ms, plausible_gen_ms, UsageRecord};
use crate::paths;
use crate::providers::jsonl_util::{self, u64f, DEDUP_WINDOW};
use crate::providers::{AgentProvider, ScanCtx};

pub struct ClaudeCodeProvider;

const AGENT: &str = "claude-code";

/// 解析语义版本;变更时启动会全量重建。
/// v1: (message.id, requestId) 去重窗口跨扫描持久化 + 截断转全量重建。
/// v2: 估算速度 = 上一条 user/tool_result 到这条 assistant 的耗时。流式重复行只延长这段耗时。
pub const PARSER_VERSION: u64 = 2;

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
    // 请求起点可能落在上一次扫描的末尾,助手回复才出现在这一次。
    let mut pending_start = load_anchor(&cursor.extra);
    let mut spans = load_spans(&cursor.extra);
    for line in &update.lines {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let ts = v
            .get("timestamp")
            .and_then(|x| x.as_str())
            .and_then(iso_to_ms)
            .unwrap_or(0);
        if kind == "user" || kind == "tool_result" {
            if ts > 0 {
                pending_start = Some(ts);
            }
            continue;
        }
        if kind != "assistant" {
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
        let output = u64f(usage, "output_tokens");
        if !msg_id.is_empty() && seen.contains(&dedup_key) {
            // 流式重复行:token 已经记过,只把估算耗时延长到更晚的结束时间。
            note_duplicate(
                &mut spans,
                &dedup_key,
                ts,
                output,
                out,
                agent,
                &session_id,
                &project,
            );
            continue;
        }
        let model = msg
            .get("model")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        if model.is_empty() {
            continue;
        }
        if model.starts_with('<') {
            pending_start = None;
            continue;
        }
        let start = pending_start;
        let mut r = UsageRecord {
            agent: agent.into(),
            session_id: session_id.clone(),
            project: Some(project.clone()),
            model: Some(model.clone()),
            ts,
            input_tokens: u64f(usage, "input_tokens"),
            output_tokens: output,
            cache_read_tokens: u64f(usage, "cache_read_input_tokens"),
            cache_write_tokens: u64f(usage, "cache_creation_input_tokens"),
            calls: 1,
            ..Default::default()
        };
        if r.total_tokens() == 0 {
            continue;
        }
        if let Some((est_output, est_ms)) = start.and_then(|start| estimated(start, ts, output)) {
            r.est_output = est_output;
            r.est_ms = est_ms;
            r.est_samples = 1;
        }
        // 用完即清,下一条助手回复没有新的用户行时不能接着用这个起点。
        pending_start = None;
        if !msg_id.is_empty() {
            seen.insert(dedup_key.clone());
            order.push(dedup_key.clone());
            spans.insert(
                dedup_key,
                SpeedSpan {
                    start: start.unwrap_or(0),
                    end: ts,
                    est_output: r.est_output,
                    est_ms: r.est_ms,
                    sampled: r.est_samples > 0,
                    model,
                    local: Some(out.len()),
                },
            );
        }
        out.push(r);
    }
    cursor.offset = update.new_offset;
    cursor.size = update.size;
    cursor.mtime_ms = jsonl_util::file_mtime_ms(path);
    jsonl_util::save_seen(&mut cursor, &order);
    save_anchor(&mut cursor, pending_start);
    save_spans(&mut cursor, &spans, &order);
    ctx.cursors.insert(key, cursor);
    Ok(())
}

/// 一次已计入的助手回复。后续流式重复行用它把耗时延长到最晚的结束时间。
struct SpeedSpan {
    start: i64,
    end: i64,
    est_output: u64,
    est_ms: u64,
    sampled: bool,
    model: String,
    /// 本次扫描里那条记录的下标。跨扫描的重复行没有它,只能再写一条耗时差量。
    local: Option<usize>,
}

fn estimated(start: i64, end: i64, output: u64) -> Option<(u64, u64)> {
    if output == 0 || start <= 0 || end <= start {
        return None;
    }
    Some((output, plausible_gen_ms(end - start)?))
}

fn load_anchor(extra: &Value) -> Option<i64> {
    extra
        .get("speedAnchor")
        .and_then(|v| v.as_i64())
        .filter(|t| *t > 0)
}

fn save_anchor(cursor: &mut crate::providers::FileCursor, pending: Option<i64>) {
    if let Some(ts) = pending {
        cursor.extra["speedAnchor"] = serde_json::json!(ts);
    } else if let Some(obj) = cursor.extra.as_object_mut() {
        obj.remove("speedAnchor");
    }
}

fn load_spans(extra: &Value) -> HashMap<String, SpeedSpan> {
    let Some(map) = extra.get("speedSpans").and_then(|v| v.as_object()) else {
        return HashMap::new();
    };
    map.iter()
        .filter_map(|(key, v)| {
            Some((
                key.clone(),
                SpeedSpan {
                    start: v.get("start")?.as_i64()?,
                    end: v.get("end").and_then(|x| x.as_i64()).unwrap_or(0),
                    est_output: v.get("estOutput").and_then(|x| x.as_u64()).unwrap_or(0),
                    est_ms: v.get("estMs").and_then(|x| x.as_u64()).unwrap_or(0),
                    sampled: v.get("sampled").and_then(|x| x.as_bool()).unwrap_or(false),
                    model: v
                        .get("model")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    local: None,
                },
            ))
        })
        .collect()
}

fn save_spans(
    cursor: &mut crate::providers::FileCursor,
    spans: &HashMap<String, SpeedSpan>,
    order: &[String],
) {
    let tail = &order[order.len().saturating_sub(DEDUP_WINDOW)..];
    let keep: HashSet<&str> = tail.iter().map(|s| s.as_str()).collect();
    let mut obj = serde_json::Map::new();
    for (key, span) in spans {
        if !keep.contains(key.as_str()) {
            continue;
        }
        obj.insert(
            key.clone(),
            serde_json::json!({
                "start": span.start,
                "end": span.end,
                "estOutput": span.est_output,
                "estMs": span.est_ms,
                "sampled": span.sampled,
                "model": span.model,
            }),
        );
    }
    cursor.extra["speedSpans"] = Value::Object(obj);
}

fn note_duplicate(
    spans: &mut HashMap<String, SpeedSpan>,
    key: &str,
    ts: i64,
    output: u64,
    out: &mut Vec<UsageRecord>,
    agent: &str,
    session_id: &Option<String>,
    project: &str,
) {
    let Some(span) = spans.get_mut(key) else {
        return;
    };
    let end = span.end.max(ts);
    // 没有用户行起点时,结束时间减 0 会变成一个天文数字,不能当成耗时。
    let measured = if span.start > 0 {
        plausible_gen_ms(end - span.start).unwrap_or(0)
    } else {
        0
    };
    let est_ms = span.est_ms.max(measured);
    let est_output = if est_ms > 0 {
        span.est_output.max(output)
    } else {
        span.est_output
    };
    let sampled = span.sampled || (est_output > 0 && est_ms > 0);
    let d_out = est_output.saturating_sub(span.est_output);
    let d_ms = est_ms.saturating_sub(span.est_ms);
    let d_samples = u64::from(sampled && !span.sampled);
    span.end = end;
    span.est_output = est_output;
    span.est_ms = est_ms;
    span.sampled = sampled;
    if d_out == 0 && d_ms == 0 && d_samples == 0 {
        return;
    }
    if let Some(idx) = span.local {
        if let Some(rec) = out.get_mut(idx) {
            rec.est_output = est_output;
            rec.est_ms = est_ms;
            rec.est_samples = u64::from(sampled);
            return;
        }
    }
    let model = span.model.clone();
    out.push(UsageRecord {
        agent: agent.into(),
        session_id: session_id.clone(),
        project: Some(project.to_string()),
        model: Some(model),
        ts: end,
        est_output: d_out,
        est_ms: d_ms,
        est_samples: d_samples,
        skip_daily: true,
        skip_hourly: true,
        ..Default::default()
    });
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

    fn user_line(ts: &str) -> String {
        format!(r#"{{"type":"user","timestamp":"{ts}","message":{{"content":"hi"}}}}"#)
    }

    fn assistant_line(msg_id: &str, req: &str, ts: &str, output: u64) -> String {
        format!(
            r#"{{"type":"assistant","requestId":"{req}","timestamp":"{ts}","message":{{"id":"{msg_id}","model":"claude-opus-5","usage":{{"input_tokens":10,"output_tokens":{output},"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}}}}"#
        )
    }

    #[test]
    fn streaming_duplicate_extends_estimated_speed_without_recounting_tokens() {
        let root = temp_root("speed");
        let file = root.join("proj").join("s1.jsonl");
        std::fs::write(
            &file,
            format!(
                "{}\n{}\n{}\n",
                user_line("2026-09-01T10:00:00.000Z"),
                assistant_line("m1", "r1", "2026-09-01T10:00:01.000Z", 25),
                assistant_line("m1", "r1", "2026-09-01T10:00:03.500Z", 250),
            ),
        )
        .unwrap();

        let mut cursors = HashMap::new();
        let mut state = serde_json::Value::Null;
        let recs = {
            let mut c = ctx(&mut cursors, &mut state);
            scan_root(&root, AGENT, &mut c).unwrap()
        };
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0].output_tokens, 25, "token 仍按第一次见到的快照计");
        assert_eq!(recs[0].est_output, 250);
        assert_eq!(recs[0].est_ms, 3_500);
        assert_eq!(recs[0].est_samples, 1);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn pending_start_is_not_reused_by_the_next_assistant() {
        let root = temp_root("clear");
        let file = root.join("proj").join("s1.jsonl");
        std::fs::write(
            &file,
            format!(
                "{}\n{}\n{}\n",
                user_line("2026-09-01T10:00:00.000Z"),
                assistant_line("m1", "r1", "2026-09-01T10:00:01.000Z", 50),
                assistant_line("m2", "r2", "2026-09-01T10:01:30.000Z", 80),
            ),
        )
        .unwrap();

        let mut cursors = HashMap::new();
        let mut state = serde_json::Value::Null;
        let recs = {
            let mut c = ctx(&mut cursors, &mut state);
            scan_root(&root, AGENT, &mut c).unwrap()
        };
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0].est_ms, 1_000);
        assert_eq!(recs[1].est_ms, 0);
        assert_eq!(recs[1].est_samples, 0);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn pending_start_survives_until_the_assistant_line() {
        let root = temp_root("anchor");
        let file = root.join("proj").join("s1.jsonl");
        std::fs::write(&file, format!("{}\n", user_line("2026-09-01T10:00:00.000Z"))).unwrap();

        let mut cursors = HashMap::new();
        let mut state = serde_json::Value::Null;
        let first = {
            let mut c = ctx(&mut cursors, &mut state);
            scan_root(&root, AGENT, &mut c).unwrap()
        };
        assert!(first.is_empty());

        let mut f = std::fs::OpenOptions::new().append(true).open(&file).unwrap();
        writeln!(
            f,
            "{}",
            assistant_line("m1", "r1", "2026-09-01T10:00:01.000Z", 40)
        )
        .unwrap();
        drop(f);

        let mut state2 = serde_json::Value::Null;
        let second = {
            let mut c = ctx(&mut cursors, &mut state2);
            scan_root(&root, AGENT, &mut c).unwrap()
        };
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].est_output, 40);
        assert_eq!(second[0].est_ms, 1_000);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn later_scan_only_adds_the_extra_estimated_time() {
        let root = temp_root("extend");
        let file = root.join("proj").join("s1.jsonl");
        std::fs::write(
            &file,
            format!(
                "{}\n{}\n",
                user_line("2026-09-01T10:00:00.000Z"),
                assistant_line("m1", "r1", "2026-09-01T10:00:01.000Z", 25),
            ),
        )
        .unwrap();

        let mut cursors = HashMap::new();
        let mut state = serde_json::Value::Null;
        {
            let mut c = ctx(&mut cursors, &mut state);
            let first = scan_root(&root, AGENT, &mut c).unwrap();
            assert_eq!(first[0].est_output, 25);
            assert_eq!(first[0].est_ms, 1_000);
        }

        let mut f = std::fs::OpenOptions::new().append(true).open(&file).unwrap();
        writeln!(
            f,
            "{}",
            assistant_line("m1", "r1", "2026-09-01T10:00:03.500Z", 250)
        )
        .unwrap();
        drop(f);

        let mut state2 = serde_json::Value::Null;
        let second = {
            let mut c = ctx(&mut cursors, &mut state2);
            scan_root(&root, AGENT, &mut c).unwrap()
        };
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].output_tokens, 0);
        assert_eq!(second[0].calls, 0);
        assert_eq!(second[0].est_output, 225);
        assert_eq!(second[0].est_ms, 2_500);
        assert_eq!(second[0].est_samples, 0);
        let _ = std::fs::remove_dir_all(root);
    }
}
