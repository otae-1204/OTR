//! DeepSeek 余额:`GET https://api.deepseek.com/user/balance`
//!
//! 凭据:keyring 里的 `DEEPSEEK_API_KEY`,回退环境变量同名。
//! 响应里 `balance_infos` 是数组(可能多币种),取第一条;
//! `is_available=false` 表示账户不可用,这也要如实展示而不是当成余额 0。

use serde_json::Value;

use super::{Balance, ProviderLimits};

pub const BALANCE_URL: &str = "https://api.deepseek.com/user/balance";
pub const CREDENTIAL_NAME: &str = "DEEPSEEK_API_KEY";

fn as_f64(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        // 实测余额是**字符串**("1.99"),不是数字
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// 解析余额响应。字段缺失一律 None → UI 显示 "--"。
pub fn parse_balance(body: &Value) -> ProviderLimits {
    let mut out = ProviderLimits::new("deepseek", "DeepSeek", "deepseek");
    out.configured = true;

    let info = body
        .get("balance_infos")
        .and_then(Value::as_array)
        .and_then(|a| a.first());
    let Some(info) = info else {
        out.error = Some("响应里没有余额信息".into());
        return out;
    };

    let amount = as_f64(info.get("total_balance"));
    let currency = info
        .get("currency")
        .and_then(Value::as_str)
        .unwrap_or("CNY")
        .to_string();
    out.balance = Some(Balance {
        amount: amount.unwrap_or(0.0),
        currency,
        // DeepSeek 不区分充值/赠送池
        cash: None,
        voucher: None,
    });

    // 可用性是一个独立事实:余额有数字但账户停用,要如实标出来
    if body.get("is_available").and_then(Value::as_bool) == Some(false) {
        out.error = Some("账户当前不可用(余额可能不足)".into());
    }
    if amount.is_none() {
        out.error = Some("余额字段缺失".into());
    }
    out
}

pub fn fetch() -> ProviderLimits {
    fetch_with_credential(CREDENTIAL_NAME)
}

/// 多账号:从指定条目名取 key
pub fn fetch_with_credential(credential_name: &str) -> ProviderLimits {
    let Some(key) = super::credential::resolve(credential_name) else {
        let hint = if credential_name == CREDENTIAL_NAME {
            "未配置 API Key。在设置里填入 DEEPSEEK_API_KEY 即可查看余额。"
        } else {
            "未配置 API Key。在设置里为该账号填入密钥即可查看余额。"
        };
        return ProviderLimits::unconfigured("deepseek", "DeepSeek", "deepseek", hint);
    };
    match super::http::fetch_json(|agent| {
        agent
            .get(BALANCE_URL)
            .set("Authorization", &format!("Bearer {key}"))
            .set("Accept", "application/json")
            .call()
    }) {
        Ok(v) => parse_balance(&v),
        Err(e) => {
            let mut out = ProviderLimits::new("deepseek", "DeepSeek", "deepseek");
            out.configured = true;
            out.error = Some(match e.status() {
                Some(401) => "API Key 无效".to_string(),
                Some(402) => "账户余额不足".to_string(),
                _ => e.message(),
            });
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 实测响应:余额是字符串 "1.99",不是数字
    #[test]
    fn parses_the_live_response_shape() {
        let body = serde_json::json!({
            "is_available": true,
            "balance_infos": [{
                "currency": "CNY",
                "total_balance": "1.99",
                "granted_balance": "0.00",
                "topped_up_balance": "1.99"
            }]
        });
        let got = parse_balance(&body);
        let b = got.balance.expect("必须有余额");
        assert!((b.amount - 1.99).abs() < 1e-9, "实测余额 ≈ 1.99 CNY");
        assert_eq!(b.currency, "CNY");
        assert_eq!(got.error, None);
    }

    /// 数字形式的余额也要能认
    #[test]
    fn numeric_balance_is_accepted_too() {
        let body = serde_json::json!({
            "is_available": true,
            "balance_infos": [{ "currency": "USD", "total_balance": 12.5 }]
        });
        let b = parse_balance(&body).balance.unwrap();
        assert!((b.amount - 12.5).abs() < 1e-9);
        assert_eq!(b.currency, "USD");
    }

    /// 规则 3:余额字段缺失 → 明说,而不是显示 0 元
    #[test]
    fn missing_amount_is_reported_not_zeroed() {
        let body = serde_json::json!({
            "is_available": true,
            "balance_infos": [{ "currency": "CNY" }]
        });
        let got = parse_balance(&body);
        assert!(got.error.is_some(), "缺字段必须报出来");
    }

    /// 账户不可用是独立事实,不能因为余额有数字就吞掉
    #[test]
    fn unavailable_account_is_surfaced() {
        let body = serde_json::json!({
            "is_available": false,
            "balance_infos": [{ "currency": "CNY", "total_balance": "0.00" }]
        });
        let got = parse_balance(&body);
        assert!(got.error.as_deref().unwrap().contains("不可用"));
    }

    /// 没有 balance_infos 时不崩,给出明确错误
    #[test]
    fn absent_balance_list_is_handled() {
        let got = parse_balance(&serde_json::json!({ "is_available": true }));
        assert!(got.balance.is_none());
        assert!(got.error.is_some());
    }
}
