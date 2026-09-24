//! Codex 订阅额度:`GET https://chatgpt.com/backend-api/wham/usage`
//!
//! 凭据读本地 `$CODEX_HOME/auth.json`(默认 `~/.codex/auth.json`),
//! 取 `tokens.access_token` 与 `tokens.account_id`。
//!
//! **OTR 只读不写**:登录态失效时提示用户去跑一次 `codex`,绝不写回
//! `auth.json` —— 那是别人(Codex CLI)的文件,两个程序同时改它会互相踩。

use serde_json::Value;

use super::{ProviderLimits, QuotaWindow};

pub const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

/// 5 小时会话窗口
pub const SESSION_WINDOW_SECONDS: i64 = 18_000;
/// 7 天周窗口
pub const WEEKLY_WINDOW_SECONDS: i64 = 604_800;
/// 30 天月窗口(免费档实测值)
pub const MONTHLY_WINDOW_SECONDS: i64 = 2_592_000;

/// 按 `limit_window_seconds` 给窗口分类。
///
/// **不能按槽位名判**:免费档只发一个窗口且落在 `primary_window` 槽里,
/// 按位置读会把它当成"5 小时",而实测它是 2592000(30 天)。
/// 未知秒数不猜,返回 None 由调用方兜底并保留原始值。
pub fn classify_window(seconds: i64) -> Option<(&'static str, &'static str)> {
    match seconds {
        SESSION_WINDOW_SECONDS => Some(("five_hour", "5 小时")),
        WEEKLY_WINDOW_SECONDS => Some(("weekly", "每周")),
        MONTHLY_WINDOW_SECONDS => Some(("monthly", "每月")),
        _ => None,
    }
}

fn as_f64(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn as_i64(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// 解析一个 wham 窗口。缺 `used_percent` 就返回 None —— 没有数字的窗口没有意义。
fn parse_window(raw: &Value, fallback_index: usize) -> Option<QuotaWindow> {
    let used = as_f64(raw.get("used_percent"))?;
    let seconds = as_i64(raw.get("limit_window_seconds"));
    let (key, label) = match seconds.and_then(classify_window) {
        Some((k, l)) => (k.to_string(), l.to_string()),
        // 未知窗口长度:按位置兜底,但**保留原始秒数**不丢数据
        None => (
            if fallback_index == 0 { "primary".into() } else { "secondary".into() },
            if fallback_index == 0 { "主窗口".into() } else { "次窗口".into() },
        ),
    };
    let reset_at = as_i64(raw.get("reset_at")).map(|s| {
        // 上游给的是 epoch **秒**;也有实现给毫秒,按量级区分
        if s > 1_000_000_000_000 { s } else { s * 1000 }
    });
    Some(
        QuotaWindow::new(&key, &label)
            .percent(Some(used.clamp(0.0, 100.0)))
            .reset(reset_at)
            .window(seconds),
    )
}

/// 解析 /wham/usage 响应体
pub fn parse_usage(body: &Value) -> ProviderLimits {
    let mut out = ProviderLimits::new("codex", "Codex CLI", "codex");
    out.configured = true;
    out.plan_label = body
        .get("plan_type")
        .and_then(Value::as_str)
        .map(str::to_string);

    let rl = body.get("rate_limit").unwrap_or(&Value::Null);
    let mut windows = Vec::new();
    for (i, slot) in ["primary_window", "secondary_window"].iter().enumerate() {
        if let Some(w) = rl.get(*slot).filter(|v| !v.is_null()) {
            if let Some(parsed) = parse_window(w, i) {
                windows.push(parsed);
            }
        }
    }
    out.windows = windows;

    // 额度用尽的账号会带 credit 窗口
    if let Some(ind) = body.pointer("/spend_control/individual_limit") {
        if let Some(limit) = as_f64(ind.get("limit")) {
            let used = as_f64(ind.get("used")).unwrap_or(0.0);
            if limit > 0.0 {
                out.windows.push(
                    QuotaWindow::new("credit", "消费额度")
                        .percent(Some((used / limit * 100.0).clamp(0.0, 100.0))),
                );
            }
        }
    }
    out
}

/// 本地 Codex 登录态路径($CODEX_HOME 优先)
pub fn auth_path() -> std::path::PathBuf {
    std::env::var_os("CODEX_HOME")
        .filter(|v| !v.to_string_lossy().trim().is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| crate::paths::home().join(".codex"))
        .join("auth.json")
}

/// 只读取 access_token 与 account_id,**不写回**
pub fn read_auth() -> Option<(String, Option<String>)> {
    read_auth_from(&auth_path())
}

/// 多账号:从指定的 CODEX_HOME 目录读
pub fn read_auth_from(path: &std::path::Path) -> Option<(String, Option<String>)> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let tokens = v.get("tokens")?;
    let access = tokens
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())?
        .to_string();
    let account = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .map(str::to_string);
    Some((access, account))
}

