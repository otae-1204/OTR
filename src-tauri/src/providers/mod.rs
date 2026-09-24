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
    /// 出口标志:解析器发现数据源被截断/重写,本次增量不可信,
    /// 要求编排层丢弃本批记录并以全量重建的方式重跑一次。
    pub force_full: bool,
    pub cursors: &'a mut HashMap<String, FileCursor>,
    /// Provider 级持久化状态(DSH/OpenCode 用它存绝对值 diff 基准)
    pub state: &'a mut serde_json::Value,
}

pub trait AgentProvider: Send + Sync {
    fn id(&self) -> &str;
    fn display_name(&self) -> &str;
    fn detect(&self) -> bool;
    fn watch_paths(&self) -> Vec<PathBuf>;
    /// 解析语义版本;变更时启动会全量重建该 Agent。
    /// 默认 1:新 Provider 上线时 kv 里没有键,自然触发一次全量扫描。
    fn parser_version(&self) -> u64 {
        1
    }
    /// UsageRecord.cost 里自带成本的币种:"CNY" 或 "USD"。
    /// None = 该 Provider 没有自带成本,只能靠定价表估算。
    ///
    /// 以前 store.rs 用 `agent != "dsh"` 猜币种,把具体 Agent 的知识写进了通用查询代码;
    /// 现在由 Provider 显式声明,Store 只按声明换算。
    fn native_cost_currency(&self) -> Option<&'static str> {
        None
    }
    /// 从 Provider 自己持久化的 state(state:<id> 这条 kv)里提取需要让用户看到的
    /// 健康提示:登录态失效、分页被截断之类。None = 一切正常。
    /// 以前这类问题只 eprintln 到 stderr,打包后的托盘应用里用户完全看不到。
    fn health(&self, _state: &serde_json::Value) -> Option<String> {
        None
    }
    /// 该 Provider 的**按天数据由外部台账负责**的桶集合,键 "日期|provider:model"。
    ///
    /// 这些桶不参与"按小时表校正按天表"(见 Store::reconcile_daily_from_hourly):
    /// 台账重写该桶时会自己写按天表,日志这边再补一次就是双计。
    /// 逐桶而不是逐日:台账只记了部分 provider 时,同一天里别的模型仍要由日志补齐。
    /// None = 该 Provider 不做按小时→按天校正。
    fn ledger_owned_buckets(
        &self,
        _state: &serde_json::Value,
    ) -> Option<std::collections::HashSet<String>> {
        None
    }

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
