use tauri::{AppHandle, Manager};

use crate::model::{
    date_str, today_str, AgentStatus, DailyUsage, RangeSummary, SessionUsage, UsageSummary,
};
use crate::settings::Settings;
use crate::store::CostBasis;
use crate::{providers, run_scan, AppState};

fn err_str<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Agent → 自带成本币种,由各 Provider 自己声明。
/// 通用查询代码以前写死 `agent != "dsh"` 猜币种,现在只认这份声明。
fn native_cost_currencies(
    state: &AppState,
    settings: &Settings,
) -> std::collections::HashMap<String, String> {
    let customs = providers::build_customs(settings);
    state
        .providers
        .iter()
        .map(|b| b.as_ref() as &dyn providers::AgentProvider)
        .chain(
            customs
                .iter()
                .map(|b| b.as_ref() as &dyn providers::AgentProvider),
        )
        .filter_map(|p| {
            p.native_cost_currency()
                .map(|c| (p.id().to_string(), c.to_string()))
        })
        .collect()
}

/// "YYYY-MM-DD" -> 当天 00:00 / 23:59:59.999 的本地时间戳(ms)
fn date_boundary_ms(date: &str, end_of_day: bool) -> Option<i64> {
    let d = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    let t = if end_of_day {
        d.and_hms_opt(23, 59, 59)?
    } else {
        d.and_hms_opt(0, 0, 0)?
    };
    t.and_local_timezone(chrono::Local)
        .single()
        .map(|dt| dt.timestamp_millis() + if end_of_day { 999 } else { 0 })
}

#[tauri::command]
pub fn list_agents(app: AppHandle) -> Vec<AgentStatus> {
    let state = app.state::<AppState>();
    let settings = crate::lock(&state.settings).clone();
    // 以前这里多查一次 agent_today 填 todayTokens/todayCost,但前端从未渲染过它们
    // (AgentCard 只读 displayName / totalTokens / 外部传入的 rangeTokens),已删。
    let t_all = state.store.agent_all().unwrap_or_default();
    let customs = providers::build_customs(&settings);
    let all = state
        .providers
        .iter()
        .map(|b| b.as_ref() as &dyn providers::AgentProvider)
        .chain(
            customs
                .iter()
                .map(|b| b.as_ref() as &dyn providers::AgentProvider),
        );
    all.map(|p| AgentStatus {
        id: p.id().to_string(),
        display_name: p.display_name().to_string(),
        detected: p.detect(),
        enabled: settings.is_enabled(p.id()),
        total_tokens: t_all.get(p.id()).map(|t| t.total_tokens).unwrap_or(0),
        // Provider 的健康问题写在它自己的 state 里,这里读出来带给 UI
        notice: state
            .store
            .get_kv(&format!("state:{}", p.id()))
            .and_then(|v| serde_json::from_str::<serde_json::Value>(&v).ok())
            .and_then(|v| p.health(&v)),
    })
    .collect()
}

#[tauri::command]
pub fn get_summary(app: AppHandle) -> std::result::Result<UsageSummary, String> {
    let state = app.state::<AppState>();
    state.store.summary().map_err(err_str)
}

#[tauri::command]
pub fn get_range_summary(
    app: AppHandle,
    agent: Option<String>,
    from: String,
    to: String,
) -> std::result::Result<RangeSummary, String> {
    let state = app.state::<AppState>();
    let settings = crate::lock(&state.settings).clone();
    let currencies = native_cost_currencies(&state, &settings);
    let basis = CostBasis::new(&settings.pricing, settings.exchange_rate, &currencies);
    let mut s = state
        .store
        .range_summary(
            agent.as_deref(),
            &from,
            &to,
            &basis,
            Some(&settings.enabled_agents),
        )
        .map_err(err_str)?;
    // cost 字段恒为 ¥;currency 只是"前端该按哪个币种展示"的提示
    s.currency = settings.currency.clone();
    Ok(s)
}

