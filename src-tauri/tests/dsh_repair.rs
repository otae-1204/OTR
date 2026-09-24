//! 端到端验收:在**真实 radar.db 的副本**上复现用户现场并验证修复。
//!
//! 默认 #[ignore]:依赖本机 ~/.dsh 与 %APPDATA%\com.otae.radar 的实际内容。
//! 手动跑:`cargo test --test dsh_repair -- --ignored --nocapture`
//!
//! **只读源库**:先把 radar.db 复制到临时目录再操作,绝不写原库。

use std::collections::HashMap;

use otr_lib::providers::dsh::DshProvider;
use otr_lib::providers::{AgentProvider, ScanCtx};
use otr_lib::store::Store;

/// 每个测试用**独立**的临时目录:两个测试并行跑时共用一份副本会互相踩
/// (一个在重建、另一个在读,断言随机失败)。
fn copy_live_db(tag: &str) -> std::path::PathBuf {
    let src = dirs::data_dir()
        .expect("no roaming dir")
        .join("com.otae.radar")
        .join("radar.db");
    assert!(src.is_file(), "找不到真实 radar.db: {}", src.display());
    let dir = std::env::temp_dir().join(format!("otr-repair-{}-{}", std::process::id(), tag));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dst = dir.join("radar.db");
    std::fs::copy(&src, &dst).unwrap();
    // WAL/SHM 一并带上,副本才是完整状态
    for suffix in ["-wal", "-shm"] {
        let s = std::path::PathBuf::from(format!("{}{}", src.display(), suffix));
        if s.is_file() {
            let _ = std::fs::copy(&s, format!("{}{}", dst.display(), suffix));
        }
    }
    dst
}

/// 某个 Agent 在某天的按天 tokens(agent 作用域,避免被别的 Agent 污染)
fn daily_tokens(store: &Store, agent: &str, date: &str) -> u64 {
    store
        .daily(Some(agent), date, date, "day")
        .unwrap()
        .iter()
        .map(|r| r.total_tokens)
        .sum()
}

/// 同一天的按小时表合计 —— 会话日志口径,是"真实发生过的用量"。
fn hourly_tokens(store: &Store, agent: &str, date: &str) -> u64 {
    store
        .daily(Some(agent), date, date, "hour")
        .unwrap()
        .iter()
        .map(|r| r.total_tokens)
        .sum()
}

/// 某个 Agent 在某天某个模型的按天 tokens(台账逐桶负责,所以要看到桶这一层)。
/// 这里只关心 token 数,成本口径随便给一个空表即可。
fn model_tokens(store: &Store, agent: &str, date: &str, model: &str) -> u64 {
    let pricing = HashMap::new();
    let native = HashMap::new();
    let basis = otr_lib::store::CostBasis::new(&pricing, 7.2, &native);
    store
        // 第 6 个参数是峰谷占比;这里只看 token 数,与计价无关
        .range_summary(Some(agent), date, date, &basis, None, None)
        .unwrap()
        .by_model
        .iter()
        .filter(|m| m.model == model)
        .map(|m| m.totals.total_tokens)
        .sum()
}

/// 按小时表里出现过的、且**不属于台账**的日期(升序)。
fn log_dates(store: &Store, agent: &str, owned: &std::collections::HashSet<String>) -> Vec<String> {
    let mut dates: Vec<String> = Vec::new();
    for row in store.daily(Some(agent), "2026-01-01", "2026-12-31", "hour").unwrap() {
        let date = row.date.split(' ').next().unwrap_or_default().to_string();
        if !dates.contains(&date) {
            dates.push(date);
        }
    }
    dates.retain(|d| !owned.contains(d));
    dates.sort();
    dates
}

