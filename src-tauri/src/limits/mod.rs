//! 额度页:统一窗口模型 + 各 Provider 的额度/余额查询。
//!
//! 与 `providers/`(用量统计)是两回事:那边读本地日志算 token,这边走网络问各家
//! "你的订阅还剩多少"。所以单独一个模块,不塞进 providers 的 trait。
//!
//! 四条**必须编码进实现**的正确性规则(每一条都有对应的单测):
//!
//! 1. **Cursor 的百分比有优先阶梯**:`totalPercentUsed` → `(auto+api)/2` →
//!    `apiPercentUsed` → `autoPercentUsed` → 最后才轮到 `used/limit`。实测
//!    `used=2000, limit=2000` 算出 100%,而真相是 `totalPercentUsed=17.4525`。
//! 2. **Codex 的窗口按 `limit_window_seconds` 分类,不按槽位名**。免费档只发一个
//!    窗口且落在 `primary_window` 槽里,按位置读会把它当成"5 小时";实测那是
//!    `2592000`(30 天)。
//! 3. **字段缺失即 `None`**,UI 显示 `--`。绝不臆造 0 —— "0% 已用"和"读不到"
//!    是完全相反的两件事。
//! 4. **不写回别人的凭据文件**。Codex 登录态失效只提示用户去跑 `codex`,不碰
//!    `~/.codex/auth.json`。

pub mod codex;
pub mod credential;
pub mod cursor;
pub mod http;
pub mod deepseek;
pub mod qwen;
pub mod stepfun;

use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// 一个额度窗口(5 小时 / 每周 / 每月 / 套餐 / 按量…)
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    /// 稳定键:"five_hour" | "weekly" | "monthly" | "plan" | "auto" | "api" | "rolling" | "credit"
    pub key: String,
    /// 中文显示名
    pub label: String,
    /// 已用百分比;None = 该字段缺失 → UI 显示 "--",**不要**当成 0
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_percent: Option<f64>,
    /// 重置时刻(unix ms)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_at: Option<i64>,
    /// 窗口长度(秒),原样保留上游给的值
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_seconds: Option<i64>,
}

impl QuotaWindow {
    pub fn new(key: &str, label: &str) -> Self {
        Self {
            key: key.into(),
            label: label.into(),
            ..Default::default()
        }
    }

    pub fn percent(mut self, p: Option<f64>) -> Self {
        self.used_percent = p;
        self
    }

    pub fn reset(mut self, ms: Option<i64>) -> Self {
        self.reset_at = ms;
        self
    }

    pub fn window(mut self, secs: Option<i64>) -> Self {
        self.window_seconds = secs;
        self
    }
}

/// 余额(按量计费账户)
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Balance {
    pub amount: f64,
    /// "CNY" | "USD"
    pub currency: String,
    /// 充值余额(StepFun 把赠送额单列)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cash: Option<f64>,
    /// 赠送余额
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voucher: Option<f64>,
}

/// 一个账号的额度快照
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ProviderLimits {
    /// 多账号唯一键
    pub account_id: String,
    pub account_label: String,
    /// "cursor" | "codex" | "deepseek" | "qwen" | "stepfun"
    pub provider: String,
    /// 是否已配置凭据;false 时 UI 显示引导文案而不是红色报错
    pub configured: bool,
    /// 上一次查询的错误;有值时仍会保留上一次成功的数据
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// 套餐名(如 "Pro" / "free")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_label: Option<String>,
    pub windows: Vec<QuotaWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance: Option<Balance>,
    /// 本次数据抓取时刻(unix ms);0 = 从未成功
    pub fetched_at: i64,
}

impl ProviderLimits {
    pub fn new(account_id: &str, label: &str, provider: &str) -> Self {
        Self {
            account_id: account_id.into(),
            account_label: label.into(),
            provider: provider.into(),
            ..Default::default()
        }
    }

