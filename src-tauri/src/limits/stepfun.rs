//! StepFun:公开按量余额 + 控制台 RPC 订阅额度。
//!
//! 两条独立的数据来源:
//!
//! 1. **按量余额**:`GET https://api.stepfun.com/v1/accounts`,吃 `STEPFUN_API_KEY`。
//!    实测未带 key 时返回 401 `Incorrect API key provided` —— 端点确实存在。
//! 2. **订阅额度**:控制台 RPC(`GetStepPlanStatus` / `QueryStepPlanRateLimit`),
//!    吃的是**控制台会话凭据**(浏览器里的 Cookie),不是 API key。
//!    实测三个端点都真实存在:前两个无 token 返回 401 `auth failed: token is missing`,
//!    `RefreshToken` 返回 400 `oasis header is invalid`。
//!
//! ⚠️ 第二条是**逆向接口**,官方可能随时改动。所以:
//! - 默认关闭,用户显式勾选才启用;
//! - 失败只影响这一张卡,不牵连其它 Provider;
//! - 续期后的 token **只留在内存**,不写回任何文件。

use serde_json::Value;

use super::{Balance, ProviderLimits, QuotaWindow};

pub const ACCOUNTS_URL: &str = "https://api.stepfun.com/v1/accounts";

/// 控制台 RPC 的基址。
///
/// **注意服务名**:控制台是 ConnectRPC(protobuf over HTTP),路径形如
/// `/api/<package>.<Service>/<Method>`。早先这里写的是 `/api/v1/<Method>`,
/// 实测已经 **404**(控制台改过一轮),正确的服务是 `Dashboard`。
pub const CONSOLE_RPC_BASE: &str = "https://platform.stepfun.com/api/step.openapi.devcenter.Dashboard";

/// 订阅额度:窗口与 Credit 余量
/// Passport(登录态)服务 —— 用 journal 换新令牌
pub const PASSPORT_BASE: &str =
    "https://platform.stepfun.com/passport/proto.api.passport.v1.PassportService";

/// 控制台自己的 appID(实测 10300;海外站是 20700)
pub const PLAN_APP_ID: &str = "10300";

pub const RPC_RATE_LIMIT: &str = "QueryStepPlanRateLimit";
/// 订阅信息:套餐名与到期时间
pub const RPC_PLAN_STATUS: &str = "GetStepPlanStatus";

pub const API_KEY_NAME: &str = "STEPFUN_API_KEY";
pub const COOKIE_NAME: &str = "STEPFUN_CONSOLE_COOKIE";
pub const TOKEN_NAME: &str = "STEPFUN_CONSOLE_TOKEN";

fn as_f64(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn as_i64(v: Option<&Value>) -> Option<i64> {
    match v? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    }
}

/// 解析用户粘贴的凭据:整段 cURL、裸 Cookie 串、裸 JWT 都要能认。
///
/// 用户是从浏览器开发者工具里复制,粘出来什么形状都有可能;
/// 与其让他按我们的格式重排,不如把三种常见形状都认了。
pub fn parse_credential(raw: &str) -> String {
    let text = raw.trim();
    // 1) 整段 cURL:-H 'cookie: xxx' / --cookie xxx / -b xxx
    for flag in ["-H", "--header", "-b", "--cookie"] {
        if let Some(idx) = text.find(flag) {
            let rest = &text[idx + flag.len()..];
            let rest = rest.trim_start_matches([' ', '=', ':']);
            let quote = rest.chars().next().filter(|c| *c == '"' || *c == '\'');
            let body = match quote {
                Some(q) => rest[1..].split(q).next().unwrap_or(""),
                None => rest.split_whitespace().next().unwrap_or(""),
            };
            let body = body
                .strip_prefix("cookie:")
                .or_else(|| body.strip_prefix("Cookie:"))
                .unwrap_or(body)
                .trim();
            if !body.is_empty() {
                return body.to_string();
            }
        }
    }
    // 2) "Cookie: xxx" 前缀
    if let Some(rest) = text
        .strip_prefix("Cookie:")
        .or_else(|| text.strip_prefix("cookie:"))
    {
        return rest.trim().to_string();
    }
    // 3) 裸串(Cookie 或 JWT)原样用
    text.to_string()
}

