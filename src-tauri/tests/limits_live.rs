//! 真实网络验收:对着各家的线上接口跑一遍,断言 A3 / A4 / A5。
//!
//! `#[ignore]`:需要本机真实登录态/密钥,不能在 CI 里跑。
//!
//!     cargo test --test limits_live -- --ignored --nocapture
//!
//! 本文件**只读**凭据:不写回 ~/.codex/auth.json,不改任何别人的文件。

use otr_lib::limits;

fn dump(name: &str, p: &limits::ProviderLimits) {
    println!("--- {name} ---");
    println!("  account      = {} ({})", p.account_label, p.account_id);
    println!("  configured   = {}", p.configured);
    println!("  plan         = {:?}", p.plan_label);
    println!("  error        = {:?}", p.error);
    for w in &p.windows {
        println!(
            "  window       = {:<10} {:>8}  reset_at={:?} seconds={:?}",
            w.key,
            w.used_percent.map(|v| format!("{v:.4}%")).unwrap_or_else(|| "--".into()),
            w.reset_at,
            w.window_seconds
        );
    }
    if let Some(b) = &p.balance {
        println!(
            "  balance      = {:.4} {} (cash={:?} voucher={:?})",
            b.amount, b.currency, b.cash, b.voucher
        );
    }
}

/// A3:Cursor 的百分比必须来自 totalPercentUsed 阶梯,而不是 used/limit 的 100%
#[test]
#[ignore]
fn a3_cursor_percent_is_not_the_used_over_limit_trap() {
    let got = limits::cursor::fetch();
    dump("Cursor", &got);
    if !got.configured {
        eprintln!("跳过:未找到 Cursor 登录态");
        return;
    }
    assert!(got.error.is_none(), "Cursor 查询失败: {:?}", got.error);
    let plan = got
        .windows
        .iter()
        .find(|w| w.key == "plan")
        .expect("必须有套餐窗口");
    let p = plan.used_percent.expect("A3:必须有百分比");
    assert!(
        (p - 100.0).abs() > 1.0,
        "A3 失败:百分比是 {p},看起来又落进了 used/limit 的 100% 陷阱"
    );
    println!("A3 通过:Cursor used_percent = {p:.4}(不是 100)");
}

/// A4:Codex 的 30 天窗口必须被识别为「每月」,不能误判成「5 小时」
#[test]
#[ignore]
fn a4_codex_thirty_day_window_is_classified_as_monthly() {
    let got = limits::codex::fetch();
    dump("Codex", &got);
    if !got.configured {
        eprintln!("跳过:未找到 Codex 登录态");
        return;
    }
    assert!(got.error.is_none(), "Codex 查询失败: {:?}", got.error);
    // 现场是免费档:只有一个 2592000 秒的窗口
    let monthly = got.windows.iter().find(|w| w.key == "monthly");
    if let Some(w) = monthly {
        assert_eq!(w.window_seconds, Some(2_592_000));
        println!("A4 通过:Codex 30 天窗口被归为「每月」({:?})", w.used_percent);
    } else {
        println!(
            "A4:本机当前账号没有 30 天窗口,实际窗口 = {:?}",
            got.windows.iter().map(|w| (w.key.clone(), w.window_seconds)).collect::<Vec<_>>()
        );
    }
    // 无论哪种档位,窗口键都必须是分类出来的,不能是 "primary"/"secondary" 兜底
    for w in &got.windows {
        assert!(
            w.key != "primary" && w.key != "secondary",
            "A4 失败:窗口 {:?} 走了位置兜底,说明秒数分类没生效",
            w
        );
    }
}

/// A5:DeepSeek 余额真实可读
#[test]
#[ignore]
fn a5_deepseek_balance_is_real() {
    let got = limits::deepseek::fetch();
    dump("DeepSeek", &got);
    if !got.configured {
        eprintln!("跳过:未配置 DEEPSEEK_API_KEY");
        return;
    }
    assert!(got.error.is_none(), "DeepSeek 查询失败: {:?}", got.error);
    let b = got.balance.expect("A5:必须有余额");
    assert!(b.amount >= 0.0 && b.amount < 1e6, "余额量级可疑: {}", b.amount);
    println!("A5 通过:DeepSeek 余额 = {:.2} {}", b.amount, b.currency);
}

