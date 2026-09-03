use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::model::UsageRecord;
use crate::providers::{claude_code, codex, zcode, AgentProvider, ScanCtx};
use crate::settings::CustomAgentConfig;

/// 用户自定义 Agent:按所选格式复用内置解析器,数据目录由用户指定
pub struct CustomProvider {
    cfg: CustomAgentConfig,
}

impl CustomProvider {
    pub fn new(cfg: CustomAgentConfig) -> Self {
        Self { cfg }
    }
}

impl AgentProvider for CustomProvider {
    fn id(&self) -> &str {
        &self.cfg.id
    }

    fn display_name(&self) -> &str {
        &self.cfg.name
    }

    fn detect(&self) -> bool {
        Path::new(&self.cfg.dir).is_dir()
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        vec![PathBuf::from(&self.cfg.dir)]
    }

    fn scan(&self, ctx: &mut ScanCtx) -> Result<Vec<UsageRecord>> {
        let root = PathBuf::from(&self.cfg.dir);
        match self.cfg.kind() {
            "codex" => codex::scan_roots(&[root], &self.cfg.id, ctx),
            "zcode" => zcode::scan_root(&root, &self.cfg.id, ctx),
            // 默认按 Claude Code 布局解析(fork 类产品大多是这个格式)
            _ => claude_code::scan_root(&root, &self.cfg.id, ctx),
        }
    }
}
