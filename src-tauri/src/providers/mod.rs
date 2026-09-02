use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::model::UsageRecord;
use crate::settings::Settings;

pub mod claude_code;
pub mod codex;
pub mod cursor;
pub mod custom;
pub mod dsh;
pub mod jsonl_util;
pub mod opencode;
pub mod pi;
pub mod zcode;

/// 单个文件的增量解析游标;extra 存放 Provider 自定义状态(如 Codex 的累计值)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct FileCursor {
    #[serde(default)]
    pub offset: u64,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub mtime_ms: i64,
    #[serde(default)]
    pub extra: serde_json::Value,
}

pub struct ScanCtx<'a> {
    pub full: bool,
    pub cursors: &'a mut HashMap<String, FileCursor>,
    /// Provider 级持久化状态(DSH/OpenCode 用它存绝对值 diff 基准)
    pub state: &'a mut serde_json::Value,
}

pub trait AgentProvider: Send + Sync {
    fn id(&self) -> &str;
    fn display_name(&self) -> &str;
    fn detect(&self) -> bool;
    fn watch_paths(&self) -> Vec<PathBuf>;
    /// 增量扫描;full 时外部已重置游标与状态,Provider 自然输出全量
    fn scan(&self, ctx: &mut ScanCtx) -> Result<Vec<UsageRecord>>;
}

/// 内置 Provider
pub fn all_providers() -> Vec<Box<dyn AgentProvider>> {
    vec![
        Box::new(dsh::DshProvider),
        Box::new(claude_code::ClaudeCodeProvider),
        Box::new(codex::CodexProvider),
        Box::new(zcode::ZcodeProvider),
        Box::new(opencode::OpencodeProvider),
        Box::new(pi::PiProvider),
        Box::new(cursor::CursorProvider),
    ]
}

/// 根据设置构建用户自定义 Provider
pub fn build_customs(settings: &Settings) -> Vec<Box<dyn AgentProvider>> {
    settings
        .custom_agents
        .iter()
        .map(|c| Box::new(custom::CustomProvider::new(c.clone())) as Box<dyn AgentProvider>)
        .collect()
}
