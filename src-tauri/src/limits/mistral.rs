//! Mistral 套餐额度。普通 API Key 读不到包含用量,要用管理后台的网页会话。
//!
//! 只把 `ory_session_*` 和 `csrftoken` 发给 Mistral 自己的站点,不跟随跳转。
//! 被转到登录页、401 或 403 都算会话过期,不会用旧 Cookie 去换一枚新的。

use serde_json::Value;

use super::{Balance, ProviderLimits, QuotaWindow};

pub const COOKIE_NAME: &str = "MISTRAL_ADMIN_COOKIE";

const SUBSCRIPTION_URL: &str = "https://admin.mistral.ai/subscription";
const CREDITS_URL: &str = "https://admin.mistral.ai/api/billing/credits";
const VIBE_URL: &str = "https://console.mistral.ai/api-ui/trpc/billing.vibeUsage?batch=1&input=%7B%220%22%3A%7B%22json%22%3Anull%2C%22meta%22%3A%7B%22values%22%3A%5B%22undefined%22%5D%2C%22v%22%3A1%7D%7D%7D";

struct Budget {
    used_percent: f64,
    reset_at: Option<i64>,
}

/// 从整段 Cookie 头里只留下会话和 CSRF。没有 `ory_session_` 就无法查询。
pub fn session_cookie(raw: &str) -> Option<String> {
    let raw = raw.trim().trim_start_matches("Cookie:").trim();
    let mut ory = None;
    let mut csrf = None;
    for part in raw.split(';') {
        let Some((name, value)) = part.trim().split_once('=') else {
            continue;
        };
        let name = name.trim();
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        if name.starts_with("ory_session_") {
            ory = Some(format!("{name}={value}"));
        } else if name.eq_ignore_ascii_case("csrftoken") {
            csrf = Some(value.to_string());
        }
    }
    let ory = ory?;
    Some(match csrf {
        Some(token) => format!("{ory}; csrftoken={token}"),
        None => ory,
    })
}

fn csrf_token(cookie: &str) -> Option<&str> {
    cookie.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        name.trim()
            .eq_ignore_ascii_case("csrftoken")
            .then_some(value.trim())
            .filter(|v| !v.is_empty())
    })
}

fn num(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64().filter(|n| n.is_finite()),
        Value::String(s) => s.trim().parse::<f64>().ok().filter(|n| n.is_finite()),
        _ => None,
    }
}

fn reset_ms(v: Option<&Value>) -> Option<i64> {
    let text = v?.as_str()?.trim();
    if text.is_empty() {
        return None;
    }
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|t| t.timestamp_millis())
}

fn budget(v: &Value) -> Option<Budget> {
    let used = num(v.get("usage_percentage")).filter(|n| *n >= 0.0)?;
    Some(Budget {
        used_percent: used.min(100.0),
        reset_at: reset_ms(v.get("reset_at")),
    })
}

fn objects_named(text: &str, key: &str) -> Vec<Value> {
    let needle = format!("\"{key}\"");
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(index) = rest.find(&needle) {
        let after = rest[index + needle.len()..].trim_start();
        let after = after.strip_prefix(':').unwrap_or(after).trim_start();
        if let Some(json) = take_json(after) {
            if let Ok(value) = serde_json::from_str::<Value>(json) {
                if value.is_object() {
                    out.push(value);
                }
            }
            rest = &after[json.len()..];
        } else {
            rest = &rest[index + needle.len()..];
        }
    }
    out
}

fn take_json(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let open = *bytes.first()?;
    if open != b'{' && open != b'[' {
        return None;
    }
    let mut stack = vec![if open == b'{' { b'}' } else { b']' }];
    let mut in_string = false;
    let mut escaped = false;
    for (index, byte) in bytes.iter().enumerate().skip(1) {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
            continue;
        }
        match *byte {
            b'"' => in_string = true,
            b'{' => stack.push(b'}'),
            b'[' => stack.push(b']'),
            b'}' | b']' => {
                if stack.last() != Some(byte) {
                    return None;
                }
                stack.pop();
                if stack.is_empty() {
                    return Some(&text[..=index]);
                }
            }
            _ => {}
        }
    }
    None
}

