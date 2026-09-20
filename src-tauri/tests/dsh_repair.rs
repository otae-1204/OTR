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

/// 复现用户现场:**增量**扫描。state:dsh 里已经存着被消费掉的按小时基线,
/// 所以台账丢掉当天记录后,这些日期的增量不会再被重放 —— 只能靠回填修回来。
#[test]
#[ignore]
fn incremental_scan_plus_backfill_restores_days_lost_to_a_stale_ledger() {
    let db = copy_live_db("incremental");
    let store = Store::open(&db).unwrap();

    let p = DshProvider;
    // 关键:拿**真实**的 state:dsh 当基线,而不是 Null —— 这才是线上路径。
    let live_state: serde_json::Value = store
        .get_kv("state:dsh")
        .and_then(|s| serde_json::from_str(&s).ok())
        .expect("线上必须有 state:dsh");
    let owned = p.ledger_owned_dates(&live_state).expect("DSH 必须声明台账日期");
    println!("台账负责的日期: {owned:?}");

    // 找出现场"按小时有数据、按天一条没有"的日期 —— 用户报的就是这些日子。
    let mut broken: Vec<String> = Vec::new();
    for row in store.daily(Some("dsh"), "2026-01-01", "2026-12-31", "hour").unwrap() {
        let date = row.date.split(' ').next().unwrap_or_default().to_string();
        if owned.contains(&date) || broken.contains(&date) {
            continue;
        }
        if daily_tokens(&store, "dsh", &date) == 0 {
            broken.push(date);
        }
    }
    broken.sort();
    println!("现场缺失按天数据的日期: {broken:?}");
    assert!(
        !broken.is_empty(),
        "这份库没有复现出问题,无法验证修复(可能已经修过了)"
    );

    // 1) 增量扫描:基线已在,这些日期的增量基本都是 0 → 主修复补不了它们
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
    let still_broken: Vec<&String> = broken
        .iter()
        .filter(|d| daily_tokens(&store, "dsh", d) == 0)
        .collect();
    println!("增量扫描后仍缺按天的日期: {still_broken:?}");

    // 2) 回填:这些日期由按小时表重算补回
    let owned = p.ledger_owned_dates(&state).expect("DSH 必须声明台账日期");
    let ledger_before = daily_tokens(&store, "dsh", "2026-08-31");
    let filled = store.backfill_daily_from_hourly("dsh", &owned).unwrap();
    println!("回填行数: {filled}");

    for date in &broken {
        let tokens = daily_tokens(&store, "dsh", date);
        println!("  {date} 按天 tokens={tokens}");
        assert!(tokens > 0, "{date} 仍未补回按天数据");
    }

    // 3) 台账负责的日期原样不动(不双计)
    assert_eq!(
        daily_tokens(&store, "dsh", "2026-08-31"),
        ledger_before,
        "回填改动了台账负责的日期 → 双计"
    );

    // 4) 幂等
    assert_eq!(store.backfill_daily_from_hourly("dsh", &owned).unwrap(), 0);
    let _ = std::fs::remove_dir_all(db.parent().unwrap());
}

/// 全量重建的安全性:会话日志里已经**不再包含** 08-14..09-17 的用量事件
/// (DSH 升级换代后旧日志被重写),所以 replace_agent 之后这些日期的按天行会整段消失。
/// 回填必须把它们从按小时表恢复回来,否则一次 parser 版本升级就等于抹掉一个月历史。
#[test]
#[ignore]
fn full_rebuild_does_not_destroy_history_thanks_to_backfill() {
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

    // 回填:按小时表仍在,历史必须被恢复
    let owned = p.ledger_owned_dates(&state).expect("DSH 必须声明台账日期");
    let filled = store.backfill_daily_from_hourly("dsh", &owned).unwrap();
    println!("回填行数: {filled}");
    for d in historical {
        assert!(
            daily_tokens(&store, "dsh", d) > 0,
            "全量重建 + 回填后 {d} 的历史仍然丢失"
        );
    }
    let _ = std::fs::remove_dir_all(db.parent().unwrap());
}