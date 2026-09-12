use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;
use crate::model::{local_date, local_hour, now_ms, UsageRecord};
use crate::paths;
use crate::providers::jsonl_util::{f64f, u64f};
use crate::providers::{AgentProvider, ScanCtx};

pub struct DshProvider;

const AGENT: &str = "dsh";
pub const PARSER_VERSION: u64 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum DshDailySource {
    Ledger,
    SessionLogs,
}

impl AgentProvider for DshProvider {
    fn id(&self) -> &'static str {
        AGENT
    }

    fn display_name(&self) -> &'static str {
        "DSH"
    }

    fn detect(&self) -> bool {
        paths::dsh_storages().is_dir()
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        vec![paths::dsh_storages(), paths::dsh_home().join("sessions")]
    }

    fn parser_version(&self) -> u64 {
        PARSER_VERSION
    }

    /// DSH 台账(cost-meter)记的是人民币实际计费金额
    fn native_cost_currency(&self) -> Option<&'static str> {
        Some("CNY")
    }

    fn scan(&self, ctx: &mut ScanCtx) -> Result<Vec<UsageRecord>> {
        let mut st: DshState =
            serde_json::from_value(std::mem::take(ctx.state)).unwrap_or_default();
        let mut records = Vec::new();

        let ledger = match load_ledger() {
            Ok(value) => value,
            Err(e) => {
                eprintln!("[dsh] ledger: {}", e);
                None
            }
        };
        let mut session_paths = Vec::new();
        collect_session_logs(&paths::dsh_home().join("sessions"), &mut session_paths);
        let daily_source = select_daily_source(
            &mut st,
            ledger.as_ref().is_some_and(ledger_has_daily_data),
            !session_paths.is_empty(),
        );

        // 有 cost-meter 台账时继续以其作为按天权威数据;无台账设备则由原始日志回退。
        if daily_source == Some(DshDailySource::Ledger) {
            if let Some(value) = ledger.as_ref() {
                scan_ledger(&mut st, &mut records, value);
            }
        }
        // 按小时表始终使用会话日志的真实事件时间;回退模式下同一增量也写入按天表。
        let logs_available = match scan_session_logs(
            &mut st,
            &mut records,
            &session_paths,
            daily_source == Some(DshDailySource::SessionLogs),
        ) {
            Ok(available) => available,
            Err(e) => {
                eprintln!("[dsh] session logs: {}", e);
                false
            }
        };
        // 某些旧设备只有台账没有会话日志,至少保留会话 at 的降级数据。
        if !logs_available {
            if let Some(value) = ledger.as_ref() {
                scan_ledger_sessions(&mut st, &mut records, value);
            }
        }
        // 会话表数据源:session_projcache.json(按会话×模型;不再写日/小时,避免与选定数据源双计)
        if let Err(e) = scan_projcache(&mut st, &mut records, &paths::dsh_storages()) {
            eprintln!("[dsh] projcache: {}", e);
        }

        *ctx.state = serde_json::to_value(&st).unwrap_or(Value::Null);
        Ok(records)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct DshEntry {
    #[serde(default)]
    input: u64,
    #[serde(default)]
    output: u64,
    #[serde(default)]
    cache_read: u64,
    #[serde(default)]
    cache_write: u64,
    #[serde(default)]
    reasoning: u64,
    #[serde(default)]
    calls: u64,
    #[serde(default)]
    cost: f64,
}

impl DshEntry {
    fn from_json(v: &Value) -> Self {
        DshEntry {
            input: u64f(v, "input"),
            output: u64f(v, "output"),
            cache_read: u64f(v, "cacheRead"),
            cache_write: u64f(v, "cacheWrite"),
            reasoning: u64f(v, "reasoning"),
            calls: u64f(v, "calls"),
            cost: f64f(v, "cost"),
        }
    }

    fn delta_from(&self, prev: &DshEntry) -> DshEntry {
        DshEntry {
            input: self.input.saturating_sub(prev.input),
            output: self.output.saturating_sub(prev.output),
            cache_read: self.cache_read.saturating_sub(prev.cache_read),
            cache_write: self.cache_write.saturating_sub(prev.cache_write),
            reasoning: self.reasoning.saturating_sub(prev.reasoning),
            calls: self.calls.saturating_sub(prev.calls),
            cost: self.cost - prev.cost,
        }
    }

    /// "这次没有新增用量"的判据。
    /// reasoning 只展示、不计入 total_tokens,但它同样是"这次调用产生了用量"的证据;
    /// 高水位保护(floor 语义)是按字段逐项 max 的、包含 reasoning,如果这里不看它,
    /// 只有 reasoning 变化的增量就会被保护住却永远不产出记录 —— 两边口径必须一致。
    fn is_zero(&self) -> bool {
        self.input + self.output + self.cache_read + self.cache_write + self.reasoning == 0
            && self.calls == 0
            && self.cost.abs() < f64::EPSILON
    }

    /// 同一个 "日期|小时|模型" 桶在多个会话文件上的合并
    fn merge(&mut self, other: &DshEntry) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
        self.reasoning += other.reasoning;
        self.calls += other.calls;
        self.cost += other.cost;
    }
}

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct DshState {
    /// 首次发现的可用按天数据源;运行中不自动切换,避免 ledger 出现/消失时重复累计。
    #[serde(default)]
    daily_source: Option<DshDailySource>,
    /// key: "日期|provider:model"
    #[serde(default)]
    ledger: HashMap<String, DshEntry>,
    /// key: "日期|会话 id|provider:model",用于无日志设备的小时降级分桶
    #[serde(default)]
    ledger_sessions: HashMap<String, DshEntry>,
    /// 从 DSH 会话日志重建的绝对小时聚合,key: "日期|小时|provider:model"
    #[serde(default)]
    hourly: HashMap<String, DshEntry>,
    /// key: "session_id|model"
    #[serde(default)]
    sessions: HashMap<String, DshEntry>,
    /// 会话日志的 per-file 解析缓存,key = 文件绝对路径
    #[serde(default)]
    file_cache: HashMap<String, DshFileCache>,
    /// projcache 文件的 (size, mtime) 指纹,key = 文件绝对路径。
    /// 现行布局把每个会话放一个文件(本机 2254 个),不缓存就会每次扫描都重新
    /// 读盘 + 解析 JSON,"热扫"从 ~0.5s 退化到 ~2s。
    #[serde(default)]
    projcache_cache: HashMap<String, DshFileStamp>,
}

/// 一个 projcache 文件的 (size, mtime) 指纹。
/// 只存指纹、不存解析摘要:文件没变时按 st.sessions 算出的 delta 必然为 0(不产出记录),
/// 所以"跳过"与"重算"的产出完全一致,而指纹体积可以忽略。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct DshFileStamp {
    #[serde(default)]
    size: u64,
    #[serde(default)]
    mtime_ms: i64,
}

