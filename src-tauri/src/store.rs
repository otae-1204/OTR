use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection};

use crate::error::Result;
use crate::model::{
    local_date, local_hour, now_ms, today_str, AgentSlice, DailyUsage, HourProfile, ModelSlice,
    RangeSummary, SessionUsage, Totals, UsageSummary,
};
use crate::providers::FileCursor;
use crate::settings::PriceEntry;

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
CREATE TABLE IF NOT EXISTS usage_daily (
  agent TEXT NOT NULL, date TEXT NOT NULL,
  model TEXT NOT NULL DEFAULT '', provider TEXT NOT NULL DEFAULT '',
  input_tokens INTEGER NOT NULL DEFAULT 0, output_tokens INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens INTEGER NOT NULL DEFAULT 0, cache_write_tokens INTEGER NOT NULL DEFAULT 0,
  reasoning_tokens INTEGER NOT NULL DEFAULT 0, calls INTEGER NOT NULL DEFAULT 0,
  cost REAL NOT NULL DEFAULT 0,
  PRIMARY KEY (agent, date, model, provider)
);
CREATE TABLE IF NOT EXISTS usage_session_models (
  agent TEXT NOT NULL, session_id TEXT NOT NULL,
  model TEXT NOT NULL DEFAULT '', provider TEXT NOT NULL DEFAULT '',
  input_tokens INTEGER NOT NULL DEFAULT 0, output_tokens INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens INTEGER NOT NULL DEFAULT 0, cache_write_tokens INTEGER NOT NULL DEFAULT 0,
  reasoning_tokens INTEGER NOT NULL DEFAULT 0, calls INTEGER NOT NULL DEFAULT 0,
  cost REAL NOT NULL DEFAULT 0, last_ts INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (agent, session_id, model, provider)
);
CREATE TABLE IF NOT EXISTS session_meta (
  agent TEXT NOT NULL, session_id TEXT NOT NULL,
  project TEXT, title TEXT, started_at INTEGER, last_active INTEGER,
  PRIMARY KEY (agent, session_id)
);
CREATE TABLE IF NOT EXISTS file_cursors (
  agent TEXT NOT NULL, path TEXT NOT NULL, data TEXT NOT NULL,
  PRIMARY KEY (agent, path)
);
CREATE TABLE IF NOT EXISTS usage_hourly (
  agent TEXT NOT NULL, date TEXT NOT NULL, hour INTEGER NOT NULL,
  model TEXT NOT NULL DEFAULT '', provider TEXT NOT NULL DEFAULT '',
  input_tokens INTEGER NOT NULL DEFAULT 0, output_tokens INTEGER NOT NULL DEFAULT 0,
  cache_read_tokens INTEGER NOT NULL DEFAULT 0, cache_write_tokens INTEGER NOT NULL DEFAULT 0,
  reasoning_tokens INTEGER NOT NULL DEFAULT 0, calls INTEGER NOT NULL DEFAULT 0,
  cost REAL NOT NULL DEFAULT 0,
  PRIMARY KEY (agent, date, hour, model, provider)
);
CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT NOT NULL);
CREATE INDEX IF NOT EXISTS idx_daily_date ON usage_daily(date);
"#;

const SQL_DAILY_UPSERT: &str = r#"
INSERT INTO usage_daily (agent,date,model,provider,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,reasoning_tokens,calls,cost)
VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)
ON CONFLICT(agent,date,model,provider) DO UPDATE SET
  input_tokens = input_tokens + excluded.input_tokens,
  output_tokens = output_tokens + excluded.output_tokens,
  cache_read_tokens = cache_read_tokens + excluded.cache_read_tokens,
  cache_write_tokens = cache_write_tokens + excluded.cache_write_tokens,
  reasoning_tokens = reasoning_tokens + excluded.reasoning_tokens,
  calls = calls + excluded.calls,
  cost = cost + excluded.cost
"#;

const SQL_SESSION_UPSERT: &str = r#"
INSERT INTO usage_session_models (agent,session_id,model,provider,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,reasoning_tokens,calls,cost,last_ts)
VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
ON CONFLICT(agent,session_id,model,provider) DO UPDATE SET
  input_tokens = input_tokens + excluded.input_tokens,
  output_tokens = output_tokens + excluded.output_tokens,
  cache_read_tokens = cache_read_tokens + excluded.cache_read_tokens,
  cache_write_tokens = cache_write_tokens + excluded.cache_write_tokens,
  reasoning_tokens = reasoning_tokens + excluded.reasoning_tokens,
  calls = calls + excluded.calls,
  cost = cost + excluded.cost,
  last_ts = MAX(last_ts, excluded.last_ts)
"#;

const SQL_HOURLY_UPSERT: &str = r#"
INSERT INTO usage_hourly (agent,date,hour,model,provider,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,reasoning_tokens,calls,cost)
VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)
ON CONFLICT(agent,date,hour,model,provider) DO UPDATE SET
  input_tokens = input_tokens + excluded.input_tokens,
  output_tokens = output_tokens + excluded.output_tokens,
  cache_read_tokens = cache_read_tokens + excluded.cache_read_tokens,
  cache_write_tokens = cache_write_tokens + excluded.cache_write_tokens,
  reasoning_tokens = reasoning_tokens + excluded.reasoning_tokens,
  calls = calls + excluded.calls,
  cost = cost + excluded.cost
"#;

const SQL_META_UPSERT: &str = r#"
INSERT INTO session_meta (agent,session_id,project,title,started_at,last_active)
VALUES (?1,?2,?3,?4,?5,?6)
ON CONFLICT(agent,session_id) DO UPDATE SET
  project = COALESCE(excluded.project, project),
  title = COALESCE(excluded.title, title),
  started_at = CASE
    WHEN started_at IS NULL OR started_at = 0 THEN excluded.started_at
    WHEN excluded.started_at IS NULL OR excluded.started_at = 0 THEN started_at
    ELSE MIN(started_at, excluded.started_at) END,
  last_active = MAX(COALESCE(last_active,0), COALESCE(excluded.last_active,0))
"#;

/// 数据版本号:只在**真的写入了行**时递增,和用量写入在同一个事务里。
/// 前端拿它当刷新键(替代原来的 generated_at),空闲轮询就不再连锁触发
/// get_range_summary / get_daily / get_sessions 这三条明细查询。
fn bump_data_version(tx: &rusqlite::Transaction) -> Result<()> {
    tx.execute(
        "INSERT INTO kv (k, v) VALUES ('data_version', '1')
         ON CONFLICT(k) DO UPDATE SET v = CAST(CAST(v AS INTEGER) + 1 AS TEXT)",
        [],
    )?;
    tx.execute(
        "INSERT INTO kv (k, v) VALUES ('data_updated_ms', ?1)
         ON CONFLICT(k) DO UPDATE SET v = excluded.v",
        params![now_ms().to_string()],
    )?;
    Ok(())
}

/// 把一批增量记录写入按天/按小时/会话/元数据表。
/// 调用方负责事务边界与提交(apply_records 累加、replace_agent 先清后写)。
/// 返回**实际写出的行数**:0 表示这批记录什么都没动(扫描很安静时不该推版本号)。
fn write_records(
    tx: &rusqlite::Transaction,
    records: &[crate::model::UsageRecord],
) -> Result<usize> {
    let mut n = 0usize;
    for r in records {
        let has_usage = r.total_tokens() > 0 || r.calls > 0 || r.cost.abs() > f64::EPSILON;
        if has_usage {
            let date = r.bucket_date.clone().unwrap_or_else(|| local_date(r.ts));
            let hour = r
                .bucket_hour
                .filter(|hour| (0..24).contains(hour))
                .unwrap_or_else(|| local_hour(r.ts));
            if !r.skip_daily {
                // 绝对总量(台账第一次看到某天)要先删后写:同一行可能已被
                // "按小时表回填"写过,直接累加会双计。
                if r.absolute_daily {
                    tx.execute(
                        "DELETE FROM usage_daily WHERE agent=?1 AND date=?2 AND model=?3 AND provider=?4",
                        params![
                            r.agent,
                            date,
                            r.model.clone().unwrap_or_default(),
                            r.provider.clone().unwrap_or_default(),
                        ],
                    )?;
                }
                tx.execute(
                    SQL_DAILY_UPSERT,
                    params![
                        r.agent,
                        date,
                        r.model.clone().unwrap_or_default(),
                        r.provider.clone().unwrap_or_default(),
                        r.input_tokens as i64,
                        r.output_tokens as i64,
                        r.cache_read_tokens as i64,
                        r.cache_write_tokens as i64,
                        r.reasoning_tokens as i64,
                        r.calls as i64,
                        r.cost,
                    ],
                )?;
            }
            if !r.skip_hourly {
                tx.execute(
                    SQL_HOURLY_UPSERT,
                    params![
                        r.agent,
                        date,
                        hour,
                        r.model.clone().unwrap_or_default(),
                        r.provider.clone().unwrap_or_default(),
                        r.input_tokens as i64,
                        r.output_tokens as i64,
                        r.cache_read_tokens as i64,
                        r.cache_write_tokens as i64,
                        r.reasoning_tokens as i64,
                        r.calls as i64,
                        r.cost,
                    ],
                )?;
            }
            n += 1;
        }
        if let Some(sid) = &r.session_id {
            let last_ts = r.touch_ts.unwrap_or(r.ts);
            let last_ts = if last_ts > 0 { last_ts } else { now_ms() };
            tx.execute(
                SQL_SESSION_UPSERT,
                params![
                    r.agent,
                    sid,
                    r.model.clone().unwrap_or_default(),
                    r.provider.clone().unwrap_or_default(),
                    r.input_tokens as i64,
                    r.output_tokens as i64,
                    r.cache_read_tokens as i64,
                    r.cache_write_tokens as i64,
                    r.reasoning_tokens as i64,
                    r.calls as i64,
                    r.cost,
                    last_ts,
                ],
            )?;
            tx.execute(
                SQL_META_UPSERT,
                params![
                    r.agent,
                    sid,
                    r.project,
                    r.title,
                    if r.ts > 0 { Some(r.ts) } else { None },
                    last_ts,
                ],
            )?;
            n += 1;
        }
    }
    Ok(n)
}