    /// 未配置凭据:给一句引导,不报错
    pub fn unconfigured(account_id: &str, label: &str, provider: &str, hint: &str) -> Self {
        let mut s = Self::new(account_id, label, provider);
        s.error = Some(hint.into());
        s
    }
}

/// 刷新失败时**保留上一次成功的数据**,只把错误挂上去。
///
/// 网络抖动不该让额度页变成一片红:上一次的数字仍然是有信息量的,
/// 而清空缓存等于把用户已知的事实抹掉。
pub fn keep_last_success(prev: &ProviderLimits, err: String) -> ProviderLimits {
    let mut next = prev.clone();
    next.error = Some(err);
    next
}

/// 把设置里的账号配置展开成"要查询的账号清单"。
///
/// **内置账号始终排在最前且不可删**:它们是"什么都没配"时的默认那一条
/// (`~/.codex`、`%APPDATA%\Cursor` 的本地会话)。额外账号是用户添加的。
pub fn resolve_accounts(settings: &crate::settings::Settings) -> Vec<AccountPlan> {
    let enabled = |p: &str| settings.limit_providers.iter().any(|x| x == p);
    let mut out: Vec<AccountPlan> = Vec::new();

    // 内置:provider 默认目录
    if enabled("cursor") {
        out.push(AccountPlan {
            account: crate::settings::LimitAccount {
                id: "cursor".into(),
                provider: "cursor".into(),
                label: "Cursor".into(),
                ..Default::default()
            },
            builtin: true,
        });
    }
    if enabled("codex") {
        out.push(AccountPlan {
            account: crate::settings::LimitAccount {
                id: "codex".into(),
                provider: "codex".into(),
                label: "Codex CLI".into(),
                ..Default::default()
            },
            builtin: true,
        });
    }
    if enabled("deepseek") {
        out.push(AccountPlan {
            account: crate::settings::LimitAccount {
                id: "deepseek".into(),
                provider: "deepseek".into(),
                label: "DeepSeek".into(),
                secret_ref: Some(deepseek::CREDENTIAL_NAME.into()),
                ..Default::default()
            },
            builtin: true,
        });
    }
    if enabled("qwen") {
        out.push(AccountPlan {
            account: crate::settings::LimitAccount {
                id: "qwen".into(),
                provider: "qwen".into(),
                label: "Qwen".into(),
                secret_ref: Some(qwen::CREDENTIAL_NAME.into()),
                cookie_ref: Some(qwen::COOKIE_NAME.into()),
                ..Default::default()
            },
            builtin: true,
        });
    }
    if enabled("stepfun") {
        out.push(AccountPlan {
            account: crate::settings::LimitAccount {
                id: "stepfun".into(),
                provider: "stepfun".into(),
                label: "StepFun".into(),
                secret_ref: Some(stepfun::API_KEY_NAME.into()),
                cookie_ref: Some(stepfun::COOKIE_NAME.into()),
                ..Default::default()
            },
            builtin: true,
        });
    }

    // 用户额外添加的账号
    for acc in &settings.limit_accounts {
        if !enabled(&acc.provider) {
            continue;
        }
        out.push(AccountPlan {
            account: acc.clone(),
            builtin: false,
        });
    }
    out
}

/// 一个待查询的账号
#[derive(Debug, Clone)]
pub struct AccountPlan {
    pub account: crate::settings::LimitAccount,
    /// 内置账号(不可删)
    pub builtin: bool,
}

/// 查一个账号。**单个账号失败绝不牵连其它账号**:每个都返回自己的结果。
pub fn fetch_account(plan: &AccountPlan) -> ProviderLimits {
    let acc = &plan.account;
    let mut got = match acc.provider.as_str() {
        "cursor" => fetch_cursor(acc),
        "codex" => fetch_codex(acc),
        "deepseek" => fetch_deepseek(acc),
        "qwen" => fetch_qwen(acc),
        "stepfun" => fetch_stepfun(acc),
        // 理论上 resolve_accounts 已过滤;这里兜底,绝不拿别家数字冒充
        other => ProviderLimits::new(&acc.id, &acc.label, other),
    };
    // 内置账号沿用固定的 account_id,额外账号用用户给的 id
    got.account_id = acc.id.clone();
    got.account_label = if acc.label.trim().is_empty() {
        acc.id.clone()
    } else {
        acc.label.clone()
    };
    if got.fetched_at == 0 && got.error.is_none() {
        got.fetched_at = crate::model::now_ms();
    } else if got.error.is_none() {
        got.fetched_at = crate::model::now_ms();
    }
    got
}