/// 单个会话文件的绝对小时聚合缓存。
/// 缓存的是**聚合结果**而不是日志正文:本机 ~/.dsh/sessions 有 2232 个文件 / 166MB,
/// 正文不可能进 kv;而聚合每个文件只有少量 "日期|小时|provider:model" 桶(单桶约 130 字节)。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct DshFileCache {
    #[serde(default)]
    size: u64,
    #[serde(default)]
    mtime_ms: i64,
    #[serde(default)]
    aggregate: HashMap<String, DshEntry>,
}

/// state:dsh 是 kv 表里的一整行 JSON,每次扫描都要读写一次,必须封顶。
/// 双重上限:文件数管条目开销,桶数管真正的体积来源。
/// 淘汰顺序用 mtime_ms(最近写过的日志才是热数据)——不额外存"最近使用时间",
/// 否则 state 每次扫描都会变,白白把整行 kv 重写一遍。
const MAX_CACHED_FILES: usize = 4096;
const MAX_CACHED_BUCKETS: usize = 32768;

/// projcache 行可能是 {val: ...} 包装,也可能是裸对象
fn row_val<'a>(row: &'a Value) -> &'a Value {
    row.get("val").unwrap_or(row)
}

fn read_json_file(path: &Path) -> Result<Value> {
    let mut last_error = None;
    for attempt in 0..3 {
        match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<Value>(&text) {
                Ok(value) => return Ok(value),
                Err(error) => last_error = Some(error),
            },
            Err(error) => return Err(error.into()),
        }
        if attempt < 2 {
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
    }
    Err(last_error
        .expect("JSON parse must fail before retry exhaustion")
        .into())
}

fn load_ledger() -> Result<Option<Value>> {
    let path = paths::dsh_storages().join("cost-meter").join("ledger.json");
    if !path.is_file() {
        return Ok(None);
    }
    read_json_file(&path).map(Some)
}

fn ledger_has_daily_data(value: &Value) -> bool {
    value
        .get("days")
        .and_then(|days| days.as_object())
        .is_some_and(|days| {
            days.values().any(|day| {
                day.get("byProviderModel")
                    .and_then(|models| models.as_object())
                    .is_some_and(|models| !models.is_empty())
            })
        })
}

fn select_daily_source(
    state: &mut DshState,
    ledger_available: bool,
    logs_available: bool,
) -> Option<DshDailySource> {
    if state.daily_source.is_none() {
        state.daily_source = if ledger_available {
            Some(DshDailySource::Ledger)
        } else if logs_available {
            Some(DshDailySource::SessionLogs)
        } else {
            None
        };
    }
    state.daily_source
}

fn date_start_ms(date: &str) -> i64 {
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .and_then(|nd| nd.and_local_timezone(chrono::Local).single())
        .map(|dt| dt.timestamp_millis())
        .unwrap_or_else(now_ms)
}

fn split_model_key(mkey: &str) -> (Option<String>, Option<String>) {
    match mkey.split_once(':') {
        Some((provider, model)) => (Some(provider.to_string()), Some(model.to_string())),
        None => (None, Some(mkey.to_string())),
    }
}

fn record_from_entry(
    entry: &DshEntry,
    model_key: &str,
    ts: i64,
    bucket_date: Option<String>,
    bucket_hour: Option<i64>,
) -> UsageRecord {
    let (provider, model) = split_model_key(model_key);
    UsageRecord {
        agent: AGENT.into(),
        model,
        provider,
        ts,
        input_tokens: entry.input,
        output_tokens: entry.output,
        cache_read_tokens: entry.cache_read,
        cache_write_tokens: entry.cache_write,
        reasoning_tokens: entry.reasoning,
        calls: entry.calls,
        cost: entry.cost,
        bucket_date,
        bucket_hour,
        ..Default::default()
    }
}

fn scan_ledger(st: &mut DshState, out: &mut Vec<UsageRecord>, ledger: &Value) {
    let Some(days) = ledger.get("days").and_then(|d| d.as_object()) else {
        return;
    };
    let mut alive: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (date, day) in days {
        if let Some(models) = day.get("byProviderModel").and_then(|m| m.as_object()) {
            let ts = date_start_ms(date);
            for (mkey, m) in models {
                let key = format!("{}|{}", date, mkey);
                alive.insert(key.clone());
                let cur = DshEntry::from_json(m);
                let prev = st.ledger.get(&key).cloned().unwrap_or_default();
                let delta = cur.delta_from(&prev);
                if !delta.is_zero() {
                    let mut record = record_from_entry(&delta, mkey, ts, None, None);
                    record.skip_hourly = true;
                    out.push(record);
                }
                st.ledger.insert(key, cur);
            }
        }
    }
    // 台账里消失的日期不再保留基准(避免状态无限膨胀)
    st.ledger.retain(|k, _| alive.contains(k));
}

fn scan_ledger_sessions(st: &mut DshState, out: &mut Vec<UsageRecord>, ledger: &Value) {
    let Some(days) = ledger.get("days").and_then(|d| d.as_object()) else {
        return;
    };
    let mut alive = std::collections::HashSet::new();
    for (date, day) in days {
        let Some(sessions) = day.get("sessions").and_then(|s| s.as_array()) else {
            continue;
        };
        for session in sessions {
            let Some(sid) = session.get("id").and_then(|s| s.as_str()) else {
                continue;
            };
            let candidate_ts = session
                .get("at")
                .and_then(|at| at.as_i64())
                .filter(|at| *at > 0);
            let ts = candidate_ts
                .filter(|at| local_date(*at) == *date)
                .unwrap_or_else(|| date_start_ms(date));
            let hour = local_hour(ts);
            let Some(models) = session.get("byProviderModel").and_then(|m| m.as_object()) else {
                continue;
            };
            for (mkey, value) in models {
                let key = format!("{}|{}|{}", date, sid, mkey);
                alive.insert(key.clone());
                let cur = DshEntry::from_json(value);
                let prev = st.ledger_sessions.get(&key).cloned().unwrap_or_default();
                let delta = cur.delta_from(&prev);
                if !delta.is_zero() {
                    let mut record =
                        record_from_entry(&delta, mkey, ts, Some(date.clone()), Some(hour));
                    record.skip_daily = true;
                    out.push(record);
                }
                st.ledger_sessions.insert(key, cur);
            }
        }
    }
    st.ledger_sessions.retain(|key, _| alive.contains(key));
}

/// 会话日志文件名的**代次**与是否压缩。
/// DSH 的会话格式有"代"的概念(见 @deepseek-ai/dsh-session-format-catalog):
/// 第 0 代叫 `session.jsonl`,之后是 `session.v{n}.jsonl`,压缩后统一加 `.zstd`。
/// 以前这里硬编码两个文件名,于是格式升到 v3 之后(只有 `session.v3.jsonl.zstd`)
/// 新会话一个都发现不了 —— 而按小时桶**只**由这条扫描产生,表现为按小时表整块消失。
fn parse_log_name(name: &str) -> Option<(u64, bool)> {
    let rest = name.strip_prefix("session")?;
    let (generation, rest) = if let Some(rest) = rest.strip_prefix(".jsonl") {
        (0u64, rest)
    } else {
        let rest = rest.strip_prefix(".v")?;
        let (digits, rest) = rest.split_once(".jsonl")?;
        (digits.parse::<u64>().ok()?, rest)
    };
    let zstd = match rest {
        "" => false,
        ".zstd" => true,
        _ => return None,
    };
    Some((generation, zstd))
}

