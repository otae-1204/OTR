//! Kimi Code 额度。`GET https://api.kimi.com/coding/v1/usages`。
//!
//! 顶层 `usage` 是控制台显示的那一档,通常是每周。`usages.limit_5h.used_ratio`
//! 是 5 小时滚动窗口,0 到 1。两边都缺才算没查到。

use serde_json::Value;

use super::{ProviderLimits, QuotaWindow};

pub const CREDENTIAL_NAME: &str = "KIMI_CODE_API_KEY";
const USAGES_URL: &str = "https://api.kimi.com/coding/v1/usages";
const ME_URL: &str = "https://api.kimi.com/coding/v1/me";

fn num(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) if !s.trim().is_empty() => s.trim().parse().ok(),
        _ => None,
    }
}

fn pick<'a>(obj: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|key| obj.get(*key))
}

fn used_percent(detail: &Value) -> Option<f64> {
    let limit = num(pick(
        detail,
        &["limit", "limit_value", "total", "quota", "max"],
    ));
    let remaining = num(pick(detail, &["remaining", "remaining_value"]));
    let used = num(pick(detail, &["used", "used_value", "current_value", "consumed"]));
    if let Some(limit) = limit.filter(|n| *n > 0.0) {
        if let Some(remaining) = remaining {
            return Some(((limit - remaining) / limit * 100.0).clamp(0.0, 100.0));
        }
        if let Some(used) = used {
            return Some((used / limit * 100.0).clamp(0.0, 100.0));
        }
    }
    num(pick(
        detail,
        &["percent", "percentage", "usedPercent", "used_percent"],
    ))
    .map(|p| if (0.0..=1.0).contains(&p) { p * 100.0 } else { p })
    .map(|p| p.clamp(0.0, 100.0))
}

/// 会员档位。新档是 Go、Plus、Pro、Max、Ultra。
/// 旧档仍用 Andante、Moderato、Allegretto、Allegro、Vivace。
/// `/me` 的 `user_level_name` 是客户端展示名；用量接口里的 `LEVEL_*` 是枚举。
fn plan_label(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let key: String = trimmed
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect();
    let known = match key.as_str() {
        "free" | "levelfree" => "Free",
        "go" | "levelgo" => "Go",
        "plus" | "levelplus" => "Plus",
        "pro" | "levelpro" => "Pro",
        "max" | "levelmax" => "Max",
        "ultra" | "levelultra" => "Ultra",
        "andante" | "levelbasic" => "Andante",
        "adagio" => "Adagio",
        "moderato" | "levelstandard" => "Moderato",
        "allegretto" | "levelintermediate" => "Allegretto",
        "allegro" | "leveladvanced" => "Allegro",
        "vivace" | "levelpremium" => "Vivace",
        _ => {
            if key.starts_with("level") {
                return None;
            }
            return Some(trimmed.to_string());
        }
    };
    Some(known.to_string())
}

pub fn parse_plan(body: &Value) -> Option<String> {
    let body = body.get("data").filter(|v| v.is_object()).unwrap_or(body);
    body.get("user_level_name")
        .or_else(|| body.get("userLevelName"))
        .and_then(Value::as_str)
        .and_then(plan_label)
        .or_else(|| {
            body.pointer("/user/membership/level")
                .or_else(|| body.pointer("/membership/level"))
                .and_then(Value::as_str)
                .and_then(plan_label)
        })
}

pub fn parse(body: &Value) -> Vec<QuotaWindow> {
    let body = body.get("data").filter(|v| v.is_object()).unwrap_or(body);
    let mut windows = Vec::new();
    if let Some(usage) = body.get("usage").filter(|v| v.is_object()) {
        if let Some(pct) = used_percent(usage) {
            windows.push(QuotaWindow::new("weekly", "每周").percent(Some(pct)));
        }
    }
    let ratio = num(
        body.pointer("/usages/limit_5h/used_ratio")
            .or_else(|| body.pointer("/usages/limit_5h/usedRatio")),
    );
    if let Some(ratio) = ratio {
        windows.push(
            QuotaWindow::new("five_hour", "5 小时")
                .percent(Some((ratio * 100.0).clamp(0.0, 100.0)))
                .window(Some(5 * 60 * 60)),
        );
    }
    windows
}

pub fn fetch_with_credential(credential_name: &str) -> ProviderLimits {
    let Some(key) = super::credential::resolve(credential_name) else {
        return ProviderLimits::unconfigured(
            "kimi",
            "Kimi",
            "kimi",
            "未配置 API Key。在设置里填入 KIMI_CODE_API_KEY。",
        );
    };
    match super::http::fetch_json(|agent| {
        agent
            .get(USAGES_URL)
            .set("Authorization", &format!("Bearer {key}"))
            .set("Accept", "application/json")
            .call()
    }) {
        Ok(body) => {
            let windows = parse(&body);
            let mut out = ProviderLimits::new("kimi", "Kimi", "kimi");
            out.configured = true;
            out.plan_label = member_plan(&key).or_else(|| parse_plan(&body));
            if windows.is_empty() {
                out.error = Some("没有查到 Coding Plan 用量".into());
            } else {
                out.windows = windows;
            }
            out
        }
        Err(e) => {
            let mut out = ProviderLimits::new("kimi", "Kimi", "kimi");
            out.configured = true;
            out.error = Some(match e.status() {
                Some(401) | Some(403) => "API Key 无效".into(),
                _ => "Kimi 额度查询失败".into(),
            });
            out
        }
    }
}

fn member_plan(key: &str) -> Option<String> {
    let body = super::http::fetch_json(|agent| {
        agent
            .get(ME_URL)
            .set("Authorization", &format!("Bearer {key}"))
            .set("Accept", "application/json")
            .call()
    })
    .ok()?;
    parse_plan(&body)
}

pub fn fetch() -> ProviderLimits {
    fetch_with_credential(CREDENTIAL_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weekly_detail_and_five_hour_ratio() {
        let windows = parse(&serde_json::json!({
            "usage": { "limit": 100, "remaining": 25, "name": "FEATURE_CODING" },
            "usages": { "limit_5h": { "used_ratio": 0.2 } }
        }));
        assert_eq!(windows[0].key, "weekly");
        assert_eq!(windows[0].used_percent, Some(75.0));
        assert_eq!(windows[1].key, "five_hour");
        assert_eq!(windows[1].used_percent, Some(20.0));
    }

    #[test]
    fn plan_names_follow_the_official_tiers() {
        assert_eq!(
            parse_plan(&serde_json::json!({ "user_level_name": "Pro" })).as_deref(),
            Some("Pro")
        );
        assert_eq!(
            parse_plan(&serde_json::json!({
                "user": { "membership": { "level": "LEVEL_INTERMEDIATE" } }
            }))
            .as_deref(),
            Some("Allegretto")
        );
        assert_eq!(
            parse_plan(&serde_json::json!({ "user_level_name": "LEVEL_BASIC" })).as_deref(),
            Some("Andante")
        );
        assert!(parse_plan(&serde_json::json!({ "user_level_name": "LEVEL_UNKNOWN" })).is_none());
    }
}
