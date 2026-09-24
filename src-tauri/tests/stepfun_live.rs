//! StepFun 订阅额度的真实网络验收(需要已配置的凭据)。
//!
//!     cargo test --test stepfun_live -- --ignored --nocapture
//!
//! 只读凭据、只发网络请求;**不写回任何文件**(续期后的 token 只留内存)。
use otr_lib::limits::stepfun;

#[test]
#[ignore]
fn stepfun_plan_reads_or_reports_clearly() {
    let raw = otr_lib::limits::credential::resolve(stepfun::COOKIE_NAME);
    println!("凭据存在 = {}", raw.is_some());
    if let Some(r) = &raw {
        println!("原始长度 = {}", r.len());
        match stepfun::debug_extract_journal(r) {
            Some(j) => {
                println!("解析出 journal = {} 字符", j.len());
                println!("  含 '...' 分隔 = {}", j.contains("..."));
                println!(
                    "  device_id = {:?}",
                    stepfun::journal_device_id(&j)
                );
            }
            None => println!("  没解析出 journal"),
        }
        match stepfun::check_credential(r) {
            Ok(()) => println!("凭据自检: 通过(含刷新令牌,可自动续期)"),
            Err(e) => println!("凭据自检: {e}"),
        }
    }

    let got = stepfun::fetch_plan();
    println!("--- 结果 ---");
    println!("  configured = {}", got.configured);
    println!("  plan_label = {:?}", got.plan_label);
    println!("  error      = {:?}", got.error);
    println!("  windows    = {}", got.windows.len());
    for w in &got.windows {
        println!(
            "    {:<14} {:?}%  reset={:?} secs={:?}",
            w.label, w.used_percent, w.reset_at, w.window_seconds
        );
    }
    println!(
        "\n结论: {}",
        if !got.windows.is_empty() {
            "读到额度数据"
        } else {
            "未读到数据(看上面 error)"
        }
    );
}
