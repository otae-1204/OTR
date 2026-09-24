//! 千问 Coding Plan 额度。
//!
//! 官方文档只让用户到 Coding Plan 页面上看用量,没有公开的 REST。
//! 那个页面实际调的是百炼控制台 RPC `queryCodingPlanInstanceInfoV2`。
//! `sk-sp-` Key 打上去会得到 `ConsoleNeedLogin`;能查到数字的是浏览器登录态
//! (Cookie + 页面里的 `sec_token`),走 `bailian-cs.console.aliyun.com`。
//!
//! 字段缺失保持 `None`,不把「读不到」写成 0%。

use serde_json::{Map, Value};

use super::QuotaWindow;

pub const CREDENTIAL_NAME: &str = "QWEN_CODING_PLAN_API_KEY";
/// 百炼控制台的 Cookie。`sk-sp-` Key 打 `queryCodingPlanInstanceInfoV2` 会得到
/// `ConsoleNeedLogin`,额度只认浏览器登录态。
pub const COOKIE_NAME: &str = "QWEN_CONSOLE_COOKIE";

const KEY_NEEDS_COOKIE: &str = "Coding Plan 的 Key 不能查额度,百炼控制台要求浏览器登录。打开 https://bailian.console.aliyun.com 的 Coding Plan 页,在开发者工具里复制请求头 Cookie,贴到设置的 Qwen 这一栏。";

const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";

const ACTION: &str = "zeldaEasy.broadscope-bailian.codingPlan.queryCodingPlanInstanceInfoV2";
/// 个人版(Token Plan Solo)不走上面那个套餐实例接口,列表会是空的。
/// 用量和订阅在 apikey 网关的 v2。Coding Plan 实例接口先试,空了再走这里。
const PERSONAL_USAGE_API: &str = "zeldaHttp.apikeyMgr./tokenplan/personal/api/v2/usage";
const PERSONAL_SUBSCRIPTION_API: &str = "zeldaHttp.apikeyMgr./tokenplan/personal/api/v2/subscription";

struct Region {
    label: &'static str,
    host: &'static str,
    region_id: &'static str,
    commodity: &'static str,
    referer: &'static str,
}

const CHINA: Region = Region {
    label: "国内",
    host: "bailian.console.aliyun.com",
    region_id: "cn-beijing",
    commodity: "sfm_codingplan_public_cn",
    referer: "https://bailian.console.aliyun.com/cn-beijing/?tab=model#/efm/coding_plan",
};

const INTL: Region = Region {
    label: "国际",
    host: "modelstudio.console.alibabacloud.com",
    region_id: "ap-southeast-1",
    commodity: "sfm_codingplan_public_intl",
    referer: "https://modelstudio.console.alibabacloud.com/ap-southeast-1/?tab=globalset#/efm/coding_plan",
};

