use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// 模型单价(美元 / 百万 tokens),来源:手动填写、models.dev 自动获取,或内置 curated 表
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct PriceEntry {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    #[serde(default)]
    pub cache_write: f64,
    /// 高峰时段单价;None = 无峰谷,整段按平价计(与加峰谷之前逐位一致)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peak: Option<PeakTier>,
}

/// 峰谷计价的高峰档(DeepSeek 北京时间工作日 09:00-12:00、14:00-18:00)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct PeakTier {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

impl Default for PeakTier {
    fn default() -> Self {
        Self {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
        }
    }
}

impl Default for PriceEntry {
    fn default() -> Self {
        Self {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
            cache_write: 0.0,
            peak: None,
        }
    }
}

/// 额度页的一个账号。
///
/// 两种凭据模式,对应两类上游:
/// - **home 模式**(Cursor / Codex):认证靠本地 CLI 会话,多账号 = 多份 profile 目录
///   (Cursor 支持 `--user-data-dir`,Codex 认 `CODEX_HOME`)。
/// - **key 模式**(DeepSeek / StepFun):额度 API 直接吃密钥。条目名由程序生成
///   (`limit.<provider>.<id>.key`),用户不用自己起名。StepFun 的订阅额度另走
///   一条控制台 Cookie(`cookie_ref`)。
///
/// 内置账号(默认 profile)由代码合成、始终存在且不可删;这里只存用户额外添加的。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct LimitAccount {
    /// 唯一键,如 "cursor-work"
    pub id: String,
    /// "cursor" | "codex" | "deepseek" | "qwen" | "stepfun"
    pub provider: String,
    /// 用户可见名
    pub label: String,
    /// 可选备注,如 "Pro"
    #[serde(default)]
    pub plan: String,
    /// home 模式:该账号的 profile 目录
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
    /// key 模式:系统凭据库里的条目名(API Key)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_ref: Option<String>,
    /// StepFun 控制台 Cookie 的条目名。内置账号是 `STEPFUN_CONSOLE_COOKIE`,
    /// 额外账号是 `limit.stepfun.<id>.cookie`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookie_ref: Option<String>,
}

impl Default for LimitAccount {
    fn default() -> Self {
        Self {
            id: String::new(),
            provider: String::new(),
            label: String::new(),
            plan: String::new(),
            home: None,
            secret_ref: None,
            cookie_ref: None,
        }
    }
}

/// 默认主题 id(内置,随应用打包)
pub const DEFAULT_THEME_ID: &str = "otr";

fn default_theme_id() -> String {
    DEFAULT_THEME_ID.to_string()
}

/// 深浅模式只认这两个值
fn valid_mode(v: &str) -> Option<&'static str> {
    match v {
        "dark" => Some("dark"),
        "light" => Some("light"),
        _ => None,
    }
}

/// 额度默认刷新间隔:60 秒。额度是慢变量,再密也没有信息增量,只会白烧请求。
fn default_refresh_secs() -> u64 {
    60
}

/// 默认开启的额度来源。
/// StepFun 依赖逆向的控制台接口,必须由用户显式勾选。
fn default_limit_providers() -> Vec<String> {
    vec![
        "cursor".into(),
        "codex".into(),
        "deepseek".into(),
        "qwen".into(),
    ]
}

/// 这个字段出现之前就有的来源。旧设置文件缺字段时当成「都见过」,
/// 升级才不会把用户关掉的 DeepSeek 重新打开。千问是后来加的,不在这里。
fn legacy_known_limit_providers() -> Vec<String> {
    vec![
        "cursor".into(),
        "codex".into(),
        "deepseek".into(),
        "stepfun".into(),
    ]
}

/// 支持的额度 Provider。**不在这个表里的一律报 unsupported**,
/// 不拿内置账号的数字冒充 —— 那会让用户以为配好了。
pub const LIMIT_PROVIDERS: &[&str] = &["cursor", "codex", "deepseek", "qwen", "stepfun"];

