use std::collections::HashMap;

use otr_lib::store::Store;
use otr_lib::{model::today_str, model::date_str};

/// 针对本机真实数据库验证 range_summary 各组合(排查"简览显示 0"):
///   cargo test --test range_smoke -- --ignored --nocapture
#[test]
#[ignore]
fn range_summary_combos() {
    // 新目录优先(OTAE),旧目录兜底(改名前的数据)
    let new_db = std::env::var("APPDATA").unwrap() + r"\com.otae.app\otae.db";
    let old_db = std::env::var("APPDATA").unwrap() + r"\com.token-show.app\token-show.db";
    let db = if std::path::Path::new(&new_db).exists() {
        new_db
    } else {
        old_db
    };
    let store = Store::open(std::path::Path::new(&db)).expect("open db");
    let pricing = HashMap::new();
    let today = today_str();
    let combos: Vec<(Option<&str>, String, String)> = vec![
        (None, date_str(chrono::Duration::days(29)), today.clone()),
        (None, today.clone(), today.clone()),
        (Some("claude-code"), "2000-01-01".into(), today.clone()),
        (Some("codex"), "2000-01-01".into(), today.clone()),
        (Some("dsh"), today.clone(), today.clone()),
        (Some("opencode"), today.clone(), today.clone()),
        (Some("opencode"), "2026-08-24".into(), "2026-08-24".into()),
        (Some("zcode"), today.clone(), today.clone()),
    ];
    for (agent, from, to) in combos {
        let s = store
            .range_summary(agent, &from, &to, &pricing, 7.2)
            .expect("range_summary ok");
        println!(
            "agent={:<12} {} ~ {} -> total={} in={} out={} cr={} calls={} cost={:.4}",
            agent.unwrap_or("(all)"),
            from,
            to,
            s.totals.total_tokens,
            s.totals.input_tokens,
            s.totals.output_tokens,
            s.totals.cache_read_tokens,
            s.totals.calls,
            s.totals.cost
        );
    }
}