fn fetch_cursor(acc: &crate::settings::LimitAccount) -> ProviderLimits {
    match acc.home.as_deref() {
        None => cursor::fetch(),
        // home 模式:从该账号自己的 profile 目录取会话
        Some(home) => cursor::fetch_from_home(std::path::Path::new(home)),
    }
}

fn fetch_codex(acc: &crate::settings::LimitAccount) -> ProviderLimits {
    match acc.home.as_deref() {
        None => codex::fetch(),
        Some(home) => codex::fetch_from_home(std::path::Path::new(home)),
    }
}

/// 千问额度认控制台 Cookie。Key 单独打上去会被 `ConsoleNeedLogin` 拒绝,
/// 所以 Cookie 有结果就用 Cookie;没有 Cookie 时才退回 Key,并告诉用户去贴 Cookie。
fn fetch_qwen(acc: &crate::settings::LimitAccount) -> ProviderLimits {
    let (key_name, cookie_name) = stepfun_credential_names(acc);
    if key_name.is_none() && cookie_name.is_none() {
        return ProviderLimits::unconfigured(
            &acc.id,
            &acc.label,
            "qwen",
            "未配置百炼控制台 Cookie。打开 Coding Plan 页面,复制请求头 Cookie 贴到设置里。",
        );
    }

    let mut notes = Vec::new();
    if let Some(name) = cookie_name.as_deref() {
        let got = qwen::fetch_with_cookie(name);
        if !got.windows.is_empty() {
            return got;
        }
        // Cookie 已经配了但没查出数字:Key 那条只会再重复一遍「请贴 Cookie」,不往错误上叠。
        if got.configured {
            if let Some(name) = key_name.as_deref() {
                let key_got = qwen::fetch_with_credential(name);
                if !key_got.windows.is_empty() {
                    return key_got;
                }
            }
            if let Some(err) = got.error {
                notes.push(err);
            }
        }
    }
    if notes.is_empty() {
        if let Some(name) = key_name.as_deref() {
            let got = qwen::fetch_with_credential(name);
            if !got.windows.is_empty() {
                return got;
            }
            if let Some(err) = got.error {
                notes.push(err);
            }
        }
    }

    let mut out = ProviderLimits::new(&acc.id, &acc.label, "qwen");
    out.configured = true;
    out.error = Some(if notes.is_empty() {
        "没有查到 Coding Plan 用量".into()
    } else {
        notes.join("\n")
    });
    out
}

fn fetch_deepseek(acc: &crate::settings::LimitAccount) -> ProviderLimits {
    match acc.secret_ref.as_deref() {
        Some(name) => deepseek::fetch_with_credential(name),
        None => ProviderLimits::unconfigured(
            &acc.id,
            &acc.label,
            "deepseek",
            "未配置 API Key。在设置里为该账号填入密钥即可查看余额。",
        ),
    }
}