/// Cookie 模式打的是另一台网关,不是 Key 模式那个 `*.console` 主机。
struct Console {
    label: &'static str,
    dashboard: &'static str,
    info_url: &'static str,
    region_id: &'static str,
    commodity: &'static str,
    origin: &'static str,
    referer: &'static str,
    domain: &'static str,
    site: &'static str,
    fe_url: &'static str,
    /// 表单里的 action。和 URL 上的 action 必须一致,网关只读 body 时 URL 上的不算。
    action: &'static str,
    /// 先打 cs 网关,不行再打控制台自己的 `/data/api.json`。
    rpcs: [&'static str; 2],
}

const CN_CONSOLE: Console = Console {
    label: "国内",
    dashboard: "https://bailian.console.aliyun.com/cn-beijing/?tab=model",
    info_url: "https://bailian.console.aliyun.com/tool/user/info.json",
    region_id: "cn-beijing",
    commodity: "sfm_codingplan_public_cn",
    origin: "https://bailian.console.aliyun.com",
    referer: "https://bailian.console.aliyun.com/cn-beijing/?tab=model",
    domain: "bailian.console.aliyun.com",
    site: "BAILIAN_ALIYUN",
    fe_url: "https://bailian.console.aliyun.com/cn-beijing/?tab=model#/efm/coding_plan",
    action: "BroadScopeAspnGateway",
    rpcs: [
        "https://bailian-cs.console.aliyun.com/data/api.json?action=BroadScopeAspnGateway&product=sfm_bailian&api=zeldaEasy.broadscope-bailian.codingPlan.queryCodingPlanInstanceInfoV2&_v=undefined",
        "https://bailian.console.aliyun.com/data/api.json?action=BroadScopeAspnGateway&product=sfm_bailian&api=zeldaEasy.broadscope-bailian.codingPlan.queryCodingPlanInstanceInfoV2&_v=undefined",
    ],
};

const INTL_CONSOLE: Console = Console {
    label: "国际",
    dashboard: "https://modelstudio.console.alibabacloud.com/ap-southeast-1/?tab=coding-plan",
    info_url: "https://modelstudio.console.alibabacloud.com/tool/user/info.json",
    region_id: "ap-southeast-1",
    commodity: "sfm_codingplan_public_intl",
    origin: "https://modelstudio.console.alibabacloud.com",
    referer: "https://modelstudio.console.alibabacloud.com/ap-southeast-1/?tab=coding-plan",
    domain: "modelstudio.console.alibabacloud.com",
    site: "MODELSTUDIO_ALIBABACLOUD",
    fe_url: "https://modelstudio.console.alibabacloud.com/ap-southeast-1/?tab=coding-plan#/efm/coding_plan",
    action: "IntlBroadScopeAspnGateway",
    rpcs: [
        "https://bailian-singapore-cs.alibabacloud.com/data/api.json?action=IntlBroadScopeAspnGateway&product=sfm_bailian&api=zeldaEasy.broadscope-bailian.codingPlan.queryCodingPlanInstanceInfoV2&_v=undefined",
        "https://modelstudio.console.alibabacloud.com/data/api.json?action=IntlBroadScopeAspnGateway&product=sfm_bailian&api=zeldaEasy.broadscope-bailian.codingPlan.queryCodingPlanInstanceInfoV2&_v=undefined",
    ],
};

pub fn fetch() -> super::ProviderLimits {
    fetch_with_credential(CREDENTIAL_NAME)
}

pub fn fetch_with_credential(credential_name: &str) -> super::ProviderLimits {
    let Some(key) = resolve_key(credential_name) else {
        let hint = if credential_name == CREDENTIAL_NAME {
            "未配置 Coding Plan API Key。在设置里填入 sk-sp- 开头的专属 Key。"
        } else {
            "未配置 API Key。在设置里为该账号填入 Coding Plan 专属 Key。"
        };
        return super::ProviderLimits::unconfigured("qwen", "Qwen", "qwen", hint);
    };

    let mut notes = Vec::new();
    for region in [CHINA, INTL] {
        match request(&key, &region) {
            Ok(body) => match read_body(&body) {
                Read::Ready(mut got) => {
                    got.configured = true;
                    return got;
                }
                Read::Retry(msg) => notes.push(format!("{}: {msg}", region.label)),
            },
            Err(e) => {
                let msg = match e.status() {
                    Some(401) | Some(403) => "API Key 无效".to_string(),
                    Some(code) => format!("http {code}"),
                    None => e.message(),
                };
                notes.push(format!("{}: {msg}", region.label));
            }
        }
    }

    let mut out = super::ProviderLimits::new("qwen", "Qwen", "qwen");
    out.configured = true;
    let login_only = !notes.is_empty() && notes.iter().all(|n| n.contains("控制台要求登录"));
    out.error = Some(if login_only {
        KEY_NEEDS_COOKIE.into()
    } else if notes.is_empty() {
        "没有查到 Coding Plan 用量".into()
    } else {
        notes.join("；")
    });
    out
}

/// 用百炼控制台 Cookie 查额度。先国内网关,失败再试国际。
pub fn fetch_with_cookie(credential_name: &str) -> super::ProviderLimits {
    let Some(raw) = super::credential::resolve(credential_name) else {
        return super::ProviderLimits::unconfigured(
            "qwen",
            "Qwen",
            "qwen",
            "未配置百炼控制台 Cookie。打开 Coding Plan 页面,复制请求头 Cookie 贴到设置里。",
        );
    };
    let cookie = normalize_cookie(&raw);
    if cookie.is_empty() {
        return super::ProviderLimits::unconfigured(
            "qwen",
            "Qwen",
            "qwen",
            "百炼控制台 Cookie 是空的。",
        );
    }

    let mut notes = Vec::new();
    // 国内登录票据拿到国际站上只会多一条「读不到 sec_token」,没有信息量。
    let regions: &[Console] = if cookie.contains("login_aliyunid_ticket") {
        &[CN_CONSOLE]
    } else {
        &[CN_CONSOLE, INTL_CONSOLE]
    };
    for region in regions {
        let sec_token = match resolve_sec_token(&cookie, region) {
            Ok(token) => token,
            Err(err) => {
                notes.push(format!("{}: {err}", region.label));
                continue;
            }
        };
        let anonymous = cookie_value(&cookie, "cna");
        let csrf = cookie_value(&cookie, "login_aliyunid_csrf");
        if let Some(mut got) = coding_plan_windows(&cookie, region, &sec_token, anonymous.as_deref(), csrf.as_deref()) {
            got.configured = true;
            if !got.windows.is_empty() {
                return got;
            }
        }
        match personal_plan(&cookie, region, &sec_token, anonymous.as_deref(), csrf.as_deref()) {
            Ok(mut got) => {
                got.configured = true;
                if !got.windows.is_empty() || got.plan_label.is_some() {
                    return got;
                }
                if let Some(err) = got.error {
                    notes.push(format!("{}: {err}", region.label));
                }
            }
            Err(err) => notes.push(format!("{}: {err}", region.label)),
        }
    }

    let mut out = super::ProviderLimits::new("qwen", "Qwen", "qwen");
    out.configured = true;
    out.error = Some(if notes.is_empty() {
        "Cookie 没有查到 Coding Plan 用量".into()
    } else {
        notes.join("；")
    });
    out
}

fn coding_plan_windows(
    cookie: &str,
    region: &Console,
    sec_token: &str,
    anonymous: Option<&str>,
    csrf: Option<&str>,
) -> Option<super::ProviderLimits> {
    let body = console_form_api(region, sec_token, anonymous, ACTION);
    for base in region.rpcs {
        let url = format!("{base}&sec_token={}", form_escape(sec_token));
        let Ok(value) = post_form(&url, cookie, region, csrf, &body) else {
            continue;
        };
        if let Read::Ready(got) = read_body(&value) {
            if !got.windows.is_empty() {
                return Some(got);
            }
        }
    }
    None
}

fn personal_plan(
    cookie: &str,
    region: &Console,
    sec_token: &str,
    anonymous: Option<&str>,
    csrf: Option<&str>,
) -> std::result::Result<super::ProviderLimits, String> {
    let usage = post_named(cookie, region, sec_token, anonymous, csrf, PERSONAL_USAGE_API)?;
    let mut got = parse_quota(&usage);
    if let Ok(sub) = post_named(
        cookie,
        region,
        sec_token,
        anonymous,
        csrf,
        PERSONAL_SUBSCRIPTION_API,
    ) {
        merge_subscription(&mut got, &sub);
    }
    if got.windows.is_empty() && got.plan_label.is_none() {
        return Err(failure_hint(&usage));
    }
    got.error = None;
    Ok(got)
}

fn post_named(
    cookie: &str,
    region: &Console,
    sec_token: &str,
    anonymous: Option<&str>,
    csrf: Option<&str>,
    api: &str,
) -> std::result::Result<Value, String> {
    let body = console_form_api(region, sec_token, anonymous, api);
    let mut last = String::new();
    for base in region.rpcs {
        let url = format!(
            "{base}&sec_token={}",
            form_escape(sec_token)
        );
        // 个人版接口的 api 在 query 上,和表单里的 Api 要是同一个。
        let url = replace_api_query(&url, api);
        match post_form(&url, cookie, region, csrf, &body) {
            Ok(value) => return Ok(value),
            Err(err) => last = err,
        }
    }
    Err(last)
}

/// `rpcs` 里写死的是 Coding Plan 的 api 名。个人版要换成自己的。
fn replace_api_query(url: &str, api: &str) -> String {
    let mut kept = Vec::new();
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    for pair in query.split('&') {
        if pair.is_empty() || pair.starts_with("api=") || pair.starts_with("sec_token=") {
            continue;
        }
        kept.push(pair.to_string());
    }
    kept.push(format!("api={}", form_escape(api)));
    if let Some(token) = url.split("sec_token=").nth(1) {
        let token = token.split('&').next().unwrap_or(token);
        kept.push(format!("sec_token={token}"));
    }
    format!("{path}?{}", kept.join("&"))
}

fn post_form(
    url: &str,
    cookie: &str,
    region: &Console,
    csrf: Option<&str>,
    body: &str,
) -> std::result::Result<Value, String> {
    super::http::fetch_json(|agent| {
        let mut req = agent
            .post(url)
            .set("Accept", "*/*")
            .set("Content-Type", "application/x-www-form-urlencoded")
            .set("Cookie", cookie)
            .set("Origin", region.origin)
            .set("Referer", region.referer)
            .set("User-Agent", BROWSER_UA)
            .set("X-Requested-With", "XMLHttpRequest");
        if let Some(csrf) = csrf {
            req = req.set("x-xsrf-token", csrf).set("x-csrf-token", csrf);
        }
        req.send_string(body)
    })
    .map_err(|e| match e.status() {
        Some(401) | Some(403) => "控制台登录已失效".to_string(),
        Some(code) => format!("http {code}"),
        None => e.message(),
    })
}

fn resolve_sec_token(cookie: &str, region: &Console) -> std::result::Result<String, String> {
    // 页面里的 SEC_TOKEN 才是这次 RPC 要的。Cookie 里同名的值经常不是它,不能优先用。
    if let Ok(html) = super::http::fetch_text(|agent| {
        agent
            .get(region.dashboard)
            .set("Cookie", cookie)
            .set("User-Agent", BROWSER_UA)
            .set(
                "Accept",
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            )
            .set("Accept-Language", "zh-CN,zh;q=0.9")
            .set("Referer", &format!("{}/", region.origin))
            .set("Sec-Fetch-Dest", "document")
            .set("Sec-Fetch-Mode", "navigate")
            .set("Sec-Fetch-Site", "same-origin")
            .call()
    }) {
        if let Some(token) = extract_sec_token(&html) {
            return Ok(token);
        }
    }
    if let Ok(text) = super::http::fetch_text(|agent| {
        agent
            .get(region.info_url)
            .set("Cookie", cookie)
            .set("User-Agent", BROWSER_UA)
            .set("Accept", "application/json, text/plain, */*")
            .set("Referer", &format!("{}/", region.origin))
            .call()
    }) {
        if let Ok(value) = serde_json::from_str::<Value>(&text) {
            if let Some(token) = find_sec_token(&value) {
                return Ok(token);
            }
        }
    }
    if let Some(token) = cookie_value(cookie, "sec_token") {
        return Ok(token);
    }
    Err("没有从控制台页面读到 sec_token。请重新登录 Coding Plan 页,从网络面板复制完整请求头 Cookie,不要用 document.cookie。".into())
}

fn console_form(region: &Console, sec_token: &str, anonymous: Option<&str>) -> String {
    console_form_api(region, sec_token, anonymous, ACTION)
}

fn console_form_api(
    region: &Console,
    sec_token: &str,
    anonymous: Option<&str>,
    api: &str,
) -> String {
    let mut cornerstone = serde_json::json!({
        "feTraceId": uuid_like(),
        "feURL": region.fe_url,
        "protocol": "V2",
        "console": "ONE_CONSOLE",
        "productCode": "p_efm",
        "domain": region.domain,
        "consoleSite": region.site,
        "userNickName": "",
        "userPrincipalName": "",
        "xsp_lang": "zh-CN",
    });
    if let Some(id) = anonymous {
        if let Some(obj) = cornerstone.as_object_mut() {
            obj.insert("X-Anonymous-Id".into(), Value::String(id.to_string()));
        }
    }
    let params = serde_json::json!({
        "Api": api,
        "V": "1.0",
        "Data": {
            "queryCodingPlanInstanceInfoRequest": {
                "commodityCode": region.commodity,
                "onlyLatestOne": true,
            },
            "cornerstoneParam": cornerstone,
        }
    });
    let params = params.to_string();
    // 和能跑通的控制台请求对齐:product/action 放在表单里,不只放在 URL 上。
    // 网关只读 body 时,URL 上的 action 等于没带,于是回一个 code=200 的空壳。
    format!(
        "product={}&action={}&region={}&language=zh-CN&params={}&sec_token={}",
        form_escape("sfm_bailian"),
        form_escape(region.action),
        form_escape(region.region_id),
        form_escape(&params),
        form_escape(sec_token)
    )
}

fn uuid_like() -> String {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{n:032x}")
}

fn form_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn normalize_cookie(raw: &str) -> String {
    let mut text = raw.trim();
    if let Some(rest) = text.strip_prefix("Cookie:") {
        text = rest.trim();
    }
    text.trim_matches('"').trim().to_string()
}

fn cookie_value(header: &str, name: &str) -> Option<String> {
    header.split(';').find_map(|part| {
        let (k, v) = part.trim().split_once('=')?;
        (k.trim() == name).then(|| v.trim().to_string()).filter(|s| !s.is_empty())
    })
}

fn extract_sec_token(html: &str) -> Option<String> {
    for key in ["SEC_TOKEN", "secToken", "sec_token"] {
        let mut search = html;
        while let Some(idx) = search.find(key) {
            let mut after = &search[idx + key.len()..];
            after = after.trim_start_matches(['"', '\'']);
            after = after.trim_start();
            if let Some(rest) = after.strip_prefix(':') {
                if let Some(token) = quoted(rest.trim_start()) {
                    if !token.is_empty() {
                        return Some(token);
                    }
                }
            }
            search = &search[idx + key.len()..];
        }
    }
    None
}

fn quoted(s: &str) -> Option<String> {
    let quote = s.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let rest = &s[quote.len_utf8()..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_string())
}

fn find_sec_token(value: &Value) -> Option<String> {
    match value {
        Value::Object(obj) => {
            for key in ["secToken", "sec_token", "SEC_TOKEN"] {
                if let Some(text) = obj.get(key).and_then(Value::as_str) {
                    let text = text.trim();
                    if !text.is_empty() {
                        return Some(text.to_string());
                    }
                }
            }
            obj.values().find_map(find_sec_token)
        }
        Value::Array(items) => items.iter().find_map(find_sec_token),
        Value::String(text) => serde_json::from_str::<Value>(text).ok().and_then(|v| find_sec_token(&v)),
        _ => None,
    }
}

fn resolve_key(credential_name: &str) -> Option<String> {
    if let Some(key) = super::credential::resolve(credential_name) {
        return Some(key);
    }
    // 内置账号再认 Qwen Code 文档里的环境变量名。额外账号只用自己的条目。
    if credential_name == CREDENTIAL_NAME {
        for name in ["BAILIAN_CODING_PLAN_API_KEY", "ALIBABA_CODING_PLAN_API_KEY"] {
            if let Some(key) = super::credential::resolve(name) {
                return Some(key);
            }
        }
    }
    None
}

fn request(key: &str, region: &Region) -> std::result::Result<Value, super::http::HttpFailure> {
    let url = format!(
        "https://{}/data/api.json?action={ACTION}&product=broadscope-bailian&api=queryCodingPlanInstanceInfoV2&currentRegionId={}",
        region.host, region.region_id
    );
    let body = serde_json::json!({
        "queryCodingPlanInstanceInfoRequest": { "commodityCode": region.commodity }
    });
    super::http::fetch_json(|agent| {
        agent
            .post(&url)
            .set("Accept", "application/json")
            .set("Authorization", &format!("Bearer {key}"))
            .set("x-api-key", key)
            .set("X-DashScope-API-Key", key)
            .set("Origin", &format!("https://{}", region.host))
            .set("Referer", region.referer)
            .send_json(body.clone())
    })
}

enum Read {
    Ready(super::ProviderLimits),
    Retry(String),
}

const LOGIN_HINT: &str = "控制台未登录。请在网络面板里筛选 bailian-cs.console.aliyun.com,复制那条请求的 Cookie。从页面文档(bailian.console.aliyun.com)复制的 Cookie 会被这个网关拒绝。";

fn read_body(body: &Value) -> Read {
    if let Some(err) = gateway_failure(body) {
        return Read::Retry(err);
    }
    if layers(body).iter().any(needs_login) {
        return Read::Retry(LOGIN_HINT.into());
    }
    if layers(body).iter().any(auth_rejected) {
        return Read::Retry("API Key 无效".into());
    }
    let got = parse_quota(body);
    if got.windows.is_empty() && got.plan_label.is_none() {
        return Read::Retry(failure_hint(body));
    }
    Read::Ready(got)
}

/// 外层经常是 `"code":"200"`,真正的失败在内层 `success:false` + `errorCode`。
fn gateway_failure(body: &Value) -> Option<String> {
    for obj in layers(body) {
        let failed = obj.get("success").and_then(Value::as_bool) == Some(false)
            || obj.get("Success").and_then(Value::as_bool) == Some(false);
        if !failed && obj.get("errorCode").is_none() && obj.get("errorMsg").is_none() {
            continue;
        }
        if !failed && obj.get("errorCode").is_none() {
            continue;
        }
        let code = field_text(&obj, &["errorCode", "Code", "code"]);
        let msg = field_text(&obj, &["errorMsg", "Message", "message", "msg"]);
        if is_login_text(&code) || is_login_text(&msg) {
            return Some(LOGIN_HINT.into());
        }
        if failed {
            let shown = [code, msg]
                .into_iter()
                .filter(|s| !s.is_empty() && s != "200" && s != "0")
                .collect::<Vec<_>>()
                .join(": ");
            if !shown.is_empty() {
                return Some(shown);
            }
        }
    }
    None
}

fn field_text(obj: &Map<String, Value>, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| obj.get(*key).and_then(Value::as_str))
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn is_login_text(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("needlogin")
        || text.contains("notlogined")
        || text.contains("未登录")
        || text.contains("请求已经过期")
}

/// 没解析出用量时,带上响应里的 code/message 和顶层键,方便看出是没登录还是字段又变了。
/// 不带任何字段的值以外的长文本,避免把 Cookie 或 Key 漏进界面。
fn failure_hint(body: &Value) -> String {
    let mut bits = Vec::new();
    for obj in layers(body) {
        if let Some(code) = obj.get("code").and_then(Value::as_str) {
            let code = code.trim();
            if !code.is_empty()
                && code != "200"
                && code != "0"
                && code.len() < 80
                && !bits.iter().any(|b: &String| b == code)
            {
                bits.push(code.to_string());
            }
        }
        for key in ["errorCode", "errorMsg", "message", "msg", "statusMessage"] {
            if let Some(msg) = obj.get(key).and_then(Value::as_str) {
                let msg = msg.trim();
                if msg.is_empty() || msg.len() > 160 || msg.to_ascii_lowercase().contains("sk-") {
                    continue;
                }
                if !bits.iter().any(|b: &String| b == msg) {
                    bits.push(msg.to_string());
                }
            }
        }
    }
    if bits.is_empty() {
        let keys = body
            .as_object()
            .map(|o| o.keys().cloned().collect::<Vec<_>>().join(", "))
            .unwrap_or_default();
        format!("响应里没有 Coding Plan 用量(顶层键: {keys})")
    } else {
        format!("响应里没有 Coding Plan 用量({})", bits.join(" / "))
    }
}

/// 剥开百炼 RPC 的信封,并顺着每个字段往下走。内层经常再套一层 JSON 字符串。
fn layers(root: &Value) -> Vec<Map<String, Value>> {
    let mut out = Vec::new();
    fn walk(out: &mut Vec<Map<String, Value>>, value: &Value, depth: usize) {
        if depth > 8 {
            return;
        }
        let Some(obj) = as_object(value) else {
            return;
        };
        let children: Vec<Value> = obj.values().cloned().collect();
        out.push(obj);
        for child in children {
            walk(out, &child, depth + 1);
        }
    }
    walk(&mut out, root, 0);
    out
}

fn as_object(value: &Value) -> Option<Map<String, Value>> {
    if let Some(obj) = value.as_object() {
        return Some(obj.clone());
    }
    let text = value.as_str()?;
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| v.as_object().cloned())
}