#[tauri::command]
pub fn get_daily(
    app: AppHandle,
    agent: Option<String>,
    from: Option<String>,
    to: Option<String>,
    granularity: Option<String>,
) -> std::result::Result<Vec<DailyUsage>, String> {
    let state = app.state::<AppState>();
    let from = from.unwrap_or_else(|| date_str(chrono::Duration::days(29)));
    let to = to.unwrap_or_else(today_str);
    let g = granularity.unwrap_or_else(|| "day".into());
    let g = match g.as_str() {
        "hour" | "month" => g,
        _ => "day".into(),
    };
    let mut rows = state
        .store
        .daily(agent.as_deref(), &from, &to, &g)
        .map_err(err_str)?;
    if agent.is_none() {
        let enabled = crate::lock(&state.settings).enabled_agents.clone();
        rows.retain(|r| enabled.iter().any(|id| id == &r.agent));
    }
    Ok(rows)
}

#[tauri::command]
pub fn get_sessions(
    app: AppHandle,
    agent: Option<String>,
    from: Option<String>,
    to: Option<String>,
    limit: Option<u32>,
) -> std::result::Result<Vec<SessionUsage>, String> {
    let state = app.state::<AppState>();
    let from_ms = from.as_deref().and_then(|d| date_boundary_ms(d, false));
    let to_ms = to.as_deref().and_then(|d| date_boundary_ms(d, true));
    let settings = crate::lock(&state.settings).clone();
    let currencies = native_cost_currencies(&state, &settings);
    let basis = CostBasis::new(&settings.pricing, settings.exchange_rate, &currencies);
    state
        .store
        .sessions(
            agent.as_deref(),
            from_ms,
            to_ms,
            limit.unwrap_or(100) as i64,
            &basis,
            Some(&settings.enabled_agents),
        )
        .map_err(err_str)
}

/// 触发一次后台增量扫描;full=true 时清空该 Agent 本地数据重扫。
///
/// 以前每次调用都无条件 spawn 一个线程:连点托盘就会排队一串扫描,一个接一个地跑完,
/// 期间界面看起来像卡住。现在改成"登记 + 合并":已有 worker 时只把一个待跑标记置上,
/// 由它跑完当前轮后在收尾处再跑一次;非全量请求遇到全量请求会升级成全量,而不是各跑一遍。
#[tauri::command]
pub fn rescan(app: AppHandle, full: Option<bool>) {
    // 已经有 worker 时只登记请求:它收尾时会看到 pending > 0 并把这一轮跑掉
    if !app.state::<AppState>().rescan.request(full.unwrap_or(false)) {
        return;
    }
    let handle = app.clone();
    std::thread::spawn(move || loop {
        match handle.state::<AppState>().rescan.next() {
            crate::RescanStep::Run { full } => run_scan(&handle, full, None),
            crate::RescanStep::Stop => break,
        }
    });
}

#[tauri::command]
pub fn get_settings(app: AppHandle) -> Settings {
    crate::lock(&app.state::<AppState>().settings).clone()
}

/// 出现过的全部模型名(设置页定价表用)
#[tauri::command]
pub fn list_models(app: AppHandle) -> Vec<String> {
    app.state::<AppState>()
        .store
        .list_models()
        .unwrap_or_default()
}

#[tauri::command]
pub fn save_settings(app: AppHandle, settings: Settings) -> std::result::Result<(), String> {
    let state = app.state::<AppState>();
    {
        let mut guard = crate::lock(&state.settings);
        *guard = settings.clone();
        guard.save(&state.settings_path).map_err(err_str)?;
    }
    // 新启用的 Agent 立即补一次扫描,并按新配置重挂文件监听
    let handle = app.clone();
    std::thread::spawn(move || run_scan(&handle, false, None));
    let current = crate::lock(&state.settings).clone();
    if let Some(w) = crate::lock(&state.watcher).as_ref() {
        w.rewatch(crate::watcher::current_watch_paths(&state, &current));
    }
    Ok(())
}
