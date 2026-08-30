use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::error::Result;
use crate::model::{iso_to_ms, UsageRecord};
use crate::paths;
use crate::providers::jsonl_util::{self, u64f};
use crate::providers::{ScanCtx, AgentProvider};

pub struct ZcodeProvider;

const AGENT: &str = "zcode";

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
        vec![
            paths::zcode_rollout(),
            paths::zcode_agents(),
        ]
    }

    fn scan(&self, ctx: &mut ScanCtx) -> Result<Vec<UsageRecord>> {
        // 双源:model-io(主会话+部分子代理,清理快)+ agents/*/transcript.jsonl
        // (子代理,留存较久)。同一调用会在两处各出现一次,按 requestId 去重;
        // 无 requestId 的重复行(同调用的流结束事件)用 签名 兜底。
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

        let mut records = Vec::new();
        // 批内去重集合(requestId 优先,缺失时用 会话+时间+总量 签名)
        let mut seen: HashSet<String> = HashSet::new();
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
        Ok(records)
    }
}

/// 扫描 "rollout/*.jsonl" 布局的目录(自定义 Agent 可复用)
pub fn scan_root(root: &Path, agent: &str, ctx: &mut ScanCtx) -> Result<Vec<UsageRecord>> {
    let mut files = Vec::new();
    jsonl_util::collect_jsonl(root, 1, &mut files);
    let mut records = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
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
    seen: &mut HashSet<String>,
    out: &mut Vec<UsageRecord>,
) -> Result<()> {
    let key = path.to_string_lossy().to_string();
    let mut cursor = ctx.cursors.get(&key).cloned().unwrap_or_default();
    let Some(update) = jsonl_util::read_appended(path, cursor.offset)? else {
        return Ok(());
    };
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
        let input_total = u64f(usage, "inputTokens");
        let cache_read = u64f(usage, "cacheReadTokens");
        let cache_write = u64f(usage, "cacheWriteTokens");
        let output = u64f(usage, "outputTokens");
        if input_total + output + cache_read + cache_write == 0 {
            continue;
        }
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

        // 去重键
        let dedup = match &request_id {
            Some(rid) => format!("r:{}", rid),
            None => format!(
                "s:{}:{}:{}",
                session_id.as_deref().unwrap_or(""),
                ts,
                input_total + output + cache_read + cache_write
            ),
        };
        if !seen.insert(dedup) {
            continue;
        }

        let r = UsageRecord {
            agent: agent.into(),
            session_id,
            model: (if transcript {
                v.pointer("/payload/model/modelId")
            } else {
                v.pointer("/model/modelId")
            })
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
            provider: (if transcript {
                v.pointer("/payload/model/providerId")
            } else {
                v.pointer("/model/providerId")
            })
            .and_then(|x| x.as_str())
            .map(|s| s.to_string()),
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