/// 成本口径(全应用唯一一套):**定价表为权威**。
///
/// 1. 模型在 settings.pricing 里有定价 → 按 tokens × 单价 × 汇率重算,**覆盖**自带成本;
/// 2. 没有定价 → 用数据自带成本,按该 Agent 声明的币种归一化;
/// 3. 都没有 → 0。
///
/// 输出恒为 ¥;展示币种由前端按 settings.currency 换算。
///
/// 以前两处口径不同:range_summary 会查定价表并写死 `agent != "dsh"` 猜币种,
/// 而 sessions() 完全不查定价表、只按 dsh 特判换算 —— 明细表和顶部大卡系统性对不上。
pub struct CostBasis<'a> {
    pricing: &'a HashMap<String, PriceEntry>,
    /// 美元 → 人民币
    rate: f64,
    /// Agent → 自带成本币种("CNY"/"USD"),由 Provider::native_cost_currency 声明
    native_currency: &'a HashMap<String, String>,
    /// 峰谷占比;None = 不做峰谷(整段按平价计,与加峰谷之前逐位一致)
    peaks: Option<&'a crate::peak::PeakShares>,
}

impl<'a> CostBasis<'a> {
    pub fn new(
        pricing: &'a HashMap<String, PriceEntry>,
        rate: f64,
        native_currency: &'a HashMap<String, String>,
    ) -> Self {
        Self {
            pricing,
            rate,
            native_currency,
            peaks: None,
        }
    }

    /// 挂上峰谷占比表:只有填了 `peak` 档的模型会受影响,其余模型逐位不变。
    pub fn with_peaks(mut self, peaks: &'a crate::peak::PeakShares) -> Self {
        self.peaks = Some(peaks);
        self
    }

