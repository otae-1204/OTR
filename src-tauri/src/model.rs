use serde::{Deserialize, Serialize};

/// 归一化后的用量记录,各 Provider 解析产物;增量语义(是"新增量"而非累计值)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsageRecord {
    pub agent: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    /// unix ms
    #[serde(default)]
    pub ts: i64,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_write_tokens: u64,
    /// 通常已含在 output 内,单独展示不加总
    #[serde(default)]
    pub reasoning_tokens: u64,
    #[serde(default)]
    pub calls: u64,
    #[serde(default)]
    pub cost: f64,
    /// 只写会话表、不写按天表(如 DSH projcache,按天数据另由 ledger 提供,避免双计)
    #[serde(default)]
    pub skip_daily: bool,
    /// 不写按小时表(按天快照没有可靠的小时信息时使用)
    #[serde(default)]
    pub skip_hourly: bool,
    /// 覆盖按天/小时桶日期(例如 DSH 会话快照的日期)
    #[serde(default)]
    pub bucket_date: Option<String>,
    /// 覆盖小时桶(0-23);缺省从 ts 的本地时间计算
    #[serde(default)]
    pub bucket_hour: Option<i64>,
    /// 会话"最后活跃"时间;缺省用 ts
    #[serde(default)]
    pub touch_ts: Option<i64>,
}

impl UsageRecord {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.cache_read_tokens + self.cache_write_tokens
    }
}

pub fn iso_to_ms(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.timestamp_millis())
}

pub fn local_date(ts_ms: i64) -> String {
    use chrono::TimeZone;
    if ts_ms <= 0 {
        return chrono::Local::now().format("%Y-%m-%d").to_string();
    }
    chrono::Local
        .timestamp_millis_opt(ts_ms)
        .single()
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

pub fn local_hour(ts_ms: i64) -> i64 {
    use chrono::{TimeZone, Timelike};
    let ts = if ts_ms > 0 { ts_ms } else { now_ms() };
    chrono::Local
        .timestamp_millis_opt(ts)
        .single()
        .map(|d| d.hour() as i64)
        .unwrap_or(0)
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub fn today_str() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

pub fn date_str(days_ago: chrono::Duration) -> String {
    (chrono::Local::now().date_naive() - days_ago)
        .format("%Y-%m-%d")
        .to_string()
}

/// 1234567 -> "1.2M"
pub fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000_000 {
        format!("{:.2}B", n as f64 / 1e9)
    } else if n >= 1_000_000 {
        format!("{:.2}M", n as f64 / 1e6)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub calls: u64,
    pub total_tokens: u64,
    pub cost: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSlice {
    pub agent: String,
    pub totals: Totals,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSlice {
    pub model: String,
    pub totals: Totals,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub generated_at: i64,
    pub today: Totals,
    pub week: Totals,
    pub month: Totals,
    pub all_time: Totals,
    pub by_agent_today: Vec<AgentSlice>,
    pub by_model_month: Vec<ModelSlice>,
}

/// 任意日期范围(可按 Agent 过滤)的统计
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RangeSummary {
    pub generated_at: i64,
    pub from: String,
    pub to: String,
    pub agent: Option<String>,
    /// 本结果的成本币种(前端按 settings.currency 切换显示)
    pub currency: String,
    pub totals: Totals,
    pub by_agent: Vec<AgentSlice>,
    pub by_model: Vec<ModelSlice>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentStatus {
    pub id: String,
    pub display_name: String,
    pub detected: bool,
    pub enabled: bool,
    pub today_tokens: u64,
    pub today_cost: f64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyUsage {
    pub date: String,
    pub agent: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub calls: u64,
    pub total_tokens: u64,
    pub cost: f64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionUsage {
    pub agent: String,
    pub session_id: Option<String>,
    pub project: Option<String>,
    pub title: Option<String>,
    pub models: Option<String>,
    pub started_at: Option<i64>,
    pub last_active: Option<i64>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub calls: u64,
    pub total_tokens: u64,
    pub cost: f64,
}
