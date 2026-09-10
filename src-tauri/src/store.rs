use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection};

use crate::error::Result;
use crate::model::{
    local_date, local_hour, now_ms, today_str, AgentSlice, DailyUsage, ModelSlice, RangeSummary,
    SessionUsage, Totals, UsageSummary,
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
        }
    }

    /// 一条 (agent, model) 聚合行的成本;tokens 用于定价重算,raw_cost 是数据自带成本
    pub fn row_cost(&self, model: &str, agent: &str, tokens: &Totals, raw_cost: f64) -> f64 {
        if let Some(price) = self.pricing.get(model) {
            return estimate_cost(tokens, price, self.rate);
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
    pub fn replace_agent(
        &self,
        agent: &str,
        records: &[crate::model::UsageRecord],
        cursors: &HashMap<String, FileCursor>,
        state: &serde_json::Value,
    ) -> Result<usize> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        for sql in [
            "DELETE FROM usage_daily WHERE agent=?1",
            "DELETE FROM usage_hourly WHERE agent=?1",
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
                t.cost = basis.row_cost(&model, &agent, &t, raw_cost);
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
    ) -> Result<Vec<DailyUsage>> {
        let conn = self.conn();
        let sql = match granularity {
            "hour" => {
                "SELECT date || ' ' || printf('%02d:00', hour) AS bucket, agent,
                        SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens),
                        SUM(cache_write_tokens), SUM(calls), SUM(cost)
                 FROM usage_hourly WHERE date >= ?1 AND date <= ?2 AND (?3 IS NULL OR agent = ?3)
                 GROUP BY bucket, agent ORDER BY bucket, agent"
            }
            "month" => {
                "SELECT substr(date,1,7) AS bucket, agent,
                        SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens),
                        SUM(cache_write_tokens), SUM(calls), SUM(cost)
                 FROM usage_daily WHERE date >= ?1 AND date <= ?2 AND (?3 IS NULL OR agent = ?3)
                 GROUP BY bucket, agent ORDER BY bucket, agent"
            }
            _ => {
                "SELECT date, agent, SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens),
                        SUM(cache_write_tokens), SUM(calls), SUM(cost)
                 FROM usage_daily WHERE date >= ?1 AND date <= ?2 AND (?3 IS NULL OR agent = ?3)
                 GROUP BY date, agent ORDER BY date, agent"
            }
        };
        let mut stmt = conn.prepare(sql)?;
        let rows = stmt.query_map(params![from, to, agent], |row| {
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
            let cost = basis.row_cost(&model, &row_agent, &totals, raw_cost);
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

/// 按定价估算费用:($/百万 tokens) × tokens ÷ 1e6 × 汇率
fn estimate_cost(t: &Totals, p: &PriceEntry, rate: f64) -> f64 {
    (t.input_tokens as f64 * p.input
        + t.output_tokens as f64 * p.output
        + t.cache_read_tokens as f64 * p.cache_read
        + t.cache_write_tokens as f64 * p.cache_write)
        / 1e6
        * rate
}

#[cfg(test)]
mod tests {
    use super::{CostBasis, Store};
    use crate::model::UsageRecord;
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
            .daily(Some("dsh"), "2026-08-31", "2026-08-31", "hour")
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].date, "2026-08-31 09:00");
        assert_eq!(rows[0].input_tokens, 10);
        store.wipe_agent("dsh").unwrap();
        assert!(store
            .daily(Some("dsh"), "2026-08-31", "2026-08-31", "hour")
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
                .range_summary(None, "2000-01-01", "2100-01-01", basis, None)
                .unwrap()
                .totals
                .cost
        };
        let session_cost = |basis: &CostBasis| {
            store
                .sessions(None, None, None, 10, basis, None)
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
}