    /// 一条 (agent, model) 聚合行的成本;tokens 用于定价重算,raw_cost 是数据自带成本。
    /// `peak` 是该行对应的峰值占比 —— 按天聚合时逐桶取,按会话聚合时取模型级兜底。
    pub fn row_cost(
        &self,
        model: &str,
        agent: &str,
        tokens: &Totals,
        raw_cost: f64,
        peak: crate::peak::PeakShare,
    ) -> f64 {
        if let Some(price) = self.pricing.get(model) {
            return estimate_cost(tokens, price, self.rate, peak);
        }
        if raw_cost.abs() <= f64::EPSILON {
            return 0.0;
        }
        match self.native_currency.get(agent).map(String::as_str) {
            Some("CNY") => raw_cost,
            // 未声明币种的一律按美元处理(与历史行为一致),但不再针对某个具体 Agent 特判
            _ => raw_cost * self.rate,
        }
    }
}

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    /// 连接锁:中毒时也继续用(见 crate::lock 的说明)
    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        crate::lock(&self.conn)
    }

    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// 全量重建前的安全网:用 SQLite 自身机制做一致性快照。
    /// 连接开着时直接复制 db 文件不安全(WAL),`VACUUM INTO` 则是事务一致的。
    pub fn snapshot(&self, dest: &Path) -> Result<()> {
        let conn = self.conn();
        conn.execute("VACUUM INTO ?1", params![dest.to_string_lossy()])?;
        Ok(())
    }

    pub fn wipe_agent(&self, agent: &str) -> Result<()> {
        let conn = self.conn();
        conn.execute("DELETE FROM usage_daily WHERE agent=?1", params![agent])?;
        conn.execute("DELETE FROM usage_hourly WHERE agent=?1", params![agent])?;
        conn.execute(
            "DELETE FROM usage_session_models WHERE agent=?1",
            params![agent],
        )?;
        conn.execute("DELETE FROM session_meta WHERE agent=?1", params![agent])?;
        conn.execute("DELETE FROM file_cursors WHERE agent=?1", params![agent])?;
        conn.execute(
            "DELETE FROM kv WHERE k=?1",
            params![format!("state:{}", agent)],
        )?;
        Ok(())
    }

    /// 记录均为增量语义,分别累加进按天/按小时/会话表;整体包在一个事务里。
    pub fn apply_records(&self, records: &[crate::model::UsageRecord]) -> Result<usize> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let n = write_records(&tx, records)?;
        if n > 0 {
            bump_data_version(&tx)?;
        }
        tx.commit()?;
        Ok(n)
    }

    /// 全量重建:清空该 Agent 的用量/会话/游标行,写入新结果与新基线。
    /// 删除与写入在**同一个事务**内提交,任何一步失败整体回滚,旧数据完好。
    ///
    /// 按天/按小时只替换**本次扫描覆盖到的日期**,其余日期的历史行原样保留。
    /// 以前是无条件 `DELETE ... WHERE agent=?1` 清空整表,于是日志换代后
    /// (旧日志被重写、已不含早期事件)一次全量重建就把那些日期的按天**和**按小时
    /// 数据一起抹掉 —— 而且因为日志里已经没有了,再也无法重建。
    /// 现在覆盖不到的日期不动,重建只影响它真正重读过的那些日子。
    pub fn replace_agent(
        &self,
        agent: &str,
        records: &[crate::model::UsageRecord],
        cursors: &HashMap<String, FileCursor>,
        state: &serde_json::Value,
    ) -> Result<usize> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        // 本次扫描**会写进按天或按小时表**的日期:只有这些日期允许被替换。
        // 判据与 write_records 对齐 —— 两个表都不写的记录(如只进会话表的 projcache)
        // 不能让它把那个日期的历史删掉。空集时 json_each 不产生行 → 一条都不删。
        let covered: std::collections::HashSet<String> = records
            .iter()
            .filter(|r| {
                let has_usage =
                    r.total_tokens() > 0 || r.calls > 0 || r.cost.abs() > f64::EPSILON;
                has_usage && !(r.skip_daily && r.skip_hourly)
            })
            .map(|r| {
                r.bucket_date
                    .clone()
                    .unwrap_or_else(|| local_date(r.ts))
            })
            .collect();
        let covered_json = serde_json::to_string(&covered)?;
        for sql in [
            "DELETE FROM usage_daily WHERE agent=?1 AND date IN (SELECT value FROM json_each(?2))",
            "DELETE FROM usage_hourly WHERE agent=?1 AND date IN (SELECT value FROM json_each(?2))",
        ] {
            tx.execute(sql, params![agent, covered_json])?;
        }
        // 会话表与游标表按 Agent 整体重建(它们不是按日期的历史,重扫即最新)
        for sql in [
            "DELETE FROM usage_session_models WHERE agent=?1",
            "DELETE FROM session_meta WHERE agent=?1",
            "DELETE FROM file_cursors WHERE agent=?1",
        ] {
            tx.execute(sql, params![agent])?;
        }
        let n = write_records(&tx, records)?;
        for (path, cursor) in cursors {
            tx.execute(
                "INSERT INTO file_cursors (agent, path, data) VALUES (?1, ?2, ?3)
                 ON CONFLICT(agent, path) DO UPDATE SET data = excluded.data",
                params![agent, path, serde_json::to_string(cursor)?],
            )?;
        }
        tx.execute(
            "INSERT INTO kv (k, v) VALUES (?1, ?2) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
            params![format!("state:{}", agent), serde_json::to_string(state)?],
        )?;
        // 全量重建本身就是一次数据变更(哪怕结果恰好为空),必须让前端重拉
        bump_data_version(&tx)?;
        tx.commit()?;
        Ok(n)
    }

    /// 用按小时表**校正**按天表:逐 (date, model, provider) 桶,只增不减。
    ///
    /// 存在的理由:DSH 的按天数据由会话日志补齐,而日志是**增量**消费的
    /// (state.hourly 是绝对高水位,同一增量不会被重放)。某天的增量一旦被判成
    /// "台账负责"(skip_daily),或者刚好落在台账读不出来的那个窗口里,那天就只进了
    /// 按小时表;按天表要么整天空白、要么只剩下一小段增量 —— 表现为"轴表有数据、
    /// 当天卡片与模型占比几乎为 0"(用户报的现场:今日只有一个模型、511.3K)。
    /// 记录不会重放,只能在库里修回来。
    ///
    /// 规则(逐桶):
    /// - 桶的按小时合计 > 按天合计(按天缺行算 0)→ 用按小时合计**替换**该行;
    /// - 按天 >= 按小时 → 原样不动(台账写过的日期按天更高,不能拿日志把它抹小);
    /// - skip_buckets(台账负责的桶)不碰,避免台账日后重写该桶时两边各记一次。
    ///
    /// 替换而不是累加:两张表记的是同一批增量,相加就是双计。
    /// 幂等:第二次跑两边已经相等,写 0 行。可每次扫描后安全重跑。
    pub fn reconcile_daily_from_hourly(
        &self,
        agent: &str,
        skip_buckets: &std::collections::HashSet<String>,
    ) -> Result<usize> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        // 只读语句先取两张表的逐桶合计,再在内存里比对、逐个写回
        // (date, model, provider, input, output, cache_read, cache_write, reasoning, calls, cost)
        type Row = (String, String, String, i64, i64, i64, i64, i64, i64, f64);
        let hourly: Vec<Row> = {
            let mut stmt = tx.prepare(
                "SELECT h.date, h.model, h.provider,\
                 SUM(h.input_tokens), SUM(h.output_tokens), SUM(h.cache_read_tokens),\
                 SUM(h.cache_write_tokens), SUM(h.reasoning_tokens), SUM(h.calls), SUM(h.cost) \
                 FROM usage_hourly h WHERE h.agent = ?1 \
                 GROUP BY h.date, h.model, h.provider",
            )?;
            let rows = stmt.query_map(params![agent], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, i64>(7)?,
                    row.get::<_, i64>(8)?,
                    row.get::<_, f64>(9)?,
                ))
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let daily: HashMap<(String, String, String), i64> = {
            let mut stmt = tx.prepare(
                "SELECT d.date, d.model, d.provider, \
                 SUM(d.input_tokens + d.output_tokens + d.cache_read_tokens + d.cache_write_tokens) \
                 FROM usage_daily d WHERE d.agent = ?1 \
                 GROUP BY d.date, d.model, d.provider",
            )?;
            let rows = stmt.query_map(params![agent], |row| {
                Ok((
                    (row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?),
                    row.get::<_, i64>(3)?,
                ))
            })?;
            rows.collect::<std::result::Result<HashMap<_, _>, _>>()?
        };
        let mut n = 0usize;
        for (date, model, provider, input, output, cache_read, cache_write, reasoning, calls, cost) in
            hourly
        {
            // 逐桶判定:键与 Provider 上报的 ledger 键同形(见 ledger_owned_buckets)
            if skip_buckets.contains(&format!("{}|{}:{}", date, provider, model)) {
                continue;
            }
            let hourly_total = input + output + cache_read + cache_write;
            let daily_total = daily
                .get(&(date.clone(), model.clone(), provider.clone()))
                .copied()
                .unwrap_or(0);
            if hourly_total <= daily_total {
                continue;
            }
            // 同一桶先删后写:按天表记的是这批增量的**同一份**,不是再加一遍
            tx.execute(
                "DELETE FROM usage_daily WHERE agent=?1 AND date=?2 AND model=?3 AND provider=?4",
                params![agent, date, model, provider],
            )?;
            tx.execute(
                SQL_DAILY_UPSERT,
                params![
                    agent, date, model, provider, input, output, cache_read, cache_write,
                    reasoning, calls, cost,
                ],
            )?;
            n += 1;
        }
        if n > 0 {
            bump_data_version(&tx)?;
        }
        tx.commit()?;
        Ok(n)
    }
    // ---------- 查询 ----------

    fn totals_eq(conn: &Connection, date: &str) -> Result<Totals> {
        Self::totals_query(conn, "WHERE date = ?1", rusqlite::params![date])
    }

    fn totals_query(conn: &Connection, cond: &str, p: impl rusqlite::Params) -> Result<Totals> {
        let sql = format!(
            "SELECT COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                    COALESCE(SUM(cache_read_tokens),0), COALESCE(SUM(cache_write_tokens),0),
                    COALESCE(SUM(calls),0), COALESCE(SUM(cost),0.0)
             FROM usage_daily {cond}"
        );
        let t = conn.query_row(&sql, p, |row| {
            Ok(Totals {
                input_tokens: row.get::<_, i64>(0)? as u64,
                output_tokens: row.get::<_, i64>(1)? as u64,
                cache_read_tokens: row.get::<_, i64>(2)? as u64,
                cache_write_tokens: row.get::<_, i64>(3)? as u64,
                calls: row.get::<_, i64>(4)? as u64,
                cost: row.get(5)?,
                total_tokens: 0,
            })
        })?;
        let total = t.input_tokens + t.output_tokens + t.cache_read_tokens + t.cache_write_tokens;
        Ok(Totals {
            total_tokens: total,
            ..t
        })
    }

    pub fn totals_for_date(&self, date: &str) -> Result<Totals> {
        let conn = self.conn();
        Self::totals_eq(&conn, date)
    }

    /// 轻量摘要:只给前端的"今日各 Agent"卡片 + 数据版本号。
    ///
    /// 以前这里跑 4 次 totals 聚合 + 2 次 GROUP BY,其中 week/month/allTime/byModelMonth
    /// 前端**一次都没用过**(只消费 byAgentToday 和 generatedAt);而 generatedAt 又是
    /// now_ms(),每轮都变 → App.tsx 的 refreshKey 每 30 秒变一次 → 连锁触发
    /// get_range_summary + get_daily + get_sessions 共 6 次 invoke。
    pub fn summary(&self) -> Result<UsageSummary> {
        let conn = self.conn();
        let today = today_str();

        let mut by_agent = Vec::new();
        {
            let mut stmt = conn.prepare(
                "SELECT agent, SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens),
                        SUM(cache_write_tokens), SUM(calls), SUM(cost)
                 FROM usage_daily WHERE date = ?1 GROUP BY agent
                 ORDER BY SUM(input_tokens+output_tokens+cache_read_tokens+cache_write_tokens) DESC",
            )?;
            let rows = stmt.query_map(params![today], |row| {
                Ok((row.get::<_, String>(0)?, totals_from_row(row, 1)))
            })?;
            for r in rows {
                let (agent, t) = r?;
                by_agent.push(crate::model::AgentSlice { agent, totals: t });
            }
        }

        Ok(UsageSummary {
            // 数据最后一次真正变化的时间(不是查询时间):StatCard 的"更新于"因此反映
            // 数据新鲜度,而不是"刚刚查过一次"
            generated_at: Self::kv_i64(&conn, "data_updated_ms")
                .filter(|v| *v > 0)
                .unwrap_or_else(now_ms),
            data_version: Self::kv_i64(&conn, "data_version").unwrap_or(0),
            by_agent_today: by_agent,
        })
    }

    fn kv_i64(conn: &Connection, key: &str) -> Option<i64> {
        conn.query_row("SELECT v FROM kv WHERE k=?1", params![key], |row| {
            row.get::<_, String>(0)
        })
        .ok()
        .and_then(|v| v.parse::<i64>().ok())
    }

    /// 任意日期范围(可按 Agent 过滤)的统计;成本走 CostBasis(定价表为权威)。
    pub fn range_summary(
        &self,
        agent: Option<&str>,
        from: &str,
        to: &str,
        basis: &CostBasis,
        // None = 不过滤;Some = 仅计入这些 Agent(设置里停用的不进主页合计)
        enabled_agents: Option<&[String]>,
        // 峰谷占比;None = 不做峰谷(平价计价,与加峰谷之前逐位一致)
        peaks: Option<&crate::peak::PeakShares>,
    ) -> Result<RangeSummary> {
        let conn = self.conn();
        let base_sql = "SELECT COALESCE(NULLIF(model,''),'(未知模型)'), agent,
                        SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens),
                        SUM(cache_write_tokens), SUM(calls), SUM(cost)
                 FROM usage_daily WHERE date >= ?1 AND date <= ?2 {agent_cond}
                 GROUP BY model, agent";

        fn merge(dst: &mut Totals, src: &Totals) {
            dst.input_tokens += src.input_tokens;
            dst.output_tokens += src.output_tokens;
            dst.cache_read_tokens += src.cache_read_tokens;
            dst.cache_write_tokens += src.cache_write_tokens;
            dst.calls += src.calls;
            dst.total_tokens += src.total_tokens;
            dst.cost += src.cost;
        }

        // 折叠 (model, agent) 行:tokens 合计 + 成本(定价覆盖 / 币种归一化)
        fn fold_rows(
            stmt: &mut rusqlite::Statement,
            params: &[&dyn rusqlite::ToSql],
            basis: &CostBasis,
            peaks: Option<&crate::peak::PeakShares>,
            mut sink: impl FnMut(&str, &str, Totals),
        ) -> Result<()> {
            let rows = stmt.query_map(params, |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    totals_from_row(row, 2),
                ))
            })?;
            for r in rows {
                let (model, agent, mut t) = r?;
                let raw_cost = t.cost;
                // 这里是区间聚合(不逐日),所以取该模型的区间整体峰值占比
                let peak = peaks.map(|p| p.model_share(&model)).unwrap_or_default();
                t.cost = basis.row_cost(&model, &agent, &t, raw_cost, peak);
                sink(&model, &agent, t);
            }
            Ok(())
        }

        let enabled_ok = |id: &str| match enabled_agents {
            None => true,
            Some(list) => list.iter().any(|a| a == id),
        };

        // Q1:按 Agent 过滤 → totals + by_model;未指定 Agent 时排除已停用的
        let mut totals = Totals::default();
        let by_model: Vec<ModelSlice> = {
            let sql = base_sql.replace("{agent_cond}", "AND (?3 IS NULL OR agent = ?3)");
            let mut stmt = conn.prepare(&sql)?;
            let mut model_map: std::collections::HashMap<String, Totals> =
                std::collections::HashMap::new();
            fold_rows(
                &mut stmt,
                &[&from, &to, &agent],
                basis,
                peaks,
                |model, ag, t| {
                    if agent.is_none() && !enabled_ok(ag) {
                        return;
                    }
                    merge(&mut totals, &t);
                    merge(model_map.entry(model.to_string()).or_default(), &t);
                },
            )?;
            let mut list: Vec<ModelSlice> = model_map
                .into_iter()
                .map(|(model, totals)| ModelSlice { model, totals })
                .collect();
            list.sort_by(|a, b| b.totals.total_tokens.cmp(&a.totals.total_tokens));
            list.truncate(12);
            list
        };

        // Q2:不按选中 Agent 过滤 → by_agent(卡片用);仍排除设置里停用的
        let by_agent: Vec<AgentSlice> = {
            let sql = base_sql.replace("{agent_cond}", "");
            let mut stmt = conn.prepare(&sql)?;
            let mut agent_map: std::collections::HashMap<String, Totals> =
                std::collections::HashMap::new();
            fold_rows(
                &mut stmt,
                &[&from, &to],
                basis,
                peaks,
                |_model, ag, t| {
                    if !enabled_ok(ag) {
                        return;
                    }
                    merge(agent_map.entry(ag.to_string()).or_default(), &t);
                },
            )?;
            let mut list: Vec<AgentSlice> = agent_map
                .into_iter()
                .map(|(agent, totals)| AgentSlice { agent, totals })
                .collect();
            list.sort_by(|a, b| b.totals.total_tokens.cmp(&a.totals.total_tokens));
            list
        };

        Ok(RangeSummary {
            generated_at: now_ms(),
            from: from.into(),
            to: to.into(),
            agent: agent.map(Into::into),
            currency: "CNY".into(),
            totals,
            by_agent,
            by_model,
        })
    }

    /// 从按小时表构建峰谷占比表(见 crate::peak)。
    ///
    /// 只读 usage_hourly:它记的是**真实发生的小时**,是判断峰谷的唯一依据。
    /// 按天表没有小时信息,所以峰谷只能挂在这份占比上,而 token 数仍取按天表 ——
    /// 两张表本机差 8%(按小时表缺近期日),拿按小时表出总额会让日总额跳变。
    pub fn peak_shares(
        &self,
        agent: Option<&str>,
        from: &str,
        to: &str,
    ) -> Result<crate::peak::PeakShares> {
        let conn = self.conn();
        let mut shares = crate::peak::PeakShares::default();
        let mut stmt = conn.prepare(
            "SELECT date, hour, model, provider,
                    SUM(input_tokens), SUM(output_tokens),
                    SUM(cache_read_tokens), SUM(cache_write_tokens)
             FROM usage_hourly
             WHERE date >= ?1 AND date <= ?2 AND (?3 IS NULL OR agent = ?3)
             GROUP BY date, hour, model, provider",
        )?;
        let rows = stmt.query_map(params![from, to, agent], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
            ))
        })?;
        for row in rows {
            let (date, hour, model, provider, input, output, cr, cw) = row?;
            // 该小时整体落在高峰或整体落在平价 —— 一小时就是一个档,没有"部分高峰"
            let on = crate::peak::is_peak_hour(&date, hour);
            let share = if on {
                crate::peak::PeakShare {
                    input: 1.0,
                    output: 1.0,
                    cache_read: 1.0,
                    cache_write: 1.0,
                }
            } else {
                crate::peak::PeakShare::default()
            };
            let weight = (input + output + cr + cw) as f64;
            if weight <= 0.0 {
                continue;
            }
            shares.add_public(&date, &model, &provider, share, weight);
        }
        Ok(shares)
    }

    /// 出现过的全部模型名(设置页定价表用)
    pub fn list_models(&self) -> Result<Vec<String>> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare("SELECT DISTINCT model FROM usage_daily WHERE model != '' ORDER BY model")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.flatten().collect())
    }

    /// 趋势数据;granularity: "day"(默认) | "hour" | "month"。
    /// date 字段承载桶键:day="YYYY-MM-DD"、hour="YYYY-MM-DD HH:00"、month="YYYY-MM"
    pub fn daily(
        &self,
        agent: Option<&str>,
        from: &str,
        to: &str,
        granularity: &str,
        model: Option<&str>,
    ) -> Result<Vec<DailyUsage>> {
        let conn = self.conn();
        let model = model.filter(|m| !m.is_empty());
        // 空模型在占比里显示成「(未知模型)」,过滤时用同一套换算。
        let model_sql =
            "AND (?4 IS NULL OR COALESCE(NULLIF(model, ''), '(未知模型)') = ?4)";
        let sql = match granularity {
            "hour" => format!(
                "SELECT date || ' ' || printf('%02d:00', hour) AS bucket, agent,
                        SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens),
                        SUM(cache_write_tokens), SUM(calls), SUM(cost)
                 FROM usage_hourly WHERE date >= ?1 AND date <= ?2 AND (?3 IS NULL OR agent = ?3)
                 {model_sql}
                 GROUP BY bucket, agent ORDER BY bucket, agent"
            ),
            "month" => format!(
                "SELECT substr(date,1,7) AS bucket, agent,
                        SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens),
                        SUM(cache_write_tokens), SUM(calls), SUM(cost)
                 FROM usage_daily WHERE date >= ?1 AND date <= ?2 AND (?3 IS NULL OR agent = ?3)
                 {model_sql}
                 GROUP BY bucket, agent ORDER BY bucket, agent"
            ),
            _ => format!(
                "SELECT date, agent, SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens),
                        SUM(cache_write_tokens), SUM(calls), SUM(cost)
                 FROM usage_daily WHERE date >= ?1 AND date <= ?2 AND (?3 IS NULL OR agent = ?3)
                 {model_sql}
                 GROUP BY date, agent ORDER BY date, agent"
            ),
        };
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![from, to, agent, model], |row| {
            let input: i64 = row.get(2)?;
            let output: i64 = row.get(3)?;
            let cr: i64 = row.get(4)?;
            let cw: i64 = row.get(5)?;
            Ok(DailyUsage {
                date: row.get(0)?,
                agent: row.get(1)?,
                input_tokens: input as u64,
                output_tokens: output as u64,
                cache_read_tokens: cr as u64,
                cache_write_tokens: cw as u64,
                calls: row.get::<_, i64>(6)? as u64,
                cost: row.get(7)?,
                total_tokens: (input + output + cr + cw) as u64,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    /// 把范围内各天的同一个钟点加在一起,固定返回 0–23 共 24 格。
    /// 只表达「习惯落在几点」。小时表可能缺近期日,调用方不要拿合计去对按天总额。
    /// `enabled`:agent 为 None 时只计入这些 Agent;指定了 agent 时忽略。
    pub fn hour_profile(
        &self,
        agent: Option<&str>,
        from: &str,
        to: &str,
        model: Option<&str>,
        enabled: Option<&[String]>,
    ) -> Result<Vec<HourProfile>> {
        let conn = self.conn();
        let model = model.filter(|m| !m.is_empty());
        let mut stmt = conn.prepare(
            "SELECT hour, agent,
                    SUM(input_tokens) + SUM(output_tokens)
                    + SUM(cache_read_tokens) + SUM(cache_write_tokens)
             FROM usage_hourly
             WHERE date >= ?1 AND date <= ?2
               AND (?3 IS NULL OR agent = ?3)
               AND (?4 IS NULL OR COALESCE(NULLIF(model, ''), '(未知模型)') = ?4)
             GROUP BY hour, agent",
        )?;
        let rows = stmt.query_map(params![from, to, agent, model], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        let mut by_hour = [0u64; 24];
        for row in rows {
            let (hour, ag, tokens) = row?;
            if !(0..24).contains(&hour) || tokens <= 0 {
                continue;
            }
            if agent.is_none() {
                if let Some(list) = enabled {
                    if !list.iter().any(|id| id == &ag) {
                        continue;
                    }
                }
            }
            by_hour[hour as usize] += tokens as u64;
        }
        Ok((0..24)
            .map(|hour| HourProfile {
                hour,
                total_tokens: by_hour[hour as usize],
            })
            .collect())
    }

    /// 会话明细;成本与 range_summary 走**同一套 CostBasis 口径**。
    ///
    /// 代价是必须在 Rust 里折价:定价表按模型查,SQL 里做不了这件事(以前正是因此
    /// 干脆不查定价表、只按 agent='dsh' 特判,于是明细表的成本列和顶部大卡从来对不上)。
    /// 做法:子查询先按会话级 MAX(last_ts) 选出最近的 limit 个会话,再取这些会话的
    /// (session, model) 行,在 Rust 里按模型逐行算成本再合回会话。
    ///
    /// 注意与 range_summary 的**语义差异**(不是 bug,两表测的不是一回事):
    /// daily 按记录日期过滤,只统计"范围内发生的用量";这里选的是"范围内活跃的会话",
    /// 金额是该会话的**完整累计**,所以跨范围开始的长会话会把范围外的部分也算进来。
    /// 实测 codex 近 30 天:大卡 3124.44 / 明细 4127.56,差额全部来自 4 个跨范围的长会话。
    /// 另外 skip_daily 的记录(如 DSH projcache)只进 sessions 不进 usage_daily。
    pub fn sessions(
        &self,
        agent: Option<&str>,
        from_ms: Option<i64>,
        to_ms: Option<i64>,
        limit: i64,
        basis: &CostBasis,
        // None = 不过滤;Some = 未指定单个 Agent 时仅返回这些 Agent 的会话
        enabled_agents: Option<&[String]>,
        // 峰谷占比;None = 平价计价
        peaks: Option<&crate::peak::PeakShares>,
    ) -> Result<Vec<SessionUsage>> {
        let conn = self.conn();
        let enabled_json = match enabled_agents {
            None => "null".to_string(),
            Some(list) => serde_json::to_string(list).unwrap_or_else(|_| "[]".into()),
        };
        // 过滤按会话级 MAX(last_ts) 而不是逐行 last_ts:会话的"最后活跃"本来就是
        // MAX(m.last_ts)(表格里展示的也是它),逐行过滤会让同一会话因某个模型行落在
        // 范围内而被整段统计进来。
        let mut stmt = conn.prepare(
            "WITH picked AS (
                 SELECT agent, session_id, MAX(last_ts) AS last_ts
                 FROM usage_session_models
                 WHERE (?1 IS NULL OR agent = ?1)
                   AND (?5 = 'null' OR ?1 IS NOT NULL
                        OR agent IN (SELECT value FROM json_each(?5)))
                 GROUP BY agent, session_id
                 HAVING (?2 IS NULL OR MAX(last_ts) >= ?2)
                    AND (?3 IS NULL OR MAX(last_ts) < ?3)
                 ORDER BY last_ts DESC
                 LIMIT ?4
             )
             SELECT m.agent, m.session_id, m.model, m.provider,
                    SUM(m.input_tokens), SUM(m.output_tokens),
                    SUM(m.cache_read_tokens), SUM(m.cache_write_tokens),
                    SUM(m.calls), SUM(m.cost),
                    p.last_ts, MAX(meta.project), MAX(meta.title), MIN(meta.started_at)
             FROM usage_session_models m
             JOIN picked p ON p.agent = m.agent AND p.session_id = m.session_id
             LEFT JOIN session_meta meta ON meta.agent = m.agent AND meta.session_id = m.session_id
             GROUP BY m.agent, m.session_id, m.model, m.provider
             ORDER BY p.last_ts DESC",
        )?;
        let rows = stmt.query_map(params![agent, from_ms, to_ms, limit, enabled_json], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                totals_from_row(row, 4),
                row.get::<_, i64>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, Option<i64>>(13)?,
            ))
        })?;

        // 行按会话的 last_ts 降序到达,所以首次出现的顺序就是会话的展示顺序
        let mut index: HashMap<(String, String), usize> = HashMap::new();
        let mut out: Vec<SessionUsage> = Vec::new();
        let mut model_names: Vec<Vec<String>> = Vec::new();
        for row in rows {
            let (row_agent, sid, model, totals, last_ts, project, title, started_at) = row?;
            let raw_cost = totals.cost;
            // 明细表按会话聚合、拿不到逐日桶,用模型级区间占比(同源同口径)
            let peak = peaks.map(|p| p.model_share(&model)).unwrap_or_default();
            let cost = basis.row_cost(&model, &row_agent, &totals, raw_cost, peak);
            let idx = match index.get(&(row_agent.clone(), sid.clone())) {
                Some(existing) => *existing,
                None => {
                    let i = out.len();
                    index.insert((row_agent.clone(), sid.clone()), i);
                    model_names.push(Vec::new());
                    out.push(SessionUsage {
                        agent: row_agent,
                        session_id: Some(sid),
                        project,
                        title,
                        models: None,
                        started_at,
                        last_active: Some(last_ts),
                        input_tokens: 0,
                        output_tokens: 0,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                        calls: 0,
                        total_tokens: 0,
                        cost: 0.0,
                    });
                    i
                }
            };
            let slot = &mut out[idx];
            slot.input_tokens += totals.input_tokens;
            slot.output_tokens += totals.output_tokens;
            slot.cache_read_tokens += totals.cache_read_tokens;
            slot.cache_write_tokens += totals.cache_write_tokens;
            slot.calls += totals.calls;
            slot.total_tokens = slot.input_tokens
                + slot.output_tokens
                + slot.cache_read_tokens
                + slot.cache_write_tokens;
            slot.cost += cost;
            slot.last_active = Some(slot.last_active.unwrap_or(0).max(last_ts));
            // 与 SQL 的 MIN() 一致:忽略 NULL,保留 0
            slot.started_at = match (slot.started_at, started_at) {
                (None, other) => other,
                (this, None) => this,
                (Some(a), Some(b)) => Some(a.min(b)),
            };
            if !model.is_empty() && !model_names[idx].iter().any(|m| m == &model) {
                model_names[idx].push(model);
            }
        }
        for (i, slot) in out.iter_mut().enumerate() {
            if !model_names[i].is_empty() {
                slot.models = Some(model_names[i].join(","));
            }
        }
        Ok(out)
    }

    pub fn agent_all(&self) -> Result<HashMap<String, Totals>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT agent, SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens),
                    SUM(cache_write_tokens), SUM(calls), SUM(cost)
             FROM usage_daily GROUP BY agent",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, totals_from_row(row, 1)))
        })?;
        let mut map = HashMap::new();
        for r in rows {
            let (agent, t) = r?;
            map.insert(agent, t);
        }
        Ok(map)
    }

    // ---------- 游标 / KV ----------

    pub fn set_cursor(&self, agent: &str, path: &str, cursor: &FileCursor) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO file_cursors (agent, path, data) VALUES (?1, ?2, ?3)
             ON CONFLICT(agent, path) DO UPDATE SET data = excluded.data",
            params![agent, path, serde_json::to_string(cursor)?],
        )?;
        Ok(())
    }

    pub fn load_cursors(&self, agent: &str) -> HashMap<String, FileCursor> {
        let conn = self.conn();
        let mut map = HashMap::new();
        let Ok(mut stmt) = conn.prepare("SELECT path, data FROM file_cursors WHERE agent=?1")
        else {
            return map;
        };
        if let Ok(rows) = stmt.query_map(params![agent], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        }) {
            for r in rows.flatten() {
                if let Ok(c) = serde_json::from_str::<FileCursor>(&r.1) {
                    map.insert(r.0, c);
                }
            }
        }
        map
    }

    pub fn set_kv(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO kv (k, v) VALUES (?1, ?2) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_kv(&self, key: &str) -> Option<String> {
        let conn = self.conn();
        conn.query_row("SELECT v FROM kv WHERE k=?1", params![key], |row| {
            row.get::<_, String>(0)
        })
        .ok()
    }
}

