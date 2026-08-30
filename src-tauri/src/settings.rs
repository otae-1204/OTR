use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// 模型单价(美元 / 百万 tokens),来源:手动填写或 models.dev 自动获取
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct PriceEntry {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
}

impl Default for PriceEntry {
    fn default() -> Self {
        Self {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
        }
    }
}

/// 用户自定义 Agent:复用内置解析器,指向任意数据目录
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomAgentConfig {
    pub id: String,
    pub name: String,
    /// "claude-code" | "codex" | "zcode"
    pub kind: String,
    pub dir: String,
}

impl CustomAgentConfig {
    pub fn kind(&self) -> &str {
        &self.kind
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub enabled_agents: Vec<String>,
    pub start_minimized: bool,
    pub theme: String,
    pub custom_agents: Vec<CustomAgentConfig>,
    /// 模型定价表(model → $/M tokens);用于给无自带成本的数据估算费用
    pub pricing: std::collections::HashMap<String, PriceEntry>,
    /// 美元 → 人民币汇率(估算换算用)
    pub exchange_rate: f64,
    /// 全局成本显示币种:"CNY" | "USD"
    pub currency: String,
    /// v1 设置没有 opencode 等新内置 Agent;首次加载 v2 时补录一次
    #[serde(default)]
    migrated_v2: bool,
    /// 已经向用户暴露过的内置 Agent;新版本新增内置 Agent 时自动启用一次
    #[serde(default)]
    known_agents: Vec<String>,
}

pub const BUILTIN_AGENTS: &[&str] = &[
    "dsh",
    "claude-code",
    "codex",
    "zcode",
    "opencode",
    "pi",
];

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled_agents: BUILTIN_AGENTS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            start_minimized: false,
            theme: "dark".into(),
            custom_agents: vec![],
            pricing: std::collections::HashMap::new(),
            exchange_rate: 7.2,
            currency: "CNY".into(),
            migrated_v2: false,
            known_agents: vec![],
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        let mut s: Settings = std::fs::read_to_string(path)
            .ok()
            .and_then(|x| serde_json::from_str(&x).ok())
            .unwrap_or_default();
        if !s.migrated_v2 {
            // 旧版设置文件补录新增内置 Agent(用户手动停用的会在升级后重新出现,可接受)
            for b in BUILTIN_AGENTS {
                if !s.enabled_agents.iter().any(|a| a == b) {
                    s.enabled_agents.push(b.to_string());
                }
            }
            s.migrated_v2 = true;
            let _ = s.save(path);
        }
        // 新版本新增的内置 Agent:没见过的一次性自动启用(用户手动停用后不会复活)
        let mut changed = false;
        for b in BUILTIN_AGENTS {
            if !s.known_agents.iter().any(|a| a == b) {
                s.known_agents.push(b.to_string());
                if !s.enabled_agents.iter().any(|a| a == b) {
                    s.enabled_agents.push(b.to_string());
                }
                changed = true;
            }
        }
        if changed {
            let _ = s.save(path);
        }
        s
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn is_enabled(&self, id: &str) -> bool {
        self.enabled_agents.iter().any(|a| a == id)
    }
}