/// 逐条目容错收集会话日志(参考 jsonl_util::collect_jsonl 的写法)。
/// 以前整条链路上任何一次 read_dir / file_type 失败都会让整个 discovery 返回 Err,
/// 调用方只能把 session_paths 留空 → 小时数据整块消失。单个坏目录不该有这种杀伤力。
///
/// **每个会话目录只取代次最高的那一个文件**:格式升级后同一段历史会以新代次
/// 重新完整编码(实测 10 个双格式目录里 v3 全是旧格式的无损超集,旧的独有事件 0 个),
/// 两个都扫会把该会话的用量翻倍。同代次同时存在 `.jsonl` 与 `.jsonl.zstd` 时取压缩版
/// (DSH 现行写法就是压缩的)。
fn collect_session_logs(root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(projects) = std::fs::read_dir(root) else {
        return;
    };
    for project in projects.flatten() {
        let Ok(kind) = project.file_type() else {
            continue;
        };
        if !kind.is_dir() {
            continue;
        }
        let Ok(sessions) = std::fs::read_dir(project.path()) else {
            eprintln!("[dsh] 会话目录不可读,跳过: {}", project.path().display());
            continue;
        };
        for session in sessions.flatten() {
            let Ok(kind) = session.file_type() else {
                continue;
            };
            if !kind.is_dir() {
                continue;
            }
            let Ok(entries) = std::fs::read_dir(session.path()) else {
                eprintln!("[dsh] 会话目录不可读,跳过: {}", session.path().display());
                continue;
            };
            let mut best: Option<(u64, bool, PathBuf)> = None;
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let Some((generation, zstd)) = parse_log_name(&name) else {
                    continue;
                };
                // 用 read_dir 已经带回来的文件类型,别再对路径做一次 stat:
                // Windows 上目录枚举已经缓存了属性,DirEntry::file_type() 近乎免费,
                // 而 path.is_file() 是每个文件一次完整的 CreateFile/GetFileAttributes
                // (本机 2255 个文件实测 222.6ms vs 1.48ms,差 150 倍)。
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if !kind.is_file() {
                    continue;
                }
                let better = match &best {
                    None => true,
                    Some((gen, was_zstd, _)) => {
                        (generation, zstd) > (*gen, *was_zstd)
                    }
                };
                if better {
                    best = Some((generation, zstd, entry.path()));
                }
            }
            if let Some((_, _, path)) = best {
                out.push(path);
            }
        }
    }
}

/// zstd 帧魔数(小端 0x28 0xB5 0x2F 0xFD)
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

/// 按**魔数**而不是扩展名判断是否 zstd 压过。
/// 只看扩展名时,一个被改名/后缀异常(或被工具重写)的 zstd 文件会被当成 UTF-8:
/// 解出来是乱码 → 每行 serde_json 解析失败 → 函数仍返回 true 但聚合为空
/// → st.hourly 被空表覆盖 → 静默丢量。魔数判断让这条路走不通。
fn read_session_text(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path)?;
    let decoded = if bytes.starts_with(&ZSTD_MAGIC) {
        zstd::stream::decode_all(bytes.as_slice())?
    } else {
        bytes
    };
    Ok(String::from_utf8_lossy(&decoded).into_owned())
}

fn scan_session_file(path: &Path, aggregate: &mut HashMap<String, DshEntry>) -> bool {
    let Ok(text) = read_session_text(path) else {
        return false;
    };
    let mut provider = "deepseek".to_string();
    let mut model = "default".to_string();
    let mut created_at = 0i64;
    let mut samples: HashMap<String, (i64, String, DshEntry)> = HashMap::new();
    for line in text.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        match event.get("type").and_then(|v| v.as_str()) {
            Some("session") => {
                created_at = event.get("createdAt").and_then(|v| v.as_i64()).unwrap_or(0);
                continue;
            }
            Some("request/header") => {
                if let Some(config) = event.pointer("/data/header/config") {
                    if let Some(value) = config.get("provider").and_then(|v| v.as_str()) {
                        if !value.is_empty() {
                            provider = value.to_string();
                        }
                    }
                    if let Some(value) = config.get("model").and_then(|v| v.as_str()) {
                        if !value.is_empty() {
                            model = value.to_string();
                        }
                    }
                }
                continue;
            }
            _ => {}
        }
        let event_time = event.get("time").and_then(|v| v.as_i64()).unwrap_or(0);
        if event_time <= 0 || (created_at > 0 && event_time < created_at) {
            continue;
        }
        let (usage, turn, step) =
            if event.get("type").and_then(|v| v.as_str()) == Some("assistant/message") {
                (
                    event.pointer("/data/usage"),
                    event
                        .pointer("/data/turn")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    event
                        .pointer("/data/step")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                )
            } else if event.get("type").and_then(|v| v.as_str()) == Some("assistant/chunk")
                && event.pointer("/data/chunk/type").and_then(|v| v.as_str()) == Some("usage")
            {
                (
                    event.pointer("/data/chunk/usage"),
                    event
                        .pointer("/data/turn")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                    event
                        .pointer("/data/step")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                )
            } else {
                continue;
            };
        let Some(usage) = usage else { continue };
        let entry = DshEntry {
            input: u64f(usage, "inputTokens"),
            output: u64f(usage, "outputTokens"),
            cache_read: u64f(usage, "cacheReadTokens"),
            cache_write: u64f(usage, "cacheWriteTokens"),
            reasoning: u64f(usage, "reasoningTokens"),
            calls: 1,
            cost: 0.0,
        };
        let key = format!("{}:{}", turn, step);
        samples.insert(key, (event_time, format!("{}:{}", provider, model), entry));
    }
    for (_sample_key, (ts, model_key, entry)) in samples {
        let key = format!("{}|{}|{}", local_date(ts), local_hour(ts), model_key);
        let bucket = aggregate.entry(key).or_default();
        bucket.input += entry.input;
        bucket.output += entry.output;
        bucket.cache_read += entry.cache_read;
        bucket.cache_write += entry.cache_write;
        bucket.reasoning += entry.reasoning;
        bucket.calls += entry.calls;
    }
    true
}