fn flight_text(html: &str) -> String {
    let marker = "self.__next_f.push(";
    let mut out = String::new();
    let mut rest = html;
    while let Some(index) = rest.find(marker) {
        let after = rest[index + marker.len()..].trim_start();
        let Some(json) = take_json(after) else {
            rest = &rest[index + marker.len()..];
            continue;
        };
        if let Ok(value) = serde_json::from_str::<Value>(json) {
            if value.get(0).and_then(Value::as_i64) == Some(1) {
                if let Some(chunk) = value.get(1).and_then(Value::as_str) {
                    out.push_str(chunk);
                }
            }
        }
        rest = &after[json.len()..];
    }
    if out.is_empty() { html.to_string() } else { out }
}

fn one_budget(text: &str, key: &str) -> Option<Budget> {
    let found: Vec<Budget> = objects_named(text, key).iter().filter_map(budget).collect();
    let first = found.first()?;
    if found.iter().any(|item| (item.used_percent - first.used_percent).abs() > 0.01) {
        return None;
    }
    Some(Budget {
        used_percent: first.used_percent,
        reset_at: first.reset_at,
    })
}

/// 管理后台文档列出的档位。按量付费是额度用完之后的开关,不是另一档套餐。
fn plan_label(raw: &str) -> Option<&'static str> {
    let key: String = raw
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect();
    match key.as_str() {
        "free" | "freemode" => Some("Free"),
        "pro" => Some("Pro"),
        "education" | "student" | "edu" => Some("Education"),
        "team" | "teams" => Some("Team"),
        "enterprise" => Some("Enterprise"),
        _ => None,
    }
}

fn plan_from_value(value: &Value) -> Option<&'static str> {
    match value {
        Value::String(text) => plan_label(text),
        Value::Object(object) => ["name", "slug", "id", "code", "type", "plan"]
            .iter()
            .find_map(|key| object.get(*key).and_then(plan_from_value)),
        _ => None,
    }
}

fn values_named(text: &str, key: &str) -> Vec<Value> {
    let needle = format!("\"{key}\"");
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(index) = rest.find(&needle) {
        let after = rest[index + needle.len()..].trim_start();
        let Some(after) = after.strip_prefix(':') else {
            rest = &rest[index + needle.len()..];
            continue;
        };
        let after = after.trim_start();
        let parsed = if after.starts_with('"') {
            take_string(after).and_then(|json| serde_json::from_str(json).ok())
        } else {
            take_json(after).and_then(|json| serde_json::from_str(json).ok())
        };
        if let Some(value) = parsed {
            let skip = if after.starts_with('"') {
                take_string(after).map(str::len).unwrap_or(1)
            } else {
                take_json(after).map(str::len).unwrap_or(1)
            };
            out.push(value);
            rest = &after[skip..];
        } else {
            rest = &rest[index + needle.len()..];
        }
    }
    out
}

fn take_string(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    if bytes.first() != Some(&b'"') {
        return None;
    }
    let mut escaped = false;
    for (index, byte) in bytes.iter().enumerate().skip(1) {
        if escaped {
            escaped = false;
        } else if *byte == b'\\' {
            escaped = true;
        } else if *byte == b'"' {
            return Some(&text[..=index]);
        }
    }
    None
}

/// 订阅页里的套餐名。认不出官方档位就不显示,避免把内部字段当成套餐。
pub fn parse_plan(html: &str) -> Option<String> {
    let text = flight_text(html);
    let mut found = Vec::new();
    for key in ["plan", "plan_name", "plan_type", "subscription_plan", "current_plan"] {
        for value in values_named(&text, key) {
            if let Some(label) = plan_from_value(&value) {
                found.push(label);
            }
        }
    }
    let first = *found.first()?;
    found.iter().all(|label| *label == first).then(|| first.to_string())
}

