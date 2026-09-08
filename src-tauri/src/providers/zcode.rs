use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;
use crate::model::{iso_to_ms, UsageRecord};
use crate::paths;
use crate::providers::jsonl_util::{self, u64f};
use crate::providers::{AgentProvider, ScanCtx};

pub struct ZcodeProvider;

const AGENT: &str = "zcode";

/// 解析规则变更时递增,启动时全量重建该 Agent。
/// v1: transcript 丢掉无模型的流结束重复与回合汇总,避免记成未知模型。
/// v2: requestId 去重窗口跨扫描持久化 + 截断转全量重建。
pub const PARSER_VERSION: u64 = 2;

/// 跨扫描保留的 requestId 上限;rollout 与 transcript 分属不同文件,
/// 同一调用可能落在两次扫描里,单次 HashSet 挡不住。
const RECENT_CAP: usize = 1024;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ZcodeState {
    #[serde(default)]
    recent: Vec<String>,
}

/// 有界去重窗口:HashSet 查重 + VecDeque 维持插入顺序,超出上限丢最旧的。
#[derive(Default)]
struct SeenWindow {
    set: HashSet<String>,
    order: std::collections::VecDeque<String>,
}

impl SeenWindow {
    fn from_recent(recent: Vec<String>) -> Self {
        let mut window = Self::default();
        for id in recent {
            window.insert(id);
        }
        window
    }

    fn insert(&mut self, id: String) {
        if self.set.insert(id.clone()) {
            self.order.push_back(id);
            while self.order.len() > RECENT_CAP {
                if let Some(old) = self.order.pop_front() {
                    self.set.remove(&old);
                }
            }
        }
    }

    fn contains(&self, id: &str) -> bool {
        self.set.contains(id)
    }

    fn into_recent(self) -> Vec<String> {
        self.order.into_iter().collect()
    }
}

impl AgentProvider for ZcodeProvider {
    fn id(&self) -> &str {
        AGENT
    }

    fn display_name(&self) -> &str {
        "ZCode"
    }

    fn detect(&self) -> bool {
        paths::zcode_rollout().is_dir()
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        vec![paths::zcode_rollout(), paths::zcode_agents()]
    }

    fn parser_version(&self) -> u64 {
        PARSER_VERSION
    }

    fn scan(&self, ctx: &mut ScanCtx) -> Result<Vec<UsageRecord>> {
        // 双源:model-io(主会话+部分子代理,清理快)+ agents/*/transcript.jsonl
        // (子代理,留存较久)。同一调用按 requestId 去重。
        // transcript 只收能解析出 modelId 的单次完成事件;model_complete(无模型的
        // 流结束重复)与 turn_complete(整轮汇总)丢弃,否则会变成未知模型并翻倍。
        let mut files = Vec::new();
        jsonl_util::collect_jsonl(&paths::zcode_rollout(), 1, &mut files);
        jsonl_util::collect_jsonl(&paths::zcode_agents(), 4, &mut files);
        let transcript: Vec<PathBuf> = files
            .iter()
            .filter(|f| {
                f.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n == "transcript.jsonl")
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
        let rollout: Vec<PathBuf> = files
            .into_iter()
            .filter(|f| !transcript.contains(f))
            .collect();

        let mut st: ZcodeState =
            serde_json::from_value(std::mem::take(ctx.state)).unwrap_or_default();
        let mut records = Vec::new();
        let mut seen = SeenWindow::from_recent(st.recent);
        for file in rollout {
            if let Err(e) = scan_file(&file, false, AGENT, ctx, &mut seen, &mut records) {
                eprintln!("[{}] {}: {}", AGENT, file.display(), e);
            }
        }
        for file in transcript {
            if let Err(e) = scan_file(&file, true, AGENT, ctx, &mut seen, &mut records) {
                eprintln!("[{}] {}: {}", AGENT, file.display(), e);
            }
        }
        st.recent = seen.into_recent();
        *ctx.state = serde_json::to_value(&st).unwrap_or(Value::Null);
        Ok(records)
    }
}

/// 扫描 "rollout/*.jsonl" 布局的目录(自定义 Agent 可复用)
pub fn scan_root(root: &Path, agent: &str, ctx: &mut ScanCtx) -> Result<Vec<UsageRecord>> {
    let mut files = Vec::new();
    jsonl_util::collect_jsonl(root, 1, &mut files);
    let mut records = Vec::new();
    let mut seen = SeenWindow::default();
    for file in files {
        if let Err(e) = scan_file(&file, false, agent, ctx, &mut seen, &mut records) {
            eprintln!("[{}] {}: {}", agent, file.display(), e);
        }
    }
    Ok(records)
}

/// 每行是一次模型调用的记录:requestId 唯一(重试会有相同 requestId 的不同 attempt,取首次);
/// usage.inputTokens = 未缓存输入 + cacheRead(inputTokens 口径已含缓存读,需拆分)
fn scan_file(
    path: &Path,
    transcript: bool,
    agent: &str,
    ctx: &mut ScanCtx,
    seen: &mut SeenWindow,
    out: &mut Vec<UsageRecord>,
) -> Result<()> {
    let key = path.to_string_lossy().to_string();
    let mut cursor = ctx.cursors.get(&key).cloned().unwrap_or_default();
    let Some(update) = jsonl_util::read_appended(path, cursor.offset)? else {
        return Ok(());
    };
    if update.truncated {
        // 日志被截断/重写,已入库的贡献无法精确扣减 → 让编排层整 Agent 重建
        ctx.force_full = true;
        return Ok(());
    }
    let file_session = if transcript {
        // agents/<sess>/<agent>/transcript.jsonl — 会话 id 在行内 sessionId
        None
    } else {
        path.file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.trim_start_matches("model-io-").to_string())
    };