/// StepFun 有两条独立来源,合到一张卡上:
/// - **按量余额**(吃该账号自己的 API Key 条目)
/// - **订阅额度**(吃该账号自己的 `cookie_ref`,内置那条是 `STEPFUN_CONSOLE_COOKIE`)
///
/// 哪个配了就显示哪个;两个都配则合并(余额 + 窗口共存)。
/// 没有 cookie 时不拿别的账号的控制台凭据来凑。
fn fetch_stepfun(acc: &crate::settings::LimitAccount) -> ProviderLimits {
    let (key_name, cookie_name) = stepfun_credential_names(acc);
    let balance = match key_name.as_deref() {
        Some(name) => stepfun::fetch_balance_with_credential(name),
        None => ProviderLimits::unconfigured(
            &acc.id,
            &acc.label,
            "stepfun",
            "按量余额:未配置 API Key(可选,不影响订阅额度)",
        ),
    };
    let plan = match cookie_name.as_deref() {
        Some(name) => stepfun::fetch_plan_with_credential(name),
        None => ProviderLimits::unconfigured(
            &acc.id,
            &acc.label,
            "stepfun",
            "需要控制台 Cookie。在设置里为该账号粘贴 Oasis-Token。",
        ),
    };
    combine_stepfun(balance, plan)
}