/// 取一个会话文件的小时聚合:文件 (size, mtime) 未变则直接复用缓存,
/// 跳过读盘 + zstd 解压 + 逐行 JSON 解析(本机 2232 个文件时这是全量扫描的绝大部分开销)。
/// 返回 None 表示该文件本次读不出来。
fn session_file_aggregate(st: &mut DshState, path: &Path) -> Option<HashMap<String, DshEntry>> {
    let key = path.to_string_lossy().to_string();
    let meta = std::fs::metadata(path).ok()?;
    let size = meta.len();
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    if let Some(hit) = st.file_cache.get(&key) {
        if hit.size == size && hit.mtime_ms == mtime_ms {
            return Some(hit.aggregate.clone());
        }
    }

    let mut aggregate = HashMap::new();
    if !scan_session_file(path, &mut aggregate) {
        // 读失败时保留旧缓存:它的桶集合是本次 floor 保护的依据
        return None;
    }
    st.file_cache.insert(
        key,
        DshFileCache {
            size,
            mtime_ms,
            aggregate: aggregate.clone(),
        },
    );
    Some(aggregate)
}

/// 按 LRU 把文件缓存收敛到上限内;本次扫描触及过的文件优先保留
fn trim_file_cache(
    cache: &mut HashMap<String, DshFileCache>,
    scanned: &std::collections::HashSet<String>,
) {
    let buckets: usize = cache.values().map(|c| c.aggregate.len()).sum();
    if cache.len() <= MAX_CACHED_FILES && buckets <= MAX_CACHED_BUCKETS {
        return;
    }
    let mut order: Vec<(bool, i64, String)> = cache
        .iter()
        .map(|(k, v)| (scanned.contains(k), v.mtime_ms, k.clone()))
        .collect();
    // 元组排序:false(本次未触及)排前面,同组内 mtime 越旧越先淘汰
    order.sort();
    let mut buckets = buckets;
    for (_, _, key) in order {
        if cache.len() <= MAX_CACHED_FILES && buckets <= MAX_CACHED_BUCKETS {
            break;
        }
        if let Some(removed) = cache.remove(&key) {
            buckets -= removed.aggregate.len();
        }
    }
    eprintln!(
        "[dsh] 文件缓存超出上限,按 LRU 收敛到 {} 个文件 / {} 个桶",
        cache.len(),
        buckets
    );
}

