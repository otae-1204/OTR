//! Cursor 订阅额度:`GET https://cursor.com/api/usage-summary`
//!
//! 认证沿用本地会话:从 `state.vscdb` 里取 `cursorAuth/accessToken`(JWT),
//! 配合 `cursorAuth/cachedSignUpType` 或 JWT 的 `sub` 拼出
//! `WorkosCursorSessionToken=<userId>::<jwt>` 这个 cookie。
//! 实测 Cursor 不校验 User-Agent,所以不伪造 UA。

use serde_json::Value;

use super::{ProviderLimits, QuotaWindow};

pub const SUMMARY_URL: &str = "https://cursor.com/api/usage-summary";

/// 从 state.vscdb 取一个 key 的文本值(只读,且用只读方式打开)
pub fn state_db_value(db: &std::path::Path, key: &str) -> Option<String> {
    if !db.is_file() {
        return None;
    }
    // 只读打开:Cursor 可能正开着这个库,不加 mode=ro 会抢写锁
    let conn = rusqlite::Connection::open_with_flags(
        db,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    conn.query_row(
        "SELECT value FROM ItemTable WHERE key = ?1",
        rusqlite::params![key],
        |row| row.get::<_, String>(0),
    )
    .ok()
    .filter(|s| !s.trim().is_empty())
}

/// 本地 Cursor 的会话 cookie(不需要用户手填)
pub fn local_cookie() -> Option<String> {
    cookie_from_user_dir(&crate::paths::cursor_user_dir())
}

/// 指定 profile 目录的会话 cookie(多账号:每个账号一份 user-data-dir)
pub fn cookie_from_user_dir(user_dir: &std::path::Path) -> Option<String> {
    let db = user_dir.join("User").join("globalStorage").join("state.vscdb");
    let jwt = state_db_value(&db, "cursorAuth/accessToken")?;
    let user = user_id_from_cli_or_jwt(user_dir, &jwt).or_else(|| user_id_from_jwt(&jwt))?;
    Some(format!("WorkosCursorSessionToken={user}%3A%3A{jwt}"))
}

/// cli-config.json 里的 authId 优先(原生账号是 "auth0|user_xxx",要剥掉前缀)
fn user_id_from_cli_or_jwt(user_dir: &std::path::Path, jwt: &str) -> Option<String> {
    let cfg = user_dir.join("cli-config.json");
    let text = std::fs::read_to_string(cfg).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let raw = v
        .get("authInfo")
        .and_then(|a| a.get("authId"))
        .and_then(Value::as_str)?;
    normalize_subject(raw).or_else(|| user_id_from_jwt(jwt))
}

fn user_id_from_jwt(jwt: &str) -> Option<String> {
    let payload = jwt.split('.').nth(1)?;
    let decoded = base64::Engine::decode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        payload,
    )
    .ok()?;
    let v: Value = serde_json::from_slice(&decoded).ok()?;
    normalize_subject(v.get("sub").and_then(Value::as_str)?)
}

/// 原生 Cursor 的 `sub` 是 "auth0|user_XXXX",cookie 里只要 "user_XXXX";
/// WorkOS 桥接过的 OAuth 则要保留整个 "<provider>|<id>"。
fn normalize_subject(subject: &str) -> Option<String> {
    if let Some(idx) = subject.rfind("|user_") {
        return Some(subject[idx + 1..].to_string());
    }
    const OAUTH_PROVIDERS: &[&str] = &["google-oauth2", "github", "oidc", "auth0"];
    let (provider, rest) = subject.split_once('|')?;
    if OAUTH_PROVIDERS.contains(&provider) && !rest.is_empty() {
        return Some(subject.to_string());
    }
    None
}

fn clamp_percent(v: f64) -> Option<f64> {
    if !v.is_finite() {
        return None;
    }
    Some(v.clamp(0.0, 100.0))
}

fn as_f64(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// `used/limit` 兜底。实测这两个字段是"价格/额度上限"语义:
/// 本机 used=2000 limit=2000 会算出 100%,而真实用量是 17.45%。
/// 所以它只能排在所有百分比字段之后,绝不能当首选。
fn percent_from_used_limit(used: Option<f64>, limit: Option<f64>) -> Option<f64> {
    let (used, limit) = (used?, limit?);
    if limit <= 0.0 {
        return None;
    }
    clamp_percent(used / limit * 100.0)
}

/// 解析 usage-summary。**优先级阶梯写死**(见模块注释规则 1)。
pub fn parse_summary(body: &Value) -> ProviderLimits {
    let mut out = ProviderLimits::new("cursor", "Cursor", "cursor");
    out.configured = true;

    let plan = body.pointer("/individualUsage/plan");
    let auto = as_f64(plan.and_then(|p| p.get("autoPercentUsed")));
    let api = as_f64(plan.and_then(|p| p.get("apiPercentUsed")));
    let total = as_f64(plan.and_then(|p| p.get("totalPercentUsed")));

    // 阶梯:total → (auto+api)/2 → api → auto → used/limit
    let percent = total
        .and_then(clamp_percent)
        .or_else(|| match (auto, api) {
            (Some(a), Some(b)) => clamp_percent((a + b) / 2.0),
            _ => None,
        })
        .or_else(|| api.and_then(clamp_percent))
        .or_else(|| auto.and_then(clamp_percent))
        .or_else(|| {
            percent_from_used_limit(
                as_f64(plan.and_then(|p| p.get("used"))),
                as_f64(plan.and_then(|p| p.get("limit"))),
            )
        });

    out.plan_label = body
        .get("planName")
        .or_else(|| body.get("membershipType"))
        .and_then(Value::as_str)
        .map(str::to_string);

    let reset_at = body
        .get("billingCycleEnd")
        .and_then(Value::as_str)
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.timestamp_millis());
    let window_seconds = match (
        body.get("billingCycleStart").and_then(Value::as_str),
        body.get("billingCycleEnd").and_then(Value::as_str),
    ) {
        (Some(a), Some(b)) => match (
            chrono::DateTime::parse_from_rfc3339(a).ok(),
            chrono::DateTime::parse_from_rfc3339(b).ok(),
        ) {
            (Some(s), Some(e)) if e > s => Some((e - s).num_seconds()),
            _ => None,
        },
        _ => None,
    };

    let mut w = QuotaWindow::new("plan", "套餐额度")
        .percent(percent)
        .reset(reset_at)
        .window(window_seconds);
    // 阶梯里用到的分道百分比原样带上,便于核对口径
    if let Some(a) = auto.and_then(clamp_percent) {
        out.windows.push(QuotaWindow::new("auto", "Auto 通道").percent(Some(a)));
    }
    if let Some(b) = api.and_then(clamp_percent) {
        out.windows.push(QuotaWindow::new("api", "API 通道").percent(Some(b)));
    }
    w.used_percent = percent;
    out.windows.insert(0, w);
    out
}