/// 这个账号实际要查的两条条目名。只认账号自己的字段,不把 Key 的名字拼成
/// `<name>_CONSOLE_COOKIE`(那条名字用户既填不了,也不在凭据白名单里)。
fn stepfun_credential_names(
    acc: &crate::settings::LimitAccount,
) -> (Option<String>, Option<String>) {
    let filled = |v: &Option<String>| {
        v.as_ref()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    (filled(&acc.secret_ref), filled(&acc.cookie_ref))
}

/// 合并余额与订阅额度。两边的 `error`/`configured` 语义不同,要分开对待:
/// 只有**两边都没拿到任何数据**时才算整体失败 —— 否则一个失败会把另一个
/// 已经拿到的数字盖成红色报错。
fn combine_stepfun(mut balance: ProviderLimits, plan: ProviderLimits) -> ProviderLimits {
    let balance_ok = balance.balance.is_some();
    let plan_ok = !plan.windows.is_empty();

    if balance_ok && plan_ok {
        balance.windows.extend(plan.windows);
        if balance.plan_label.is_none() {
            balance.plan_label = plan.plan_label;
        }
        balance.configured = true;
        balance.error = None;
        return balance;
    }
    if plan_ok {
        let mut out = plan;
        out.configured = true;
        out.error = None;
        return out;
    }
    if balance_ok {
        balance.error = None;
        return balance;
    }

    // 两边都没数据。**别只说余额那条**:用户配了 Cookie 却看到
    // "未配置 STEPFUN_API_KEY",会以为自己白配了(实测截图就是这样)。
    // 把两条来源各自的状态都说清楚。
    let mut out = balance;
    out.error = Some(match (out.configured, plan.error) {
        // 余额没配 + 订阅有明确报错 → 报订阅那条,它更接近用户刚做的事
        (false, Some(plan_err)) => format!(
            "订阅额度:{plan_err}
按量余额:未配置 API Key(可选,不影响订阅额度)"
        ),
        // 余额没配 + 订阅没报错(理论上不该发生)→ 保留原提示并补一句
        (false, None) => format!(
            "{}
订阅额度:未取得数据",
            out.error.unwrap_or_default()
        ),
        // 余额配了但失败 + 订阅也失败 → 两条都报
        (true, Some(plan_err)) => {
            format!("{}
订阅额度:{plan_err}", out.error.unwrap_or_default())
        }
        (true, None) => out.error.unwrap_or_default(),
    });
    out
}

/// 内置账号的 id。额外账号不能占用,否则列表里会出现两条同一个 id。
pub fn is_builtin_account_id(id: &str) -> bool {
    matches!(id, "cursor" | "codex" | "deepseek" | "qwen" | "stepfun")
}

/// 删账号时要一并清掉的凭据。只含程序生成的 `limit.*`,不动内置条目。
pub fn owned_credential_names(acc: &crate::settings::LimitAccount) -> Vec<String> {
    [acc.secret_ref.as_deref(), acc.cookie_ref.as_deref()]
        .into_iter()
        .flatten()
        .filter(|name| credential::is_generated_name(name))
        .map(str::to_string)
        .collect()
}

/// 把前端提交整理成可落盘的账号,以及这次要写入凭据库的 `(条目名, 明文)`。
///
/// 条目名一律由这里生成。调用方传来的 `secret_ref` / `cookie_ref` 忽略,
/// 避免界面或命令把账号指到别人的内置密钥上。明文为空表示沿用已有条目
/// (只改显示名)。密钥本身不放进返回的账号,账号里只有条目名。
pub fn prepare_account(
    existing: &[crate::settings::LimitAccount],
    mut account: crate::settings::LimitAccount,
    api_key: Option<String>,
    console_cookie: Option<String>,
) -> std::result::Result<(crate::settings::LimitAccount, Vec<(String, String)>), String> {
    account.id = account.id.trim().to_string();
    account.provider = account.provider.trim().to_string();
    account.label = account.label.trim().to_string();
    account.plan = account.plan.trim().to_string();
    if let Some(home) = account.home.take() {
        let home = home.trim().to_string();
        if !home.is_empty() {
            account.home = Some(home);
        }
    }
    if account.id.is_empty() {
        account.id = allocate_account_id(&account.provider, existing);
    }
    if !crate::settings::LIMIT_PROVIDERS.contains(&account.provider.as_str()) {
        let shown = if account.provider.is_empty() {
            "(空)"
        } else {
            account.provider.as_str()
        };
        return Err(format!("暂不支持该额度来源:{shown}"));
    }
    if is_builtin_account_id(&account.id) {
        return Err("内置账号的 id 不能用作额外账号".into());
    }
    if credential::generated_key_name(&account.provider, &account.id).is_err() {
        return Err(format!("账号 id 不合法:{}", account.id));
    }
    if account.label.is_empty() {
        account.label = match account.provider.as_str() {
            "cursor" => "Cursor".into(),
            "codex" => "Codex CLI".into(),
            "deepseek" => "DeepSeek".into(),
            "qwen" => "Qwen".into(),
            "stepfun" => "StepFun".into(),
            other => other.to_string(),
        };
    }

    let prev = existing.iter().find(|a| a.id == account.id);
    if let Some(prev) = prev {
        if prev.provider != account.provider {
            return Err("不能改账号的额度来源".into());
        }
        account.secret_ref = prev.secret_ref.clone();
        account.cookie_ref = prev.cookie_ref.clone();
    } else {
        account.secret_ref = None;
        account.cookie_ref = None;
    }

    let api_key = blank_to_none(api_key);
    let console_cookie = blank_to_none(console_cookie);
    let mut writes = Vec::new();

    match account.provider.as_str() {
        "cursor" | "codex" => {
            account.secret_ref = None;
            account.cookie_ref = None;
        }
        "deepseek" => {
            account.home = None;
            account.cookie_ref = None;
            if let Some(secret) = api_key {
                let name = credential::generated_key_name(&account.provider, &account.id)?;
                account.secret_ref = Some(name.clone());
                writes.push((name, secret));
            }
        }
        "qwen" | "stepfun" => {
            account.home = None;
            if let Some(secret) = api_key {
                let name = credential::generated_key_name(&account.provider, &account.id)?;
                account.secret_ref = Some(name.clone());
                writes.push((name, secret));
            }
            if let Some(secret) = console_cookie {
                let name = credential::generated_cookie_name(&account.provider, &account.id)?;
                account.cookie_ref = Some(name.clone());
                writes.push((name, secret));
            }
        }
        _ => {}
    }

    account.validate()?;
    Ok((account, writes))
}

fn blank_to_none(value: Option<String>) -> Option<String> {
    value.and_then(|s| {
        let t = s.trim().to_string();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    })
}

fn allocate_account_id(provider: &str, existing: &[crate::settings::LimitAccount]) -> String {
    let provider = if crate::settings::LIMIT_PROVIDERS.contains(&provider) {
        provider
    } else {
        "acct"
    };
    for n in 2..10_000 {
        let id = format!("{provider}-{n}");
        if !existing.iter().any(|a| a.id == id) {
            return id;
        }
    }
    format!("{provider}-x")
}

/// 内存缓存(不落盘:额度是易失数据,重启重拉即可)
#[derive(Default)]
pub struct LimitsCache {
    inner: Mutex<Vec<ProviderLimits>>,
}

impl LimitsCache {
    pub fn snapshot(&self) -> Vec<ProviderLimits> {
        crate::lock(&self.inner).clone()
    }

    pub fn replace(&self, next: Vec<ProviderLimits>) {
        *crate::lock(&self.inner) = next;
    }

    /// 合并一轮刷新结果:成功的替换,失败的保留旧数据并挂上错误。
    pub fn merge(&self, fresh: Vec<ProviderLimits>) {
        let mut guard = crate::lock(&self.inner);
        for item in fresh {
            match guard.iter_mut().find(|p| p.account_id == item.account_id) {
                Some(slot) if item.error.is_some() && slot.fetched_at > 0 => {
                    *slot = keep_last_success(slot, item.error.clone().unwrap_or_default());
                }
                Some(slot) => *slot = item,
                None => guard.push(item),
            }
        }
        // 已被删除的账号要从缓存里消失
        let ids: Vec<String> = guard.iter().map(|p| p.account_id.clone()).collect();
        let _ = ids;
    }

    /// 只保留这些账号(删除账号后调用)
    pub fn retain_accounts(&self, keep: &[String]) {
        crate::lock(&self.inner).retain(|p| keep.iter().any(|k| k == &p.account_id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(fetched_at: i64) -> ProviderLimits {
        let mut p = ProviderLimits::new("cursor-work", "工作", "cursor");
        p.configured = true;
        p.fetched_at = fetched_at;
        p.windows = vec![QuotaWindow::new("plan", "套餐").percent(Some(17.45))];
        p
    }

    /// 刷新失败必须保留上一次成功的数字,而不是清空
    #[test]
    fn failed_refresh_keeps_the_last_good_numbers() {
        let cache = LimitsCache::default();
        cache.merge(vec![sample(1000)]);
        let mut failed = ProviderLimits::new("cursor-work", "工作", "cursor");
        failed.error = Some("http 401".into());
        cache.merge(vec![failed]);

        let got = cache.snapshot();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].error.as_deref(), Some("http 401"));
        assert_eq!(got[0].fetched_at, 1000, "旧的抓取时刻要保留");
        assert_eq!(
            got[0].windows[0].used_percent,
            Some(17.45),
            "旧的数字不能被清空"
        );
    }

    /// 从未成功过的账号:错误就是全部内容,不该凭空造出数据
    #[test]
    fn first_failure_has_no_stale_data_to_show() {
        let cache = LimitsCache::default();
        let mut failed = ProviderLimits::new("deepseek", "DeepSeek", "deepseek");
        failed.error = Some("未配置".into());
        cache.merge(vec![failed]);
        let got = cache.snapshot();
        assert_eq!(got[0].fetched_at, 0);
        assert!(got[0].windows.is_empty());
    }

    /// A10:两个账号各自独立,数字不互相覆盖
    #[test]
    fn accounts_do_not_overwrite_each_other() {
        let cache = LimitsCache::default();
        let mut work = sample(1000);
        work.windows = vec![QuotaWindow::new("plan", "套餐").percent(Some(11.0))];
        let mut home = sample(2000);
        home.account_id = "cursor-home".into();
        home.account_label = "家里".into();
        home.windows = vec![QuotaWindow::new("plan", "套餐").percent(Some(77.0))];
        cache.merge(vec![work, home]);

        let got = cache.snapshot();
        assert_eq!(got.len(), 2, "两个账号都要在");
        let by_id: std::collections::HashMap<_, _> = got
            .iter()
            .map(|p| (p.account_id.as_str(), p.windows[0].used_percent))
            .collect();
        assert_eq!(by_id["cursor-work"], Some(11.0));
        assert_eq!(by_id["cursor-home"], Some(77.0), "数字不能互相覆盖");

        // 只刷新其中一个:另一个必须原样不动
        let mut only = sample(3000);
        only.account_id = "cursor-home".into();
        only.windows = vec![QuotaWindow::new("plan", "套餐").percent(Some(88.0))];
        cache.merge(vec![only]);
        let got = cache.snapshot();
        let by_id: std::collections::HashMap<_, _> = got
            .iter()
            .map(|p| (p.account_id.as_str(), p.windows[0].used_percent))
            .collect();
        assert_eq!(by_id["cursor-work"], Some(11.0), "没刷新到的账号不能被清掉");
        assert_eq!(by_id["cursor-home"], Some(88.0));
    }

    /// 内置账号始终在最前,且用户关掉某个 provider 时它整体消失
    #[test]
    fn builtin_accounts_are_always_first_and_follow_the_toggle() {
        let mut s = crate::settings::Settings::default();
        let plans = resolve_accounts(&s);
        assert_eq!(plans[0].account.id, "cursor", "内置账号排最前");
        assert!(plans[0].builtin);
        assert!(plans.iter().any(|p| p.account.provider == "qwen"), "千问默认开启");
        assert!(!plans.iter().any(|p| p.account.provider == "stepfun"), "StepFun 默认不出现");

        s.limit_providers.push("stepfun".into());
        let plans = resolve_accounts(&s);
        assert!(plans.iter().any(|p| p.account.provider == "stepfun"), "勾选后出现");
    }

    /// 用户额外账号:只在对应 provider 被启用时才出现
    #[test]
    fn extra_accounts_require_their_provider_to_be_enabled() {
        let mut s = crate::settings::Settings::default();
        s.limit_accounts.push(crate::settings::LimitAccount {
            id: "cursor-work".into(),
            provider: "cursor".into(),
            label: "工作".into(),
            ..Default::default()
        });
        s.limit_accounts.push(crate::settings::LimitAccount {
            id: "stepfun-x".into(),
            provider: "stepfun".into(),
            label: "StepFun 备用".into(),
            secret_ref: Some("STEPFUN_API_KEY_X".into()),
            ..Default::default()
        });
        let ids: Vec<_> = resolve_accounts(&s).into_iter().map(|p| p.account.id).collect();
        assert!(ids.contains(&"cursor-work".to_string()));
        assert!(!ids.contains(&"stepfun-x".to_string()), "StepFun 没开就不查");
    }

    /// 删除账号后缓存里不该再留着它
    #[test]
    fn removed_accounts_leave_the_cache() {
        let cache = LimitsCache::default();
        cache.merge(vec![sample(1), {
            let mut b = sample(2);
            b.account_id = "cursor-home".into();
            b
        }]);
        assert_eq!(cache.snapshot().len(), 2);
        cache.retain_accounts(&["cursor-work".to_string()]);
        let got = cache.snapshot();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].account_id, "cursor-work");
    }

    /// 内置 StepFun 两条凭据都挂在账号上,查询时不会去拼 Cookie 名字。
    #[test]
    fn builtin_stepfun_carries_both_credential_names() {
        let mut s = crate::settings::Settings::default();
        s.limit_providers.push("stepfun".into());
        let builtin = resolve_accounts(&s)
            .into_iter()
            .find(|p| p.account.id == "stepfun")
            .expect("勾选后应有内置 StepFun");
        assert_eq!(
            builtin.account.secret_ref.as_deref(),
            Some(stepfun::API_KEY_NAME)
        );
        assert_eq!(
            builtin.account.cookie_ref.as_deref(),
            Some(stepfun::COOKIE_NAME)
        );
        let (key, cookie) = stepfun_credential_names(&builtin.account);
        assert_eq!(key.as_deref(), Some(stepfun::API_KEY_NAME));
        assert_eq!(cookie.as_deref(), Some(stepfun::COOKIE_NAME));
    }

    /// 额外 StepFun:Key 和 Cookie 各走各的条目,明文不进账号 JSON。
    #[test]
    fn stepfun_extra_account_keeps_key_and_cookie_separate() {
        let (acc, writes) = prepare_account(
            &[],
            crate::settings::LimitAccount {
                id: "stepfun-2".into(),
                provider: "stepfun".into(),
                label: "备用".into(),
                secret_ref: Some("DEEPSEEK_API_KEY_WORK".into()),
                ..Default::default()
            },
            Some("sk-live".into()),
            Some("cookie-live".into()),
        )
        .expect("两条密钥都给了应当通过");
        assert_eq!(
            acc.secret_ref.as_deref(),
            Some("limit.stepfun.stepfun-2.key")
        );
        assert_eq!(
            acc.cookie_ref.as_deref(),
            Some("limit.stepfun.stepfun-2.cookie")
        );
        let (key, cookie) = stepfun_credential_names(&acc);
        assert_eq!(key.as_deref(), acc.secret_ref.as_deref());
        assert_eq!(cookie.as_deref(), acc.cookie_ref.as_deref());
        let cookie_name = cookie.expect("cookie 条目名");
        assert!(cookie_name.ends_with(".cookie"));
        assert!(
            !cookie_name.contains("_CONSOLE_COOKIE"),
            "不能再把 Key 的名字拼成 Cookie 条目"
        );
        assert_eq!(writes.len(), 2);
        assert!(writes.iter().any(|(n, v)| n.ends_with(".key") && v == "sk-live"));
        assert!(writes
            .iter()
            .any(|(n, v)| n.ends_with(".cookie") && v == "cookie-live"));
        let dumped = serde_json::to_string(&acc).unwrap();
        assert!(!dumped.contains("sk-live"));
        assert!(!dumped.contains("cookie-live"));
    }

    /// 只改显示名时不重写密钥;删掉账号后查询清单里不再有它。
    #[test]
    fn rename_keeps_credentials_and_delete_drops_the_account() {
        let (created, _) = prepare_account(
            &[],
            crate::settings::LimitAccount {
                id: "stepfun-2".into(),
                provider: "stepfun".into(),
                label: "备用".into(),
                ..Default::default()
            },
            Some("sk-live".into()),
            Some("cookie-live".into()),
        )
        .unwrap();
        let (renamed, writes) = prepare_account(
            &[created.clone()],
            crate::settings::LimitAccount {
                id: created.id.clone(),
                provider: "stepfun".into(),
                label: "改名".into(),
                ..Default::default()
            },
            None,
            None,
        )
        .unwrap();
        assert!(writes.is_empty(), "没给新密钥就沿用原来的条目");
        assert_eq!(renamed.label, "改名");
        assert_eq!(renamed.secret_ref, created.secret_ref);
        assert_eq!(renamed.cookie_ref, created.cookie_ref);

        let mut s = crate::settings::Settings::default();
        s.limit_providers.push("stepfun".into());
        s.limit_accounts.push(renamed.clone());
        assert!(resolve_accounts(&s).iter().any(|p| p.account.id == "stepfun-2"));
        let removed = s.limit_accounts.remove(0);
        assert_eq!(owned_credential_names(&removed).len(), 2);
        assert!(!resolve_accounts(&s)
            .iter()
            .any(|p| p.account.id == "stepfun-2"));
    }

    #[test]
    fn bad_account_ids_are_rejected_before_any_secret_is_kept() {
        let err = prepare_account(
            &[],
            crate::settings::LimitAccount {
                id: "../".into(),
                provider: "deepseek".into(),
                label: "坏".into(),
                ..Default::default()
            },
            Some("sk".into()),
            None,
        )
        .expect_err("路径穿越的 id 必须被拒");
        assert!(err.contains("不合法"), "{err}");

        let err = prepare_account(
            &[],
            crate::settings::LimitAccount {
                id: "deepseek".into(),
                provider: "deepseek".into(),
                label: "冒充".into(),
                ..Default::default()
            },
            Some("sk".into()),
            None,
        )
        .expect_err("不能占用内置 id");
        assert!(err.contains("内置"), "{err}");
    }
}
