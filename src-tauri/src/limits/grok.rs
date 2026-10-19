//! Grok 订阅额度:读本地 `grok` CLI 登录态,问 CLI 自己的账单接口。
//!
//! ```text
//! GET https://cli-chat-proxy.grok.com/v1/billing?format=credits
//! GET https://cli-chat-proxy.grok.com/v1/user?include=subscription
//! ```
//!
//! 凭据在 `$GROK_HOME/auth.json`(缺省 `~/.grok/auth.json`),取 `key`。
//! **只读不写**。过期或 401 只提示用户跑一次 `grok`,不刷新、不改这个文件。
//! 没有登录态时不发网络。
//!
//! `creditUsagePercent` 是已用百分比,不是剩余率。缺字段保持不显示,不写成 0%。

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{Balance, ProviderLimits, QuotaWindow};

pub const BILLING_URL: &str = "https://cli-chat-proxy.grok.com/v1/billing?format=credits";
pub const USER_URL: &str = "https://cli-chat-proxy.grok.com/v1/user?include=subscription";

const MISSING_LOGIN: &str = "未找到 Grok 登录态。运行一次 grok 完成登录即可。";
const EXPIRED: &str = "登录态已过期,请运行一次 grok 刷新(OTR 不会改写它的凭据)";
const REJECTED: &str = "登录态失效,请运行一次 grok 刷新(OTR 不会改写它的凭据)";

pub fn auth_path() -> PathBuf {
    crate::paths::grok_home().join("auth.json")
}

pub fn fetch() -> ProviderLimits {
    fetch_from_file(&auth_path())
}

pub fn fetch_from_home(home: &Path) -> ProviderLimits {
    fetch_from_file(&home.join("auth.json"))
}

fn fetch_from_file(path: &Path) -> ProviderLimits {
    match load_access_token(path) {
        TokenLoad::Missing => {
            ProviderLimits::unconfigured("grok", "Grok", "grok", MISSING_LOGIN)
        }
        TokenLoad::Unreadable => {
            let mut out = ProviderLimits::new("grok", "Grok", "grok");
            out.error = Some("登录态文件无法读取。运行一次 grok 重新登录即可。".into());
            out
        }
        TokenLoad::Expired => {
            let mut out = ProviderLimits::new("grok", "Grok", "grok");
            out.configured = true;
            out.error = Some(EXPIRED.into());
            out
        }
        TokenLoad::Ready(token) => fetch_with_token(&token),
    }
}

fn fetch_with_token(token: &str) -> ProviderLimits {
    let billing = super::http::fetch_json(|agent| {
        agent
            .get(BILLING_URL)
            .set("Authorization", &format!("Bearer {token}"))
            .set("Accept", "application/json")
            .set("x-grok-client-mode", "cli")
            .call()
    });
    let billing = match billing {
        Ok(body) => body,
        Err(err) => {
            let mut out = ProviderLimits::new("grok", "Grok", "grok");
            out.configured = true;
            out.error = Some(match err.status() {
                Some(401) | Some(403) => REJECTED.into(),
                Some(404) => "该账号当前没有额度数据".into(),
                _ => err.message(),
            });
            return out;
        }
    };
    let tier = super::http::fetch_json(|agent| {
        agent
            .get(USER_URL)
            .set("Authorization", &format!("Bearer {token}"))
            .set("Accept", "application/json")
            .set("x-grok-client-mode", "cli")
            .call()
    })
    .ok()
    .as_ref()
    .and_then(subscription_tier);
    assemble(&billing, tier)
}

enum TokenLoad {
    Missing,
    Unreadable,
    Expired,
    Ready(String),
}

fn load_access_token(path: &Path) -> TokenLoad {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return TokenLoad::Missing,
        Err(_) => return TokenLoad::Unreadable,
    };
    let value: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(_) => return TokenLoad::Unreadable,
    };
    let Some(token) = select_access_token(&value) else {
        return TokenLoad::Missing;
    };
    if access_token_expired(&token) {
        TokenLoad::Expired
    } else {
        TokenLoad::Ready(token)
    }
}