fn scan_session_logs(
    st: &mut DshState,
    out: &mut Vec<UsageRecord>,
    paths: &[PathBuf],
    include_daily: bool,
) -> Result<bool> {
    if paths.is_empty() {
        return Ok(false);
    }
    let mut current: HashMap<String, DshEntry> = HashMap::new();
    let mut usable = false;
    // 本次读失败、但缓存里还留着旧聚合的文件
    let mut failed_with_cache: Vec<String> = Vec::new();
    let mut scanned: std::collections::HashSet<String> = std::collections::HashSet::new();

    for path in paths {
        let key = path.to_string_lossy().to_string();
        scanned.insert(key.clone());
        match session_file_aggregate(st, path) {
            Some(aggregate) => {
                usable = true;
                for (bucket, entry) in aggregate {
                    current.entry(bucket).or_default().merge(&entry);
                }
            }
            None => {
                if st.file_cache.contains_key(&key) {
                    failed_with_cache.push(key);
                }
            }
        }
    }

    // 有文件读不到时,只把它**自己名下**的桶顶回旧高水位。
    // 以前是"只要有一个文件失败就对所有 key 做 floor",于是别的文件被轮转/compaction
    // 造成的计数归零会被旧高水位写回当基线,之后那个桶的增量永远 saturating_sub 成 0、
    // 无法自愈。只保护失败文件名下的桶,别的桶就能正常落到新基线。
    if !failed_with_cache.is_empty() {
        let mut protected: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for key in &failed_with_cache {
            if let Some(cache) = st.file_cache.get(key) {
                protected.extend(cache.aggregate.keys().map(String::as_str));
            }
        }
        for (bucket, previous) in &st.hourly {
            if current.contains_key(bucket) || !protected.contains(bucket.as_str()) {
                continue;
            }
            current.insert(bucket.clone(), previous.clone());
        }
    }
    trim_file_cache(&mut st.file_cache, &scanned);

    if !usable {
        // 日志文件存在但当前仍在写入/损坏时,不要切换到台账降级路径,
        // 否则日志恢复后同一增量可能被重复记入小时表。
        return Ok(true);
    }
    for (key, cur) in &current {
        let prev = st.hourly.get(key).cloned().unwrap_or_default();
        let delta = cur.delta_from(&prev);
        if delta.is_zero() {
            continue;
        }
        let mut parts = key.splitn(3, '|');
        let date = parts.next().unwrap_or_default().to_string();
        let hour = parts
            .next()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        let model_key = parts.next().unwrap_or_default();
        let mut record = record_from_entry(
            &delta,
            model_key,
            date_start_ms(&date),
            Some(date),
            Some(hour),
        );
        record.skip_daily = !include_daily;
        out.push(record);
    }
    st.hourly = current;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::{
        collect_session_logs, local_date, local_hour, scan_session_file, scan_session_logs,
        select_daily_source, DshDailySource, DshEntry, DshState, ZSTD_MAGIC,
    };
    use serde_json::Value;
    use std::collections::HashMap;
    use std::fs;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(ext: &str) -> std::path::PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("otr-dsh-{suffix}.{ext}"))
    }

    /// 造一份会话日志正文;(turn, step, input, output) 四元组各产生一条用量事件
    fn session_log(ts: i64, model: &str, entries: &[(u64, u64, u64, u64)]) -> String {
        let mut out = format!(
            "{}\n{}\n",
            serde_json::json!({"type":"session","id":"s-1","createdAt":ts - 1}),
            serde_json::json!({
                "type":"request/header","time":ts,
                "data":{"header":{"config":{"provider":"p","model":model}}}
            })
        );
        for (turn, step, input, output) in entries {
            out.push_str(&format!(
                "{}\n",
                serde_json::json!({
                    "type":"assistant/message","time":ts,
                    "data":{"turn":turn,"step":step,"usage":{"inputTokens":input,"outputTokens":output}}
                })
            ));
        }
        out
    }

    #[test]
    fn session_log_uses_event_hour_and_replaces_streaming_sample() {
        let path = temp_path("jsonl");
        let first_ts = 1_780_000_000_000i64;
        let second_ts = first_ts + 3_600_000;
        let mut file = fs::File::create(&path).unwrap();
        let events = [
            serde_json::json!({
                "type": "session", "id": "s-1", "createdAt": first_ts - 1
            }),
            serde_json::json!({
                "type": "request/header", "time": first_ts,
                "data": {"header": {"config": {"provider": "p", "model": "m"}}}
            }),
            serde_json::json!({
                "type": "assistant/message", "time": first_ts,
                "data": {"turn": 1, "step": 1, "usage": {"inputTokens": 10, "outputTokens": 2}}
            }),
            serde_json::json!({
                "type": "assistant/message", "time": second_ts,
                "data": {"turn": 1, "step": 1, "usage": {"inputTokens": 20, "outputTokens": 3}}
            }),
            serde_json::json!({
                "type": "assistant/message", "time": second_ts,
                "data": {"turn": 1, "step": 2, "usage": {"inputTokens": 7, "outputTokens": 1}}
            }),
        ];
        for event in events {
            writeln!(file, "{event}").unwrap();
        }
        let mut aggregate = HashMap::new();
        assert!(scan_session_file(&path, &mut aggregate));
        let first_key = format!("{}|{}|p:m", local_date(first_ts), local_hour(first_ts));
        let second_key = format!("{}|{}|p:m", local_date(second_ts), local_hour(second_ts));
        assert_eq!(aggregate.get(&first_key).map(|v| v.input), None);
        assert_eq!(aggregate.get(&second_key).map(|v| v.input), Some(27));
        assert_eq!(aggregate.get(&second_key).map(|v| v.output), Some(4));
        assert_eq!(aggregate.get(&second_key).map(|v| v.calls), Some(2));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn old_hourly_state_delta_is_zero_for_unchanged_snapshot() {
        let current = DshEntry {
            input: 10,
            calls: 1,
            ..Default::default()
        };
        assert!(current.delta_from(&current).is_zero());
    }

    #[test]
    fn raw_session_logs_feed_daily_when_ledger_is_unavailable() {
        let path = temp_path("jsonl");
        let ts = 1_780_000_000_000i64;
        fs::write(
            &path,
            format!(
                "{}\n{}\n{}\n",
                serde_json::json!({"type":"session","createdAt":ts - 1}),
                serde_json::json!({
                    "type":"request/header","time":ts,
                    "data":{"header":{"config":{"provider":"p","model":"m"}}}
                }),
                serde_json::json!({
                    "type":"assistant/message","time":ts,
                    "data":{"turn":1,"step":1,"usage":{"inputTokens":12,"outputTokens":3}}
                })
            ),
        )
        .unwrap();
        let mut state = DshState::default();
        let mut records = Vec::new();
        assert!(scan_session_logs(&mut state, &mut records, &[path.clone()], true).unwrap());
        assert_eq!(records.len(), 1);
        assert!(!records[0].skip_daily);
        assert!(!records[0].skip_hourly);
        assert_eq!(records[0].input_tokens, 12);

        let mut second = Vec::new();
        assert!(scan_session_logs(&mut state, &mut second, &[path.clone()], true).unwrap());
        assert!(second.is_empty());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn daily_source_does_not_switch_after_fallback_has_started() {
        let mut state = DshState::default();
        assert_eq!(
            select_daily_source(&mut state, false, true),
            Some(DshDailySource::SessionLogs)
        );
        assert_eq!(
            select_daily_source(&mut state, true, true),
            Some(DshDailySource::SessionLogs)
        );

        let mut ledger_state = DshState::default();
        assert_eq!(
            select_daily_source(&mut ledger_state, true, true),
            Some(DshDailySource::Ledger)
        );
    }

    #[test]
    fn initial_scan_keeps_usable_logs_when_another_log_is_broken() {
        let valid = temp_path("jsonl");
        let broken = temp_path("zstd");
        let ts = 1_780_000_000_000i64;
        fs::write(
            &valid,
            format!(
                "{}\n{}\n",
                serde_json::json!({
                    "type":"request/header","time":ts,
                    "data":{"header":{"config":{"provider":"p","model":"m"}}}
                }),
                serde_json::json!({
                    "type":"assistant/message","time":ts,
                    "data":{"turn":1,"step":1,"usage":{"inputTokens":5,"outputTokens":1}}
                })
            ),
        )
        .unwrap();
        // 真·损坏的 zstd:魔数正确但帧数据是垃圾,解压必然失败
        let mut corrupt = ZSTD_MAGIC.to_vec();
        corrupt.extend_from_slice(b"garbage frame");
        fs::write(&broken, corrupt).unwrap();

        let mut state = DshState::default();
        let mut records = Vec::new();
        assert!(scan_session_logs(
            &mut state,
            &mut records,
            &[valid.clone(), broken.clone()],
            true
        )
        .unwrap());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].input_tokens, 5);

        fs::remove_file(valid).unwrap();
        fs::remove_file(broken).unwrap();
    }

    #[test]
    fn session_file_cache_skips_unchanged_files_and_refreshes_on_write() {
        let path = temp_path("jsonl");
        let ts = 1_780_000_000_000i64;
        fs::write(&path, session_log(ts, "m", &[(1, 1, 10, 2)])).unwrap();

        let mut state = DshState::default();
        let mut first = Vec::new();
        assert!(scan_session_logs(&mut state, &mut first, &[path.clone()], true).unwrap());
        assert_eq!(first.iter().map(|r| r.input_tokens).sum::<u64>(), 10);
        let cached = state
            .file_cache
            .get(&path.to_string_lossy().to_string())
            .expect("首次解析必须写入 per-file 缓存");
        assert_eq!(cached.aggregate.len(), 1);

        // 文件没动:命中缓存,不应产生任何新记录
        let mut second = Vec::new();
        assert!(scan_session_logs(&mut state, &mut second, &[path.clone()], true).unwrap());
        assert!(second.is_empty(), "未变化的文件不该再产出记录");

        // 追加一次调用:size 变化 → 缓存失效,只补新增量
        fs::write(&path, session_log(ts, "m", &[(1, 1, 10, 2), (1, 2, 5, 1)])).unwrap();
        let mut third = Vec::new();
        assert!(scan_session_logs(&mut state, &mut third, &[path.clone()], true).unwrap());
        assert_eq!(third.iter().map(|r| r.input_tokens).sum::<u64>(), 5);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn rotated_log_drops_baseline_even_when_another_log_fails() {
        let a = temp_path("jsonl");
        let b = temp_path("jsonl");
        let ts = 1_780_000_000_000i64;
        fs::write(&a, session_log(ts, "m1", &[(1, 1, 10, 0)])).unwrap();
        fs::write(&b, session_log(ts, "m2", &[(1, 1, 20, 0)])).unwrap();

        let mut state = DshState::default();
        let mut first = Vec::new();
        scan_session_logs(&mut state, &mut first, &[a.clone(), b.clone()], true).unwrap();
        let key_a = format!("{}|{}|p:m1", local_date(ts), local_hour(ts));
        let key_b = format!("{}|{}|p:m2", local_date(ts), local_hour(ts));
        assert_eq!(state.hourly.get(&key_a).map(|e| e.input), Some(10));
        assert_eq!(state.hourly.get(&key_b).map(|e| e.input), Some(20));

        // A 被轮转/compaction,用量事件整段消失;B 同一刻读不出来
        fs::write(&a, session_log(ts, "m1", &[])).unwrap();
        let mut corrupt = ZSTD_MAGIC.to_vec();
        corrupt.extend_from_slice(b"garbage frame");
        fs::write(&b, corrupt).unwrap();

        let mut second = Vec::new();
        scan_session_logs(&mut state, &mut second, &[a.clone(), b.clone()], true).unwrap();

        assert!(
            state.hourly.get(&key_a).is_none(),
            "轮转归零是真实回落,必须落到新基线;旧口径会把它顶回高水位,此后该桶增量永远被 saturating_sub 成 0"
        );
        assert_eq!(
            state.hourly.get(&key_b).map(|e| e.input),
            Some(20),
            "读失败的日志要保住自己的高水位,不能倒退"
        );
        assert!(second.is_empty());

        fs::remove_file(a).unwrap();
        fs::remove_file(b).unwrap();
    }

    #[test]
    fn zstd_session_log_is_decoded_without_extension_hint() {
        // 故意用 .jsonl 后缀装 zstd 内容:按扩展名判断会解出乱码 → 静默丢量
        let path = temp_path("jsonl");
        let ts = 1_780_000_000_000i64;
        let plain = session_log(ts, "m", &[(1, 1, 12, 3)]);
        fs::write(&path, zstd::stream::encode_all(plain.as_bytes(), 3).unwrap()).unwrap();

        let mut state = DshState::default();
        let mut records = Vec::new();
        assert!(scan_session_logs(&mut state, &mut records, &[path.clone()], true).unwrap());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].input_tokens, 12);
        assert_eq!(records[0].output_tokens, 3);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn collect_session_logs_tolerates_missing_root() {
        let mut out = Vec::new();
        let missing = std::env::temp_dir().join("otr-dsh-missing-root-xyz");
        collect_session_logs(&missing, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn parse_log_name_recognizes_generations() {
        use super::parse_log_name;
        assert_eq!(parse_log_name("session.jsonl"), Some((0, false)));
        assert_eq!(parse_log_name("session.jsonl.zstd"), Some((0, true)));
        assert_eq!(parse_log_name("session.v3.jsonl.zstd"), Some((3, true)));
        assert_eq!(parse_log_name("session.v3.jsonl"), Some((3, false)));
        assert_eq!(parse_log_name("session.v12.jsonl.zstd"), Some((12, true)));
        // 非日志文件不能被误收
        assert_eq!(parse_log_name("session.v3.jsonl.zstd.tmp"), None);
        assert_eq!(parse_log_name("session.v.jsonl"), None);
        assert_eq!(parse_log_name("session.vX.jsonl"), None);
        assert_eq!(parse_log_name("events.jsonl"), None);
        assert_eq!(parse_log_name("session.jsonl.bak"), None);
    }

    /// 格式升级后同一目录里会同时留下旧代与新代日志。只取代次最高的那一个:
    /// 两个都收会让该会话的用量翻倍(实测 v3 是旧格式的无损超集)。
    #[test]
    fn collect_session_logs_keeps_only_highest_generation() {
        let root = temp_path("dir");
        let project = root.join("proj");
        let dual = project.join("session-dual");
        let v3only = project.join("session-v3only");
        let legacyonly = project.join("session-legacyonly");
        for dir in [&dual, &v3only, &legacyonly] {
            fs::create_dir_all(dir).unwrap();
        }
        fs::write(dual.join("session.jsonl.zstd"), b"old").unwrap();
        fs::write(dual.join("session.v3.jsonl.zstd"), b"new").unwrap();
        fs::write(v3only.join("session.v3.jsonl.zstd"), b"new").unwrap();
        fs::write(legacyonly.join("session.jsonl.zstd"), b"old").unwrap();
        // 噪音:非日志文件与目录都要被忽略
        fs::write(dual.join("session.v3.jsonl.zstd.tmp"), b"junk").unwrap();
        fs::create_dir_all(dual.join("nested")).unwrap();

        let mut out = Vec::new();
        collect_session_logs(&root, &mut out);
        out.sort();
        assert_eq!(
            out.len(),
            3,
            "每个会话目录只应产出一个文件,双格式目录不能两个都收: {out:?}"
        );
        assert!(out.contains(&dual.join("session.v3.jsonl.zstd")));
        assert!(out.contains(&v3only.join("session.v3.jsonl.zstd")));
        assert!(out.contains(&legacyonly.join("session.jsonl.zstd")));
        assert!(!out.contains(&dual.join("session.jsonl.zstd")));

        fs::remove_dir_all(root).unwrap();
    }

    /// 同代次同时存在 .jsonl 与 .jsonl.zstd 时取压缩版(现行 DSH 就写压缩的)
    #[test]
    fn collect_session_logs_prefers_compressed_on_tie() {
        let root = temp_path("dir");
        let dir = root.join("proj").join("session-x");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("session.v3.jsonl"), b"plain").unwrap();
        fs::write(dir.join("session.v3.jsonl.zstd"), b"packed").unwrap();

        let mut out = Vec::new();
        collect_session_logs(&root, &mut out);
        assert_eq!(out, vec![dir.join("session.v3.jsonl.zstd")]);

        fs::remove_dir_all(root).unwrap();
    }

    /// 造一份按记录目录布局的 projcache 文件(DSH 0.1.5-rc.1 现行写法)
    fn projcache_record(created: i64, last_prompt: i64, model: &str) -> Value {
        serde_json::json!({
            "version": 7,
            "record": {
                "identity": {"formatVersion": 3, "createdAt": created, "cwd": "/proj/x"},
                "rows": {
                    "title": {"ver": 1, "seq": 1, "val": "标题"},
                    "sessionListMetadata": {"ver": 1, "seq": 1, "val": {"lastPromptAt": last_prompt}},
                    "costUsage": {"ver": 4, "seq": 1, "val": {
                        "provider": "deepseek-official",
                        "model": model,
                        "byModel": {model: {"input": 100, "output": 20, "cacheRead": 5,
                                            "cacheWrite": 0, "reasoning": 1, "cost": 0.5}}
                    }}
                }
            }
        })
    }

    /// 现行布局是**按记录目录**,只读老的单一文件会让新会话一条都进不来
    /// (表现为"会话明细"最近几天整段空白)。
    #[test]
    fn scan_projcache_reads_per_record_directory() {
        let root = temp_path("dir");
        let dir = root.join("session_projcache").join("sessions");
        fs::create_dir_all(&dir).unwrap();
        let created = 1_780_000_000_000i64;
        let last_prompt = created + 7_200_000;
        fs::write(
            dir.join("session-abc.json"),
            serde_json::to_string(&projcache_record(created, last_prompt, "deepseek-v4-flash")).unwrap(),
        )
        .unwrap();
        // 坏记录不该让整张表消失
        fs::write(dir.join("session-broken.json"), b"{not json").unwrap();

        let mut st = DshState::default();
        let mut out = Vec::new();
        super::scan_projcache(&mut st, &mut out, &root).unwrap();

        assert_eq!(out.len(), 1, "应只产出坏记录之外的那一条: {out:?}");
        let r = &out[0];
        assert_eq!(r.session_id.as_deref(), Some("session-abc"));
        assert_eq!(r.input_tokens, 100);
        assert_eq!(r.output_tokens, 20);
        assert_eq!(r.model.as_deref(), Some("deepseek-v4-flash"));
        assert_eq!(r.provider.as_deref(), Some("deepseek-official"));
        assert_eq!(r.title.as_deref(), Some("标题"));
        assert_eq!(r.project.as_deref(), Some("/proj/x"));
        // 会话表按"最后活跃"过滤:必须是真实提问时间,不是扫描时刻。
        // 写成 now_ms() 的话全量重建后所有历史会话都会被算进"当天"。
        assert_eq!(
            r.touch_ts,
            Some(last_prompt),
            "最后活跃要取 sessionListMetadata.lastPromptAt,不能用 now_ms()"
        );
        assert_ne!(r.touch_ts, Some(crate::model::now_ms()));

        fs::remove_dir_all(root).unwrap();
    }

    /// 老的单文件布局要作为兜底继续可读(实测仍有 3 个会话只存在于单文件)
    #[test]
    fn scan_projcache_falls_back_to_legacy_single_file() {
        let root = temp_path("dir");
        fs::create_dir_all(&root).unwrap();
        let created = 1_780_000_000_000i64;
        let legacy = serde_json::json!({
            "tables": {"sessions": {
                "session-old": {
                    "identity": {"createdAt": created, "cwd": "/proj/old"},
                    "rows": {
                        "title": {"ver": 1, "seq": 1, "val": "老会话"},
                        "sessionListMetadata": {"ver": 1, "seq": 1, "val": {"lastPromptAt": created + 1000}},
                        "costUsage": {"ver": 4, "seq": 1, "val": {
                            "provider": "deepseek-official", "model": "m",
                            "byModel": {"m": {"input": 7, "output": 3, "cost": 0.1}}
                        }}
                    }
                }
            }}
        });
        fs::write(
            root.join("session_projcache.json"),
            serde_json::to_string(&legacy).unwrap(),
        )
        .unwrap();

        let mut st = DshState::default();
        let mut out = Vec::new();
        super::scan_projcache(&mut st, &mut out, &root).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].session_id.as_deref(), Some("session-old"));
        assert_eq!(out[0].input_tokens, 7);
        assert_eq!(out[0].title.as_deref(), Some("老会话"));
        assert_eq!(out[0].touch_ts, Some(created + 1000));

        fs::remove_dir_all(root).unwrap();
    }

    /// 两种布局同时存在时,目录里的会话不能被单文件再算一遍(否则用量翻倍)
    #[test]
    fn scan_projcache_does_not_double_count_across_layouts() {
        let root = temp_path("dir");
        let dir = root.join("session_projcache").join("sessions");
        fs::create_dir_all(&dir).unwrap();
        let created = 1_780_000_000_000i64;
        let sid = "session-same";
        let record = projcache_record(created, created + 5000, "m");
        fs::write(
            dir.join(format!("{sid}.json")),
            serde_json::to_string(&record).unwrap(),
        )
        .unwrap();
        // 单文件里同一个会话(内容一致,老布局)
        let legacy = serde_json::json!({
            "tables": {"sessions": {sid: record["record"]}}
        });
        fs::write(
            root.join("session_projcache.json"),
            serde_json::to_string(&legacy).unwrap(),
        )
        .unwrap();

        let mut st = DshState::default();
        let mut out = Vec::new();
        super::scan_projcache(&mut st, &mut out, &root).unwrap();
        assert_eq!(out.len(), 1, "同一会话只应产出一次: {out:?}");
        assert_eq!(out[0].input_tokens, 100);

        fs::remove_dir_all(root).unwrap();
    }

    /// 第二次扫描必须靠 (size, mtime) 指纹跳过重新解析:
    /// 现行布局是每个会话一个文件(本机 2254 个),不缓存的话每次扫描都要重读全部 JSON,
    /// "热扫"会从亚秒级退化到 ~2s(实测)。
    #[test]
    fn scan_projcache_skips_unchanged_files_on_rescan() {
        let root = temp_path("dir");
        let dir = root.join("session_projcache").join("sessions");
        fs::create_dir_all(&dir).unwrap();
        let created = 1_780_000_000_000i64;
        let path = dir.join("session-a.json");
        fs::write(
            &path,
            serde_json::to_string(&projcache_record(created, created + 1000, "m")).unwrap(),
        )
        .unwrap();

        let mut st = DshState::default();
        let mut first = Vec::new();
        super::scan_projcache(&mut st, &mut first, &root).unwrap();
        assert_eq!(first.len(), 1, "首次扫描要产出记录");
        assert_eq!(
            st.projcache_cache.len(),
            1,
            "首次扫描要记下指纹,供下次跳过"
        );

        // 内容未变 → 不产出任何记录(与旧实现一致:delta 为 0),但也不该再解析一次
        let mut second = Vec::new();
        super::scan_projcache(&mut st, &mut second, &root).unwrap();
        assert!(second.is_empty(), "文件没变时不该产出新记录: {second:?}");

        // 内容变了(大小也变) → 指纹失效,必须重新解析并产出增量
        let mut bumped = projcache_record(created, created + 2000, "m");
        bumped["record"]["rows"]["costUsage"]["val"]["byModel"]["m"]["input"] =
            serde_json::json!(300);
        fs::write(&path, serde_json::to_string(&bumped).unwrap()).unwrap();
        let mut third = Vec::new();
        super::scan_projcache(&mut st, &mut third, &root).unwrap();
        assert_eq!(third.len(), 1, "文件改动后必须重新解析: {third:?}");
        assert_eq!(third[0].input_tokens, 200, "增量应为 300-100");

        // 文件消失后指纹要跟着清掉,缓存不会无限增长
        fs::remove_file(&path).unwrap();
        let mut fourth = Vec::new();
        super::scan_projcache(&mut st, &mut fourth, &root).unwrap();
        assert!(
            st.projcache_cache.is_empty(),
            "文件删掉后指纹也要清理: {:?}",
            st.projcache_cache
        );

        fs::remove_dir_all(root).unwrap();
    }
}

