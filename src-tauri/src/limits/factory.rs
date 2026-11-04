//! Factory Droid 额度。`GET https://api.factory.ai/api/billing/limits`。
//!
//! 标准池的 5 小时、每周、每月各自带 `usedPercent`。附加 Core 池只有真有用量才显示。
//! `extraUsageBalanceCents` 是美元余额,单独放余额,不折进百分比。

use serde_json::Value;

use super::{Balance, ProviderLimits, QuotaWindow};

pub const CREDENTIAL_NAME: &str = "FACTORY_API_KEY";
const LIMITS_URL: &str = "https://api.factory.ai/api/billing/limits";

fn num(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) if !s.trim().is_empty() => s.trim().parse().ok(),
        _ => None,
    }
}

fn one(pool: &Value, field: &str, key: &str, label: &str, seconds: Option<i64>) -> Option<QuotaWindow> {
    let pct = num(pool.pointer(&format!("/{field}/usedPercent")))?;
    Some(
        QuotaWindow::new(key, label)
            .percent(Some(pct.clamp(0.0, 100.0)))
            .window(seconds),
    )
}

fn pool_has_signal(pool: &Value) -> bool {
    ["fiveHour", "weekly", "monthly"].iter().any(|field| {
        let entry = pool.get(*field);
        num(entry.and_then(|v| v.get("usedPercent"))).is_some_and(|n| n > 0.0)
            || entry.and_then(|v| v.get("windowEnd")).is_some()
            || entry.and_then(|v| v.get("secondsRemaining")).is_some()
    })
}

pub fn parse(body: &Value) -> ProviderLimits {
    let mut out = ProviderLimits::new("factory", "Factory", "factory");
    out.configured = true;
    if body.get("usesTokenRateLimitsBilling").and_then(Value::as_bool) != Some(true) {
        out.error = Some("没有查到 Token 限额".into());
        return out;
    }
    let Some(standard) = body.pointer("/limits/standard") else {
        out.error = Some("没有查到 Token 限额".into());
        return out;
    };
    for (field, key, label, seconds) in [
        ("fiveHour", "five_hour", "5 小时", Some(5 * 60 * 60)),
        ("weekly", "weekly", "每周", Some(7 * 24 * 60 * 60)),
        ("monthly", "monthly", "每月", None),
    ] {
        if let Some(window) = one(standard, field, key, label, seconds) {
            out.windows.push(window);
        }
    }
    if let Some(core) = body.pointer("/limits/core").filter(|pool| pool_has_signal(pool)) {
        for (field, key, label, seconds) in [
            ("fiveHour", "five_hour_core", "Core 5 小时", Some(5 * 60 * 60)),
            ("weekly", "weekly_core", "Core 每周", Some(7 * 24 * 60 * 60)),
            ("monthly", "monthly_core", "Core 每月", None),
        ] {
            if let Some(window) = one(core, field, key, label, seconds) {
                out.windows.push(window);
            }
        }
    }
    if let Some(cents) = num(body.get("extraUsageBalanceCents")) {
        out.balance = Some(Balance {
            amount: (cents / 100.0).max(0.0),
            currency: "USD".into(),
            cash: None,
            voucher: None,
        });
    }
    if out.windows.is_empty() && out.balance.is_none() {
        out.error = Some("没有查到 Token 限额".into());
    } else {
        out.error = None;
    }
    out
}

pub fn fetch_with_credential(credential_name: &str) -> ProviderLimits {
    let Some(key) = super::credential::resolve(credential_name) else {
        return ProviderLimits::unconfigured(
            "factory",
            "Factory",
            "factory",
            "未配置 API Key。在设置里填入 FACTORY_API_KEY。",
        );
    };
    match super::http::fetch_json(|agent| {
        agent
            .get(LIMITS_URL)
            .set("Authorization", &format!("Bearer {key}"))
            .set("Accept", "application/json")
            .call()
    }) {
        Ok(body) => parse(&body),
        Err(e) => {
            let mut out = ProviderLimits::new("factory", "Factory", "factory");
            out.configured = true;
            out.error = Some(match e.status() {
                Some(401) | Some(403) => "API Key 无效".into(),
                _ => "Factory 额度查询失败".into(),
            });
            out
        }
    }
}

pub fn fetch() -> ProviderLimits {
    fetch_with_credential(CREDENTIAL_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_pools_keep_their_percent_and_balance_is_dollars() {
        let got = parse(&serde_json::json!({
            "usesTokenRateLimitsBilling": true,
            "limits": {
                "standard": {
                    "fiveHour": { "usedPercent": 12.5 },
                    "weekly": { "usedPercent": 40 }
                },
                "core": { "fiveHour": { "usedPercent": 0 } }
            },
            "extraUsageBalanceCents": 250
        }));
        assert!(got.error.is_none());
        assert_eq!(got.windows.len(), 2);
        assert_eq!(got.windows[0].used_percent, Some(12.5));
        assert!((got.balance.unwrap().amount - 2.5).abs() < 1e-9);
    }
}
