use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection};

use crate::error::Result;
use crate::model::{
    date_str, local_date, local_hour, now_ms, today_str, AgentSlice, DailyUsage, ModelSlice,
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

pub struct Store {
    conn: Mutex<Connection>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    pub fn wipe_agent(&self, agent: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
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
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
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
            }
        }
        tx.commit()?;
        Ok(n)
    }

    // ---------- 查询 ----------

    fn totals_eq(conn: &Connection, date: &str) -> Result<Totals> {
        Self::totals_query(conn, "WHERE date = ?1", rusqlite::params![date])
    }

    fn totals_ge(conn: &Connection, from: &str) -> Result<Totals> {
        Self::totals_query(conn, "WHERE date >= ?1", rusqlite::params![from])
    }

    fn totals_all(conn: &Connection) -> Result<Totals> {
        Self::totals_query(conn, "", rusqlite::params![])
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
        let conn = self.conn.lock().unwrap();
        Self::totals_eq(&conn, date)
    }

    pub fn summary(&self) -> Result<UsageSummary> {
        let conn = self.conn.lock().unwrap();
        let today = today_str();
        let week_from = date_str(chrono::Duration::days(6));
        let month_from = date_str(chrono::Duration::days(29));
        let today_t = Self::totals_eq(&conn, &today)?;
        let week_t = Self::totals_ge(&conn, &week_from)?;
        let month_t = Self::totals_ge(&conn, &month_from)?;
        let all_t = Self::totals_all(&conn)?;

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

        let mut by_model = Vec::new();
        {
            let mut stmt = conn.prepare(
                "SELECT COALESCE(NULLIF(model,''),'(未知模型)'), SUM(input_tokens), SUM(output_tokens),
                        SUM(cache_read_tokens), SUM(cache_write_tokens), SUM(calls), SUM(cost)
                 FROM usage_daily WHERE date >= ?1 GROUP BY model
                 ORDER BY SUM(input_tokens+output_tokens+cache_read_tokens+cache_write_tokens) DESC
                 LIMIT 12",
            )?;
            let rows = stmt.query_map(params![month_from], |row| {
                Ok((row.get::<_, String>(0)?, totals_from_row(row, 1)))
            })?;
            for r in rows {
                let (model, t) = r?;
                by_model.push(crate::model::ModelSlice { model, totals: t });
            }
        }

        Ok(UsageSummary {
            generated_at: now_ms(),
            today: today_t,
            week: week_t,
            month: month_t,
            all_time: all_t,
            by_agent_today: by_agent,
            by_model_month: by_model,
        })
    }

    /// 任意日期范围(可按 Agent 过滤)的统计。
    /// 成本规则:模型在 pricing 里有定价 → 按 tokens×定价×汇率 重算(**覆盖自带成本**);
    /// 没有定价 → 用自带成本,并按来源币种归一化(DSH 为 ¥,其余为 $×汇率)。输出统一为 ¥。
    pub fn range_summary(
        &self,
        agent: Option<&str>,
        from: &str,
        to: &str,
        pricing: &std::collections::HashMap<String, PriceEntry>,
        exchange_rate: f64,
        // None = 不过滤;Some = 仅计入这些 Agent(设置里停用的不进主页合计)
        enabled_agents: Option<&[String]>,
    ) -> Result<RangeSummary> {
        let conn = self.conn.lock().unwrap();
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
            p: &std::collections::HashMap<String, PriceEntry>,
            rate: f64,
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
                if let Some(pe) = p.get(&model) {
                    t.cost = estimate_cost(&t, pe, rate);
                } else if raw_cost.abs() > f64::EPSILON && agent != "dsh" {
                    t.cost = raw_cost * rate;
                }
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
        let mut by_model = Vec::new();
        {
            let sql = base_sql.replace("{agent_cond}", "AND (?3 IS NULL OR agent = ?3)");
            let mut stmt = conn.prepare(&sql)?;
            let mut model_map: std::collections::HashMap<String, Totals> =
                std::collections::HashMap::new();
            fold_rows(
                &mut stmt,
                &[&from, &to, &agent],
                pricing,
                exchange_rate,
                |model, ag, t| {
                    if agent.is_none() && !enabled_ok(ag) {
                        return;
                    }
                    merge(&mut totals, &t);
                    merge(model_map.entry(model.to_string()).or_default(), &t);
                },
            )?;
            by_model = model_map
                .into_iter()
                .map(|(model, totals)| ModelSlice { model, totals })
                .collect();
            by_model.sort_by(|a, b| b.totals.total_tokens.cmp(&a.totals.total_tokens));
            by_model.truncate(12);
        }

        // Q2:不按选中 Agent 过滤 → by_agent(卡片用);仍排除设置里停用的
        let mut by_agent = Vec::new();
        {
            let sql = base_sql.replace("{agent_cond}", "");
            let mut stmt = conn.prepare(&sql)?;
            let mut agent_map: std::collections::HashMap<String, Totals> =
                std::collections::HashMap::new();
            fold_rows(
                &mut stmt,
                &[&from, &to],
                pricing,
                exchange_rate,
                |_model, ag, t| {
                    if !enabled_ok(ag) {
                        return;
                    }
                    merge(agent_map.entry(ag.to_string()).or_default(), &t);
                },
            )?;
            by_agent = agent_map
                .into_iter()
                .map(|(agent, totals)| AgentSlice { agent, totals })
                .collect();
            by_agent.sort_by(|a, b| b.totals.total_tokens.cmp(&a.totals.total_tokens));
        }

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
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
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

    pub fn sessions(
        &self,
        agent: Option<&str>,
        from_ms: Option<i64>,
        to_ms: Option<i64>,
        limit: i64,
        exchange_rate: f64,
        // None = 不过滤;Some = 未指定单个 Agent 时仅返回这些 Agent 的会话
        enabled_agents: Option<&[String]>,
    ) -> Result<Vec<SessionUsage>> {
        let conn = self.conn.lock().unwrap();
        let enabled_json = match enabled_agents {
            None => "null".to_string(),
            Some(list) => serde_json::to_string(list).unwrap_or_else(|_| "[]".into()),
        };
        let mut stmt = conn.prepare(
            "SELECT m.agent, m.session_id, MAX(meta.project), MAX(meta.title),
                    GROUP_CONCAT(DISTINCT NULLIF(m.model,'')),
                    MIN(meta.started_at), MAX(m.last_ts),
                    SUM(m.input_tokens), SUM(m.output_tokens), SUM(m.cache_read_tokens),
                    SUM(m.cache_write_tokens), SUM(m.calls),
                    SUM(CASE WHEN m.agent = 'dsh' THEN m.cost ELSE m.cost * ?4 END)
             FROM usage_session_models m
             LEFT JOIN session_meta meta ON meta.agent = m.agent AND meta.session_id = m.session_id
             WHERE (?1 IS NULL OR m.agent = ?1)
               AND (?2 IS NULL OR m.last_ts >= ?2)
               AND (?3 IS NULL OR m.last_ts < ?3)
               AND (?6 = 'null' OR ?1 IS NOT NULL
                    OR m.agent IN (SELECT value FROM json_each(?6)))
             GROUP BY m.agent, m.session_id
             ORDER BY MAX(m.last_ts) DESC LIMIT ?5",
        )?;
        let rows = stmt.query_map(
            params![agent, from_ms, to_ms, exchange_rate, limit, enabled_json],
            |row| {
                let input: i64 = row.get(7)?;
                let output: i64 = row.get(8)?;
                let cr: i64 = row.get(9)?;
                let cw: i64 = row.get(10)?;
                Ok(SessionUsage {
                    agent: row.get(0)?,
                    session_id: row.get(1)?,
                    project: row.get(2)?,
                    title: row.get(3)?,
                    models: row.get(4)?,
                    started_at: row.get(5)?,
                    last_active: row.get(6)?,
                    input_tokens: input as u64,
                    output_tokens: output as u64,
                    cache_read_tokens: cr as u64,
                    cache_write_tokens: cw as u64,
                    calls: row.get::<_, i64>(11)? as u64,
                    cost: row.get(12)?,
                    total_tokens: (input + output + cr + cw) as u64,
                })
            },
        )?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn agent_today(&self, date: &str) -> Result<HashMap<String, Totals>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT agent, SUM(input_tokens), SUM(output_tokens), SUM(cache_read_tokens),
                    SUM(cache_write_tokens), SUM(calls), SUM(cost)
             FROM usage_daily WHERE date = ?1 GROUP BY agent",
        )?;
        let rows = stmt.query_map(params![date], |row| {
            Ok((row.get::<_, String>(0)?, totals_from_row(row, 1)))
        })?;
        let mut map = HashMap::new();
        for r in rows {
            let (agent, t) = r?;
            map.insert(agent, t);
        }
        Ok(map)
    }

    pub fn agent_all(&self) -> Result<HashMap<String, Totals>> {
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO file_cursors (agent, path, data) VALUES (?1, ?2, ?3)
             ON CONFLICT(agent, path) DO UPDATE SET data = excluded.data",
            params![agent, path, serde_json::to_string(cursor)?],
        )?;
        Ok(())
    }

    pub fn load_cursors(&self, agent: &str) -> HashMap<String, FileCursor> {
        let conn = self.conn.lock().unwrap();
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
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO kv (k, v) VALUES (?1, ?2) ON CONFLICT(k) DO UPDATE SET v = excluded.v",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn get_kv(&self, key: &str) -> Option<String> {
        let conn = self.conn.lock().unwrap();
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
    use super::Store;
    use crate::model::UsageRecord;
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
}
