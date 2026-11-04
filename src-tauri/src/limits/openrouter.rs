//! OpenRouter 额度:密钥限额和账户余额是两个接口。
//!
//! `GET https://openrouter.ai/api/v1/key` 给出这把 key 的上限和本期用量。
//! `GET https://openrouter.ai/api/v1/credits` 给出账户充值总额和累计消耗。
//! 一边失败时仍展示另一边;两边都失败才算这次查询失败。

use serde_json::Value;

use super::{Balance, ProviderLimits, QuotaWindow};

pub const KEY_URL: &str = "https://openrouter.ai/api/v1/key";
pub const CREDITS_URL: &str = "https://openrouter.ai/api/v1/credits";
pub const CREDENTIAL_NAME: &str = "OPENROUTER_API_KEY";

fn finite(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// 密钥限额。没有正数上限,或缺少本期用量,就不编一个 0%。
pub fn key_window(data: &Value) -> Option<QuotaWindow> {
    let limit = finite(data.get("limit")).filter(|n| *n > 0.0)?;
    let reset = data
        .get("limit_reset")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let usage_field = match reset.as_str() {
        "daily" => "usage_daily",
        "weekly" => "usage_weekly",
        "monthly" => "usage_monthly",
        _ => "usage",
    };
    let remaining = finite(data.get("limit_remaining"));
    let used = if let Some(remaining) = remaining {
        (limit - remaining).max(0.0)
    } else {
        let spent = finite(data.get(usage_field))?;
        let byok = if data.get("include_byok_in_limit").and_then(Value::as_bool) == Some(true) {
            finite(data.get(&format!("byok_{usage_field}")))?
        } else {
            0.0
        };
        (spent.max(0.0) + byok.max(0.0)).min(limit)
    };
    let (key, label) = match reset.as_str() {
        "daily" => ("rolling", "每日限额"),
        "weekly" => ("weekly", "每周限额"),
        "monthly" => ("monthly", "每月限额"),
        _ => ("plan", "密钥限额"),
    };
    Some(
        QuotaWindow::new(key, label).percent(Some((used / limit * 100.0).clamp(0.0, 100.0))),
    )
}

/// 账户余额。两边数字都在才计算剩余,缺一边就留空。
pub fn credits_balance(data: &Value) -> Option<Balance> {
    let total = finite(data.get("total_credits")).filter(|n| *n >= 0.0)?;
    let used = finite(data.get("total_usage")).filter(|n| *n >= 0.0)?;
    Some(Balance {
        amount: (total - used).max(0.0),
        currency: "USD".into(),
        cash: None,
        voucher: None,
    })
}

pub fn parse(key_body: Option<&Value>, credits_body: Option<&Value>) -> ProviderLimits {
    let mut out = ProviderLimits::new("openrouter", "OpenRouter", "openrouter");
    out.configured = true;
    let key_data = key_body.and_then(|v| v.get("data"));
    let credits_data = credits_body.and_then(|v| v.get("data"));
    if let Some(data) = key_data {
        if let Some(window) = key_window(data) {
            out.windows.push(window);
        }
        out.plan_label = match data.get("is_free_tier").and_then(Value::as_bool) {
            Some(true) => Some("Free".into()),
            Some(false) => Some("Pay-as-you-go".into()),
            None => None,
        };
        if data.get("is_management_key").and_then(Value::as_bool) == Some(true) {
            out.plan_label = Some("Management".into());
        }
    }
    if let Some(data) = credits_data {
        out.balance = credits_balance(data);
    }
    if out.windows.is_empty() && out.balance.is_none() {
        out.error = Some("没有查到密钥限额或账户余额".into());
    }
    out
}

pub fn fetch() -> ProviderLimits {
    fetch_with_credential(CREDENTIAL_NAME)
}

pub fn fetch_with_credential(credential_name: &str) -> ProviderLimits {
    let Some(key) = super::credential::resolve(credential_name) else {
        let hint = if credential_name == CREDENTIAL_NAME {
            "未配置 API Key。在设置里填入 OPENROUTER_API_KEY 即可查看限额和余额。"
        } else {
            "未配置 API Key。在设置里为该账号填入密钥即可查看限额和余额。"
        };
        return ProviderLimits::unconfigured("openrouter", "OpenRouter", "openrouter", hint);
    };
    let load = |url: &str| {
        super::http::fetch_json(|agent| {
            agent
                .get(url)
                .set("Authorization", &format!("Bearer {key}"))
                .set("Accept", "application/json")
                .call()
        })
    };
    let key_body = load(KEY_URL).ok();
    let credits_body = load(CREDITS_URL).ok();
    if key_body.is_none() && credits_body.is_none() {
        let mut out = ProviderLimits::new("openrouter", "OpenRouter", "openrouter");
        out.configured = true;
        out.error = Some("OpenRouter 额度查询失败".into());
        return out;
    }
    parse(key_body.as_ref(), credits_body.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weekly_key_limit_uses_remaining_when_present() {
        let window = key_window(&serde_json::json!({
            "limit": 100,
            "limit_reset": "weekly",
            "limit_remaining": 25,
            "usage_weekly": 70
        }))
        .unwrap();
        assert_eq!(window.key, "weekly");
        assert_eq!(window.label, "每周限额");
        assert_eq!(window.used_percent, Some(75.0));
    }

    #[test]
    fn missing_limit_does_not_invent_zero() {
        assert!(key_window(&serde_json::json!({ "usage": 3 })).is_none());
    }

    #[test]
    fn credits_are_remaining_dollars() {
        let balance = credits_balance(&serde_json::json!({
            "total_credits": 20,
            "total_usage": 4.5
        }))
        .unwrap();
        assert!((balance.amount - 15.5).abs() < 1e-9);
        assert_eq!(balance.currency, "USD");
    }

    #[test]
    fn one_side_failing_still_shows_the_other() {
        let got = parse(
            None,
            Some(&serde_json::json!({ "data": { "total_credits": 10, "total_usage": 1 } })),
        );
        assert!(got.error.is_none());
        assert!(got.windows.is_empty());
        assert!(got.balance.is_some());
    }
}