/// 把一个会话的 projcache 记录(identity + rows)转成用量记录。
/// 现行"按记录目录"与老"单文件"两种布局共用这段逻辑,区别只在怎么拿到 identity/rows。
fn scan_projcache_session(
    st: &mut DshState,
    out: &mut Vec<UsageRecord>,
    sid: &str,
    identity: &Value,
    rows: &Value,
) {
    let created_at = identity
        .get("createdAt")
        .and_then(|x| x.as_i64())
        .unwrap_or(0);
    let cwd = identity
        .get("cwd")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string());
    // 优先 costUsage(带 provider/model/成本),缺失则退回 tokenUsage.totals
    let cost_usage = rows.get("costUsage").map(row_val);
    // 标题尽力而为
    let title = rows
        .get("title")
        .map(row_val)
        .and_then(|t| {
            t.get("title")
                .and_then(|x| x.as_str())
                .or_else(|| t.as_str())
        })
        .map(|s| s.to_string());
    // "最后活跃"用 DSH 自己记的最后一次提问时间。
    // 以前这里写 now_ms():全量重建会清空 state:dsh ⇒ 每个会话都被盖上"刚刚",
    // 于是"当天"筛选会把全部历史会话都列出来,而不是只列当天真正动过的。
    let last_active = rows
        .get("sessionListMetadata")
        .map(row_val)
        .and_then(|m| m.get("lastPromptAt"))
        .and_then(|x| x.as_i64())
        .filter(|v| *v > 0)
        .or_else(|| {
            cost_usage
                .and_then(|c| c.get("createdAt"))
                .and_then(|x| x.as_i64())
                .filter(|v| *v > 0)
        })
        .unwrap_or(created_at);
    let touch_ts = if last_active > 0 { last_active } else { now_ms() };
    let by_model = cost_usage.and_then(|c| c.get("byModel")).cloned();
    let mut used_by_model = false;
    if let Some(models) = by_model.as_ref().and_then(|v| v.as_object()) {
        used_by_model = true;
        for (model, m) in models {
            let key = format!("{}|{}", sid, model);
            let cur = DshEntry::from_json(m);
            let prev = st.sessions.get(&key).cloned().unwrap_or_default();
            let delta = cur.delta_from(&prev);
            if delta.is_zero() {
                continue;
            }
            let provider = cost_usage
                .and_then(|c| c.get("provider"))
                .and_then(|x| x.as_str())
                .map(|s| s.to_string());
            out.push(UsageRecord {
                agent: AGENT.into(),
                session_id: Some(sid.to_string()),
                project: cwd.clone(),
                title: title.clone(),
                model: Some(model.clone()),
                provider,
                ts: created_at,
                touch_ts: Some(touch_ts),
                input_tokens: delta.input,
                output_tokens: delta.output,
                cache_read_tokens: delta.cache_read,
                cache_write_tokens: delta.cache_write,
                reasoning_tokens: delta.reasoning,
                calls: delta.calls,
                cost: delta.cost,
                skip_daily: true,
                skip_hourly: true,
                ..Default::default()
            });
            st.sessions.insert(key, cur);
        }
    }
    if !used_by_model {
        if let Some(tok) = rows.get("tokenUsage").map(row_val) {
            let totals = tok.get("totals").cloned().unwrap_or(Value::Null);
            let cur = DshEntry {
                input: u64f(&totals, "uncachedInputTokens"),
                output: u64f(&totals, "outputTokens"),
                cache_read: u64f(&totals, "cacheReadTokens"),
                cache_write: u64f(&totals, "cacheWriteTokens"),
                ..Default::default()
            };
            let key = format!("{}|", sid);
            let prev = st.sessions.get(&key).cloned().unwrap_or_default();
            let delta = cur.delta_from(&prev);
            if !delta.is_zero() {
                out.push(UsageRecord {
                    agent: AGENT.into(),
                    session_id: Some(sid.to_string()),
                    project: cwd.clone(),
                    title: title.clone(),
                    ts: created_at,
                    touch_ts: Some(touch_ts),
                    input_tokens: delta.input,
                    output_tokens: delta.output,
                    cache_read_tokens: delta.cache_read,
                    cache_write_tokens: delta.cache_write,
                    skip_daily: true,
                    skip_hourly: true,
                    ..Default::default()
                });
                st.sessions.insert(key, cur);
            }
        }
    }
}