    for line in &update.lines {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        // model-io: response.usage 在顶层;transcript: payload.usage
        let usage = if transcript {
            v.pointer("/payload/usage")
        } else {
            v.pointer("/response/usage")
        };
        let Some(usage) = usage else {
            continue;
        };
        if transcript && !transcript_usage_is_per_call(&v, usage) {
            continue;
        }
        let input_total = u64f(usage, "inputTokens");
        let cache_read = u64f(usage, "cacheReadTokens");
        let cache_write = u64f(usage, "cacheWriteTokens");
        let output = u64f(usage, "outputTokens");
        if input_total + output + cache_read + cache_write == 0 {
            continue;
        }
        let Some(model) = extract_model(&v, transcript) else {
            // 无模型的 usage 不入库,避免 UI 出现「未知模型」
            continue;
        };
        let ts = v
            .get("completedAt")
            .or_else(|| v.get("timestamp"))
            .or_else(|| v.get("startedAt"))
            .and_then(|x| x.as_str())
            .and_then(iso_to_ms)
            .unwrap_or(0);
        let request_id = if transcript {
            v.pointer("/payload/requestId")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
        } else {
            v.get("requestId")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
        };
        let session_id = v
            .get("sessionId")
            .and_then(|x| x.as_str())
            .map(|s| s.to_string())
            .or_else(|| file_session.clone());

        let dedup = match &request_id {
            Some(rid) => format!("r:{}", rid),
            None => format!(
                "s:{}:{}:{}",
                session_id.as_deref().unwrap_or(""),
                ts,
                input_total + output + cache_read + cache_write
            ),
        };
        if seen.contains(&dedup) {
            continue;
        }
        seen.insert(dedup);

        let r = UsageRecord {
            agent: agent.into(),
            session_id,
            model: Some(model),
            provider: extract_provider(&v, transcript),
            ts,
            input_tokens: input_total.saturating_sub(cache_read),
            output_tokens: output,
            cache_read_tokens: cache_read,
            cache_write_tokens: cache_write,
            calls: 1,
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

/// transcript 里一次调用会写多条带 usage 的事件,只收单次完成,丢掉重复与回合汇总。
fn transcript_usage_is_per_call(v: &Value, usage: &Value) -> bool {
    let ty = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
    if ty == "turn_complete" || ty == "model_complete" {
        return false;
    }
    // 回合汇总即使改了 type 名,usage 里也会带 modelRequestCount
    if usage.get("modelRequestCount").is_some() {
        return false;
    }
    true
}

fn nonempty_str(v: &Value) -> Option<&str> {
    v.as_str().map(str::trim).filter(|s| !s.is_empty())
}

fn extract_model(v: &Value, transcript: bool) -> Option<String> {
    let node = if transcript {
        v.pointer("/payload/model")
    } else {
        v.pointer("/model")
    };
    if let Some(node) = node {
        if let Some(id) = node.get("modelId").and_then(nonempty_str) {
            return Some(id.to_string());
        }
        if let Some(s) = nonempty_str(node) {
            return Some(s.to_string());
        }
    }
    if transcript {
        v.pointer("/payload/modelRef/modelId")
            .and_then(nonempty_str)
            .map(|s| s.to_string())
    } else {
        v.pointer("/response/modelId")
            .and_then(nonempty_str)
            .map(|s| s.to_string())
    }
}

fn extract_provider(v: &Value, transcript: bool) -> Option<String> {
    let node = if transcript {
        v.pointer("/payload/model")
    } else {
        v.pointer("/model")
    };
    if let Some(id) = node
        .and_then(|n| n.get("providerId"))
        .and_then(nonempty_str)
    {
        return Some(id.to_string());
    }
    if transcript {
        v.pointer("/payload/modelRef/providerId")
            .and_then(nonempty_str)
            .map(|s| s.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{FileCursor, ScanCtx};
    use std::collections::HashMap;
    use std::fs;
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    static FIXTURE_SEQ: AtomicU64 = AtomicU64::new(0);

    fn fixture_dir() -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let n = FIXTURE_SEQ.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("otr-zcode-{}-{suffix}-{n}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn write_jsonl(path: &Path, lines: &[Value]) {
        let mut file = fs::File::create(path).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
    }

    fn usage(input: u64, output: u64, cache_read: u64) -> Value {
        serde_json::json!({
            "inputTokens": input,
            "outputTokens": output,
            "totalTokens": input + output,
            "cacheReadTokens": cache_read,
            "cacheWriteTokens": 0
        })
    }

    fn model_io_line(request_id: &str, model: &str) -> Value {
        serde_json::json!({
            "completedAt": "2026-09-02T23:11:29.010Z",
            "requestId": request_id,
            "model": {"modelId": model, "providerId": "builtin:zai-start-plan"},
            "response": {"usage": usage(8194, 100, 6272), "modelId": model.to_lowercase()},
            "sessionId": "sess_main",
            "startedAt": "2026-09-02T23:11:00.000Z"
        })
    }

    fn network_status_completed(request_id: &str, model: &str, ts: &str) -> Value {
        serde_json::json!({
            "type": "model_network_status",
            "timestamp": ts,
            "sessionId": "sess_sub",
            "payload": {
                "type": "model_request_completed",
                "requestId": request_id,
                "model": {"modelId": model, "providerId": "builtin:zai-start-plan"},
                "usage": usage(8194, 100, 6272)
            }
        })
    }

    fn model_complete(ts: &str) -> Value {
        serde_json::json!({
            "type": "model_complete",
            "timestamp": ts,
            "sessionId": "sess_sub",
            "payload": {
                "usage": usage(8194, 100, 6272)
            }
        })
    }

    fn turn_complete() -> Value {
        serde_json::json!({
            "type": "turn_complete",
            "timestamp": "2026-09-02T23:12:00.000Z",
            "sessionId": "sess_sub",
            "payload": {
                "usage": {
                    "source": "provider",
                    "modelRequestCount": 2,
                    "inputTokens": 20000,
                    "outputTokens": 200,
                    "totalTokens": 20200,
                    "cacheReadTokens": 0,
                    "cacheWriteTokens": 0
                }
            }
        })
    }

    fn scan_files(files: &[(&Path, bool)]) -> Vec<UsageRecord> {
        let mut cursors: HashMap<String, FileCursor> = HashMap::new();
        let mut state = Value::Null;
        let mut ctx = ScanCtx {
            full: true,
            force_full: false,
            cursors: &mut cursors,
            state: &mut state,
        };
        let mut seen = SeenWindow::default();
        let mut records = Vec::new();
        for (path, transcript) in files {
            scan_file(path, *transcript, AGENT, &mut ctx, &mut seen, &mut records).unwrap();
        }
        records
    }

    #[test]
    fn model_io_keeps_named_model() {
        let root = fixture_dir();
        let path = root.join("model-io-sess_main.jsonl");
        write_jsonl(&path, &[model_io_line("req-1", "GLM-5.3-Flash")]);
        let records = scan_files(&[(&path, false)]);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].model.as_deref(), Some("GLM-5.3-Flash"));
        assert_eq!(
            records[0].provider.as_deref(),
            Some("builtin:zai-start-plan")
        );
        assert_eq!(records[0].input_tokens, 8194 - 6272);
        assert_eq!(records[0].cache_read_tokens, 6272);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn transcript_keeps_completed_and_drops_duplicates() {
        let root = fixture_dir();
        let path = root.join("transcript.jsonl");
        write_jsonl(
            &path,
            &[
                network_status_completed("req-1", "GLM-5.3-Flash", "2026-09-02T23:11:29.010Z"),
                model_complete("2026-09-02T23:11:29.048Z"),
                turn_complete(),
            ],
        );
        let records = scan_files(&[(&path, true)]);
        assert_eq!(
            records.len(),
            1,
            "model_complete/turn_complete must be dropped"
        );
        assert_eq!(records[0].model.as_deref(), Some("GLM-5.3-Flash"));
        assert!(records
            .iter()
            .all(|r| r.model.as_deref().is_some_and(|m| !m.is_empty())));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn request_id_dedups_model_io_and_transcript() {
        let root = fixture_dir();
        let io = root.join("model-io-sess_main.jsonl");
        let tr = root.join("transcript.jsonl");
        write_jsonl(&io, &[model_io_line("req-1", "GLM-5.3-Flash")]);
        write_jsonl(
            &tr,
            &[
                network_status_completed("req-1", "GLM-5.3-Flash", "2026-09-02T23:11:29.010Z"),
                model_complete("2026-09-02T23:11:29.048Z"),
            ],
        );
        let records = scan_files(&[(&io, false), (&tr, true)]);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].model.as_deref(), Some("GLM-5.3-Flash"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn usage_without_model_is_dropped() {
        let root = fixture_dir();
        let path = root.join("model-io-sess_anon.jsonl");
        write_jsonl(
            &path,
            &[serde_json::json!({
                "requestId": "req-x",
                "response": {"usage": usage(10, 1, 0)},
                "sessionId": "s"
            })],
        );
        let records = scan_files(&[(&path, false)]);
        assert!(records.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn model_complete_alone_does_not_become_unknown() {
        let root = fixture_dir();
        let path = root.join("transcript.jsonl");
        write_jsonl(&path, &[model_complete("2026-09-02T23:11:29.048Z")]);
        let records = scan_files(&[(&path, true)]);
        assert!(records.is_empty());
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod window_tests {
    use super::{SeenWindow, ZcodeState, RECENT_CAP};

    #[test]
    fn seen_window_survives_state_roundtrip() {
        let mut window = SeenWindow::from_recent(vec![]);
        window.insert("r:abc".into());
        let state = ZcodeState {
            recent: window.into_recent(),
        };
        // 模拟 kv 持久化 → 下次扫描重新加载
        let json = serde_json::to_value(&state).unwrap();
        let back: ZcodeState = serde_json::from_value(json).unwrap();
        let reloaded = SeenWindow::from_recent(back.recent);
        assert!(reloaded.contains("r:abc"), "requestId 去重窗口必须跨扫描保留");
        assert!(!reloaded.contains("r:other"));
    }

    #[test]
    fn seen_window_is_bounded() {
        let mut window = SeenWindow::from_recent(vec![]);
        for i in 0..RECENT_CAP + 50 {
            window.insert(format!("r:{i}"));
        }
        let recent = window.into_recent();
        assert_eq!(recent.len(), RECENT_CAP, "窗口必须封顶,否则 kv 无限增长");
        assert_eq!(recent.last().unwrap(), &format!("r:{}", RECENT_CAP + 49));
        assert_eq!(recent.first().unwrap(), "r:50", "最旧的必须被淘汰");
    }
}