/// 用户现场:按天表**不是空的**,而是被截断成一小段(今日只剩 511.3K、只有一个模型),
/// 按小时表却是完整的。增量语义下这段增量不会重放,只能靠按小时表校正回来。
#[test]
#[ignore]
fn reconcile_restores_days_the_daily_table_only_partially_recorded() {
    let db = copy_live_db("reconcile");
    let store = Store::open(&db).unwrap();
    let p = DshProvider;
    let live_state: serde_json::Value = store
        .get_kv("state:dsh")
        .and_then(|s| serde_json::from_str(&s).ok())
        .expect("线上必须有 state:dsh");
    let owned = p.ledger_owned_buckets(&live_state).expect("DSH 必须声明台账桶");
    println!("台账负责的桶: {owned:?}");

    let dates = log_dates(&store, "dsh", &owned);
    let broken: Vec<(String, u64, u64)> = dates
        .iter()
        .map(|d| {
            (
                d.clone(),
                daily_tokens(&store, "dsh", d),
                hourly_tokens(&store, "dsh", d),
            )
        })
        .filter(|(_, daily, hourly)| *daily < *hourly)
        .collect();
    println!("现场按天少于按小时的日期: {broken:?}");
    assert!(
        !broken.is_empty(),
        "这份库没有复现出问题,无法验证修复(可能已经修过了)"
    );

    // 1) 增量扫描:基线已在,这些日期的增量基本都是 0 → 扫描本身补不了它们
    let mut cursors = HashMap::new();
    let mut state = live_state.clone();
    let mut ctx = ScanCtx {
        full: false,
        force_full: false,
        cursors: &mut cursors,
        state: &mut state,
    };
    let records = p.scan(&mut ctx).expect("scan 失败");
    store.apply_records(&records).unwrap();

    // 2) 校正:这些日期由按小时表重算补回
    let owned = p.ledger_owned_buckets(&state).expect("DSH 必须声明台账桶");
    let ledger_before = model_tokens(&store, "dsh", "2026-08-31", "deepseek-v4-flash-vision-exp");
    let filled = store.reconcile_daily_from_hourly("dsh", &owned).unwrap();
    println!("校正行数: {filled}");

    for (date, before, hourly) in &broken {
        let after = daily_tokens(&store, "dsh", date);
        println!("  {date} 按天 {before} -> {after}(按小时 {hourly})");
        // 校正只增不减,所以按天最终 = 按小时 + 台账写下的那部分盈余。
        // 不变式是"按天不再低于按小时",不是"逐位相等"。
        assert!(
            after > *before,
            "{date} 按天没有被校正(仍是 {after})→ 当天统计仍然偏小"
        );
        assert!(
            after >= *hourly,
            "{date} 校正后按天 {after} 仍低于按小时 {hourly} → 当天统计仍然偏小"
        );
    }

    // 3) 台账负责的**桶**原样不动(不双计)。逐桶判定:08-31 那天台账只认领了
    //    deepseek-official:deepseek-v4-flash-vision-exp,同一天别的模型仍归日志校正。
    let owned_model = "deepseek-v4-flash-vision-exp";
    let after = model_tokens(&store, "dsh", "2026-08-31", owned_model);
    assert_eq!(
        after, ledger_before,
        "校正改动了台账负责的桶({owned_model}) → 双计"
    );

    // 4) 幂等
    assert_eq!(store.reconcile_daily_from_hourly("dsh", &owned).unwrap(), 0);
    let _ = std::fs::remove_dir_all(db.parent().unwrap());
}

/// 全量重建的安全性:会话日志里已经**不再包含** 08-14..09-17 的用量事件
/// (DSH 升级换代后旧日志被重写),所以 replace_agent 之后这些日期的按天行会整段消失。
/// 校正必须把它们从按小时表恢复回来,否则一次 parser 版本升级就等于抹掉一个月历史。
#[test]
#[ignore]
fn full_rebuild_does_not_destroy_history_thanks_to_reconcile() {
    let db = copy_live_db("rebuild");
    let store = Store::open(&db).unwrap();
    let p = DshProvider;

    let historical = ["2026-08-14", "2026-08-21", "2026-09-16", "2026-09-17"];
    for d in historical {
        assert!(
            daily_tokens(&store, "dsh", d) > 0,
            "前提不成立:{d} 本应有历史按天数据"
        );
    }

    // 全量重建:清空 dsh 所有行,只用当前日志重写(日志已不含上述日期)
    let mut cursors = HashMap::new();
    let mut state = serde_json::Value::Null;
    let mut ctx = ScanCtx { full: true, force_full: false, cursors: &mut cursors, state: &mut state };
    let records = p.scan(&mut ctx).expect("scan 失败");
    store
        .replace_agent("dsh", &records, &cursors, &state)
        .unwrap();

    let lost: Vec<&str> = historical
        .iter()
        .copied()
        .filter(|d| daily_tokens(&store, "dsh", d) == 0)
        .collect();
    println!("全量重建后按天缺失的历史日期: {lost:?}");

    // 校正:按小时表仍在,历史必须被恢复
    let owned = p.ledger_owned_buckets(&state).expect("DSH 必须声明台账桶");
    let filled = store.reconcile_daily_from_hourly("dsh", &owned).unwrap();
    println!("校正行数: {filled}");
    for d in historical {
        assert!(
            daily_tokens(&store, "dsh", d) > 0,
            "全量重建 + 校正后 {d} 的历史仍然丢失"
        );
    }
    let _ = std::fs::remove_dir_all(db.parent().unwrap());
}
