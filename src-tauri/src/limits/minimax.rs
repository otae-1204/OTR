//! MiniMax Coding Plan。剩余百分比在 `model_name = general` 这一行。
//!
//! 先问 Token Plan 接口,没有窗口再问旧的 remains 接口。国内和国际域名都试,
//! 哪个先给出窗口用哪个。`status == 3` 且百分比为空或已满,是「这个档位没有权益」
//! 的占位,不画成 100%。

use serde_json::Value;

use super::{ProviderLimits, QuotaWindow};

pub const CREDENTIAL_NAME: &str = "MINIMAX_CODING_API_KEY";

const URLS: &[&str] = &[
    "https://api.minimaxi.com/v1/token_plan/remains",
    "https://api.minimaxi.com/v1/api/openplatform/coding_plan/remains",
    "https://api.minimax.io/v1/token_plan/remains",
    "https://api.minimax.io/v1/api/openplatform/coding_plan/remains",
];

fn num(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) if !s.trim().is_empty() => s.trim().parse().ok(),
        _ => None,
    }
}

fn rows(body: &Value) -> Option<&Vec<Value>> {
    body.pointer("/data/model_remains")
        .and_then(Value::as_array)
        .or_else(|| body.get("model_remains").and_then(Value::as_array))
}

fn placeholder(item: &Value, percent: &str, status: &str) -> bool {
    if num(item.get(status)) != Some(3.0) {
        return false;
    }
    match num(item.get(percent)) {
        None => true,
        Some(pct) => pct >= 100.0,
    }
}

fn window_from_remaining(
    item: &Value,
    percent_field: &str,
    status_field: &str,
    end_field: &str,
    key: &str,
    label: &str,
    window_seconds: i64,
) -> Option<QuotaWindow> {
    if placeholder(item, percent_field, status_field) {
        return None;
    }
    let remain = num(item.get(percent_field))?;
    let used = (100.0 - remain).clamp(0.0, 100.0);
    let reset = num(item.get(end_field)).map(|ms| {
        if ms < 1_000_000_000_000.0 {
            (ms * 1000.0) as i64
        } else {
            ms as i64
        }
    });
    Some(
        QuotaWindow::new(key, label)
            .percent(Some(used))
            .reset(reset)
            .window(Some(window_seconds)),
    )
}

pub fn parse(body: &Value) -> Vec<QuotaWindow> {
    let Some(item) = rows(body).and_then(|list| {
        list.iter()
            .find(|row| row.get("model_name").and_then(Value::as_str) == Some("general"))
    }) else {
        return Vec::new();
    };
    let mut windows = Vec::new();
    if let Some(w) = window_from_remaining(
        item,
        "current_interval_remaining_percent",
        "current_interval_status",
        "end_time",
        "five_hour",
        "5 小时",
        5 * 60 * 60,
    ) {
        windows.push(w);
    }
    if let Some(w) = window_from_remaining(
        item,
        "current_weekly_remaining_percent",
        "current_weekly_status",
        "weekly_end_time",
        "weekly",
        "每周",
        7 * 24 * 60 * 60,
    ) {
        windows.push(w);
    }
    windows
}

pub fn fetch_with_credential(credential_name: &str) -> ProviderLimits {
    let Some(key) = super::credential::resolve(credential_name) else {
        return ProviderLimits::unconfigured(
            "minimax",
            "MiniMax",
            "minimax",
            "未配置 API Key。在设置里填入 MINIMAX_CODING_API_KEY。",
        );
    };
    let mut saw_auth = false;
    for url in URLS {
        match super::http::fetch_json(|agent| {
            agent
                .get(*url)
                .set("Authorization", &format!("Bearer {key}"))
                .set("Accept", "application/json")
                .call()
        }) {
            Ok(body) => {
                let windows = parse(&body);
                if !windows.is_empty() {
                    let mut out = ProviderLimits::new("minimax", "MiniMax", "minimax");
                    out.configured = true;
                    out.plan_label = Some("Token Plan".into());
                    out.windows = windows;
                    return out;
                }
            }
            Err(e) if e.status() == Some(401) || e.status() == Some(403) => saw_auth = true,
            Err(_) => {}
        }
    }
    let mut out = ProviderLimits::new("minimax", "MiniMax", "minimax");
    out.configured = true;
    out.error = Some(if saw_auth {
        "API Key 无效,或这把密钥不属于当前区域".into()
    } else {
        "没有查到 Coding Plan 用量".into()
    });
    out
}

pub fn fetch() -> ProviderLimits {
    fetch_with_credential(CREDENTIAL_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remaining_percent_becomes_used_percent() {
        let body = serde_json::json!({
            "data": {
                "model_remains": [{
                    "model_name": "general",
                    "current_interval_remaining_percent": "80",
                    "current_interval_status": 1,
                    "end_time": 1_700_000_000_000i64,
                    "current_weekly_remaining_percent": "40",
                    "current_weekly_status": 1
                }, {
                    "model_name": "video"
                }]
            }
        });
        let windows = parse(&body);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].key, "five_hour");
        assert_eq!(windows[0].used_percent, Some(20.0));
        assert_eq!(windows[0].reset_at, Some(1_700_000_000_000));
        assert_eq!(windows[1].used_percent, Some(60.0));
    }

    #[test]
    fn status_three_placeholder_is_dropped() {
        let body = serde_json::json!({
            "model_remains": [{
                "model_name": "general",
                "current_interval_status": 3,
                "current_interval_remaining_percent": "100"
            }]
        });
        assert!(parse(&body).is_empty());
    }
}