fn needs_login(obj: &Map<String, Value>) -> bool {
    let code = obj
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let message = ["message", "msg", "statusMessage"]
        .iter()
        .find_map(|key| obj.get(*key).and_then(Value::as_str))
        .unwrap_or_default()
        .to_ascii_lowercase();
    code.contains("needlogin")
        || code.contains("notlogined")
        || message.contains("notlogined")
        || message.contains("needlogin")
        || message.contains("未登录")
}

fn auth_rejected(obj: &Map<String, Value>) -> bool {
    if let Some(n) = obj.get("statusCode").and_then(as_f64) {
        let code = n as u16;
        if code == 401 || code == 403 {
            return true;
        }
    }
    let code = obj
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    code.contains("unauthor") || code.contains("invalidcredential")
}

/// 从响应里取出套餐名和三个用量窗口。没有数字就不造窗口。
pub fn parse_quota(body: &Value) -> super::ProviderLimits {
    let mut out = super::ProviderLimits::new("qwen", "Qwen", "qwen");
    out.configured = true;

    let layers = layers(body);
    let instances = layers.iter().find_map(|obj| {
        obj.get("codingPlanInstanceInfos")
            .or_else(|| obj.get("coding_plan_instance_infos"))
            .and_then(Value::as_array)
    });

    let selected = instances.and_then(|items| pick_instance(items));
    if let Some(inst) = selected {
        out.plan_label = plan_name(inst);
        if let Some(quota) = quota_info(inst) {
            push_windows(&mut out.windows, quota);
        }
    }
    if out.windows.is_empty() {
        if let Some(quota) = layers.iter().find_map(quota_info) {
            push_windows(&mut out.windows, quota);
        }
    }
    // 新版个人网关不给 used/total,只给 0~1 的百分比。
    if out.windows.is_empty() {
        if let Some(obj) = layers.iter().find(|obj| {
            obj.contains_key("per5HourPercentage")
                || obj.contains_key("per1WeekPercentage")
                || obj.contains_key("per1MonthPercentage")
        }) {
            push_percentage_windows(&mut out.windows, obj);
        }
    }
    if out.plan_label.is_none() {
        out.plan_label = layers.iter().find_map(plan_name);
    }
    if out.windows.is_empty() && out.plan_label.is_some() {
        out.error = Some("套餐已识别,但响应里没有 5 小时/每周/每月用量".into());
    }
    out
}

