//! 智谱 / Z.ai Coding Plan。
//!
//! `GET /api/monitor/usage/quota/limit`,Bearer 密钥。国内域名和国际域名都试。
//! `TOKENS_LIMIT` 和 `CREDIT_LIMIT` 字段一样:短窗口是 5 小时,长窗口是每周。
//! `TIME_LIMIT` 是 MCP 月度桶;它的 unit/number 会写成 1 分钟,那个长度丢掉。

use serde_json::Value;

use super::{ProviderLimits, QuotaWindow};

pub const CREDENTIAL_NAME: &str = "ZAI_API_KEY";

const PATH: &str = "/api/monitor/usage/quota/limit";
const BASES: &[&str] = &["https://open.bigmodel.cn", "https://api.z.ai"];

fn num(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) if !s.trim().is_empty() => s.trim().parse().ok(),
        _ => None,
    }
}

fn used_percent(limit: &Value) -> Option<f64> {
    let total = num(limit.get("usage"));
    let remaining = num(limit.get("remaining"));
    let current = num(limit.get("currentValue")).or_else(|| num(limit.get("current_value")));
    if let Some(total) = total.filter(|n| *n > 0.0) {
        let used = if let Some(remaining) = remaining {
            let from_remaining = total - remaining;
            current.map(|c| from_remaining.max(c)).unwrap_or(from_remaining)
        } else {
            current?
        };
        return Some((used.max(0.0) / total * 100.0).clamp(0.0, 100.0));
    }
    num(limit.get("percentage"))
        .or_else(|| num(limit.get("usedPercent")))
        .or_else(|| num(limit.get("used_percent")))
        .map(|p| p.clamp(0.0, 100.0))
}

/// unit: 5 分钟,3 小时,1 天,6 周。
fn window_seconds(limit: &Value) -> Option<i64> {
    let unit = num(limit.get("unit"))? as i64;
    let number = num(limit.get("number"))? as i64;
    if number <= 0 {
        return None;
    }
    let minutes = match unit {
        5 => number,
        3 => number * 60,
        1 => number * 24 * 60,
        6 => number * 7 * 24 * 60,
        _ => return None,
    };
    Some(minutes * 60)
}

pub fn parse(body: &Value) -> Vec<QuotaWindow> {
    let Some(limits) = body.pointer("/data/limits").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut tokens = Vec::new();
    let mut time_limit = None;
    for limit in limits {
        let kind = limit
            .get("type")
            .or_else(|| limit.get("limit_type"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_uppercase();
        if used_percent(limit).is_none() {
            continue;
        }
        match kind.as_str() {
            "TOKENS_LIMIT" | "CREDIT_LIMIT" => tokens.push(limit),
            "TIME_LIMIT" => time_limit = Some(limit),
            _ => {}
        }
    }
    tokens.sort_by_key(|limit| window_seconds(limit).unwrap_or(i64::MAX));
    let mut windows = Vec::new();
    let session = if tokens.len() >= 2 {
        Some(tokens[0])
    } else {
        tokens
            .first()
            .copied()
            .filter(|limit| window_seconds(limit).is_some_and(|secs| secs <= 6 * 60 * 60))
    };
    let weekly = if tokens.len() >= 2 {
        tokens.last().copied()
    } else if session.is_none() {
        tokens.first().copied()
    } else {
        None
    };
    if let Some(limit) = session {
        if let Some(pct) = used_percent(limit) {
            windows.push(
                QuotaWindow::new("five_hour", "5 小时")
                    .percent(Some(pct))
                    .window(window_seconds(limit)),
            );
        }
    }
    if let Some(limit) = weekly {
        if let Some(pct) = used_percent(limit) {
            windows.push(
                QuotaWindow::new("weekly", "每周")
                    .percent(Some(pct))
                    .window(window_seconds(limit)),
            );
        }
    }
    if let Some(limit) = time_limit {
        if let Some(pct) = used_percent(limit) {
            windows.push(QuotaWindow::new("monthly", "MCP").percent(Some(pct)));
        }
    }
    windows
}

fn resolve_key(credential_name: &str) -> Option<String> {
    if let Some(key) = super::credential::resolve(credential_name) {
        return Some(key);
    }
    if credential_name != CREDENTIAL_NAME {
        return None;
    }
    for name in ["GLM_API_KEY", "ZHIPU_API_KEY", "Z_AI_API_KEY"] {
        if let Ok(value) = std::env::var(name) {
            let value = value.trim().to_string();
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

pub fn fetch_with_credential(credential_name: &str) -> ProviderLimits {
    let Some(key) = resolve_key(credential_name) else {
        return ProviderLimits::unconfigured(
            "zai",
            "智谱",
            "zai",
            "未配置 API Key。在设置里填入 ZAI_API_KEY 或 GLM_API_KEY。",
        );
    };
    for base in BASES {
        let url = format!("{base}{PATH}");
        if let Ok(body) = super::http::fetch_json(|agent| {
            agent
                .get(&url)
                .set("Authorization", &format!("Bearer {key}"))
                .set("Accept", "application/json")
                .call()
        }) {
            let windows = parse(&body);
            if !windows.is_empty() {
                let mut out = ProviderLimits::new("zai", "智谱", "zai");
                out.configured = true;
                out.plan_label = Some("Coding Plan".into());
                out.windows = windows;
                return out;
            }
        }
    }
    let mut out = ProviderLimits::new("zai", "智谱", "zai");
    out.configured = true;
    out.error = Some("没有查到 Coding Plan 用量".into());
    out
}

pub fn fetch() -> ProviderLimits {
    fetch_with_credential(CREDENTIAL_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_token_window_is_five_hours_and_long_one_is_weekly() {
        let body = serde_json::json!({
            "data": { "limits": [
                { "type": "TOKENS_LIMIT", "usage": 100, "remaining": 75, "unit": 3, "number": 5 },
                { "type": "CREDIT_LIMIT", "percentage": 10, "unit": 6, "number": 1 },
                { "type": "TIME_LIMIT", "percentage": 4, "unit": 5, "number": 1 }
            ]}
        });
        let windows = parse(&body);
        assert_eq!(windows[0].key, "five_hour");
        assert_eq!(windows[0].used_percent, Some(25.0));
        assert_eq!(windows[0].window_seconds, Some(5 * 60 * 60));
        assert_eq!(windows[1].key, "weekly");
        assert_eq!(windows[1].used_percent, Some(10.0));
        assert_eq!(windows[2].key, "monthly");
        assert_eq!(windows[2].window_seconds, None);
    }
}