fn totals_from_row(row: &rusqlite::Row, base: usize) -> Totals {
    let input: i64 = row.get(base).unwrap_or(0);
    let output: i64 = row.get(base + 1).unwrap_or(0);
    let cr: i64 = row.get(base + 2).unwrap_or(0);
    let cw: i64 = row.get(base + 3).unwrap_or(0);
    let calls: i64 = row.get(base + 4).unwrap_or(0);
    let cost: f64 = row.get(base + 5).unwrap_or(0.0);
    Totals {
        input_tokens: input as u64,
        output_tokens: output as u64,
        cache_read_tokens: cr as u64,
        cache_write_tokens: cw as u64,
        calls: calls as u64,
        total_tokens: (input + output + cr + cw) as u64,
        cost,
    }
}

/// 按定价估算费用:($/百万 tokens) × tokens ÷ 1e6 × 汇率。
///
/// 峰谷:某类 token 有 p 的比例落在高峰,单价就是
/// `(1-p) × 平价 + p × 高峰价`。没有 `peak` 档(p.peak == None)时退化成
/// `p.peak 取平价`,与加峰谷之前**逐位相同**。
fn estimate_cost(
    t: &Totals,
    p: &PriceEntry,
    rate: f64,
    peak: crate::peak::PeakShare,
) -> f64 {
    let blend = |off: f64, on: Option<f64>, share: f64| match on {
        Some(on) => (1.0 - share) * off + share * on,
        None => off,
    };
    let peak_tier = p.peak.clone().unwrap_or_default();
    let on = p.peak.is_some().then_some(peak_tier);
    let (pi, po, pr, pw) = match on {
        Some(tier) => (
            Some(tier.input),
            Some(tier.output),
            Some(tier.cache_read),
            Some(tier.cache_write),
        ),
        None => (None, None, None, None),
    };
    (t.input_tokens as f64 * blend(p.input, pi, peak.input)
        + t.output_tokens as f64 * blend(p.output, po, peak.output)
        + t.cache_read_tokens as f64 * blend(p.cache_read, pr, peak.cache_read)
        + t.cache_write_tokens as f64 * blend(p.cache_write, pw, peak.cache_write))
        / 1e6
        * rate
}

