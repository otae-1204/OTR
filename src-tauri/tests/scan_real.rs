use std::collections::HashMap;

use otr_lib::providers::custom::CustomProvider;
use otr_lib::providers::{all_providers, FileCursor, ScanCtx, AgentProvider};
use otr_lib::settings::CustomAgentConfig;

/// 对本机真实数据做一次全量扫描冒烟验证:
///   cargo test -- --ignored --nocapture
/// 输出各 Provider 的记录数与 token 总量,可与独立脚本核算的数字比对
#[test]
#[ignore]
fn scan_real_data_smoke() {
    for p in all_providers() {
        if !p.detect() {
            println!("{}: not detected", p.id());
            continue;
        }
        let mut cursors: HashMap<String, FileCursor> = HashMap::new();
        let mut state = serde_json::Value::Null;
        let mut ctx = ScanCtx {
            full: true,
            cursors: &mut cursors,
            state: &mut state,
        };
        let recs = p.scan(&mut ctx).expect("scan should not fail");
        // 台账口径(进入按天表的数据);DSH 的会话流记录 skip_daily=true,与台账天然重复
        let daily: u64 = recs
            .iter()
            .filter(|r| !r.skip_daily)
            .map(|r| r.total_tokens())
            .sum();
        let total: u64 = recs.iter().map(|r| r.total_tokens()).sum();
        let input: u64 = recs.iter().map(|r| r.input_tokens).sum();
        let output: u64 = recs.iter().map(|r| r.output_tokens).sum();
        let cache_read: u64 = recs.iter().map(|r| r.cache_read_tokens).sum();
        println!(
            "{}: records={} daily_feed={} all_records={} in={} out={} cacheRead={}",
            p.id(),
            recs.len(),
            daily,
            total,
            input,
            output,
            cache_read
        );
        assert!(
            recs.iter().all(|r| r.ts >= 0),
            "{}: invalid timestamp",
            p.id()
        );
    }
}

/// 自定义 Agent 链路:用 CodeBuddy 的真实目录(claude-code 布局)验证 CustomProvider
#[test]
#[ignore]
fn scan_custom_agent_smoke() {
    let cfg = CustomAgentConfig {
        id: "custom-codebuddy".into(),
        name: "CodeBuddy".into(),
        kind: "claude-code".into(),
        dir: format!(
            "{}\\.codebuddy\\projects",
            std::env::var("USERPROFILE").unwrap_or_default()
        ),
    };
    let p = CustomProvider::new(cfg);
    assert!(p.detect(), "codebuddy dir should exist");
    assert_eq!(p.id(), "custom-codebuddy");
    assert_eq!(p.display_name(), "CodeBuddy");
    let mut cursors: HashMap<String, FileCursor> = HashMap::new();
    let mut state = serde_json::Value::Null;
    let mut ctx = ScanCtx {
        full: true,
        cursors: &mut cursors,
        state: &mut state,
    };
    let recs = p.scan(&mut ctx).expect("custom scan should not fail");
    let total: u64 = recs.iter().map(|r| r.total_tokens()).sum();
    println!(
        "custom-codebuddy: records={} total={} (目录存在,用量多少取决于实际使用)",
        recs.len(),
        total
    );
}