impl LimitAccount {
    /// 校验一份账号配置;返回中文错误原因。
    pub fn validate(&self) -> std::result::Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("账号 id 不能为空".into());
        }
        if !LIMIT_PROVIDERS.contains(&self.provider.as_str()) {
            return Err(format!(
                "暂不支持该额度来源:{}",
                if self.provider.is_empty() { "(空)" } else { &self.provider }
            ));
        }
        // home 模式要目录。DeepSeek 要 API Key。StepFun 的订阅 Cookie 和
        // 按量 Key 至少有一个,否则这张卡没有任何可查的东西。
        match self.provider.as_str() {
            "cursor" | "codex" => {
                let home = self.home.as_deref().unwrap_or("").trim();
                if home.is_empty() {
                    return Err(format!("{} 需要指定 profile 目录", self.provider));
                }
                if !Path::new(home).is_dir() {
                    return Err(format!("profile 目录不存在:{home}"));
                }
            }
            "deepseek" => {
                let secret = self.secret_ref.as_deref().unwrap_or("").trim();
                if secret.is_empty() {
                    return Err("DeepSeek 需要 API Key".into());
                }
            }
            "qwen" => {
                let secret = self.secret_ref.as_deref().unwrap_or("").trim();
                let cookie = self.cookie_ref.as_deref().unwrap_or("").trim();
                if secret.is_empty() && cookie.is_empty() {
                    return Err("Qwen 至少需要控制台 Cookie 或 API Key".into());
                }
            }
            "stepfun" => {
                let secret = self.secret_ref.as_deref().unwrap_or("").trim();
                let cookie = self.cookie_ref.as_deref().unwrap_or("").trim();
                if secret.is_empty() && cookie.is_empty() {
                    return Err("StepFun 至少需要控制台 Cookie 或 API Key".into());
                }
            }
            _ => {}
        }
        Ok(())
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
    /// **生效的**深浅模式:"dark" | "light"。历史字段名,含义是模式;主题见 `theme_id`。
    /// 选中只有一种模式的主题时它跟着变;用户自己的选择在 `preferred_mode`。
    pub theme: String,
    /// 用户**偏好的**深浅模式:"dark" | "light"。只由设置页的深浅按钮修改,单模式主题不改它,
    /// 换回支持该模式的主题时按它恢复。旧设置文件没有这个字段:字段级 `default` 给空串,
    /// 读取时由 `normalize_preferred_mode` 从 `theme` 推导(不能用容器级默认值,那会一律变成 "dark")。
    #[serde(default)]
    pub preferred_mode: String,
    /// 当前主题 id:内置 "otr",或 `<数据目录>/themes/` 里某个清单的 id。
    /// 后端只负责存取;找不到 / 校验失败时由前端回退到默认主题(不改这个值)。
    #[serde(default = "default_theme_id")]
    pub theme_id: String,
    pub custom_agents: Vec<CustomAgentConfig>,
    /// 模型定价表(model → $/M tokens)。**成本的唯一权威**:填了定价的模型一律按它重算,
    /// 没填的才回退到数据自带成本(币种由 Provider::native_cost_currency 声明)。
    pub pricing: std::collections::HashMap<String, PriceEntry>,
    /// 定价来源:model → "manual" | "models.dev:<provider>"。只用于 UI 提示,
    /// 以及"从 models.dev 同步时要不要先问用户"的判断。
    #[serde(default)]
    pub pricing_source: std::collections::HashMap<String, String>,
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
    /// 额度后台刷新间隔(秒)
    #[serde(default = "default_refresh_secs")]
    pub refresh_secs: u64,
    /// 额度页的**额外**账号(内置账号由代码合成,不在这里)
    #[serde(default)]
    pub limit_accounts: Vec<LimitAccount>,
    /// 显式启用过的额度 Provider。StepFun 是逆向接口,默认关闭。
    ///
    /// 用 `default = "default_limit_providers"` 而不是裸 `#[serde(default)]`:
    /// 后者对旧设置文件给出**空表**,升级后额度页会一片空白。
    #[serde(default = "default_limit_providers")]
    pub limit_providers: Vec<String>,
    /// 已经向用户介绍过的额度来源。新加的来源(如千问)只自动打开一次,
    /// 用户关掉之后不会在下次启动时复活。
    #[serde(default = "legacy_known_limit_providers")]
    known_limit_providers: Vec<String>,
}