#[cfg(test)]
mod tests {
    use super::{estimate_cost, CostBasis, Store};
    use crate::model::{Totals, UsageRecord};
    use crate::providers::FileCursor;
    use crate::settings::PriceEntry;
    use std::collections::HashMap;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_db() -> std::path::PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("otr-store-{suffix}.db"))
    }

    fn record() -> UsageRecord {
        UsageRecord {
            agent: "dsh".into(),
            model: Some("m".into()),
            provider: Some("p".into()),
            ts: 1_780_000_000_000,
            input_tokens: 10,
            calls: 1,
            ..Default::default()
        }
    }

    #[test]
    fn replace_agent_replaces_instead_of_accumulating() {
        let path = temp_db();
        let store = Store::open(&path).unwrap();
        let recs: Vec<UsageRecord> = (0..3).map(|_| record()).collect();
        store.apply_records(&recs).unwrap();
        let date = crate::model::local_date(1_780_000_000_000);
        assert_eq!(store.totals_for_date(&date).unwrap().input_tokens, 30);

        // 重建只写 1 条:结果必须是 10(替换),而不是 40(累加)
        let cursors = std::collections::HashMap::new();
        let state = serde_json::json!({"marker": 1});
        store
            .replace_agent("dsh", &recs[..1], &cursors, &state)
            .unwrap();
        assert_eq!(store.totals_for_date(&date).unwrap().input_tokens, 10);
        assert_eq!(store.get_kv("state:dsh").unwrap(), "{\"marker\":1}");
        let _ = std::fs::remove_file(path);
    }

    /// absolute_daily(台账首次看到某天的绝对总量)必须先删后写,不能与回填的行累加。
    #[test]
    fn absolute_daily_replaces_instead_of_accumulating() {
        let path = temp_db();
        let store = Store::open(&path).unwrap();

        // 回填先写下 100
        let mut backfilled = record();
        backfilled.bucket_date = Some("2026-08-31".into());
        backfilled.input_tokens = 100;
        store.apply_records(&[backfilled]).unwrap();
        assert_eq!(store.totals_for_date("2026-08-31").unwrap().input_tokens, 100);

        // 台账报到当天绝对总量 30 → 结果必须是 30,不是 130
        let mut absolute = record();
        absolute.bucket_date = Some("2026-08-31".into());
        absolute.input_tokens = 30;
        absolute.absolute_daily = true;
        store.apply_records(&[absolute]).unwrap();
        assert_eq!(
            store.totals_for_date("2026-08-31").unwrap().input_tokens,
            30,
            "绝对总量必须替换,不能与回填的行相加"
        );

        // 普通增量语义不变:再写 5 条 = 35
        let mut increment = record();
        increment.bucket_date = Some("2026-08-31".into());
        increment.input_tokens = 5;
        store.apply_records(&[increment]).unwrap();
        assert_eq!(store.totals_for_date("2026-08-31").unwrap().input_tokens, 35);
        let _ = std::fs::remove_file(path);
    }
    /// 全量重建只替换**本次扫描覆盖到的日期**,覆盖不到的日期历史必须原样保留。
    /// 以前是无条件清空整表:日志换代后旧事件已不在日志里,一次重建就把那些日期
    /// 的按天+按小时数据永久抹掉(日志里已经没有,无法再重建)。
    #[test]
    fn replace_agent_preserves_days_outside_the_new_scan() {
        let path = temp_db();
        let store = Store::open(&path).unwrap();

        // 旧历史:08-14(新扫描不会再产出这个日期,因为日志里已经没有了)
        let mut old = record();
        old.bucket_date = Some("2026-08-14".into());
        old.bucket_hour = Some(5);
        store.apply_records(&[old]).unwrap();
        let before = store.totals_for_date("2026-08-14").unwrap();
        assert_eq!(before.input_tokens, 10);

        // 重建:新扫描只覆盖 09-20
        let mut fresh = record();
        fresh.bucket_date = Some("2026-09-20".into());
        fresh.bucket_hour = Some(7);
        fresh.input_tokens = 3;
        let cursors = std::collections::HashMap::new();
        store
            .replace_agent("dsh", std::slice::from_ref(&fresh), &cursors, &serde_json::Value::Null)
            .unwrap();

        assert_eq!(
            store.totals_for_date("2026-08-14").unwrap().input_tokens,
            before.input_tokens,
            "重建覆盖不到的日期被删掉了 → 历史丢失"
        );
        assert_eq!(store.totals_for_date("2026-09-20").unwrap().input_tokens, 3);
        // 覆盖到的日期仍然只写本次结果(不累加)
        store
            .replace_agent("dsh", std::slice::from_ref(&fresh), &cursors, &serde_json::Value::Null)
            .unwrap();
        assert_eq!(store.totals_for_date("2026-09-20").unwrap().input_tokens, 3);
        let _ = std::fs::remove_file(path);
    }

    /// 只进会话表、两张用量表都不写的记录(projcache)不能让重建删掉那个日期的历史。
    #[test]
    fn replace_agent_ignores_records_that_write_no_usage_tables() {
        let path = temp_db();
        let store = Store::open(&path).unwrap();
        let mut old = record();
        old.bucket_date = Some("2026-08-14".into());
        store.apply_records(&[old]).unwrap();

        let mut session_only = record();
        session_only.bucket_date = Some("2026-08-14".into());
        session_only.skip_daily = true;
        session_only.skip_hourly = true;
        let cursors = std::collections::HashMap::new();
        store
            .replace_agent("dsh", &[session_only], &cursors, &serde_json::Value::Null)
            .unwrap();
        assert_eq!(
            store.totals_for_date("2026-08-14").unwrap().input_tokens,
            10,
            "两张用量表都不写的记录不该触发删除"
        );
        let _ = std::fs::remove_file(path);
    }
    /// 校正:整天缺按天数据的桶被补上;按天已更高的日期原样不动(幂等、不双计)。
    #[test]
    fn reconcile_fills_buckets_whose_daily_rows_are_missing() {
        let path = temp_db();
        let store = Store::open(&path).unwrap();

        // 09-18:只有按小时数据(模拟"按天交给台账"期间被丢掉的日期)
        let mut hourly_only = record();
        hourly_only.skip_daily = true;
        hourly_only.bucket_date = Some("2026-09-18".into());
        hourly_only.bucket_hour = Some(3);
        store.apply_records(&[hourly_only]).unwrap();
        assert_eq!(store.totals_for_date("2026-09-18").unwrap().input_tokens, 0);

        // 09-19:按天/按小时都有 —— 台账写过的历史,校正绝不能碰
        let mut both = record();
        both.bucket_date = Some("2026-09-19".into());
        both.bucket_hour = Some(1);
        store.apply_records(&[both]).unwrap();
        let before = store.totals_for_date("2026-09-19").unwrap();

        let skip = std::collections::HashSet::new();
        assert_eq!(store.reconcile_daily_from_hourly("dsh", &skip).unwrap(), 1);
        assert_eq!(
            store.totals_for_date("2026-09-18").unwrap().input_tokens,
            10,
            "整天缺按天数据的桶必须被补上"
        );
        assert_eq!(
            store.totals_for_date("2026-09-19").unwrap().input_tokens,
            before.input_tokens,
            "按天已经更高的桶不能被校正抹小"
        );

        // 幂等:再跑一次不该再写任何行
        assert_eq!(store.reconcile_daily_from_hourly("dsh", &skip).unwrap(), 0);
        let _ = std::fs::remove_file(path);
    }

    /// 用户现场的真正形状:当天按天表**不是空的**,而是只有台账认领窗口里那一小段
    /// 增量(今日只剩 511.3K / 单模型),而按小时表是完整的。整天判空的守卫看不见
    /// 这种情况,必须逐桶按"按天 < 按小时"来校正。
    #[test]
    fn reconcile_fixes_partially_written_days() {
        let path = temp_db();
        let store = Store::open(&path).unwrap();

        // 按小时表:当天两个模型共 30(这批增量当时被判成"按天交给台账",只进了小时表)
        let mut a = record();
        a.model = Some("m".into());
        a.bucket_date = Some("2026-09-21".into());
        a.bucket_hour = Some(9);
        a.input_tokens = 20;
        a.skip_daily = true;
        let mut b = record();
        b.model = Some("g".into());
        b.bucket_date = Some("2026-09-21".into());
        b.bucket_hour = Some(10);
        b.input_tokens = 10;
        b.skip_daily = true;
        store.apply_records(&[a, b]).unwrap();

        // 按天表:只有一个模型的一小段(增量窗口里漏写的那部分)
        let mut partial = record();
        partial.model = Some("m".into());
        partial.bucket_date = Some("2026-09-21".into());
        partial.skip_hourly = true;
        partial.input_tokens = 3;
        store.apply_records(&[partial]).unwrap();
        assert_eq!(store.totals_for_date("2026-09-21").unwrap().input_tokens, 3);

        let skip = std::collections::HashSet::new();
        assert_eq!(
            store.reconcile_daily_from_hourly("dsh", &skip).unwrap(),
            2,
            "两个桶都要按按小时表校正"
        );
        let t = store.totals_for_date("2026-09-21").unwrap();
        assert_eq!(t.input_tokens, 30, "按天必须等于按小时的合计,不是 3 也不是 33");
        assert_eq!(store.reconcile_daily_from_hourly("dsh", &skip).unwrap(), 0);
        let _ = std::fs::remove_file(path);
    }

    /// 台账负责的桶不参与校正(否则台账重写该桶时会两边各记一次);
    /// 逐桶判定:同一天里台账没认领的模型仍要校正。
    #[test]
    fn reconcile_skips_buckets_owned_by_the_ledger() {
        let path = temp_db();
        let store = Store::open(&path).unwrap();
        let mut hourly = record();
        hourly.skip_daily = true;
        hourly.bucket_date = Some("2026-09-18".into());
        hourly.bucket_hour = Some(3);
        store.apply_records(&[hourly]).unwrap();

        let skip: std::collections::HashSet<String> =
            ["2026-09-18|p:m".to_string()].into_iter().collect();
        assert_eq!(store.reconcile_daily_from_hourly("dsh", &skip).unwrap(), 0);
        assert_eq!(store.totals_for_date("2026-09-18").unwrap().input_tokens, 0);

        let other: std::collections::HashSet<String> =
            ["2026-09-18|p:other".to_string()].into_iter().collect();
        assert_eq!(
            store.reconcile_daily_from_hourly("dsh", &other).unwrap(),
            1,
            "台账没认领的桶必须自己校正"
        );
        assert_eq!(store.totals_for_date("2026-09-18").unwrap().input_tokens, 10);
        let _ = std::fs::remove_file(path);
    }
    #[test]
    fn replace_agent_persists_cursors() {
        let path = temp_db();
        let store = Store::open(&path).unwrap();
        let mut cursors = std::collections::HashMap::new();
        cursors.insert(
            "p".to_string(),
            FileCursor {
                offset: 42,
                ..Default::default()
            },
        );
        store
            .replace_agent("dsh", &[], &cursors, &serde_json::Value::Null)
            .unwrap();
        let loaded = store.load_cursors("dsh");
        assert_eq!(loaded.get("p").unwrap().offset, 42);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn apply_records_can_split_daily_and_hourly_writes() {
        let path = temp_db();
        let store = Store::open(&path).unwrap();
        let mut daily = record();
        daily.skip_hourly = true;
        let mut hourly = record();
        hourly.skip_daily = true;
        hourly.bucket_date = Some("2026-08-31".into());
        hourly.bucket_hour = Some(9);
        store.apply_records(&[daily, hourly]).unwrap();
        assert_eq!(
            store
                .totals_for_date(&crate::model::local_date(1_780_000_000_000))
                .unwrap()
                .input_tokens,
            10
        );
        let rows = store
            .daily(Some("dsh"), "2026-08-31", "2026-08-31", "hour", None)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].date, "2026-08-31 09:00");
        assert_eq!(rows[0].input_tokens, 10);
        store.wipe_agent("dsh").unwrap();
        assert!(store
            .daily(Some("dsh"), "2026-08-31", "2026-08-31", "hour", None)
            .unwrap()
            .is_empty());
        let _ = std::fs::remove_file(path);
    }

    /// 数据版本号只在真的写入时递增:前端拿它当刷新键,空扫描必须保持不动。
    #[test]
    fn data_version_only_advances_when_something_is_written() {
        let path = temp_db();
        let store = Store::open(&path).unwrap();
        assert_eq!(store.summary().unwrap().data_version, 0, "空库版本号应为 0");

        store.apply_records(&[]).unwrap();
        assert_eq!(
            store.summary().unwrap().data_version,
            0,
            "空批次不该推版本号,否则空闲时每 2 秒一次的扫描会让前端一直重拉"
        );

        store.apply_records(&[record()]).unwrap();
        let v1 = store.summary().unwrap().data_version;
        assert_eq!(v1, 1);

        store.apply_records(&[]).unwrap();
        assert_eq!(store.summary().unwrap().data_version, v1);

        store.apply_records(&[record()]).unwrap();
        assert_eq!(store.summary().unwrap().data_version, v1 + 1);

        // generated_at 是"数据最后变化时间"而不是查询时间:连续查询必须一致
        let a = store.summary().unwrap().generated_at;
        let b = store.summary().unwrap().generated_at;
        assert_eq!(
            a, b,
            "generated_at 每轮都变的话,前端刷新键会连锁触发 6 次 invoke"
        );
        assert!(a > 1_700_000_000_000, "应当是毫秒时间戳: {a}");

        let _ = std::fs::remove_file(path);
    }

    /// 峰谷:peak=None 时与旧公式**逐位相同**;有 peak 时按占比在平价/高峰之间加权。
    #[test]
    fn peak_pricing_is_bit_identical_without_a_peak_tier() {
        let t = Totals {
            input_tokens: 1_000_000,
            output_tokens: 500_000,
            cache_read_tokens: 2_000_000,
            cache_write_tokens: 100_000,
            calls: 1,
            total_tokens: 3_600_000,
            cost: 0.0,
        };
        let flat = PriceEntry {
            input: 0.15,
            output: 0.6,
            cache_read: 0.003,
            cache_write: 0.15,
            peak: None,
        };
        let all_peak = crate::peak::PeakShare {
            input: 1.0,
            output: 1.0,
            cache_read: 1.0,
            cache_write: 1.0,
        };
        // 没有 peak 档:占比传什么都一样
        let a = estimate_cost(&t, &flat, 7.2, crate::peak::PeakShare::default());
        let b = estimate_cost(&t, &flat, 7.2, all_peak);
        assert_eq!(a.to_bits(), b.to_bits(), "无峰谷档时必须逐位一致");

        // 有 peak 档:全平价 == 平价公式;全高峰 == 2 倍
        let tiered = PriceEntry {
            peak: Some(crate::settings::PeakTier {
                input: 0.3,
                output: 1.2,
                cache_read: 0.006,
                cache_write: 0.3,
            }),
            ..flat.clone()
        };
        let off = estimate_cost(&t, &tiered, 7.2, crate::peak::PeakShare::default());
        assert_eq!(off.to_bits(), a.to_bits(), "全平价必须等于平价公式");
        let on = estimate_cost(&t, &tiered, 7.2, all_peak);
        assert!((on - a * 2.0).abs() < 1e-9, "高峰价是平价的 2 倍: {on} vs {}", a * 2.0);

        // 一半在高峰
        let half = crate::peak::PeakShare {
            input: 0.5,
            output: 0.5,
            cache_read: 0.5,
            cache_write: 0.5,
        };
        let mixed = estimate_cost(&t, &tiered, 7.2, half);
        assert!((mixed - a * 1.5).abs() < 1e-9, "一半高峰应当是 1.5 倍");
    }

    /// 按小时表 → 峰谷占比:高峰小时算 1、平价小时算 0,按 token 量加权。
    #[test]
    fn peak_shares_come_from_the_hourly_table() {
        let path = temp_db();
        let store = Store::open(&path).unwrap();
        let mut peak_hour = record();
        peak_hour.bucket_date = Some("2026-09-21".into()); // 周一
        peak_hour.bucket_hour = Some(10); // 高峰
        peak_hour.skip_daily = true;
        peak_hour.input_tokens = 30;
        let mut off_hour = record();
        off_hour.bucket_date = Some("2026-09-21".into());
        off_hour.bucket_hour = Some(20); // 平价
        off_hour.skip_daily = true;
        off_hour.input_tokens = 10;
        store.apply_records(&[peak_hour, off_hour]).unwrap();

        let shares = store
            .peak_shares(Some("dsh"), "2026-09-21", "2026-09-21")
            .unwrap();
        let got = shares.get("2026-09-21", "m", "p");
        assert!((got.input - 0.75).abs() < 1e-9, "30/40 在高峰: {got:?}");

        // 周末整天都是平价
        let mut weekend = record();
        weekend.bucket_date = Some("2026-09-19".into()); // 周六
        weekend.bucket_hour = Some(10);
        weekend.skip_daily = true;
        weekend.input_tokens = 5;
        store.apply_records(&[weekend]).unwrap();
        let shares = store
            .peak_shares(Some("dsh"), "2026-09-19", "2026-09-19")
            .unwrap();
        assert_eq!(shares.get("2026-09-19", "m", "p").input, 0.0);
        let _ = std::fs::remove_file(path);
    }

    /// 明细表(sessions)与顶部大卡(range_summary)必须共用同一套成本口径。
    /// 以前 sessions() 完全不查定价表、只按 agent='dsh' 特判换算,两边系统性对不上。
    #[test]
    fn sessions_and_range_summary_share_one_cost_basis() {
        let path = temp_db();
        let store = Store::open(&path).unwrap();
        let mut rec = record();
        rec.session_id = Some("s1".into());
        rec.input_tokens = 1_000_000;
        rec.cost = 1.0; // 自带成本 1(单位由 Agent 币种决定)
        store.apply_records(&[rec]).unwrap();

        let none = HashMap::new();
        let cny = HashMap::from([("dsh".to_string(), "CNY".to_string())]);
        let range_of = |basis: &CostBasis| {
            store
                .range_summary(None, "2000-01-01", "2100-01-01", basis, None, None)
                .unwrap()
                .totals
                .cost
        };
        let session_cost = |basis: &CostBasis| {
            store
                .sessions(None, None, None, 10, basis, None, None)
                .unwrap()
                .first()
                .unwrap()
                .cost
        };

        // 1) 没有定价 -> 用自带成本;dsh 声明为 CNY,不乘汇率
        let basis = CostBasis::new(&none, 7.2, &cny);
        assert!((range_of(&basis) - 1.0).abs() < 1e-9);
        assert!((session_cost(&basis) - 1.0).abs() < 1e-9, "明细表必须和大卡同口径");

        // 2) 有定价 -> 两边都按 tokens × 单价 × 汇率 重算,覆盖自带成本
        let pricing = HashMap::from([(
            "m".to_string(),
            PriceEntry {
                input: 2.0,
                output: 0.0,
                cache_read: 0.0,
                cache_write: 0.0,
                peak: None,
            },
        )]);
        let expected = 1_000_000.0 / 1e6 * 2.0 * 7.2;
        let basis = CostBasis::new(&pricing, 7.2, &cny);
        assert!((range_of(&basis) - expected).abs() < 1e-9);
        assert!(
            (session_cost(&basis) - expected).abs() < 1e-9,
            "明细表必须按定价重算,而不是只乘汇率: {}",
            session_cost(&basis)
        );

        // 3) 没声明币种的 Agent 按美元处理(与历史行为一致,但不再针对 dsh 特判)
        let no_currency = HashMap::new();
        let basis = CostBasis::new(&none, 7.2, &no_currency);
        assert!((session_cost(&basis) - 7.2).abs() < 1e-9);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn daily_model_filter_matches_named_and_unknown() {
        let path = temp_db();
        let store = Store::open(&path).unwrap();
        let mut alpha = record();
        alpha.model = Some("alpha".into());
        alpha.bucket_date = Some("2026-10-01".into());
        alpha.input_tokens = 10;
        let mut beta = record();
        beta.model = Some("beta".into());
        beta.bucket_date = Some("2026-10-01".into());
        beta.input_tokens = 100;
        let mut unknown = record();
        unknown.model = None;
        unknown.bucket_date = Some("2026-10-01".into());
        unknown.input_tokens = 4;
        store.apply_records(&[alpha, beta, unknown]).unwrap();

        let named = store
            .daily(None, "2026-10-01", "2026-10-01", "day", Some("alpha"))
            .unwrap();
        assert_eq!(named.iter().map(|r| r.input_tokens).sum::<u64>(), 10);
        let blank = store
            .daily(
                None,
                "2026-10-01",
                "2026-10-01",
                "day",
                Some("(未知模型)"),
            )
            .unwrap();
        assert_eq!(blank.iter().map(|r| r.input_tokens).sum::<u64>(), 4);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn hour_profile_sums_the_same_clock_hour_and_filters_model() {
        let path = temp_db();
        let store = Store::open(&path).unwrap();
        let mut day1 = record();
        day1.model = Some("alpha".into());
        day1.bucket_date = Some("2026-10-01".into());
        day1.bucket_hour = Some(14);
        day1.input_tokens = 10;
        let mut day2 = day1.clone();
        day2.bucket_date = Some("2026-10-02".into());
        day2.input_tokens = 5;
        let mut morning = day1.clone();
        morning.bucket_hour = Some(3);
        morning.input_tokens = 7;
        let mut other = day1.clone();
        other.model = Some("beta".into());
        other.input_tokens = 100;
        store.apply_records(&[day1, day2, morning, other]).unwrap();

        let hours = store
            .hour_profile(None, "2026-10-01", "2026-10-02", Some("alpha"), None)
            .unwrap();
        assert_eq!(hours.len(), 24);
        assert_eq!(hours[14].hour, 14);
        assert_eq!(hours[14].total_tokens, 15);
        assert_eq!(hours[3].total_tokens, 7);
        assert_eq!(hours[0].total_tokens, 0);
        let _ = std::fs::remove_file(path);
    }
}