/// 一个文件里可以有多条登录。取带 `key`、过期时刻最晚的那条。
pub fn select_access_token(root: &Value) -> Option<String> {
    let entries = root.as_object()?;
    let mut best: Option<(i64, String)> = None;
    let mut fallback: Option<String> = None;
    for entry in entries.values() {
        let Some(key) = entry
            .get("key")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|key| !key.is_empty())
        else {
            continue;
        };
        let exp = entry
            .get("expires_at")
            .and_then(Value::as_str)
            .and_then(crate::model::iso_to_ms)
            .or_else(|| jwt_exp_ms(key));
        match exp {
            Some(exp) => {
                let replace = best.as_ref().map(|(prev, _)| exp >= *prev).unwrap_or(true);
                if replace {
                    best = Some((exp, key.to_string()));
                }
            }
            None if fallback.is_none() => fallback = Some(key.to_string()),
            None => {}
        }
    }
    best.map(|(_, key)| key).or(fallback)
}

fn access_token_expired(token: &str) -> bool {
    match jwt_exp_ms(token) {
        Some(exp) => exp <= crate::model::now_ms(),
        None => false,
    }
}

fn jwt_exp_ms(token: &str) -> Option<i64> {
    let payload = token.split('.').nth(1)?;
    let bytes = b64url_decode(payload)?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let exp = value.get("exp")?;
    let raw = exp.as_i64().or_else(|| exp.as_f64().map(|n| n as i64))?;
    if raw <= 0 {
        return None;
    }
    if raw > 1_000_000_000_000 {
        Some(raw)
    } else {
        Some(raw.saturating_mul(1000))
    }
}

fn b64url_decode(input: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    engine.decode(input).ok().or_else(|| {
        let mut padded = input.to_string();
        while padded.len() % 4 != 0 {
            padded.push('=');
        }
        base64::engine::general_purpose::URL_SAFE.decode(padded).ok()
    })
}

pub fn subscription_tier(body: &Value) -> Option<String> {
    body.get("subscriptionTier")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|tier| !tier.is_empty())
        .map(str::to_string)
}

/// 把账单响应收成窗口和余额。`tier` 来自用户接口,没有就空着。
pub fn assemble(billing: &Value, tier: Option<String>) -> ProviderLimits {
    let mut out = ProviderLimits::new("grok", "Grok", "grok");
    out.configured = true;
    out.plan_label = tier.filter(|name| !name.is_empty());
    let config = billing.get("config").unwrap_or(billing);

    let pool = finite_percent(config.get("creditUsagePercent"));
    if let Some(window) = usage_window(config) {
        out.windows.push(window);
    }
    // productUsage 是同一周池按产品拆开,不是第二个额度。
    // 只有 Grok Build 在用时,它的百分比和「每周」是同一个数,再画一格就是重复。
    out.windows.extend(product_windows(
        config.get("productUsage").and_then(Value::as_array),
        pool,
    ));
    if out.windows.iter().all(|window| window.key != "weekly") {
        if let Some(window) = legacy_monthly(config) {
            out.windows.push(window);
        }
    }
    if let Some(window) = on_demand_window(config) {
        out.windows.push(window);
    }
    out.balance = prepaid_balance(config);

    if out.windows.is_empty() && out.balance.is_none() {
        let keys = config
            .as_object()
            .map(|obj| {
                let mut names: Vec<&str> = obj.keys().map(String::as_str).collect();
                names.sort();
                names.join(", ")
            })
            .unwrap_or_default();
        out.error = Some(if keys.is_empty() {
            "响应里没有额度字段".into()
        } else {
            format!("没有认出额度字段: {keys}")
        });
    }
    out
}

fn usage_window(config: &Value) -> Option<QuotaWindow> {
    let used = finite_percent(config.get("creditUsagePercent"))?;
    let period = config.get("currentPeriod").unwrap_or(&Value::Null);
    let kind = period
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let (key, label) = period_labels(kind);
    let start = period
        .get("start")
        .and_then(Value::as_str)
        .and_then(crate::model::iso_to_ms);
    let end = period
        .get("end")
        .and_then(Value::as_str)
        .and_then(crate::model::iso_to_ms);
    let seconds = match (start, end) {
        (Some(start), Some(end)) if end > start => Some((end - start) / 1000),
        _ => None,
    };
    Some(
        QuotaWindow::new(key, label)
            .percent(Some(used))
            .reset(end)
            .window(seconds),
    )
}