/// A8:StepFun 默认关闭;启用但无凭据时给明确提示,不崩溃
#[test]
#[ignore]
fn a8_stepfun_is_off_by_default_and_degrades_gracefully() {
    let s = otr_lib::settings::Settings::default();
    assert!(
        !s.limit_providers.iter().any(|p| p == "stepfun"),
        "A8 失败:StepFun 不该默认开启"
    );
    let plans = limits::resolve_accounts(&s);
    assert!(
        !plans.iter().any(|p| p.account.provider == "stepfun"),
        "A8 失败:未启用时不该出现在账号清单里"
    );
    // 即便强行问一次,也只能得到引导文案
    let got = limits::stepfun::fetch_plan();
    dump("StepFun(未配置)", &got);
    assert!(!got.windows.is_empty() || got.error.is_some(), "要么有数据要么有说明");
    println!("A8 通过:StepFun 默认关闭,无凭据时给出说明而非崩溃");
}

/// A9:凭据清单只吐布尔值,任何地方都不出现密钥明文。
///
/// 这条是额度页唯一必须守住的边界:密钥一旦能从某个命令读出来,它就会出现在
/// 前端内存、开发者工具和任何日志里。
#[test]
#[ignore]
fn a9_credential_listing_never_exposes_secrets() {
    let list = limits::credential::list();
    for (name, present) in &list {
        println!("  {name} present={present}");
    }

    // 收集所有真实存在的密钥值(如果配了)
    let secrets: Vec<(&str, String)> = list
        .iter()
        .filter_map(|(name, _)| {
            limits::credential::resolve(name).map(|s| (name.as_str(), s.trim().to_string()))
        })
        .filter(|(_, s)| s.len() >= 8) // 太短的值会误报
        .collect();
    println!("  实际配置了 {} 个凭据,逐个反查是否泄漏", secrets.len());

    // 1) 清单本身
    let listing = serde_json::to_string(&list).unwrap();
    // 2) 账号视图(含 secretRef —— 那只是**条目名**,不能是值)
    let mut s = otr_lib::settings::Settings::default();
    s.limit_accounts.push(otr_lib::settings::LimitAccount {
        id: "probe".into(),
        provider: "deepseek".into(),
        label: "probe".into(),
        secret_ref: Some("DEEPSEEK_API_KEY".into()),
        ..Default::default()
    });
    let accounts = serde_json::to_string(
        &limits::resolve_accounts(&s)
            .into_iter()
            .map(|p| p.account)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    // 3) 真实的额度结果(最容易顺手把整个响应带出去的地方)
    let fetched = serde_json::to_string(&limits::deepseek::fetch()).unwrap();
    let settings_json = serde_json::to_string(&s).unwrap();

    for (name, secret) in &secrets {
        for (what, haystack) in [
            ("凭据清单", &listing),
            ("账号视图", &accounts),
            ("额度结果", &fetched),
            ("设置序列化", &settings_json),
        ] {
            assert!(
                !haystack.contains(secret.as_str()),
                "A9 失败:{what} 里出现了 {name} 的密钥明文"
            );
        }
    }
    println!(
        "A9 通过:{} 个条目只暴露存在性,{} 个真实密钥均未出现在任何返回值里",
        list.len(),
        secrets.len()
    );
}

/// A10:两个账号各自独立(用真实 fetch 走一遍,确认 account_id 不串)
#[test]
#[ignore]
fn a10_two_accounts_stay_separate() {
    let mut s = otr_lib::settings::Settings::default();
    s.limit_accounts.push(otr_lib::settings::LimitAccount {
        id: "cursor-work".into(),
        provider: "cursor".into(),
        label: "工作".into(),
        home: Some(
            std::env::temp_dir()
                .to_string_lossy()
                .to_string(),
        ),
        ..Default::default()
    });
    let plans = limits::resolve_accounts(&s);
    let ids: Vec<_> = plans.iter().map(|p| p.account.id.clone()).collect();
    println!("账号清单 = {ids:?}");
    assert!(ids.contains(&"cursor".to_string()), "内置账号在");
    assert!(ids.contains(&"cursor-work".to_string()), "额外账号在");

    let fetched: Vec<_> = plans
        .iter()
        .filter(|p| p.account.provider == "cursor")
        .map(limits::fetch_account)
        .collect();
    for p in &fetched {
        dump(&format!("账号 {}", p.account_id), p);
    }
    // 每个账号的 account_id 必须是自己的,不能被后一个覆盖成同一个
    let got: Vec<_> = fetched.iter().map(|p| p.account_id.clone()).collect();
    let mut uniq = got.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(uniq.len(), got.len(), "A10 失败:account_id 重复了 → 会互相覆盖: {got:?}");
    println!("A10 通过:{} 个账号各自独立", got.len());
}