pub const BUILTIN_AGENTS: &[&str] = &[
    "dsh",
    "claude-code",
    "codex",
    "zcode",
    "opencode",
    "pi",
    "cursor",
];

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled_agents: BUILTIN_AGENTS.iter().map(|s| s.to_string()).collect(),
            start_minimized: false,
            theme: "dark".into(),
            preferred_mode: "dark".into(),
            theme_id: default_theme_id(),
            custom_agents: vec![],
            pricing: std::collections::HashMap::new(),
            pricing_source: std::collections::HashMap::new(),
            exchange_rate: 7.2,
            currency: "CNY".into(),
            migrated_v2: false,
            known_agents: vec![],
            refresh_secs: default_refresh_secs(),
            limit_accounts: vec![],
            limit_providers: default_limit_providers(),
            known_limit_providers: {
                let mut known = legacy_known_limit_providers();
                known.push("qwen".into());
                known
            },
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        let mut s: Settings = std::fs::read_to_string(path)
            .ok()
            .and_then(|x| serde_json::from_str(&x).ok())
            .unwrap_or_default();
        // 只在内存里补齐;下次任何保存都会把它写进文件,不为此单独写盘
        s.normalize_preferred_mode();
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
        // DeepSeek 定价修正:内置 curated 表把已知的错误单价换成现行官方价并补上峰谷档。
        // 只动"逐位命中已知错误值"的条目,用户自填的其它值原样保留(见 pricing::migrate)。
        if crate::pricing::migrate(&mut s) {
            changed = true;
        }
        // 千问是后加的额度来源:旧设置文件里没见过它,就自动打开一次。
        if !s.known_limit_providers.iter().any(|p| p == "qwen") {
            s.known_limit_providers.push("qwen".into());
            if !s.limit_providers.iter().any(|p| p == "qwen") {
                s.limit_providers.push("qwen".into());
            }
            changed = true;
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

    /// 偏好模式缺失或不合法时,用 `theme`(上次生效的模式)推导;两者都不合法时用暗色。
    /// 升级前选过单模式主题的用户,`theme` 已被改写过,真正的偏好无从得知 —— 这里只能取它。
    pub fn normalize_preferred_mode(&mut self) {
        if valid_mode(&self.preferred_mode).is_none() {
            self.preferred_mode = valid_mode(&self.theme).unwrap_or("dark").to_string();
        }
    }

    /// 托盘「恢复默认主题」:主题回到内置默认。默认主题两种模式都有,所以生效模式 = 偏好模式;
    /// 其余设置(Agent、定价、额度账号……)原样保留。
    pub fn reset_theme(&mut self) {
        self.normalize_preferred_mode();
        self.theme_id = DEFAULT_THEME_ID.to_string();
        self.theme = self.preferred_mode.clone();
    }

    pub fn is_enabled(&self, id: &str) -> bool {
        self.enabled_agents.iter().any(|a| a == id)
    }

    /// 额度账号由专门的命令维护。设置页其它区块保存的是打开时的快照,
    /// 整份覆盖会把刚加的账号和来源开关抹掉。
    pub fn keep_limit_config_from(&mut self, current: &Settings) {
        self.limit_accounts = current.limit_accounts.clone();
        self.limit_providers = current.limit_providers.clone();
        self.refresh_secs = current.refresh_secs;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(provider: &str) -> LimitAccount {
        LimitAccount {
            id: format!("{provider}-work"),
            provider: provider.into(),
            label: "工作".into(),
            ..Default::default()
        }
    }

    /// home 模式(Cursor/Codex)必须给一个真实存在的目录
    #[test]
    fn home_mode_requires_an_existing_directory() {
        let mut a = account("cursor");
        assert!(a.validate().is_err(), "没给目录应当被拒");
        a.home = Some("C:/__otr_definitely_missing__".into());
        assert!(a.validate().is_err(), "目录不存在应当被拒");
        a.home = Some(std::env::temp_dir().to_string_lossy().to_string());
        assert!(a.validate().is_ok(), "存在的目录应当通过");
    }

    /// key 模式:DeepSeek 必须有 API Key 条目;StepFun 有 Cookie 或 Key 其一即可
    #[test]
    fn key_mode_requires_a_credential_reference() {
        let mut a = account("deepseek");
        assert!(a.validate().is_err());
        a.secret_ref = Some("limit.deepseek.deepseek-work.key".into());
        assert!(a.validate().is_ok());
    }

    #[test]
    fn stepfun_accepts_a_cookie_without_an_api_key() {
        let mut a = account("stepfun");
        assert!(a.validate().is_err(), "两条都空应当被拒");
        a.cookie_ref = Some("limit.stepfun.stepfun-work.cookie".into());
        assert!(a.validate().is_ok(), "只配订阅 Cookie 应当通过");
    }

    /// 不支持的 provider **明确报错**,不拿别家数字冒充
    #[test]
    fn unsupported_providers_are_rejected_loudly() {
        let mut a = account("gemini");
        a.home = Some(std::env::temp_dir().to_string_lossy().to_string());
        let err = a.validate().expect_err("必须被拒");
        assert!(err.contains("暂不支持"), "错误要说清楚: {err}");

        let mut empty = account("");
        empty.provider = String::new();
        assert!(empty.validate().is_err());
    }

    #[test]
    fn empty_id_is_rejected() {
        let mut a = account("deepseek");
        a.id = "  ".into();
        a.secret_ref = Some("X".into());
        assert!(a.validate().is_err());
    }

    /// 默认配置:Cursor/Codex/DeepSeek 开,StepFun 关(逆向接口,用户显式开启)
    #[test]
    fn stepfun_is_off_by_default() {
        let s = Settings::default();
        assert!(s.limit_providers.iter().any(|p| p == "cursor"));
        assert!(s.limit_providers.iter().any(|p| p == "codex"));
        assert!(s.limit_providers.iter().any(|p| p == "deepseek"));
        assert!(s.limit_providers.iter().any(|p| p == "qwen"));
        assert!(
            !s.limit_providers.iter().any(|p| p == "stepfun"),
            "StepFun 依赖逆向接口,必须默认关闭"
        );
        assert_eq!(s.refresh_secs, 60);
    }

    /// 旧设置文件没有新字段时,反序列化不该失败(serde default)
    #[test]
    fn old_settings_files_load_without_the_new_fields() {
        let old = r#"{"enabledAgents":["dsh"],"currency":"CNY","exchangeRate":7.2}"#;
        let s: Settings = serde_json::from_str(old).expect("旧文件必须还能读");
        assert_eq!(s.limit_accounts.len(), 0);
        assert_eq!(s.refresh_secs, 60);
        assert!(s.limit_providers.iter().any(|p| p == "cursor"));
        assert_eq!(s.theme_id, DEFAULT_THEME_ID, "旧文件缺 themeId 时用默认主题");
        assert_eq!(s.theme, "dark");
    }

    /// 主题选择往返:themeId 以 camelCase 落盘,读回一致;模式字段不受影响
    #[test]
    fn theme_id_round_trips_and_keeps_mode() {
        let mut s = Settings::default();
        s.theme_id = "nord".into();
        s.theme = "light".into();
        let text = serde_json::to_string(&s).unwrap();
        assert!(text.contains("\"themeId\":\"nord\""), "{text}");
        let back: Settings = serde_json::from_str(&text).unwrap();
        assert_eq!(back.theme_id, "nord");
        assert_eq!(back.theme, "light");
    }

    /// 旧设置文件没有 preferredMode:从 theme(上次生效的模式)推导,而不是一律变成暗色
    #[test]
    fn preferred_mode_is_derived_from_theme_for_old_files() {
        let mut s: Settings =
            serde_json::from_str(r#"{"theme":"light","themeId":"warm-paper"}"#).unwrap();
        assert_eq!(s.preferred_mode, "", "字段级 default 必须给空串,才分得清「缺字段」");
        s.normalize_preferred_mode();
        assert_eq!(s.preferred_mode, "light");
        assert_eq!(s.theme, "light");
        assert_eq!(s.theme_id, "warm-paper");

        let mut dark: Settings = serde_json::from_str(r#"{"theme":"dark"}"#).unwrap();
        dark.normalize_preferred_mode();
        assert_eq!(dark.preferred_mode, "dark");
    }

    /// 已有 preferredMode 时原样保留,哪怕它与生效模式不同(单模式主题正在生效)
    #[test]
    fn preferred_mode_survives_a_single_mode_theme() {
        let mut s: Settings = serde_json::from_str(
            r#"{"theme":"light","preferredMode":"dark","themeId":"warm-paper"}"#,
        )
        .unwrap();
        s.normalize_preferred_mode();
        assert_eq!(s.preferred_mode, "dark");
        assert_eq!(s.theme, "light");
        let text = serde_json::to_string(&s).unwrap();
        assert!(text.contains("\"preferredMode\":\"dark\""), "{text}");
        assert!(text.contains("\"theme\":\"light\""), "{text}");
    }

    /// 两个字段都不合法(手改坏了)→ 暗色
    #[test]
    fn invalid_modes_fall_back_to_dark() {
        let mut s: Settings =
            serde_json::from_str(r#"{"theme":"purple","preferredMode":"blue"}"#).unwrap();
        s.normalize_preferred_mode();
        assert_eq!(s.preferred_mode, "dark");
        let mut from_theme: Settings =
            serde_json::from_str(r#"{"theme":"light","preferredMode":""}"#).unwrap();
        from_theme.normalize_preferred_mode();
        assert_eq!(from_theme.preferred_mode, "light");
    }

    /// load() 负责补齐偏好模式(前端 get_settings 拿到的就是补齐后的值)
    #[test]
    fn load_fills_in_the_preferred_mode() {
        let dir = std::env::temp_dir().join(format!("otr-pref-mode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(&path, r#"{"theme":"light","themeId":"warm-paper","migratedV2":true}"#)
            .unwrap();
        let s = Settings::load(&path);
        assert_eq!(s.preferred_mode, "light");
        assert_eq!(s.theme_id, "warm-paper");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 托盘「恢复默认主题」:只动主题相关字段,生效模式回到偏好模式,其余设置原样保留
    #[test]
    fn reset_theme_only_touches_the_theme() {
        let mut s = Settings::default();
        s.theme_id = "neon-night".into();
        s.theme = "dark".into();
        s.preferred_mode = "light".into();
        s.enabled_agents = vec!["dsh".into()];
        s.start_minimized = true;
        s.currency = "USD".into();
        s.pricing.insert("m".into(), PriceEntry { input: 1.0, ..Default::default() });
        s.limit_accounts.push(account("deepseek"));
        let before = s.clone();

        s.reset_theme();
        assert_eq!(s.theme_id, DEFAULT_THEME_ID);
        assert_eq!(s.theme, "light", "默认主题两种模式都有,生效模式 = 偏好模式");
        assert_eq!(s.preferred_mode, "light");
        assert_eq!(s.enabled_agents, before.enabled_agents);
        assert_eq!(s.start_minimized, before.start_minimized);
        assert_eq!(s.currency, before.currency);
        assert_eq!(s.pricing, before.pricing);
        assert_eq!(s.limit_accounts, before.limit_accounts);

        // 旧文件没有偏好模式:按 theme 推导后再重置
        let mut old: Settings =
            serde_json::from_str(r#"{"theme":"light","themeId":"warm-paper"}"#).unwrap();
        old.reset_theme();
        assert_eq!(old.theme_id, DEFAULT_THEME_ID);
        assert_eq!(old.theme, "light");
    }

    /// 重置后落盘再读回:themeId 是 otr,文件里其它字段(含未知的旧字段之外的所有设置)不丢
    #[test]
    fn reset_theme_round_trips_through_the_settings_file() {
        let dir = std::env::temp_dir().join(format!("otr-reset-theme-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(
            &path,
            r#"{"enabledAgents":["dsh","codex"],"theme":"light","preferredMode":"dark","themeId":"broken-theme","currency":"USD","exchangeRate":6.9,"startMinimized":true,"migratedV2":true}"#,
        )
        .unwrap();
        let mut s = Settings::load(&path);
        s.reset_theme();
        s.save(&path).unwrap();
        let back = Settings::load(&path);
        assert_eq!(back.theme_id, DEFAULT_THEME_ID);
        assert_eq!(back.theme, "dark");
        assert_eq!(back.preferred_mode, "dark");
        assert!(back.enabled_agents.iter().any(|a| a == "codex"));
        assert_eq!(back.currency, "USD");
        assert_eq!(back.exchange_rate, 6.9);
        assert!(back.start_minimized);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 往返:账号配置存下来再读回来必须一致
    #[test]
    fn limit_accounts_round_trip_through_json() {
        let mut s = Settings::default();
        let mut a = account("codex");
        a.home = Some("D:/profiles/codex-work".into());
        a.plan = "Pro".into();
        a.cookie_ref = Some("limit.stepfun.stepfun-2.cookie".into());
        s.limit_accounts.push(a.clone());
        let text = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&text).unwrap();
        assert_eq!(back.limit_accounts, vec![a]);
    }

    /// 保存主题、定价这类设置时,不能用打开页面时的旧快照盖掉刚加的账号。
    #[test]
    fn other_settings_saves_keep_the_live_limit_config() {
        let mut live = Settings::default();
        live.limit_providers.push("stepfun".into());
        live.limit_accounts.push(LimitAccount {
            id: "stepfun-2".into(),
            provider: "stepfun".into(),
            label: "备用".into(),
            cookie_ref: Some("limit.stepfun.stepfun-2.cookie".into()),
            ..Default::default()
        });
        let mut stale = Settings::default();
        stale.theme = "light".into();
        stale.keep_limit_config_from(&live);
        assert_eq!(stale.theme, "light");
        assert_eq!(stale.limit_accounts, live.limit_accounts);
        assert!(stale.limit_providers.iter().any(|p| p == "stepfun"));
    }

    /// 旧设置文件升级时自动打开千问一次;用户之后关掉,下次启动不再打开。
    #[test]
    fn qwen_is_enabled_once_for_existing_settings() {
        let dir = std::env::temp_dir().join(format!("otr-qwen-settings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(
            &path,
            r#"{"enabledAgents":["dsh"],"limitProviders":["cursor","codex"],"currency":"CNY","exchangeRate":7.2}"#,
        )
        .unwrap();

        let first = Settings::load(&path);
        assert!(first.limit_providers.iter().any(|p| p == "qwen"));
        assert!(
            !first.limit_providers.iter().any(|p| p == "deepseek"),
            "用户没开的来源不能被升级重新打开"
        );
        assert_eq!(
            first.limit_providers.iter().filter(|p| *p == "qwen").count(),
            1
        );

        let mut turned_off = Settings::load(&path);
        turned_off.limit_providers.retain(|p| p != "qwen");
        turned_off.save(&path).unwrap();
        let again = Settings::load(&path);
        assert!(
            !again.limit_providers.iter().any(|p| p == "qwen"),
            "关掉之后不能在下次启动时复活"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
