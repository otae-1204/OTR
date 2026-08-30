use tauri::{AppHandle, Manager};

use crate::model::{
    AgentStatus, DailyUsage, RangeSummary, SessionUsage, UsageSummary, date_str, today_str,
};
use crate::settings::Settings;
use crate::{providers, run_scan, AppState};

fn err_str<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
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
    let settings = state.settings.lock().unwrap().clone();
    let t_today = state.store.agent_today(&today_str()).unwrap_or_default();
    let t_all = state.store.agent_all().unwrap_or_default();
    let customs = providers::build_customs(&settings);
    let all = state
        .providers
        .iter()
        .map(|b| b.as_ref() as &dyn providers::AgentProvider)
        .chain(customs.iter().map(|b| b.as_ref() as &dyn providers::AgentProvider));
    all.map(|p| AgentStatus {
            id: p.id().to_string(),
            display_name: p.display_name().to_string(),
            detected: p.detect(),
            enabled: settings.is_enabled(p.id()),
            today_tokens: t_today.get(p.id()).map(|t| t.total_tokens).unwrap_or(0),
            today_cost: t_today.get(p.id()).map(|t| t.cost).unwrap_or(0.0),
            total_tokens: t_all.get(p.id()).map(|t| t.total_tokens).unwrap_or(0),
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
    let settings = state.settings.lock().unwrap().clone();
    let mut s = state
        .store
        .range_summary(
            agent.as_deref(),
            &from,
            &to,
            &settings.pricing,
            settings.exchange_rate,
        )
        .map_err(err_str)?;
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
    state
        .store
        .daily(agent.as_deref(), &from, &to, &g)
        .map_err(err_str)
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
    let settings = state.settings.lock().unwrap().clone();
    state
        .store
        .sessions(
            agent.as_deref(),
            from_ms,
            to_ms,
            limit.unwrap_or(100) as i64,
            settings.exchange_rate,
        )
        .map_err(err_str)
}

/// 触发一次后台增量扫描;full=true 时清空该 Agent 本地数据重扫
#[tauri::command]
pub fn rescan(app: AppHandle, full: Option<bool>) {
    let handle = app.clone();
    std::thread::spawn(move || run_scan(&handle, full.unwrap_or(false), None));
}

#[tauri::command]
pub fn get_settings(app: AppHandle) -> Settings {
    app.state::<AppState>().settings.lock().unwrap().clone()
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
        let mut guard = state.settings.lock().unwrap();
        *guard = settings.clone();
        guard.save(&state.settings_path).map_err(err_str)?;
    }
    // 新启用的 Agent 立即补一次扫描,并按新配置重挂文件监听
    let handle = app.clone();
    std::thread::spawn(move || run_scan(&handle, false, None));
    let current = state.settings.lock().unwrap().clone();
    if let Some(w) = state.watcher.lock().unwrap().as_ref() {
        w.rewatch(crate::watcher::current_watch_paths(&state, &current));
    }
    Ok(())
}
