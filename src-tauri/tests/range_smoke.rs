use std::collections::HashMap;

use otr_lib::providers::all_providers;
use otr_lib::settings::Settings;
use otr_lib::store::{CostBasis, Store};
use otr_lib::{model::date_str, model::today_str};

/// 针对本机真实数据库验证 range_summary / sessions 各组合:
///   cargo test --release --test range_smoke -- --ignored --nocapture
///
/// 数据库位置随应用改名变过(com.otae.app / com.token-show.app -> com.otae.radar),
/// 所以按 新 -> 旧 顺序探测;一个都没有就跳过而不是 panic(标了 #[ignore] 没人跑,
/// 之前一直在对着早就废弃的路径 open db)。
#[test]
#[ignore]
fn range_summary_combos() {
    let appdata = std::env::var("APPDATA").unwrap_or_default();
    let dir = format!(r"{appdata}\com.otae.radar");
    let candidates = [
        format!(r"{dir}\radar.db"),
        format!(r"{appdata}\com.otae.app\otae.db"),
        format!(r"{appdata}\com.token-show.app\token-show.db"),
    ];
    let Some(db) = candidates.iter().find(|p| std::path::Path::new(p).is_file()) else {
        println!("未找到本机数据库,跳过。探测过: {candidates:?}");
        return;
    };
    println!("db = {db}");

    let store = Store::open(std::path::Path::new(db)).expect("open db");

    // 用真实的 settings.json(定价表/汇率/启用列表),否则测不出成本口径。
    // 这里直接反序列化而不走 Settings::load —— 后者会回写文件,测试不该动用户设置。
    let settings: Settings = std::fs::read_to_string(format!(r"{dir}\settings.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    println!(
        "settings: pricing={} 条 汇率={} 币种={} 启用={:?}",
        settings.pricing.len(),
        settings.exchange_rate,
        settings.currency,
        settings.enabled_agents
    );

    // Agent -> 自带成本币种,和 commands.rs 里组装的一致
    let currencies: HashMap<String, String> = all_providers()
        .iter()
        .filter_map(|p| {
            p.native_cost_currency()
                .map(|c| (p.id().to_string(), c.to_string()))
        })
        .collect();
    println!("自带成本币种: {currencies:?}");

    let basis = CostBasis::new(&settings.pricing, settings.exchange_rate, &currencies);
    let enabled = settings.enabled_agents.clone();

    let today = today_str();
    let combos: Vec<(Option<&str>, String, String)> = vec![
        (None, date_str(chrono::Duration::days(29)), today.clone()),
        (None, today.clone(), today.clone()),
        (Some("claude-code"), "2000-01-01".into(), today.clone()),
        (Some("codex"), "2000-01-01".into(), today.clone()),
        (Some("dsh"), today.clone(), today.clone()),
        (Some("opencode"), today.clone(), today.clone()),
        (Some("zcode"), today.clone(), today.clone()),
    ];
    for (agent, from, to) in combos {
        let s = store
            .range_summary(agent, &from, &to, &basis, Some(&enabled))
            .expect("range_summary ok");
        println!(
            "range agent={:<12} {} ~ {} -> total={} in={} out={} cr={} calls={} cost={:.4}",
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

    // 明细表与大卡现在共用 CostBasis。两张表的口径本来就不完全相同
    // (skip_daily 的记录只进 sessions 不进 usage_daily,例如 DSH projcache),
    // 所以这里只打印对照,不做相等断言。
    let from = date_str(chrono::Duration::days(29));
    let from_ms = chrono::NaiveDate::parse_from_str(&from, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .and_then(|t| t.and_local_timezone(chrono::Local).single())
        .map(|dt| dt.timestamp_millis());
    let summary = store
        .range_summary(None, &from, &today, &basis, Some(&enabled))
        .expect("range_summary ok");
    // 上限给足,避免"最近 N 条"截断导致两边不可比
    let sessions = store
        .sessions(None, from_ms, None, 100_000, &basis, Some(&enabled))
        .expect("sessions ok");
    let mut by_agent: HashMap<String, f64> = HashMap::new();
    for row in &sessions {
        *by_agent.entry(row.agent.clone()).or_default() += row.cost;
    }
    println!("--- 近 30 天按 Agent 成本:大卡(usage_daily) vs 明细(usage_session_models) ---");
    println!("(两表口径本就不同:daily 按记录日期过滤;sessions 是「列出范围内活跃的会话、");
    println!(" 展示该会话的完整累计」。跨范围开始的长会话会把范围外的部分算进明细,所以 codex/dsh 会有差额)");
    let window_start = from_ms.unwrap_or(0);
    for slice in &summary.by_agent {
        let rows: Vec<_> = sessions.iter().filter(|s| s.agent == slice.agent).collect();
        let spilling: Vec<_> = rows
            .iter()
            .filter(|s| s.started_at.is_some_and(|t| t > 0 && t < window_start))
            .collect();
        let spilling_cost: f64 = spilling.iter().map(|s| s.cost).sum();
        println!(
            "{:<12} 大卡={:>12.4}  明细={:>12.4}  会话数={:<5} 其中跨范围开始的={:<4} 这批占了成本 {:>10.4}",
            slice.agent,
            slice.totals.cost,
            by_agent.get(&slice.agent).copied().unwrap_or(0.0),
            rows.len(),
            spilling.len(),
            spilling_cost
        );
    }
}