fn period_labels(kind: &str) -> (&'static str, &'static str) {
    let kind = kind
        .trim()
        .trim_start_matches("USAGE_PERIOD_TYPE_")
        .to_ascii_uppercase();
    if kind.contains("WEEK") {
        ("weekly", "每周")
    } else if kind.contains("MONTH") {
        ("monthly", "每月")
    } else if kind.contains("DAY") {
        ("daily", "每日")
    } else {
        ("period", "当前周期")
    }
}

fn product_windows(items: Option<&Vec<Value>>, pool: Option<f64>) -> Vec<QuotaWindow> {
    let mut rows: Vec<QuotaWindow> = items
        .into_iter()
        .flatten()
        .filter_map(product_window)
        .collect();
    let nonzero: Vec<&QuotaWindow> = rows
        .iter()
        .filter(|window| window.used_percent.unwrap_or(0.0) > 0.0)
        .collect();
    let splits_the_pool = nonzero.len() >= 2
        && nonzero.iter().any(|window| !same_percent(window.used_percent, pool));
    if !splits_the_pool {
        return Vec::new();
    }
    rows.retain(|window| window.used_percent.unwrap_or(0.0) > 0.0);
    rows
}

fn same_percent(left: Option<f64>, right: Option<f64>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => (left - right).abs() < 0.5,
        _ => false,
    }
}

fn product_window(item: &Value) -> Option<QuotaWindow> {
    let used = finite_percent(item.get("usagePercent"))?;
    let name = item.get("product").and_then(Value::as_str).unwrap_or("").trim();
    if name.is_empty() {
        return None;
    }
    let key = format!("product_{}", product_key(name));
    Some(QuotaWindow::new(&key, &product_label(name)).percent(Some(used)))
}

fn product_key(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('_') {
            out.push('_');
        }
    }
    out.trim_matches('_').to_string()
}

fn product_label(name: &str) -> String {
    match name {
        "Api" | "API" => "API".into(),
        "GrokBuild" => "Grok Build".into(),
        "Chat" => "Chat".into(),
        "Imagine" => "Imagine".into(),
        "Voice" => "Voice".into(),
        other => other.to_string(),
    }
}

/// 没有周池百分比时,旧字段 `monthlyLimit` / `used` 的单位是美分。
fn legacy_monthly(config: &Value) -> Option<QuotaWindow> {
    if config.get("creditUsagePercent").is_some() {
        return None;
    }
    let limit = cent_val(config.get("monthlyLimit"))?;
    let used = cent_val(config.get("used"))?;
    if limit <= 0.0 {
        return None;
    }
    let reset = config
        .get("billingPeriodEnd")
        .and_then(Value::as_str)
        .and_then(crate::model::iso_to_ms);
    Some(
        QuotaWindow::new("monthly", "每月")
            .percent(Some((used / limit * 100.0).clamp(0.0, 100.0)))
            .reset(reset),
    )
}

fn on_demand_window(config: &Value) -> Option<QuotaWindow> {
    let cap = cent_val(config.get("onDemandCap"))?;
    let used = cent_val(config.get("onDemandUsed"))?;
    if cap <= 0.0 {
        return None;
    }
    Some(
        QuotaWindow::new("credit", "按量")
            .percent(Some((used / cap * 100.0).clamp(0.0, 100.0))),
    )
}

fn prepaid_balance(config: &Value) -> Option<Balance> {
    let cents = cent_val(config.get("prepaidBalance"))?;
    if cents <= 0.0 {
        return None;
    }
    Some(Balance {
        amount: cents / 100.0,
        currency: "USD".into(),
        cash: None,
        voucher: None,
    })
}

fn finite_percent(value: Option<&Value>) -> Option<f64> {
    let number = as_f64(value)?;
    if !number.is_finite() {
        return None;
    }
    Some(number.clamp(0.0, 100.0))
}

/// `{ "val": 150 }` 或裸数字,单位是美分。缺字段返回 None,不把缺当成 0。
fn cent_val(value: Option<&Value>) -> Option<f64> {
    let value = value?;
    if value.is_null() {
        return None;
    }
    let raw = value.get("val").unwrap_or(value);
    as_f64(Some(raw)).filter(|number| number.is_finite())
}

