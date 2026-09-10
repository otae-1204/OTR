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
            force_full: false,
            cursors: &mut cursors,
            state: &mut state,
        };
        let recs = p.scan(&mut ctx).expect("scan should not fail");
        // 实际进入按天表的数据;DSH 有台账时取台账,无台账时由会话日志回退。
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

/// DSH per-file 缓存回归:同一份 state 连扫两次,第二次必须全部命中缓存。
/// 批次 4 的性能验收:冷扫 ~20s+ → 热扫亚秒级,且小时聚合逐字不变(纯记忆化)。
///   cargo test --release --test scan_real -- --ignored --nocapture
#[test]
#[ignore]
fn dsh_rescan_hits_file_cache() {
    let p = otr_lib::providers::dsh::DshProvider;
    if !p.detect() {
        println!("dsh: not detected");
        return;
    }
    let mut cursors: HashMap<String, FileCursor> = HashMap::new();
    let mut state = serde_json::Value::Null;

    let t0 = std::time::Instant::now();
    let first = {
        let mut ctx = ScanCtx {
            full: true,
            force_full: false,
            cursors: &mut cursors,
            state: &mut state,
        };
        p.scan(&mut ctx).expect("cold scan")
    };
    let cold = t0.elapsed();
    let hourly_after_cold = state.get("hourly").cloned();

    let t1 = std::time::Instant::now();
    let second = {
        let mut ctx = ScanCtx {
            full: false,
            force_full: false,
            cursors: &mut cursors,
            state: &mut state,
        };
        p.scan(&mut ctx).expect("warm scan")
    };
    let warm = t1.elapsed();

    let cached = state
        .get("fileCache")
        .and_then(|v| v.as_object())
        .map(|m| m.len())
        .unwrap_or(0);
    let state_kib = serde_json::to_vec(&state).map(|v| v.len()).unwrap_or(0) / 1024;
    println!(
        "dsh 冷扫 {cold:?}(records={}) / 热扫 {warm:?}(records={}) fileCache={cached} 个文件 state={state_kib} KiB",
        first.len(),
        second.len()
    );
    assert!(cached > 0, "state 里必须有 per-file 缓存");
    assert!(second.is_empty(), "文件没变时热扫不该产出任何新记录");
    assert_eq!(
        state.get("hourly").cloned(),
        hourly_after_cold,
        "热扫必须逐字复用缓存聚合,不能把小时基线冲掉"
    );
    assert!(warm * 5 < cold, "热扫必须显著快于冷扫");
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
    if !p.detect() {
        // 本机没装 CodeBuddy 时不该让整轮冒烟失败(以前硬 assert,导致 --ignored 跑必挂一条)
        println!("custom-codebuddy: 目录不存在,跳过");
        return;
    }
    assert_eq!(p.id(), "custom-codebuddy");
    assert_eq!(p.display_name(), "CodeBuddy");
    let mut cursors: HashMap<String, FileCursor> = HashMap::new();
    let mut state = serde_json::Value::Null;
    let mut ctx = ScanCtx {
        full: true,
        force_full: false,
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