/// 从 cookie 串里挑出 token 值。
/// 找不到就返回 None。
pub fn token_from_cookie(cookie: &str) -> Option<String> {
    for part in cookie.split(';') {
        let part = part.trim();
        let Some((name, value)) = part.split_once('=') else {
            continue;
        };
        let name = name.trim().to_ascii_lowercase();
        if name.contains("token") || name.contains("auth") {
            let v = value.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// 解析 base64url 的 JWT payload(不校验签名,只读字段)。
fn jwt_payload(jwt: &str) -> Option<Value> {
    use base64::Engine as _;
    let part = jwt.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(part)
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// 控制台会话的 `Oasis-Token` 值是**复合 journal**:`<访问令牌JWT>...<刷新令牌JWT>`。
///
/// **字面的三个点是格式的一部分**(对应 passport 的 `journal_b64`),不是装饰。
/// 这一点踩过坑:早先把它当"从聊天记录里粘进来的省略号"剥掉,只发前半段,
/// 服务端一律回 `token is illegal`;整条原样发才 200。
///
/// 只有一个 JWT 时(用户在控制台请求头里复制的 `oasis-token`)也接受。
pub fn extract_journal(raw: &str) -> Option<String> {
    let text = parse_credential(raw);
    // 从 `Oasis-Token=...` 里取值
    let value = text
        .split(';')
        .find_map(|p| {
            let (n, v) = p.trim().split_once('=')?;
            let n = n.trim().to_ascii_lowercase();
            (n == "oasis-token" || n == "oasis_token").then(|| v.trim().to_string())
        })
        .unwrap_or_else(|| text.trim().to_string());

    // journal = 两段 JWT 用 `...` 连接。判据:按 `...` 切开后每段都是三段式 JWT。
    let parts: Vec<&str> = value.split("...").collect();
    if parts.len() == 2
        && parts
            .iter()
            .all(|p| p.split('.').count() == 3 && !p.trim().is_empty())
    {
        return Some(format!("{}...{}", parts[0].trim(), parts[1].trim()));
    }
    // 单个 JWT 也接受
    if value.split('.').count() == 3 && !value.trim().is_empty() {
        return Some(value.trim().to_string());
    }
    None
}

/// 从 journal 里挑出 `device_id`(用作 `Oasis-Webid` / `Oasis-Did`)。
///
/// 参考实现就是这么做的:遍历 journal 的每一段,取第一个带 `device_id` 的。
pub fn journal_device_id(journal: &str) -> Option<String> {
    for part in journal.split("...") {
        if let Some(d) = jwt_payload(part) {
            if let Some(id) = d.get("device_id").and_then(Value::as_str) {
                if !id.is_empty() {
                    return Some(id.to_string());
                }
            }
        }
    }
    None
}

/// 凭据自检:把服务端那句含糊的 401 提前成本地可读的提示。
pub fn check_credential(raw: &str) -> std::result::Result<(), String> {
    match extract_journal(raw) {
        Some(j) => {
            if j.contains("...") {
                Ok(())
            } else {
                Err(
                    "凭据里只有一个 JWT(没有刷新令牌),约 30 分钟就会过期。\
                     建议在控制台用「以 cURL 格式复制」整段粘贴,这样含刷新令牌、能自动续期。"
                        .into(),
                )
            }
        }
        None => Err(
            "凭据里没找到 Oasis-Token。请在控制台 F12 → Network 里右键任一请求 →\
             Copy as cURL,把整段粘进来(含 Cookie 头即可)"
                .into(),
        ),
    }
}

/// 把 `*_left_rate` 转成"已用百分比"。
///
/// **字段是"剩余率"不是"已用率"** —— 名字里的 `left` 很关键,直接当已用会
/// 得到完全相反的结论(比如实际用了一半,却显示"剩 50% 已用 50%"这种巧合,
/// 而用了 10% 时会显示成"已用 90%")。
///
/// 比例口径按 0..1 处理;若上游某天改成百分数(`>1`),这里退化成除 100,
/// 免得整整差 100 倍。
fn used_percent_from_left_rate(v: f64) -> f64 {
    let left = if v > 1.0 { v / 100.0 } else { v };
    ((1.0 - left) * 100.0).clamp(0.0, 100.0)
}

/// 解析按量余额响应
pub fn parse_accounts(body: &Value) -> ProviderLimits {
    let mut out = ProviderLimits::new("stepfun", "StepFun", "stepfun");
    out.configured = true;
    // 常见形状:{"balance": {...}} 或 {"data": {...}} 或直接顶层
    let node = body
        .get("balance")
        .or_else(|| body.get("data"))
        .unwrap_or(body);
    let cash = as_f64(node.get("cash_balance"))
        .or_else(|| as_f64(node.get("cash")))
        .or_else(|| as_f64(node.get("topped_up_balance")));
    let voucher = as_f64(node.get("voucher_balance"))
        .or_else(|| as_f64(node.get("voucher")))
        .or_else(|| as_f64(node.get("granted_balance")));
    let total = as_f64(node.get("total_balance"))
        .or_else(|| as_f64(node.get("balance")))
        .or_else(|| match (cash, voucher) {
            (Some(c), Some(v)) => Some(c + v),
            (Some(c), None) => Some(c),
            (None, Some(v)) => Some(v),
            _ => None,
        });
    match total {
        Some(amount) => {
            out.balance = Some(Balance {
                amount,
                currency: node
                    .get("currency")
                    .and_then(Value::as_str)
                    .unwrap_or("CNY")
                    .to_string(),
                cash,
                voucher,
            });
        }
        None => out.error = Some("响应里没有余额字段".into()),
    }
    out
}

/// epoch 秒 → 毫秒(上游也可能直接给毫秒,按量级区分)。
///
/// **0 与负数视为"没有"**:上游在"无该窗口"时会填 `"0"`(protojson 把
/// int64 编成字符串),直接转出来就是 1970 年,UI 会显示一个荒唐的倒计时。
fn to_ms(s: i64) -> Option<i64> {
    if s <= 0 {
        return None;
    }
    Some(if s > 1_000_000_000_000 { s } else { s * 1000 })
}

/// 解析 `QueryStepPlanRateLimit` 的响应 —— **这是额度的主来源**。
///
/// 字段名来自控制台 JS 里的 protobuf 描述(不是猜的):
///
/// ```text
/// five_hour_usage_left_rate   2   (剩余率,0..1)
/// five_hour_usage_reset_time  3
/// weekly_usage_left_rate      5   (剩余率,0..1)
/// weekly_usage_reset_time     6
/// plan_family                10
/// plan_credit_rate_limit     11  → subscription_credit_left_rate / credit_buckets
/// ```
pub fn parse_rate_limit(body: &Value) -> ProviderLimits {
    let mut out = ProviderLimits::new("stepfun", "StepFun", "stepfun");
    out.configured = true;
    // ConnectRPC 的 JSON 编码直接平铺在顶层;兼容包一层 data 的情况
    let node = body.get("data").unwrap_or(body);

    // `plan_family` 决定该看哪一组字段:
    //   1 = CODING(旧套餐)→ 5 小时 / 每周 两个滚动窗口
    //   2 = TOKEN(新套餐)→ Credit 月池
    // 拿错组会显示一堆恒为 0/100 的假数字(实测见过 5 小时显示"已用 100%"),
    // 所以按 family 只取该看的那一组,而不是"有字段就显示"。
    let family = as_i64(node.get("plan_family"));

    let mut windows = Vec::new();
    if family != Some(2) {
        for (key, label, left_field, reset_field, secs) in [
            (
                "five_hour",
                "5 小时",
                "five_hour_usage_left_rate",
                "five_hour_usage_reset_time",
                18_000i64,
            ),
            (
                "weekly",
                "每周",
                "weekly_usage_left_rate",
                "weekly_usage_reset_time",
                604_800i64,
            ),
        ] {
            let left = as_f64(node.get(left_field));
            let reset = as_i64(node.get(reset_field)).and_then(to_ms);
            // 旧套餐才有意义;两者都没有就不显示
            if left.is_some() || reset.is_some() {
                windows.push(
                    QuotaWindow::new(key, label)
                        .percent(left.map(used_percent_from_left_rate))
                        .reset(reset)
                        .window(Some(secs)),
                );
            }
        }
    }

    // Credit 池只在 TOKEN 家族(family=2)有意义。CODING 家族里这些字段
    // 恒为 1.0(满额),显示出来就是一条永远"已用 0%"的假进度条。
    if family == Some(2) {
        if let Some(cl) = node.get("plan_credit_rate_limit") {
        // 套餐 Credit:比例与绝对量是同一件事的两种表示,只留比例那条,
        // 免得卡片上出现两行一模一样的百分比(实测见过)。
        let sub_left = as_f64(cl.get("subscription_credit_left_rate"));
        if sub_left.is_some() {
            let reset = as_i64(cl.get("subscription_credit_reset_time")).and_then(to_ms);
            windows.push(
                QuotaWindow::new("credit", "套餐 Credit")
                    .percent(sub_left.map(used_percent_from_left_rate))
                    .reset(reset),
            );
        }
        // 加油包是独立池,只在真有额度(比例 > 0)时显示
        if let Some(topup) = as_f64(cl.get("topup_credit_left_rate")) {
            if topup > 0.0 {
                windows.push(
                    QuotaWindow::new("topup", "加油包 Credit")
                        .percent(Some(used_percent_from_left_rate(topup))),
                );
            }
        }
        }
    }

    out.windows = windows;
    if out.windows.is_empty() {
        out.error = Some(format!(
            "订阅额度字段无法识别(逆向接口可能已变更)。响应键: {}",
            key_list(node)
        ));
    }
    out
}

/// 解析 `GetStepPlanStatus` 的响应 —— 只用来取**套餐名与到期时间**。
///
/// `subscription.name` 是套餐名(如 "Mini Plan"),`subscription.expired_at` 是到期。
pub fn parse_plan_status(body: &Value) -> ProviderLimits {
    let mut out = ProviderLimits::new("stepfun", "StepFun", "stepfun");
    out.configured = true;
    let node = body.get("data").unwrap_or(body);

    if let Some(sub) = node.get("subscription").filter(|v| !v.is_null()) {
        out.plan_label = sub
            .get("name")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string);
        if let Some(exp) = as_i64(sub.get("expired_at")).and_then(to_ms) {
            out.windows.push(
                QuotaWindow::new("plan", "套餐有效期")
                    .reset(Some(exp))
                    .window(None),
            );
        }
    }
    out
}

fn key_list(node: &Value) -> String {
    node.as_object()
        .map(|o| {
            o.keys()
                .map(String::as_str)
                .take(10)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

/// 合并两次 RPC 的结果:额度窗口来自 RateLimit,套餐名来自 PlanStatus。
pub fn merge_plan(a: ProviderLimits, b: ProviderLimits) -> ProviderLimits {
    let mut out = a;
    if out.plan_label.is_none() {
        out.plan_label = b.plan_label;
    }
    // 套餐有效期单独一个窗口,别和用量窗口混在一起
    for w in b.windows {
        if w.key == "plan" {
            out.windows.push(w);
        }
    }
    if out.windows.is_empty() {
        out.error = b.error.or(out.error);
    }
    out
}

/// 按量余额(key 模式)
pub fn fetch_balance() -> ProviderLimits {
    fetch_balance_with_credential(API_KEY_NAME)
}

/// 多账号:从指定条目名取 key
pub fn fetch_balance_with_credential(credential_name: &str) -> ProviderLimits {
    let Some(key) = super::credential::resolve(credential_name) else {
        let hint = if credential_name == API_KEY_NAME {
            "未配置 STEPFUN_API_KEY。填入后即可查看按量余额。"
        } else {
            "未配置 API Key。在设置里为该账号填入密钥即可查看按量余额。"
        };
        return ProviderLimits::unconfigured("stepfun", "StepFun", "stepfun", hint);
    };
    match super::http::fetch_json(|agent| {
        agent
            .get(ACCOUNTS_URL)
            .set("Authorization", &format!("Bearer {key}"))
            .set("Accept", "application/json")
            .call()
    }) {
        Ok(v) => parse_accounts(&v),
        Err(e) => {
            let mut out = ProviderLimits::new("stepfun", "StepFun", "stepfun");
            out.configured = true;
            out.error = Some(match e.status() {
                Some(401) => "API Key 无效".to_string(),
                _ => e.message(),
            });
            out
        }
    }
}

/// 给验收测试用的可见入口:暴露 journal 解析结果,便于排查"粘进来的到底是什么"。
pub fn debug_extract_journal(raw: &str) -> Option<String> {
    extract_journal(raw)
}

/// 控制台 RPC 的公共请求头。
///
/// 这几个头缺一不可(照参考实现 + 实测):
/// - `Oasis-Token`:整条 journal(`<访问>...<刷新>`)
/// - `Oasis-Webid` / `Oasis-Did`:同一个 device_id,两个名字都要带
/// - `Oasis-appID` / `Oasis-Platform`:控制台的身份标识
fn plan_headers(journal: &str, device_id: Option<&str>) -> Vec<(&'static str, String)> {
    let mut h = vec![
        ("Content-Type", "application/json".to_string()),
        ("Accept", "application/json".to_string()),
        ("Oasis-Platform", "web".to_string()),
        ("Oasis-appID", PLAN_APP_ID.to_string()),
        ("Oasis-Token", journal.to_string()),
    ];
    if let Some(id) = device_id {
        h.push(("Oasis-Webid", id.to_string()));
        h.push(("Oasis-Did", id.to_string()));
    }
    h
}

/// 调一个控制台 RPC。鉴权靠 `Oasis-Token` 头(整条 journal)。
fn call_rpc(method: &str, journal: &str, device_id: Option<&str>) -> std::result::Result<Value, String> {
    let url = format!("{CONSOLE_RPC_BASE}/{method}");
    let headers = plan_headers(journal, device_id);
    super::http::fetch_json(|agent| {
        let mut req = agent.post(&url);
        for (k, v) in &headers {
            req = req.set(k, v);
        }
        req.send_string("{}")
    })
    .map_err(|e| match e.status() {
        Some(401) | Some(403) => format!(
            "控制台凭据失效,请在设置里重贴 STEPFUN_CONSOLE_COOKIE({})",
            e.message()
        ),
        Some(404) => format!("接口不存在(控制台可能又改版了): {method}"),
        Some(code) => format!("{method}: http {code}"),
        None => format!("{method}: {}", e.message()),
    })
}

/// 用整条 journal 调 `RefreshToken` 换新令牌。
///
/// **关键**:必须用**同一条 journal**(两个 JWT 都在)去换,换回来的
/// `refreshToken.raw` 要拼成新 journal 继续用。只拿后半段(刷新令牌)单独去换
/// 会失败 —— 实测那样回 400 `oasis header is invalid`。
///
/// 新 journal **只留在内存**,不写回任何文件(那是浏览器的会话,不该由我们落盘)。
fn renew_journal(journal: &str, device_id: Option<&str>) -> Option<String> {
    let url = format!("{PASSPORT_BASE}/RefreshToken");
    let headers = plan_headers(journal, device_id);
    let body = super::http::fetch_json(|agent| {
        let mut req = agent.post(&url);
        for (k, v) in &headers {
            req = req.set(k, v);
        }
        req.send_string("{}")
    })
    .ok()?;
    let access = body.pointer("/accessToken/raw").and_then(Value::as_str)?;
    let refresh = body.pointer("/refreshToken/raw").and_then(Value::as_str);
    Some(match refresh {
        Some(r) if !r.is_empty() => format!("{access}...{r}"),
        _ => access.to_string(),
    })
}

/// 控制台订阅额度(需要浏览器里的 `Oasis-Token`)。
///
/// 两次 RPC:RateLimit 给窗口与 Credit,PlanStatus 给套餐名/到期。
/// 任一成功就出结果,两个都失败才报错(报第一条,便于定位)。
pub fn fetch_plan() -> ProviderLimits {
    fetch_plan_with_credential(COOKIE_NAME)
}

/// 多账号:从指定条目名取 Oasis-Token
pub fn fetch_plan_with_credential(credential_name: &str) -> ProviderLimits {
    let Some(raw) = super::credential::resolve(credential_name) else {
        let where_to = if credential_name == COOKIE_NAME {
            "STEPFUN_CONSOLE_COOKIE"
        } else {
            "该账号的控制台 Cookie"
        };
        return ProviderLimits::unconfigured(
            "stepfun",
            "StepFun",
            "stepfun",
            &format!(
                "订阅额度需要控制台凭据:浏览器登录 platform.stepfun.com,\
                 F12 → Network 里右键任一请求 → Copy as cURL,把整段粘到\
                 {where_to}(整段含刷新令牌,能自动续期)。\
                 (逆向接口,官方可能变更;失败只影响这一张卡)"
            ),
        );
    };
    // 抠不出 journal:没必要发出去换一句看不懂的 401,直接给指引
    let Some(mut journal) = extract_journal(&raw) else {
        let hint = check_credential(&raw)
            .err()
            .unwrap_or_else(|| "凭据里没找到 Oasis-Token".into());
        return ProviderLimits::unconfigured("stepfun", "StepFun", "stepfun", &hint);
    };
    // 只有单个 JWT(没刷新令牌)时提醒一句:约 30 分钟就会过期
    if let Err(hint) = check_credential(&raw) {
        eprintln!("[otr] StepFun 凭据提示: {hint}");
    }
    let mut device_id = journal_device_id(&journal);

    let mut rate = call_rpc(RPC_RATE_LIMIT, &journal, device_id.as_deref());
    let mut plan = call_rpc(RPC_PLAN_STATUS, &journal, device_id.as_deref());

    // 401 → 用同一条 journal 换新令牌,再试一次(照参考实现:只续一次)
    let expired = matches!(&rate, Err(e) if e.contains("凭据失效"))
        || matches!(&plan, Err(e) if e.contains("凭据失效"));
    if expired {
        match renew_journal(&journal, device_id.as_deref()) {
            Some(fresh) => {
                // 续期后的令牌可能换了 device_id,重新取一次
                if let Some(id) = journal_device_id(&fresh) {
                    device_id = Some(id);
                }
                journal = fresh;
                rate = call_rpc(RPC_RATE_LIMIT, &journal, device_id.as_deref());
                plan = call_rpc(RPC_PLAN_STATUS, &journal, device_id.as_deref());
            }
            None => eprintln!("[otr] StepFun 续期失败,凭据可能已失效"),
        }
    }

    match (rate, plan) {
        (Ok(r), Ok(p)) => merge_plan(parse_rate_limit(&r), parse_plan_status(&p)),
        (Ok(r), Err(_)) => parse_rate_limit(&r),
        (Err(e), Ok(p)) => {
            let mut out = parse_plan_status(&p);
            if out.windows.is_empty() {
                out.error = Some(e);
            }
            out
        }
        (Err(e), Err(_)) => {
            let mut out = ProviderLimits::new("stepfun", "StepFun", "stepfun");
            out.configured = true;
            out.error = Some(e);
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // 余额与订阅额度的合并逻辑在 mod.rs(它是账号编排的一部分)
    use crate::limits::combine_stepfun;

    /// 用户粘贴的形状三种都要能认
    #[test]
    fn credential_parsing_accepts_curl_cookie_and_bare_forms() {
        assert_eq!(
            parse_credential("curl 'https://x' -H 'cookie: a=1; b=2' --data '{}'"),
            "a=1; b=2"
        );
        assert_eq!(parse_credential("Cookie: a=1; b=2"), "a=1; b=2");
        assert_eq!(parse_credential("a=1; b=2"), "a=1; b=2");
        assert_eq!(
            parse_credential("curl 'https://x' -H \"cookie: a=1\""),
            "a=1"
        );
        // 裸 JWT 原样保留
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ4In0.sig";
        assert_eq!(parse_credential(jwt), jwt);
    }

    #[test]
    fn token_is_picked_out_of_the_cookie() {
        assert_eq!(
            token_from_cookie("session=x; oasis_token=abc123; other=y").as_deref(),
            Some("abc123")
        );
        assert_eq!(token_from_cookie("plain=1"), None);
    }

    /// 余额:充值池 + 赠送池,总额缺失时相加
    #[test]
    fn balance_splits_cash_and_voucher() {
        let body = serde_json::json!({
            "balance": { "currency": "CNY", "cash_balance": "30.00", "voucher_balance": "5.50" }
        });
        let b = parse_accounts(&body).balance.unwrap();
        assert!((b.amount - 35.5).abs() < 1e-9);
        assert_eq!(b.cash, Some(30.0));
        assert_eq!(b.voucher, Some(5.5));
    }

    /// 订阅额度:字段名取自控制台 protobuf 描述(不是猜的)。
    ///
    /// **`*_left_rate` 是"剩余率"**,必须换算成已用 —— 这是最容易搞反的一处。
    #[test]
    fn rate_limit_maps_left_rate_to_used_percent() {
        let body = serde_json::json!({
            "five_hour_usage_left_rate": 0.575,
            "five_hour_usage_reset_time": 1790000000,
            "weekly_usage_left_rate": 0.88,
            "weekly_usage_reset_time": 1790000000000i64
        });
        let got = parse_rate_limit(&body);
        assert_eq!(got.windows.len(), 2);
        // 剩 57.5% → 已用 42.5%
        assert_eq!(got.windows[0].key, "five_hour");
        assert!((got.windows[0].used_percent.unwrap() - 42.5).abs() < 1e-9);
        assert_eq!(got.windows[0].reset_at, Some(1_790_000_000_000));
        // 剩 88% → 已用 12%
        assert_eq!(got.windows[1].key, "weekly");
        assert!((got.windows[1].used_percent.unwrap() - 12.0).abs() < 1e-9);
        assert_eq!(got.windows[1].reset_at, Some(1_790_000_000_000));
    }

    /// 剩余率是 1.0(满额)时已用必须是 0,不能反过来
    #[test]
    fn a_full_quota_reports_zero_used() {
        let got = parse_rate_limit(&serde_json::json!({ "weekly_usage_left_rate": 1.0 }));
        assert_eq!(got.windows[0].used_percent, Some(0.0));
        let got = parse_rate_limit(&serde_json::json!({ "weekly_usage_left_rate": 0.0 }));
        assert_eq!(got.windows[0].used_percent, Some(100.0));
    }

    /// 上游若改成百分数口径(>1),不能整整差 100 倍
    #[test]
    fn a_percentage_style_left_rate_is_not_off_by_100x() {
        let got = parse_rate_limit(&serde_json::json!({ "weekly_usage_left_rate": 88.0 }));
        assert!((got.windows[0].used_percent.unwrap() - 12.0).abs() < 1e-9);
    }

    /// TOKEN 家族(family=2):只出 Credit 那一条,不要 5 小时/每周
    /// —— 那组字段在这个家族里恒为 0,显示出来是假数字(实测踩过)
    #[test]
    fn the_token_family_shows_credit_only() {
        let body = serde_json::json!({
            "plan_family": 2,
            "five_hour_usage_left_rate": 0,
            "weekly_usage_left_rate": 0,
            "plan_credit_rate_limit": { "subscription_credit_left_rate": 0.25 }
        });
        let got = parse_rate_limit(&body);
        assert_eq!(got.windows.len(), 1, "只该有 Credit 一条: {:?}", got.windows);
        assert_eq!(got.windows[0].key, "credit");
        assert!((got.windows[0].used_percent.unwrap() - 75.0).abs() < 1e-9, "剩 25% → 已用 75%");
    }

    /// CODING 家族(family=1):出 5 小时/每周,不出 Credit
    #[test]
    fn the_coding_family_shows_the_rolling_windows() {
        let body = serde_json::json!({
            "plan_family": 1,
            "five_hour_usage_left_rate": 0.5,
            "weekly_usage_left_rate": 0.8,
            "plan_credit_rate_limit": { "subscription_credit_left_rate": 1.0 }
        });
        let got = parse_rate_limit(&body);
        let keys: Vec<&str> = got.windows.iter().map(|w| w.key.as_str()).collect();
        assert!(keys.contains(&"five_hour"));
        assert!(keys.contains(&"weekly"));
        assert!(!keys.contains(&"credit"), "CODING 家族不该显示 Credit: {keys:?}");
    }

    /// 上游把"没有该窗口"填成 `"0"`,不能转成 1970 年显示一个荒唐的倒计时
    #[test]
    fn a_zero_reset_time_means_no_reset_not_1970() {
        let body = serde_json::json!({
            "plan_family": 1,
            "five_hour_usage_left_rate": 1.0,
            "five_hour_usage_reset_time": "0"
        });
        let got = parse_rate_limit(&body);
        assert_eq!(got.windows[0].reset_at, None, "0 应当作没有,而不是 1970");
        assert_eq!(to_ms(0), None);
        assert_eq!(to_ms(-5), None);
        assert_eq!(to_ms(1_790_000_000), Some(1_790_000_000_000));
    }

    /// 加油包没有额度(比例 0)时不显示那一行
    #[test]
    fn an_empty_topup_bucket_is_hidden() {
        let body = serde_json::json!({
            "plan_family": 2,
            "plan_credit_rate_limit": {
                "subscription_credit_left_rate": 1.0,
                "topup_credit_left_rate": 0
            }
        });
        let got = parse_rate_limit(&body);
        assert_eq!(got.windows.len(), 1, "加油包为 0 就不该出现: {:?}", got.windows);
        // 有额度时才显示
        let body2 = serde_json::json!({
            "plan_family": 2,
            "plan_credit_rate_limit": {
                "subscription_credit_left_rate": 1.0,
                "topup_credit_left_rate": 0.5
            }
        });
        let got2 = parse_rate_limit(&body2);
        assert_eq!(got2.windows.len(), 2);
        assert!(got2.windows.iter().any(|w| w.key == "topup"));
    }

    /// "套餐有效期"是**到期日**不是配额:它的 `used_percent` 必须是 `None`,
    /// 否则前端会把它当配额渲染成"已用 --" + 倒计时,读起来像坏掉的一行。
    ///
    /// 前端据此把它单列成日期行(见 Limits.tsx 的 `isDateOnly`)。
    #[test]
    fn the_expiry_window_has_no_percentage() {
        let body = serde_json::json!({
            "subscription": { "name": "Plus", "expired_at": 1795058749 }
        });
        let got = parse_plan_status(&body);
        assert_eq!(got.windows.len(), 1);
        let w = &got.windows[0];
        assert_eq!(w.key, "plan");
        assert_eq!(w.used_percent, None, "到期日没有'已用多少'的概念");
        assert!(w.reset_at.is_some(), "要有到期时间");
        assert_eq!(w.window_seconds, None);
    }

    /// PlanStatus 只出套餐名与到期,不产用量窗口
    #[test]
    fn plan_status_yields_name_and_expiry() {
        let body = serde_json::json!({
            "subscription": { "name": "Mini Plan", "expired_at": 1796000000 }
        });
        let got = parse_plan_status(&body);
        assert_eq!(got.plan_label.as_deref(), Some("Mini Plan"));
        assert_eq!(got.windows.len(), 1);
        assert_eq!(got.windows[0].key, "plan");
        assert_eq!(got.windows[0].reset_at, Some(1_796_000_000_000));
    }

    /// 规则 3:字段认不出时**不造数字**,而是把响应键报出来
    #[test]
    fn unrecognised_plan_shape_reports_keys_instead_of_inventing_numbers() {
        let body = serde_json::json!({ "data": { "foo": 1, "bar": 2 } });
        let got = parse_rate_limit(&body);
        assert!(got.windows.is_empty(), "不能凭空造出窗口");
        let err = got.error.expect("必须给出可诊断的错误");
        assert!(err.contains("foo"), "错误里要带上响应键: {err}");
    }

    /// 合并:余额 + 订阅窗口共存,且失败的一边不盖掉成功的一边
    #[test]
    fn merging_keeps_both_sources_and_ignores_a_one_sided_failure() {
        let mut bal = ProviderLimits::new("stepfun", "StepFun", "stepfun");
        bal.configured = true;
        bal.balance = Some(Balance { amount: 30.0, currency: "CNY".into(), cash: None, voucher: None });
        let mut plan = ProviderLimits::new("stepfun", "StepFun", "stepfun");
        plan.configured = true;
        plan.plan_label = Some("Mini Plan".into());
        plan.windows.push(QuotaWindow::new("weekly", "每周").percent(Some(12.0)));

        let got = combine_stepfun(bal, plan);
        assert!(got.balance.is_some(), "余额要保留");
        assert_eq!(got.windows.len(), 1, "订阅窗口要并进来");
        assert_eq!(got.plan_label.as_deref(), Some("Mini Plan"));
        assert!(got.error.is_none(), "两边都有数据时不该有错误");

        // 余额失败、订阅成功 → 不能把订阅数据染成红色
        let mut bad = ProviderLimits::new("stepfun", "StepFun", "stepfun");
        bad.configured = true;
        bad.error = Some("API Key 无效".into());
        let mut plan2 = ProviderLimits::new("stepfun", "StepFun", "stepfun");
        plan2.configured = true;
        plan2.windows.push(QuotaWindow::new("weekly", "每周").percent(Some(5.0)));
        let got = combine_stepfun(bad, plan2);
        assert_eq!(got.windows.len(), 1);
        assert!(got.error.is_none(), "订阅成功就不该报余额的错");
    }

    /// **实测截图踩到的坑**:用户配了 Cookie、没配 API Key,卡片却只显示
    /// "未配置 API Key",让人以为自己白配了。两条来源都要说清楚。
    /// 文案不写死内置条目名:额外账号用的是自己的 `limit.*` 条目。
    #[test]
    fn a_configured_cookie_is_not_hidden_behind_a_missing_api_key_message() {
        let mut bal = ProviderLimits::new("stepfun", "StepFun", "stepfun");
        bal.configured = false;
        bal.error = Some("未配置 API Key。填入后即可查看按量余额。".into());
        let mut plan = ProviderLimits::new("stepfun", "StepFun", "stepfun");
        plan.configured = true;
        plan.error = Some("凭据失效,请重新从浏览器复制 Oasis-Token(http 401)".into());

        let got = combine_stepfun(bal, plan);
        let err = got.error.expect("必须有可诊断的说明");
        assert!(err.contains("Oasis-Token"), "要提订阅额度那条: {err}");
        assert!(
            err.contains("未配置 API Key"),
            "也要保留余额那条,但别让它独占: {err}"
        );
        assert!(err.contains("可选"), "要说明 API Key 是可选的: {err}");
    }

    /// 默认关闭:没配凭据时给引导文案,不是红色报错。
    ///
    /// 用**不存在的条目名**,别碰 `COOKIE_NAME` —— 那个名字在真实机器上
    /// 可能真的存在(用户配过),测试会随环境时好时坏。
    #[test]
    fn missing_credentials_produce_guidance_not_a_crash() {
        let got = fetch_plan_with_credential("OTR_TEST_NO_SUCH_COOKIE_NAME");
        assert!(!got.configured);
        let err = got.error.clone().unwrap_or_default();
        assert!(
            err.contains("该账号的控制台 Cookie"),
            "额外账号不要指回内置条目名: {err}"
        );
        assert!(err.contains("cURL"), "要给出具体做法: {err}");
        assert!(got.windows.is_empty());
    }

    // 造两个结构合法的假 JWT(header 是 base64url 的 {"alg":"HS256"})
    const H: &str = "eyJhbGciOiJIUzI1NiJ9";
    fn jwt_with(payload_json: &str, sig: &str) -> String {
        use base64::Engine as _;
        let p = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(payload_json.as_bytes());
        format!("{H}.{p}.{sig}")
    }

    /// **最关键的一条**:`Oasis-Token` 的值是复合 journal
    /// `<访问令牌>...<刷新令牌>`,**那三个点是格式的一部分**。
    ///
    /// 早先把它当"从聊天记录粘进来的省略号"剥掉、只发前半段,服务端一律回
    /// `token is illegal`;整条原样发才 200。这条测试锁死"不许再剥"。
    #[test]
    fn the_ellipsis_is_part_of_the_journal_format() {
        let access = jwt_with(r#"{"activated":true}"#, "sigA");
        let refresh = jwt_with(r#"{"device_id":"dev1"}"#, "sigB");
        let journal = format!("{access}...{refresh}");

        // 裸 journal
        assert_eq!(extract_journal(&journal).as_deref(), Some(journal.as_str()));
        // 包在 cookie 里
        assert_eq!(
            extract_journal(&format!("Oasis-Token={journal}")).as_deref(),
            Some(journal.as_str())
        );
        // 整段 cURL
        assert_eq!(
            extract_journal(&format!("curl 'https://x' -H 'Cookie: Oasis-Token={journal}'"))
                .as_deref(),
            Some(journal.as_str())
        );
        // 三个点必须还在
        assert!(extract_journal(&journal).unwrap().contains("..."));
    }

    /// 单个 JWT(控制台请求头里复制的)也接受,但要提醒它会过期
    #[test]
    fn a_lone_access_token_is_accepted_with_a_warning() {
        let access = jwt_with(r#"{"activated":true}"#, "sigA");
        assert_eq!(extract_journal(&access).as_deref(), Some(access.as_str()));
        let warn = check_credential(&access).unwrap_err();
        assert!(warn.contains("30 分钟"), "要说明寿命: {warn}");
        // 带刷新令牌的不报警
        let both = format!("{access}...{}", jwt_with(r#"{"device_id":"d"}"#, "s"));
        assert!(check_credential(&both).is_ok());
    }

    /// 从 journal 里取 device_id 当 `Oasis-Webid` / `Oasis-Did`
    #[test]
    fn device_id_comes_out_of_the_journal() {
        let access = jwt_with(r#"{"activated":true}"#, "sigA");
        let refresh = jwt_with(r#"{"device_id":"c76f0b46"}"#, "sigB");
        assert_eq!(
            journal_device_id(&format!("{access}...{refresh}")).as_deref(),
            Some("c76f0b46")
        );
        // 两段都没有 device_id
        assert_eq!(journal_device_id(&access), None);
        // 畸形输入不 panic
        assert_eq!(journal_device_id("garbage...more"), None);
    }

    /// 认不出 journal 时给一句能照做的中文说明
    #[test]
    fn a_credential_without_a_journal_is_reported_locally() {
        let err = check_credential("not-a-token").unwrap_err();
        assert!(err.contains("Oasis-Token"), "要指明去哪复制: {err}");
        assert!(err.contains("cURL"), "要给出具体做法: {err}");
    }

    /// 请求头必须齐:Oasis-Token / appID / Platform / Webid / Did
    #[test]
    fn plan_headers_carry_everything_the_console_sends() {
        let h: std::collections::HashMap<_, _> =
            plan_headers("J...R", Some("dev1")).into_iter().collect();
        assert_eq!(h["Oasis-Token"], "J...R");
        assert_eq!(h["Oasis-appID"], "10300");
        assert_eq!(h["Oasis-Platform"], "web");
        assert_eq!(h["Oasis-Webid"], "dev1");
        assert_eq!(h["Oasis-Did"], "dev1", "两个名字都要带");
        // 没有 device_id 时不带这两个头
        let h2: std::collections::HashMap<_, _> =
            plan_headers("J", None).into_iter().collect();
        assert!(!h2.contains_key("Oasis-Webid"));
        assert!(!h2.contains_key("Oasis-Did"));
    }

    /// 余额缺失 → 明说,而不是显示 0 元
    #[test]
    fn missing_balance_field_is_reported() {
        let got = parse_accounts(&serde_json::json!({ "balance": { "currency": "CNY" } }));
        assert!(got.balance.is_none());
        assert!(got.error.is_some());
    }
}