fn as_f64(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jwt(exp: i64) -> String {
        use base64::Engine as _;
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let head = engine.encode(b"{}");
        let body = engine.encode(format!(r#"{{"exp":{exp}}}"#));
        format!("{head}.{body}.sig")
    }

    #[test]
    fn weekly_pool_products_and_prepaid_are_split() {
        let billing = serde_json::json!({
            "config": {
                "creditUsagePercent": 11.2,
                "currentPeriod": {
                    "type": "WEEKLY",
                    "start": "2026-07-17T06:34:33.775Z",
                    "end": "2026-07-24T06:34:33.775Z"
                },
                "productUsage": [
                    { "product": "Api", "usagePercent": 11 },
                    { "product": "GrokBuild", "usagePercent": 0 },
                    { "product": "Chat" }
                ],
                "onDemandCap": { "val": 0 },
                "onDemandUsed": { "val": 0 },
                "prepaidBalance": { "val": 500 },
                "monthlyLimit": { "val": 15000 },
                "used": { "val": 4839 }
            }
        });
        let got = assemble(&billing, Some("SuperGrok".into()));
        assert_eq!(got.plan_label.as_deref(), Some("SuperGrok"));
        assert!(got.error.is_none(), "{:?}", got.error);
        let weekly = got.windows.iter().find(|w| w.key == "weekly").unwrap();
        assert_eq!(weekly.label, "每周");
        assert_eq!(weekly.used_percent, Some(11.2));
        assert_eq!(weekly.window_seconds, Some(7 * 24 * 3600));
        assert!(weekly.reset_at.is_some());
        assert!(
            got.windows.iter().all(|w| !w.key.starts_with("product_")),
            "只有一个产品和周池相同,不再单独画一格"
        );
        assert!(
            got.windows.iter().all(|w| w.key != "monthly"),
            "已经有周池时不再用旧的月额度造第二行"
        );
        assert!(got.windows.iter().all(|w| w.key != "credit"));
        assert_eq!(got.balance.as_ref().map(|b| (b.amount, b.currency.as_str())), Some((5.0, "USD")));
    }

    #[test]
    fn product_split_stays_when_two_products_share_the_pool() {
        let billing = serde_json::json!({
            "config": {
                "creditUsagePercent": 20,
                "currentPeriod": { "type": "WEEKLY" },
                "productUsage": [
                    { "product": "Api", "usagePercent": 12 },
                    { "product": "GrokBuild", "usagePercent": 8 },
                    { "product": "Chat", "usagePercent": 0 }
                ]
            }
        });
        let got = assemble(&billing, None);
        assert!(got.windows.iter().any(|w| w.key == "weekly"));
        assert!(got.windows.iter().any(|w| w.key == "product_api" && w.used_percent == Some(12.0)));
        assert!(got.windows.iter().any(|w| w.key == "product_grokbuild" && w.used_percent == Some(8.0)));
        assert!(got.windows.iter().all(|w| w.key != "product_chat"));
    }

    #[test]
    fn missing_percent_is_not_zero_and_legacy_monthly_is_cents() {
        let billing = serde_json::json!({
            "config": {
                "monthlyLimit": { "val": 15000 },
                "used": { "val": 4839 },
                "billingPeriodEnd": "2026-08-01T00:00:00Z"
            }
        });
        let got = assemble(&billing, None);
        assert_eq!(got.windows.len(), 1);
        assert_eq!(got.windows[0].key, "monthly");
        assert!((got.windows[0].used_percent.unwrap() - 32.26).abs() < 0.01);
        assert!(got.balance.is_none());
        assert!(got.error.is_none());
    }

    #[test]
    fn unrecognized_body_names_the_keys() {
        let got = assemble(&serde_json::json!({"config": {"foo": 1}}), None);
        let err = got.error.unwrap();
        assert!(err.contains("foo"), "{err}");
        assert!(got.windows.is_empty());
    }

    #[test]
    fn latest_expiring_entry_is_chosen() {
        let early = jwt(1_700_000_000);
        let late = jwt(1_800_000_000);
        let root = serde_json::json!({
            "a": { "key": early, "expires_at": "2024-01-01T00:00:00Z" },
            "b": { "key": late, "expires_at": "2027-01-01T00:00:00Z" }
        });
        assert_eq!(select_access_token(&root).as_deref(), Some(late.as_str()));
    }

    #[test]
    fn expired_jwt_is_detected_locally() {
        let expired = jwt(1_000_000_000);
        assert!(access_token_expired(&expired));
        let fresh = jwt(4_000_000_000);
        assert!(!access_token_expired(&fresh));
    }
}