fn pick_instance(items: &[Value]) -> Option<&Map<String, Value>> {
    let objs: Vec<&Map<String, Value>> = items.iter().filter_map(Value::as_object).collect();
    objs.iter()
        .copied()
        .find(|o| is_active(o))
        .or_else(|| objs.iter().copied().find(|o| !is_inactive(o)))
}

fn is_active(obj: &Map<String, Value>) -> bool {
    status_of(obj).is_some_and(|s| s == "ACTIVE" || s == "VALID")
}

fn is_inactive(obj: &Map<String, Value>) -> bool {
    status_of(obj).is_some_and(|s| {
        matches!(
            s.as_str(),
            "EXPIRED" | "INVALID" | "INACTIVE" | "DISABLED" | "TERMINATED" | "STOPPED"
        )
    })
}

fn status_of(obj: &Map<String, Value>) -> Option<String> {
    obj.get("status")
        .or_else(|| obj.get("instanceStatus"))
        .and_then(Value::as_str)
        .map(|s| s.trim().to_ascii_uppercase())
        .filter(|s| !s.is_empty())
}

fn plan_name(obj: &Map<String, Value>) -> Option<String> {
    ["planName", "instanceName", "packageName"]
        .iter()
        .find_map(|k| obj.get(*k))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn quota_info(obj: &Map<String, Value>) -> Option<&Map<String, Value>> {
    obj.get("codingPlanQuotaInfo")
        .or_else(|| obj.get("coding_plan_quota_info"))
        .and_then(Value::as_object)
        .or_else(|| obj.contains_key("per5HourTotalQuota").then_some(obj))
}

fn push_percentage_windows(out: &mut Vec<QuotaWindow>, quota: &Map<String, Value>) {
    push_percentage(
        out,
        quota,
        "five_hour",
        "5 小时",
        18_000,
        &["per5HourPercentage"],
        &["per5HourResetTime", "per5HourQuotaNextRefreshTime"],
    );
    push_percentage(
        out,
        quota,
        "weekly",
        "每周",
        604_800,
        &["per1WeekPercentage", "perWeekPercentage"],
        &["per1WeekResetTime", "perWeekResetTime", "perWeekQuotaNextRefreshTime"],
    );
    push_percentage(
        out,
        quota,
        "monthly",
        "每月",
        2_592_000,
        &["per1MonthPercentage", "perMonthPercentage", "perBillMonthPercentage"],
        &[
            "per1MonthResetTime",
            "perMonthResetTime",
            "perBillMonthResetTime",
            "perBillMonthQuotaNextRefreshTime",
        ],
    );
}

fn merge_subscription(out: &mut super::ProviderLimits, body: &Value) {
    let Some(obj) = layers(body).into_iter().find(|obj| {
        obj.contains_key("specCode") || obj.contains_key("instanceCode")
    }) else {
        return;
    };
    if out.plan_label.is_none() {
        if let Some(spec) = obj.get("specCode").and_then(Value::as_str) {
            let spec_lower = spec.to_ascii_lowercase();
            let name = match spec_lower.as_str() {
                "essential" => "Essential",
                "lite" => "Lite",
                "standard" => "Standard",
                "pro" => "Pro",
                other => other,
            };
            let instance = obj.get("instanceCode").and_then(Value::as_str).unwrap_or("");
            out.plan_label = Some(if instance.contains("tokenplansolo") {
                format!("Solo {name}")
            } else if instance.contains("codingplan") {
                name.to_string()
            } else {
                name.to_string()
            });
        }
    }
    if let Some(end) = obj.get("endTime").and_then(reset_ms) {
        if !out.windows.iter().any(|w| w.key == "plan") {
            out.windows
                .push(QuotaWindow::new("plan", "套餐有效期").reset(Some(end)));
        }
    }
}

/// 样本里 0.10 表示 10%。大于 1 的数当成已经是百分数。
fn push_percentage(
    out: &mut Vec<QuotaWindow>,
    quota: &Map<String, Value>,
    key: &str,
    label: &str,
    seconds: i64,
    percent_keys: &[&str],
    reset_keys: &[&str],
) {
    let Some(raw) = first_number(quota, percent_keys) else {
        return;
    };
    let used = if raw <= 1.0 { raw * 100.0 } else { raw };
    let reset = reset_keys
        .iter()
        .find_map(|k| quota.get(*k).and_then(reset_ms));
    out.push(
        QuotaWindow::new(key, label)
            .percent(Some(used.clamp(0.0, 100.0)))
            .reset(reset)
            .window(Some(seconds)),
    );
}

fn push_windows(out: &mut Vec<QuotaWindow>, quota: &Map<String, Value>) {
    push_window(
        out,
        quota,
        "five_hour",
        "5 小时",
        18_000,
        &["per5HourUsedQuota", "perFiveHourUsedQuota"],
        &["per5HourTotalQuota", "perFiveHourTotalQuota"],
        &[
            "per5HourQuotaNextRefreshTime",
            "perFiveHourQuotaNextRefreshTime",
        ],
    );
    push_window(
        out,
        quota,
        "weekly",
        "每周",
        604_800,
        &["perWeekUsedQuota"],
        &["perWeekTotalQuota"],
        &["perWeekQuotaNextRefreshTime"],
    );
    push_window(
        out,
        quota,
        "monthly",
        "每月",
        2_592_000,
        &["perBillMonthUsedQuota", "perMonthUsedQuota"],
        &["perBillMonthTotalQuota", "perMonthTotalQuota"],
        &[
            "perBillMonthQuotaNextRefreshTime",
            "perMonthQuotaNextRefreshTime",
        ],
    );
}

/// 有总量才生成窗口。已用量缺失时百分比是 `None`(界面显示 --),不能当成 0。
fn push_window(
    out: &mut Vec<QuotaWindow>,
    quota: &Map<String, Value>,
    key: &str,
    label: &str,
    seconds: i64,
    used_keys: &[&str],
    total_keys: &[&str],
    reset_keys: &[&str],
) {
    let Some(total) = first_number(quota, total_keys).filter(|n| n.is_finite() && *n > 0.0) else {
        return;
    };
    let percent = first_number(quota, used_keys).map(|used| (used.max(0.0) / total * 100.0).clamp(0.0, 100.0));
    let reset = reset_keys.iter().find_map(|k| quota.get(*k).and_then(reset_ms));
    out.push(
        QuotaWindow::new(key, label)
            .percent(percent)
            .reset(reset)
            .window(Some(seconds)),
    );
}

fn first_number(obj: &Map<String, Value>, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|k| obj.get(*k).and_then(as_f64))
}

fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64().filter(|x| x.is_finite()),
        Value::String(s) => s.trim().parse::<f64>().ok().filter(|x| x.is_finite()),
        _ => None,
    }
}

fn reset_ms(v: &Value) -> Option<i64> {
    if let Some(n) = as_f64(v) {
        return epoch_ms(n);
    }
    let text = v.as_str()?.trim();
    if let Ok(n) = text.parse::<f64>() {
        return epoch_ms(n);
    }
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|d| d.timestamp_millis())
}

fn epoch_ms(n: f64) -> Option<i64> {
    if !n.is_finite() || n <= 0.0 {
        return None;
    }
    if n >= 1_000_000_000_000.0 {
        Some(n as i64)
    } else if n >= 1_000_000_000.0 {
        Some((n as i64).saturating_mul(1000))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_active_instance_and_ignores_the_expired_one() {
        let body = serde_json::json!({
            "data": {
                "codingPlanInstanceInfos": [
                    {"status":"EXPIRED","planName":"Old","codingPlanQuotaInfo":{
                        "per5HourUsedQuota": 99, "per5HourTotalQuota": 100
                    }},
                    {"status":"ACTIVE","planName":"Pro","codingPlanQuotaInfo":{
                        "per5HourUsedQuota":"1200","per5HourTotalQuota":"6000",
                        "per5HourQuotaNextRefreshTime":"2030-01-01T00:00:00Z",
                        "perWeekUsedQuota": 9000, "perWeekTotalQuota": 45000,
                        "perWeekQuotaNextRefreshTime": 1893456000000_i64,
                        "perBillMonthUsedQuota": 1000, "perBillMonthTotalQuota": 90000,
                        "perBillMonthQuotaNextRefreshTime": 1893456000
                    }}
                ]
            }
        });
        let got = parse_quota(&body);
        assert_eq!(got.plan_label.as_deref(), Some("Pro"));
        assert_eq!(got.windows.len(), 3);
        assert_eq!(got.windows[0].key, "five_hour");
        assert!((got.windows[0].used_percent.unwrap() - 20.0).abs() < 1e-9);
        assert_eq!(got.windows[0].reset_at, Some(1_893_456_000_000));
        assert!((got.windows[1].used_percent.unwrap() - 20.0).abs() < 1e-9);
        assert_eq!(got.windows[1].reset_at, Some(1_893_456_000_000));
        assert!((got.windows[2].used_percent.unwrap() - (1000.0 / 90000.0 * 100.0)).abs() < 1e-6);
        assert_eq!(got.error, None);
    }

    /// 百炼有时把真正的 body 再套成 JSON 字符串
    #[test]
    fn parses_string_wrapped_success_body() {
        let inner = serde_json::json!({
            "codingPlanInstanceInfos": [{
                "status": "VALID",
                "planName": "Lite",
                "codingPlanQuotaInfo": {
                    "per5HourUsedQuota": 0,
                    "per5HourTotalQuota": 1000
                }
            }]
        });
        let body = serde_json::json!({
            "data": { "successResponse": { "body": inner.to_string() } }
        });
        let got = parse_quota(&body);
        assert_eq!(got.plan_label.as_deref(), Some("Lite"));
        assert_eq!(got.windows.len(), 1);
        assert_eq!(got.windows[0].used_percent, Some(0.0));
    }

    /// 只有总量、没有已用量:不能显示成 0%
    #[test]
    fn missing_used_count_is_not_zero() {
        let body = serde_json::json!({
            "codingPlanQuotaInfo": { "per5HourTotalQuota": 6000 }
        });
        let got = parse_quota(&body);
        assert_eq!(got.windows.len(), 1);
        assert_eq!(got.windows[0].used_percent, None);
    }

    #[test]
    fn console_login_requirement_is_a_retry_not_a_fake_quota() {
        let body = serde_json::json!({"data":{"code":"ConsoleNeedLogin"}});
        assert!(matches!(read_body(&body), Read::Retry(_)));
        assert!(parse_quota(&body).windows.is_empty());
    }

    /// 外层 code=200,真正的失败在内层 success:false。不能报成「没有用量(200)」。
    #[test]
    fn nested_not_logined_is_not_reported_as_empty_usage() {
        let body = serde_json::json!({
            "code": "200",
            "successResponse": true,
            "httpStatusCode": "200",
            "data": {
                "success": false,
                "errorCode": "BailianGateway.Login.NotLogined",
                "errorMsg": "BailianGateway.Login.NotLogined"
            }
        });
        match read_body(&body) {
            Read::Retry(msg) => {
                assert!(msg.contains("bailian-cs"), "{msg}");
                assert!(!msg.contains("没有 Coding Plan 用量"), "{msg}");
            }
            Read::Ready(_) => panic!("未登录不该被当成有用量"),
        }
    }

    #[test]
    fn percentage_payload_becomes_used_percent() {
        let body = serde_json::json!({
            "code": "200",
            "data": {
                "per5HourPercentage": 0.2,
                "per5HourResetTime": 1893456000000_i64,
                "per1WeekPercentage": 0.1,
                "per1WeekResetTime": 1893456000000_i64
            }
        });
        let got = parse_quota(&body);
        assert_eq!(got.windows.len(), 2);
        assert!((got.windows[0].used_percent.unwrap() - 20.0).abs() < 1e-9);
        assert!((got.windows[1].used_percent.unwrap() - 10.0).abs() < 1e-9);
    }

    /// 网关未登录时 message 是 BailianGateway.Login.NotLogined,code 往往不是 NeedLogin
    #[test]
    fn not_logined_message_is_treated_as_login_failure() {
        let body = serde_json::json!({
            "successResponse": true,
            "message": "BailianGateway.Login.NotLogined"
        });
        match read_body(&body) {
            Read::Retry(msg) => assert!(msg.contains("登录"), "{msg}"),
            Read::Ready(_) => panic!("未登录不该被当成有用量"),
        }
    }

    /// 个人版 Token Plan:套餐实例列表是空的,用量在 DataV2 里,只有月窗口。
    #[test]
    fn personal_token_plan_month_window_and_plan_name() {
        let usage = serde_json::json!({
            "code": "200",
            "data": { "DataV2": { "data": { "data": {
                "per1MonthPercentage": 0.0,
                "per1MonthResetTime": 1792771200000_i64
            }, "success": true } } },
            "successResponse": true
        });
        let sub = serde_json::json!({
            "data": { "DataV2": { "data": { "data": {
                "specCode": "essential",
                "instanceCode": "sfm_tokenplansolo_public_cn-example",
                "status": "VALID",
                "endTime": 1792771200000_i64,
                "remainingDays": 30
            } } } }
        });
        let mut got = parse_quota(&usage);
        merge_subscription(&mut got, &sub);
        assert_eq!(got.plan_label.as_deref(), Some("Solo Essential"));
        let month = got.windows.iter().find(|w| w.key == "monthly").expect("每月");
        assert_eq!(month.used_percent, Some(0.0));
        assert!(got.windows.iter().any(|w| w.key == "plan"));
    }

    #[test]
    fn sec_token_is_read_from_the_console_shell() {
        let html = r#"window.ALIYUN_CONSOLE_CONFIG = { SEC_TOKEN: "tok-1", other: 1 }"#;
        assert_eq!(extract_sec_token(html).as_deref(), Some("tok-1"));
        let quoted = r#"{"sec_token":"tok-2"}"#;
        assert_eq!(extract_sec_token(quoted).as_deref(), Some("tok-2"));
    }

    #[test]
    fn cookie_header_yields_one_named_value() {
        let header = "cna=abc; login_aliyunid_csrf=csrf1; other=x";
        assert_eq!(cookie_value(header, "cna").as_deref(), Some("abc"));
        assert_eq!(cookie_value(header, "login_aliyunid_csrf").as_deref(), Some("csrf1"));
        assert_eq!(normalize_cookie("Cookie: a=b"), "a=b");
    }

    #[test]
    fn console_form_carries_commodity_and_token() {
        let body = console_form(&CN_CONSOLE, "sec/tok", Some("anon"));
        assert!(body.contains("sec_token=sec%2Ftok"), "{body}");
        assert!(body.contains("sfm_codingplan_public_cn"), "{body}");
        assert!(body.starts_with("product=sfm_bailian&action=BroadScopeAspnGateway"), "{body}");
    }

    /// 控制台网关把实例嵌在 data.DataV2.data.data 里
    #[test]
    fn parses_datav2_envelope() {
        let body = serde_json::json!({
            "code": "200",
            "data": { "DataV2": { "data": { "data": {
                "codingPlanInstanceInfos": [{
                    "status": "VALID",
                    "planName": "Pro",
                    "codingPlanQuotaInfo": {
                        "per5HourUsedQuota": 30,
                        "per5HourTotalQuota": 6000
                    }
                }]
            }}}}
        });
        let got = parse_quota(&body);
        assert_eq!(got.plan_label.as_deref(), Some("Pro"));
        assert_eq!(got.windows.len(), 1);
        assert!((got.windows[0].used_percent.unwrap() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn plan_without_counters_is_reported_instead_of_invented() {
        let body = serde_json::json!({
            "data": { "codingPlanInstanceInfos": [{"status":"ACTIVE","planName":"Pro"}] }
        });
        let got = parse_quota(&body);
        assert_eq!(got.plan_label.as_deref(), Some("Pro"));
        assert!(got.windows.is_empty());
        assert!(got.error.as_deref().unwrap().contains("没有"));
    }
}