/// 会话表数据源。
/// 现行布局是**按记录目录** `session_projcache/sessions/<id>.json`(DSH 0.1.5-rc.1),
/// 数据在 `record.{identity,rows}` 下;老的单一文件 `session_projcache.json`
/// 数据在 `tables.sessions.<id>.{identity,rows}` 下,升级后不再更新但仍是兜底。
/// 只读单文件时,新会话一条都进不来 —— 表现为"会话明细"里最近几天整段空白。
fn scan_projcache(st: &mut DshState, out: &mut Vec<UsageRecord>, storages: &Path) -> Result<()> {
    let dir = storages.join("session_projcache").join("sessions");
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut scanned: std::collections::HashSet<String> = std::collections::HashSet::new();

    // 1) 按记录目录(现行):文件名(去掉 .json)就是会话 id,与 session_meta 里的 id 一致。
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(stem) = name.strip_suffix(".json") else {
                continue;
            };
            // 同上:用目录枚举自带的属性,避免每个文件再来一次 stat。
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if !kind.is_file() {
                continue;
            }
            let path = entry.path();
            // 会话 id 来自文件名,和内容无关,所以先登记:即便下面因缓存跳过,
            // 单文件兜底也不该把同一个会话再算一遍。
            seen.insert(stem.to_string());
            let key = path.to_string_lossy().into_owned();
            scanned.insert(key.clone());

            // (size, mtime) 没变就跳过解析。文件没变时按 st.sessions 算出的 delta 必为 0,
            // 所以跳过与重算的产出完全一致;但省掉的是每次扫描 2254 次读盘 + JSON 解析,
            // 这正是"热扫"从 ~0.5s 退化到 ~2s 的原因。
            // 全量重建时 state 被清零(lib.rs 里 full ⇒ state = Null),缓存自然失效,
            // 不会出现"数据库清空了、会话却没重新写回"。
            let Some(stamp) = entry_stamp(&entry) else {
                continue;
            };
            if st.projcache_cache.get(&key) == Some(&stamp) {
                continue;
            }
            // 单个坏记录不该让整张会话表消失
            let Ok(v) = read_json_file(&path) else {
                eprintln!("[dsh] projcache 记录不可读,跳过: {}", path.display());
                continue;
            };
            let Some(record) = v.get("record") else {
                continue;
            };
            let identity = record.get("identity").cloned().unwrap_or(Value::Null);
            let rows = record.get("rows").cloned().unwrap_or(Value::Null);
            st.projcache_cache.insert(key, stamp);
            scan_projcache_session(st, out, stem, &identity, &rows);
        }
    }
    // 已消失的文件不留指纹,缓存规模跟随实际文件数(本机 2254),不会无限增长
    st.projcache_cache.retain(|k, _| scanned.contains(k));

    // 2) 单文件兜底:只补目录里没有的会话(实测 2234 个老键里仅 3 个只存在于单文件,
    //    且两种布局的 id 命名没有交叉形式,所以不会重复计)。
    let legacy = storages.join("session_projcache.json"); // storages 直接来自调用方
    if legacy.is_file() {
        let v = read_json_file(&legacy)?;
        if let Some(sessions) = v.pointer("/tables/sessions").and_then(|s| s.as_object()) {
            for (sid, entry) in sessions {
                if seen.contains(sid) {
                    continue;
                }
                let identity = entry.get("identity").cloned().unwrap_or(Value::Null);
                let rows = entry.get("rows").cloned().unwrap_or(Value::Null);
                scan_projcache_session(st, out, sid, &identity, &rows);
            }
        }
    }
    Ok(())
}

/// 取一个目录项自带的 (size, mtime) 指纹。
/// 刻意用 `DirEntry::metadata()` 而不是 `fs::metadata(entry.path())`:Windows 的目录枚举
/// 已经把属性一起返回,前者近乎免费,后者是每个文件一次完整 stat(实测 150 倍差距)。
fn entry_stamp(entry: &std::fs::DirEntry) -> Option<DshFileStamp> {
    let meta = entry.metadata().ok()?;
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Some(DshFileStamp {
        size: meta.len(),
        mtime_ms,
    })
}