pub fn fetch() -> ProviderLimits {
    fetch_with_cookie_opt(local_cookie())
}

/// 多账号:从指定的 user-data-dir 取会话
pub fn fetch_from_home(home: &std::path::Path) -> ProviderLimits {
    fetch_with_cookie_opt(cookie_from_user_dir(home))
}

fn fetch_with_cookie_opt(cookie: Option<String>) -> ProviderLimits {
    let Some(cookie) = cookie else {
        return ProviderLimits::unconfigured(
            "cursor",
            "Cursor",
            "cursor",
            "未找到 Cursor 登录态。打开一次 Cursor 并登录即可。",
        );
    };
    // 认系统代理:实测 chatgpt.com 直连必超时,而 ureq 不像 curl 那样自动读系统代理
    match super::http::fetch_json(|agent| {
        agent
            .get(SUMMARY_URL)
            .set("Cookie", &cookie)
            .set("Accept", "application/json")
            .call()
    }) {
        Ok(v) => parse_summary(&v),
        Err(e) => {
            let mut out = ProviderLimits::new("cursor", "Cursor", "cursor");
            out.configured = true;
            out.error = Some(match e.status() {
                Some(401) | Some(403) => "登录态失效,请打开 Cursor 重新登录".to_string(),
                Some(_) => e.message(),
                None => e.message(),
            });
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 规则 1:实测 used=2000/limit=2000 会算出 100%,必须优先采信 totalPercentUsed。
    #[test]
    fn total_percent_wins_over_the_used_limit_trap() {
        let body = serde_json::json!({
            "individualUsage": { "plan": {
                "used": 2000, "limit": 2000, "bonus": 6639,
                "totalPercentUsed": 17.4525
            }},
            "billingCycleStart": "2026-09-01T00:00:00Z",
            "billingCycleEnd": "2026-10-01T00:00:00Z",
        });
        let got = parse_summary(&body);
        let p = got.windows[0].used_percent.expect("必须有百分比");
        assert!((p - 17.4525).abs() < 1e-9, "实测值应当是 17.45,不是 100: got {p}");
        assert_eq!(got.windows[0].window_seconds, Some(30 * 86400));
    }

    /// 没有 totalPercentUsed 时按 auto/api 两道平均
    #[test]
    fn falls_back_to_the_average_of_auto_and_api_lanes() {
        let body = serde_json::json!({
            "individualUsage": { "plan": { "autoPercentUsed": 10.0, "apiPercentUsed": 20.0 } }
        });
        let got = parse_summary(&body);
        assert!((got.windows[0].used_percent.unwrap() - 15.0).abs() < 1e-9);
    }

    /// 只有单道时用那一道
    #[test]
    fn single_lane_is_used_when_the_other_is_absent() {
        let body = serde_json::json!({
            "individualUsage": { "plan": { "apiPercentUsed": 33.0 } }
        });
        let got = parse_summary(&body);
        assert_eq!(got.windows[0].used_percent, Some(33.0));
    }

    /// 什么百分比都没有时才轮到 used/limit
    #[test]
    fn used_over_limit_is_the_last_resort() {
        let body = serde_json::json!({
            "individualUsage": { "plan": { "used": 250, "limit": 1000 } }
        });
        let got = parse_summary(&body);
        assert_eq!(got.windows[0].used_percent, Some(25.0));
    }

    /// 规则 3:字段全缺 → None(UI 显示 --),绝不臆造 0
    #[test]
    fn missing_fields_stay_none_rather_than_zero() {
        let got = parse_summary(&serde_json::json!({}));
        assert_eq!(got.windows[0].used_percent, None, "缺字段必须是 None,不能是 0");
        assert_eq!(got.windows[0].reset_at, None);
        assert_eq!(got.windows[0].window_seconds, None);
        assert!(got.balance.is_none());
    }

    /// limit 为 0 时不能除出 inf/NaN
    #[test]
    fn zero_limit_does_not_produce_infinity() {
        let body = serde_json::json!({
            "individualUsage": { "plan": { "used": 5, "limit": 0 } }
        });
        assert_eq!(parse_summary(&body).windows[0].used_percent, None);
    }

    /// JWT subject 归一化:原生账号剥前缀,WorkOS 桥接保留
    #[test]
    fn subject_normalization_matches_the_cookie_format() {
        assert_eq!(
            normalize_subject("auth0|user_ABC123").as_deref(),
            Some("user_ABC123")
        );
        assert_eq!(
            normalize_subject("google-oauth2|1098765").as_deref(),
            Some("google-oauth2|1098765")
        );
        assert_eq!(normalize_subject("garbage"), None);
    }
}