pub fn fetch() -> ProviderLimits {
    fetch_with_auth(read_auth())
}

/// 多账号:从指定的 profile 目录读 auth.json
pub fn fetch_from_home(home: &std::path::Path) -> ProviderLimits {
    fetch_with_auth(read_auth_from(&home.join("auth.json")))
}

fn fetch_with_auth(auth: Option<(String, Option<String>)>) -> ProviderLimits {
    let Some((token, account_id)) = auth else {
        return ProviderLimits::unconfigured(
            "codex",
            "Codex CLI",
            "codex",
            "未找到 Codex 登录态。运行一次 codex 完成登录即可。",
        );
    };
    // 认系统代理:**chatgpt.com 直连必超时**,这是 Codex 拉不到额度的真正原因
    let result = super::http::fetch_json(|agent| {
        let mut req = agent
            .get(USAGE_URL)
            .set("Authorization", &format!("Bearer {token}"))
            .set("Accept", "application/json");
        // 部分档位不带 account id 会直接 4xx
        if let Some(id) = &account_id {
            req = req.set("ChatGPT-Account-Id", id);
        }
        req.call()
    });
    match result {
        Ok(v) => parse_usage(&v),
        Err(e) => {
            let mut out = ProviderLimits::new("codex", "Codex CLI", "codex");
            out.configured = true;
            out.error = Some(match e.status() {
                Some(401) | Some(403) => {
                    "登录态失效,请运行一次 codex 刷新(OTR 不会改写它的凭据)".to_string()
                }
                Some(404) => "该账号当前没有额度数据".to_string(),
                _ => e.message(),
            });
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 规则 2:免费档的 30 天窗口落在 primary 槽里,必须按秒数认出"每月"
    #[test]
    fn thirty_day_window_is_recognised_not_mislabelled_as_five_hour() {
        let body = serde_json::json!({
            "plan_type": "free",
            "rate_limit": {
                "primary_window": {
                    "used_percent": 12.5,
                    "limit_window_seconds": 2592000,
                    "reset_at": 1790000000
                }
            }
        });
        let got = parse_usage(&body);
        assert_eq!(got.windows.len(), 1);
        assert_eq!(got.windows[0].key, "monthly", "2592000 秒是每月,不是 5 小时");
        assert_eq!(got.windows[0].label, "每月");
        assert_eq!(got.windows[0].window_seconds, Some(2_592_000));
        assert_eq!(got.plan_label.as_deref(), Some("free"));
    }

    /// 5 小时与 7 天各自归位(顺序无关)
    #[test]
    fn session_and_weekly_are_classified_regardless_of_slot() {
        let body = serde_json::json!({
            "rate_limit": {
                "primary_window": { "used_percent": 40.0, "limit_window_seconds": 604800 },
                "secondary_window": { "used_percent": 80.0, "limit_window_seconds": 18000 }
            }
        });
        let got = parse_usage(&body);
        let by_key: std::collections::HashMap<_, _> =
            got.windows.iter().map(|w| (w.key.as_str(), w.used_percent)).collect();
        assert_eq!(by_key["weekly"], Some(40.0), "槽位名与窗口长度不一致时以长度为准");
        assert_eq!(by_key["five_hour"], Some(80.0));
    }

    /// 未知窗口长度:按位置兜底,但原始秒数必须保留,不能丢
    #[test]
    fn unknown_window_length_keeps_the_raw_seconds() {
        let body = serde_json::json!({
            "rate_limit": { "primary_window": { "used_percent": 5.0, "limit_window_seconds": 12345 } }
        });
        let got = parse_usage(&body);
        assert_eq!(got.windows[0].window_seconds, Some(12345), "原始值不能丢");
        assert_eq!(got.windows[0].used_percent, Some(5.0));
    }

    /// 规则 3:没有 used_percent 的窗口不进列表,缺字段就是 None
    #[test]
    fn window_without_a_percent_is_dropped_not_zeroed() {
        let body = serde_json::json!({
            "rate_limit": { "primary_window": { "limit_window_seconds": 18000 } }
        });
        assert!(parse_usage(&body).windows.is_empty());
    }

    /// 重置时间:epoch 秒与毫秒都能认
    #[test]
    fn reset_timestamps_accept_seconds_and_milliseconds() {
        let secs = serde_json::json!({
            "rate_limit": { "primary_window": { "used_percent": 1.0, "limit_window_seconds": 18000, "reset_at": 1790000000 } }
        });
        assert_eq!(parse_usage(&secs).windows[0].reset_at, Some(1_790_000_000_000));
        let ms = serde_json::json!({
            "rate_limit": { "primary_window": { "used_percent": 1.0, "limit_window_seconds": 18000, "reset_at": 1790000000000i64 } }
        });
        assert_eq!(parse_usage(&ms).windows[0].reset_at, Some(1_790_000_000_000));
    }

    /// 空响应不该造出任何窗口
    #[test]
    fn empty_body_yields_no_windows() {
        let got = parse_usage(&serde_json::json!({}));
        assert!(got.windows.is_empty());
        assert_eq!(got.plan_label, None);
    }

    /// 规则 4:auth.json 只读 —— 解析函数不返回任何可写路径,
    /// 且 read_auth 在文件缺失时安静地返回 None(由上层给引导文案)
    #[test]
    fn missing_auth_file_is_not_an_error() {
        // 用一个必然不存在的 CODEX_HOME,确认读不到就是 None,不会 panic
        std::env::set_var("CODEX_HOME", "C:/__otr_nonexistent_codex_home__");
        assert!(read_auth().is_none());
        assert!(auth_path().ends_with("auth.json"));
        std::env::remove_var("CODEX_HOME");
    }

    /// **登录态获取不到时必须立刻返回,一次网络都不发。**
    ///
    /// 这是"Codex 没登录 → App 未响应"那条报障的边界:没有凭据就没有可发的请求,
    /// 此时若还要去连 chatgpt.com,失败会一路走到连接超时(ureq 默认 30 秒),
    /// 而调用方一旦在 UI 线程上等它,整个窗口就冻住了。
    ///
    /// 这里用一个不存在的 profile 目录,断言:返回 unconfigured、带引导文案、
    /// 且耗时远小于任何网络超时。
    #[test]
    fn missing_login_returns_immediately_without_touching_the_network() {
        let missing = std::path::Path::new("C:/__otr_no_such_codex_profile__");
        let started = std::time::Instant::now();
        let got = fetch_from_home(missing);
        let elapsed = started.elapsed();

        assert!(!got.configured, "没有登录态就该是未配置,而不是假装已配置");
        assert!(
            got.error.as_deref().unwrap_or("").contains("登录"),
            "要给一句能照做的引导,实际: {:?}",
            got.error
        );
        assert!(got.windows.is_empty(), "没登录就不该编出任何窗口数字");
        assert_eq!(got.balance, None);
        // 只要不发网络,这一步是纯文件读取,毫秒级;1 秒是极宽松的上界
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "无登录态竟然花了 {elapsed:?} —— 说明它去发网络请求了"
        );
    }
}