pub fn parse_subscription(html: &str) -> (Option<QuotaWindow>, Option<QuotaWindow>) {
    let text = flight_text(html);
    let api = one_budget(&text, "api_budget").map(|item| {
        QuotaWindow::new("plan", "包含额度")
            .percent(Some(item.used_percent))
            .reset(item.reset_at)
    });
    let vibe = one_budget(&text, "vibe_budget").map(|item| {
        QuotaWindow::new("monthly", "Vibe")
            .percent(Some(item.used_percent))
            .reset(item.reset_at)
    });
    (api, vibe)
}

pub fn parse_credits(body: &Value) -> Option<Balance> {
    let body = body.get("data").filter(|v| v.is_object()).unwrap_or(body);
    let currency = body
        .get("currency")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    let wallet = num(body.get("wallet_amount"))?;
    let notes = num(body.get("credit_notes_amount")).unwrap_or(0.0);
    let ongoing = num(body.get("ongoing_usage_balance")).unwrap_or(0.0);
    let amount = wallet + notes - ongoing;
    if amount < 0.0 {
        return None;
    }
    Some(Balance {
        amount,
        currency: currency.to_string(),
        cash: None,
        voucher: None,
    })
}

fn find_percent(value: &Value) -> Option<f64> {
    if let Some(object) = value.as_object() {
        if let Some(pct) = num(object.get("usage_percentage")).filter(|n| *n >= 0.0) {
            return Some(pct.min(100.0));
        }
        for child in object.values() {
            if let Some(pct) = find_percent(child) {
                return Some(pct);
            }
        }
    } else if let Some(list) = value.as_array() {
        for child in list {
            if let Some(pct) = find_percent(child) {
                return Some(pct);
            }
        }
    }
    None
}

pub fn parse_vibe(body: &Value) -> Option<QuotaWindow> {
    find_percent(body).map(|pct| QuotaWindow::new("monthly", "Vibe").percent(Some(pct)))
}

fn expired() -> ProviderLimits {
    let mut out = ProviderLimits::new("mistral", "Mistral", "mistral");
    out.configured = true;
    out.error = Some("登录已过期。在浏览器重新登录 admin.mistral.ai 后,再贴一次 Cookie。".into());
    out
}

fn get(cookie: &str, url: &str, csrf: Option<&str>) -> Result<(u16, String), String> {
    super::http::fetch_text_no_redirect(|agent| {
        let mut req = agent
            .get(url)
            .set("Cookie", cookie)
            .set("Accept", "text/html,application/json");
        if let Some(token) = csrf {
            req = req.set("X-CSRFTOKEN", token);
        }
        req.call()
    })
    .map_err(|e| e.message())
}

