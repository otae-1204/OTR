use tauri::{AppHandle, Manager};

use crate::model::{
    date_str, today_str, AgentStatus, DailyUsage, RangeSummary, SessionUsage, UsageSummary,
};
use crate::limits;
use crate::settings::{LimitAccount, Settings};
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
    // 峰谷:按小时表算出各桶的高峰占比。只有填了 peak 档的模型会受影响。
    let peaks = state
        .store
        .peak_shares(agent.as_deref(), &from, &to)
        .unwrap_or_default();
    let basis = basis.with_peaks(&peaks);
    let mut s = state
        .store
        .range_summary(
            agent.as_deref(),
            &from,
            &to,
            &basis,
            Some(&settings.enabled_agents),
            Some(&peaks),
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
    // 明细表与会话同区间取峰谷占比,保证两张表成本口径一致
    let (from_s, to_s) = (
        from.unwrap_or_else(|| date_str(chrono::Duration::days(29))),
        to.unwrap_or_else(today_str),
    );
    let peaks = state
        .store
        .peak_shares(agent.as_deref(), &from_s, &to_s)
        .unwrap_or_default();
    let basis = basis.with_peaks(&peaks);
    state
        .store
        .sessions(
            agent.as_deref(),
            from_ms,
            to_ms,
            limit.unwrap_or(100) as i64,
            &basis,
            Some(&settings.enabled_agents),
            Some(&peaks),
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

// ---- 额度页 ----

/// 读内存缓存,**不发网络** —— 前端切到额度页要秒开。
#[tauri::command]
pub fn get_limits(app: AppHandle) -> Vec<limits::ProviderLimits> {
    app.state::<AppState>().limits.snapshot()
}

/// 强制拉取。`account` 只给单个账号时只刷新那一个(失败也只影响它)。
///
/// **必须是 async 命令**。Tauri 对非 async 命令判为 `ExecutionContext::Blocking`,
/// 命令体是**内联在 IPC 调用里**执行的(见 tauri-macros 的 `body_blocking`),
/// 也就是跑在窗口/事件循环那条线程上。这里做的是阻塞式网络 I/O,于是:
///
/// - 某家登录态失效(如 Codex 没有 `auth.json` → 走网络请求拿到 401),
///   或者域名连不通要等连接超时,整条链路的耗时直接冻结 UI;
/// - 多账号是**串行**拉的,耗时相加;
/// - 而 http 层还会"环境变量代理 → 系统代理 → 直连"逐档重试,再乘一遍。
///
/// 改成 async 后命令体被 `respond_async` 派到 async 线程池,UI 线程不再等待。
/// 里面仍是阻塞调用,所以用 `spawn_blocking` 而不是直接在 async 上下文里跑 ——
/// 阻塞一个 tokio worker 会饿死其它任务。
#[tauri::command]
pub async fn refresh_limits(
    app: AppHandle,
    account: Option<String>,
) -> std::result::Result<Vec<limits::ProviderLimits>, String> {
    let state = app.state::<AppState>();
    let settings = crate::lock(&state.settings).clone();
    let plans = limits::resolve_accounts(&settings);
    let plans: Vec<_> = match account.as_deref() {
        None => plans,
        Some(id) => plans.into_iter().filter(|p| p.account.id == id).collect(),
    };

    // 网络 I/O 放阻塞线程池:UI 线程和 tokio worker 都不该被它占住
    let fresh = tauri::async_runtime::spawn_blocking(move || {
        plans.iter().map(limits::fetch_account).collect::<Vec<_>>()
    })
    .await
    .map_err(|e| format!("额度刷新任务失败: {e}"))?;

    if account.is_none() {
        state.limits.replace(fresh);
    } else {
        state.limits.merge(fresh);
    }
    Ok(state.limits.snapshot())
}

/// 设置里的账号清单(含内置那条)+ 该 Provider 是否启用
#[tauri::command]
pub fn list_limit_accounts(app: AppHandle) -> Vec<LimitAccountView> {
    let state = app.state::<AppState>();
    let settings = crate::lock(&state.settings).clone();
    limits::resolve_accounts(&settings)
        .into_iter()
        .map(|p| {
            let key_present = p
                .account
                .secret_ref
                .as_deref()
                .map(limits::credential::exists)
                .unwrap_or(false);
            let cookie_present = p
                .account
                .cookie_ref
                .as_deref()
                .map(limits::credential::exists)
                .unwrap_or(false);
            LimitAccountView {
                id: p.account.id,
                provider: p.account.provider,
                label: p.account.label,
                plan: p.account.plan,
                home: p.account.home,
                secret_ref: p.account.secret_ref,
                cookie_ref: p.account.cookie_ref,
                key_present,
                cookie_present,
                builtin: p.builtin,
            }
        })
        .collect()
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitAccountView {
    pub id: String,
    pub provider: String,
    pub label: String,
    pub plan: String,
    pub home: Option<String>,
    pub secret_ref: Option<String>,
    pub cookie_ref: Option<String>,
    /// 该账号的 API Key 条目是否已写入。只有布尔值,没有密钥。
    pub key_present: bool,
    /// StepFun 控制台 Cookie 是否已写入。
    pub cookie_present: bool,
    /// 内置账号不可删
    pub builtin: bool,
}

/// 新增/更新一个额外账号,并在同一次调用里写入密钥。
///
/// `api_key` / `console_cookie` 只在这次命令里出现:写进系统凭据库后就丢掉,
/// 不进设置 JSON,也不出现在任何返回值里。空字符串表示沿用已有条目。
/// 凭据写入失败时账号记录不落盘。
#[tauri::command]
pub fn save_limit_account(
    app: AppHandle,
    account: LimitAccount,
    api_key: Option<String>,
    console_cookie: Option<String>,
) -> std::result::Result<(), String> {
    let state = app.state::<AppState>();
    let mut settings = crate::lock(&state.settings);
    let (account, writes) = limits::prepare_account(
        &settings.limit_accounts,
        account,
        api_key,
        console_cookie,
    )?;
    let is_new = !settings.limit_accounts.iter().any(|a| a.id == account.id);
    let mut written: Vec<String> = Vec::new();
    for (name, secret) in &writes {
        if let Err(e) = limits::credential::set(name, secret) {
            // 新建失败就撤掉刚写的条目。更新失败不能删:同一个名字上还留着旧密钥。
            if is_new {
                for n in written.iter().rev() {
                    let _ = limits::credential::delete(n);
                }
            }
            return Err(e);
        }
        written.push(name.clone());
    }
    let mut next = settings.clone();
    let account_id = account.id.clone();
    match next.limit_accounts.iter_mut().find(|a| a.id == account_id) {
        Some(slot) => *slot = account,
        None => next.limit_accounts.push(account),
    }
    if let Err(e) = next.save(&state.settings_path) {
        if is_new {
            for n in written.iter().rev() {
                let _ = limits::credential::delete(n);
            }
        }
        return Err(err_str(e));
    }
    *settings = next;
    Ok(())
}

/// 删除一个额外账号。**内置账号删不掉**(它不是存在设置里的,是合成出来的)。
#[tauri::command]
pub fn delete_limit_account(app: AppHandle, id: String) -> std::result::Result<(), String> {
    let state = app.state::<AppState>();
    let mut settings = crate::lock(&state.settings);
    let Some(removed) = settings
        .limit_accounts
        .iter()
        .find(|a| a.id == id)
        .cloned()
    else {
        return Err("内置账号不能删除(可在设置里取消勾选该额度来源)".into());
    };
    let credential_names = limits::owned_credential_names(&removed);
    let mut next = settings.clone();
    next.limit_accounts.retain(|a| a.id != id);
    next.save(&state.settings_path).map_err(err_str)?;
    // 缓存里也要跟着消失,否则删除后卡片还挂着旧数字
    let keep: Vec<String> = limits::resolve_accounts(&next)
        .into_iter()
        .map(|p| p.account.id)
        .collect();
    *settings = next;
    state.limits.retain_accounts(&keep);
    for name in credential_names {
        if let Err(e) = limits::credential::delete(&name) {
            eprintln!("[otr] 删除账号凭据失败 {name}: {e}");
        }
    }
    Ok(())
}

/// 勾选/取消勾选一个额度来源(StepFun 默认关闭,靠这里打开)
#[tauri::command]
pub fn set_limit_provider(
    app: AppHandle,
    provider: String,
    enabled: bool,
) -> std::result::Result<(), String> {
    if !crate::settings::LIMIT_PROVIDERS.contains(&provider.as_str()) {
        return Err(format!("不支持的额度来源:{provider}"));
    }
    let state = app.state::<AppState>();
    let mut settings = crate::lock(&state.settings);
    let mut next = settings.clone();
    if enabled {
        if !next.limit_providers.iter().any(|p| p == &provider) {
            next.limit_providers.push(provider);
        }
    } else {
        next.limit_providers.retain(|p| p != &provider);
    }
    next.save(&state.settings_path).map_err(err_str)?;
    let keep: Vec<String> = limits::resolve_accounts(&next)
        .into_iter()
        .map(|p| p.account.id)
        .collect();
    *settings = next;
    state.limits.retain_accounts(&keep);
    Ok(())
}

/// 凭据存在性 —— **只有布尔值,永不返回密钥**。
#[tauri::command]
pub fn list_limit_credentials() -> Vec<CredentialView> {
    limits::credential::list()
        .into_iter()
        .map(|(name, present)| CredentialView { name, present })
        .collect()
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialView {
    pub name: String,
    pub present: bool,
}

/// 写入凭据(进系统凭据库)。传空字符串等于删除。
#[tauri::command]
pub fn save_limit_credential(name: String, secret: String) -> std::result::Result<(), String> {
    if !limits::credential::is_allowed_name(&name) {
        return Err(format!("未知的凭据条目:{name}"));
    }
    limits::credential::set(&name, &secret)
}

#[tauri::command]
pub fn delete_limit_credential(name: String) -> std::result::Result<(), String> {
    if !limits::credential::is_allowed_name(&name) {
        return Err(format!("未知的凭据条目:{name}"));
    }
    limits::credential::delete(&name)
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
pub fn save_settings(app: AppHandle, mut settings: Settings) -> std::result::Result<(), String> {
    let state = app.state::<AppState>();
    {
        let mut guard = crate::lock(&state.settings);
        settings.keep_limit_config_from(&guard);
        *guard = settings;
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

/// 需要手填 Cookie 的套餐(Qwen / StepFun)教程窗口。只在用户点「教程」时创建。
///
/// 必须是 async。同步命令跑在事件循环那条线程上,而建 WebView 又要等事件循环
/// 回消息,两边互相等:窗口标题出来了,页面停在白底,关闭消息也进不去。
#[tauri::command]
pub async fn open_cookie_guide(app: AppHandle, provider: String) -> std::result::Result<(), String> {
    let (label, title, id) = match provider.as_str() {
        "qwen" => ("guide-qwen", "Qwen · Cookie 教程", "qwen"),
        "stepfun" => ("guide-stepfun", "StepFun · Cookie 教程", "stepfun"),
        _ => return Err("这个来源没有 Cookie 教程".into()),
    };
    if let Some(win) = app.get_webview_window(label) {
        // 白屏的那扇关不掉时,先拆掉再重建
        let _ = win.destroy();
    }
    let script = format!("window.__OTR_GUIDE__=\"{id}\";");
    tauri::WebviewWindowBuilder::new(&app, label, tauri::WebviewUrl::App("index.html".into()))
        .title(title)
        .inner_size(440.0, 640.0)
        .min_inner_size(360.0, 480.0)
        .resizable(true)
        .closable(true)
        .center()
        .focused(true)
        .initialization_script(script)
        .build()
        .map_err(err_str)?;
    Ok(())
}
