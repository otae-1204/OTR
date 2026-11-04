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
    /// 该按天值是**当天绝对总量**,而不是相对上次的增量 → 写库时先删后写(替换)。
    /// 台账首次看到某天(没有旧基线)时就是这种情况。
    /// 不替换的话,与"日志回填"写下的同一行相加会双计。
    #[serde(default)]
    pub absolute_daily: bool,

    /// 覆盖按天/小时桶日期(例如 DSH 会话快照的日期)
    #[serde(default)]
    pub bucket_date: Option<String>,
    /// 覆盖小时桶(0-23);缺省从 ts 的本地时间计算
    #[serde(default)]
    pub bucket_hour: Option<i64>,
    /// 会话"最后活跃"时间;缺省用 ts
    #[serde(default)]
    pub touch_ts: Option<i64>,
    /// 真实吐字：第一个字到最后一个字的输出 token。0 = 这次没有真实速度样本。
    #[serde(default)]
    pub gen_output: u64,
    /// 真实吐字耗时，毫秒。不含首字之前的等待。
    #[serde(default)]
    pub gen_ms: u64,
    /// 计入真实速度的请求次数。
    #[serde(default)]
    pub gen_samples: u64,
    /// 估算速度：整段请求（可含首字等待）覆盖的输出 token。
    #[serde(default)]
    pub est_output: u64,
    #[serde(default)]
    pub est_ms: u64,
    #[serde(default)]
    pub est_samples: u64,
    /// 首字延迟合计，毫秒。展示时除以 ttft_samples。
    #[serde(default)]
    pub ttft_ms: u64,
    #[serde(default)]
    pub ttft_samples: u64,
}

/// 单次生成耗时的可信区间。短于 50ms 分不清是不是时钟粒度；
/// 长于 30 分钟多半是卡住的流，放进平均会把速度拉成接近 0。
pub fn plausible_gen_ms(ms: i64) -> Option<u64> {
    if (50..=30 * 60 * 1000).contains(&ms) {
        Some(ms as u64)
    } else {
        None
    }
}

/// 首字延迟：请求开始到第一个字。短于 1ms 没有意义，长于 3 分钟多半对错了时钟。
pub fn plausible_ttft_ms(ms: i64) -> Option<u64> {
    if (1..=180_000).contains(&ms) {
        Some(ms as u64)
    } else {
        None
    }
}

impl UsageRecord {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens + self.cache_read_tokens + self.cache_write_tokens
    }

    /// 这条记录要写入速度表。估算耗时可以单独增加(流式补齐),不必同时带上输出 token。
    pub fn writes_speed(&self) -> bool {
        (self.gen_ms > 0 && self.gen_output > 0)
            || self.est_ms > 0
            || self.est_output > 0
            || self.ttft_samples > 0
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
    /// 真实吐字覆盖的输出 token。速度 = 它 ÷ gen_ms。
    pub gen_output_tokens: u64,
    pub gen_ms: u64,
    pub gen_samples: u64,
    /// 估算速度（整段请求，可含首字等待）覆盖的输出 token。
    pub est_output_tokens: u64,
    pub est_ms: u64,
    pub est_samples: u64,
    /// 首字延迟合计与样本数。平均延迟 = ttft_ms ÷ ttft_samples。
    pub ttft_ms: u64,
    pub ttft_samples: u64,
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
    /// 数据最后一次真正变化的时间(kv data_updated_ms);从未写过数据时回退到当前时间
    pub generated_at: i64,
    /// 只在真的写入用量时递增的版本号。前端拿它当刷新键:
    /// 空闲轮询不再连锁触发明细查询,数据没变时子组件也不重拉。
    pub data_version: i64,
    pub by_agent_today: Vec<AgentSlice>,
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
    /// 全部时间累计 tokens(卡片右侧那个"累计")
    pub total_tokens: u64,
    /// 需要用户注意的健康提示(登录失效/分页截断…);None = 正常
    pub notice: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyUsage {
    pub date: String,
    pub agent: String,
    /// 按模型拆分时是模型名;按 Agent 汇总时为空。
    #[serde(default)]
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub calls: u64,
    pub total_tokens: u64,
    pub cost: f64,
}

/// 一天中的某个小时,把范围内各天的同一小时加在一起。hour 为 0–23。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HourProfile {
    pub hour: i64,
    pub total_tokens: u64,
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