pub fn fetch_with_credential(credential_name: &str) -> ProviderLimits {
    let Some(raw) = super::credential::resolve(credential_name) else {
        return ProviderLimits::unconfigured(
            "mistral",
            "Mistral",
            "mistral",
            "未配置登录 Cookie。在浏览器登录 admin.mistral.ai,把请求头里的 Cookie 贴到设置。",
        );
    };
    let Some(cookie) = session_cookie(&raw) else {
        let mut out = ProviderLimits::new("mistral", "Mistral", "mistral");
        out.configured = true;
        out.error = Some("Cookie 里没有 ory_session_。请贴整段 Cookie 请求头。".into());
        return out;
    };
    let csrf = csrf_token(&cookie).map(str::to_string);
    let credits = match get(&cookie, CREDITS_URL, None) {
        Ok((status, _)) if (300..400).contains(&status) || status == 401 || status == 403 => {
            return expired();
        }
        Ok((status, body)) if (200..300).contains(&status) => {
            serde_json::from_str::<Value>(&body).ok().and_then(|v| parse_credits(&v))
        }
        _ => None,
    };
    let mut windows = Vec::new();
    let mut vibe = None;
    let mut plan = None;
    match get(&cookie, SUBSCRIPTION_URL, None) {
        Ok((status, _)) if (300..400).contains(&status) || status == 401 || status == 403 => {
            return expired();
        }
        Ok((status, html)) if (200..300).contains(&status) => {
            let (api, found_vibe) = parse_subscription(&html);
            plan = parse_plan(&html);
            if let Some(window) = api {
                windows.push(window);
            }
            vibe = found_vibe;
        }
        _ => {}
    }
    if vibe.is_none() {
        if let Some(token) = csrf.as_deref() {
            if let Ok((status, body)) = get(&cookie, VIBE_URL, Some(token)) {
                if (300..400).contains(&status) || status == 401 || status == 403 {
                    return expired();
                }
                if (200..300).contains(&status) {
                    if let Ok(json) = serde_json::from_str::<Value>(&body) {
                        vibe = parse_vibe(&json);
                    }
                }
            }
        }
    }
    if let Some(window) = vibe {
        windows.push(window);
    }
    let mut out = ProviderLimits::new("mistral", "Mistral", "mistral");
    out.configured = true;
    out.plan_label = plan;
    out.windows = windows;
    out.balance = credits;
    if out.windows.is_empty() && out.balance.is_none() {
        out.error = Some("没有查到套餐额度或余额。确认 Cookie 来自已登录的 admin.mistral.ai。".into());
    }
    out
}

pub fn fetch() -> ProviderLimits {
    fetch_with_credential(COOKIE_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_the_session_and_csrf() {
        let cookie = session_cookie(
            "Cookie: theme=dark; ory_session_abc=sess; csrftoken=tok; other=1",
        )
        .unwrap();
        assert_eq!(cookie, "ory_session_abc=sess; csrftoken=tok");
        assert!(session_cookie("theme=dark").is_none());
    }

    #[test]
    fn reads_budgets_from_the_flight_payload() {
        let html = r#"<script>self.__next_f.push([1,"{\"budget\":{\"api_budget\":{\"usage_percentage\":25,\"initial_budget\":20,\"currency\":\"EUR\",\"reset_at\":\"2026-12-01T00:00:00Z\"},\"vibe_budget\":{\"usage_percentage\":10,\"initial_budget\":5,\"currency\":\"EUR\"}}}"])</script>"#;
        let (api, vibe) = parse_subscription(html);
        assert_eq!(api.unwrap().used_percent, Some(25.0));
        assert_eq!(vibe.unwrap().used_percent, Some(10.0));
    }

    #[test]
    fn conflicting_budgets_are_left_out() {
        let html = r#"{"api_budget":{"usage_percentage":10,"initial_budget":1,"currency":"EUR"}}{"api_budget":{"usage_percentage":80,"initial_budget":1,"currency":"EUR"}}"#;
        assert!(parse_subscription(html).0.is_none());
    }

    #[test]
    fn plan_names_follow_the_official_tiers() {
        let html = r#"<script>self.__next_f.push([1,"{\"plan\":\"team\",\"budget\":{\"api_budget\":{\"usage_percentage\":25,\"initial_budget\":20,\"currency\":\"EUR\"}}}"])</script>"#;
        assert_eq!(parse_plan(html).as_deref(), Some("Team"));
        assert_eq!(plan_label("free_mode"), Some("Free"));
        assert_eq!(plan_label("education"), Some("Education"));
        assert_eq!(plan_label("pay_as_you_go"), None);
    }

    #[test]
    fn credits_subtract_ongoing_usage() {
        let balance = parse_credits(&serde_json::json!({
            "wallet_amount": 10,
            "credit_notes_amount": 2,
            "ongoing_usage_balance": 3,
            "currency": "EUR"
        }))
        .unwrap();
        assert!((balance.amount - 9.0).abs() < 1e-9);
        assert_eq!(balance.currency, "EUR");
    }
}
