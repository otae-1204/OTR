//! 定价修正与峰谷计价的验收(在真实库副本上跑,不动线上数据)。
//!
//! `#[ignore]`:需要本机真实数据库。
//!
//!     cargo test --test pricing_peak_acc -- --ignored --nocapture
//!
//! 断言 A6(修正生效且不越权)与 A7(峰谷生效且不改 token 数)。
use std::collections::HashMap;

use otr_lib::pricing;
use otr_lib::settings::{PeakTier, PriceEntry, Settings};
use otr_lib::store::{CostBasis, Store};

fn copy_live_db(tag: &str) -> std::path::PathBuf {
    let src = dirs::data_dir().unwrap().join("com.otae.radar").join("radar.db");
    let dir = std::env::temp_dir().join(format!("otr-acc-{}-{}", std::process::id(), tag));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dst = dir.join("radar.db");
    std::fs::copy(&src, &dst).unwrap();
    for s in ["-wal", "-shm"] {
        let p = std::path::PathBuf::from(format!("{}{}", src.display(), s));
        if p.is_file() { let _ = std::fs::copy(&p, format!("{}{}", dst.display(), s)); }
    }
    dst
}

/// 线上迁移**之前**的实际取值(2026-09-21 实测记录)
const WRONG: &[(&str, f64, f64, f64)] = &[
    ("deepseek-v4.1-flash", 2.0, 1.0, 0.02),
    ("deepseek-v4-pro", 0.435, 0.87, 0.003625),
    ("deepseek-v4-pro-0813", 1.32, 3.96, 0.044),
    ("deepseek-v4-flash", 0.14, 0.28, 0.0028),
];

#[test]
#[ignore]
fn acc_pricing_and_peak() {
    let db = copy_live_db("acc");
    let store = Store::open(&db).unwrap();

    // 现在的线上设置(已迁移)
    let now = Settings::load(&dirs::data_dir().unwrap().join("com.otae.radar").join("settings.json"));
    // 还原成迁移前的样子
    let mut before = now.clone();
    for (m, i, o, c) in WRONG {
        before.pricing.insert((*m).to_string(), PriceEntry {
            input: *i, output: *o, cache_read: *c, cache_write: 0.0, peak: None,
        });
    }
    // 再跑一次迁移,验证它把上面这些改回来
    let mut after = before.clone();
    let changed = pricing::migrate(&mut after);
    println!("迁移是否改动 = {changed}");
    for (m, i, o, c) in WRONG {
        let b = before.pricing.get(*m).unwrap();
        let a = after.pricing.get(*m).unwrap();
        println!("  {m}: ({i},{o},{c}) -> ({},{},{})  peak={:?}", a.input, a.output, a.cache_read, a.peak.as_ref().map(|p| p.input));
        assert_ne!((a.input, a.output, a.cache_read), (b.input, b.output, b.cache_read), "{m} 的已知错误值没有被修正");
        assert!(a.peak.is_some(), "{m} 必须补上峰谷档");
    }
    // 幂等
    assert!(!pricing::migrate(&mut after), "迁移必须幂等");

    let cur = HashMap::new();
    let from = "2000-01-01"; let to = "2100-01-01";
    let cost_of = |s: &Settings, use_peak: bool| -> (f64, HashMap<String, f64>) {
        let base = CostBasis::new(&s.pricing, s.exchange_rate, &cur);
        let peaks = store.peak_shares(None, from, to).unwrap();
        let basis = if use_peak { base.with_peaks(&peaks) } else { base };
        let r = store.range_summary(None, from, to, &basis, None, if use_peak { Some(&peaks) } else { None }).unwrap();
        (r.totals.cost, r.by_model.iter().map(|m| (m.model.clone(), m.totals.cost)).collect())
    };

    let (c_before, models_before) = cost_of(&before, false);
    let (c_off, models_off) = cost_of(&after, false);
    let (c_peak, models_peak) = cost_of(&after, true);
    println!("全应用成本: 修正前 {c_before:.2} → 修正后(纯平价) {c_off:.2} → 加峰谷 {c_peak:.2}");
    assert!(c_off < c_before, "定价修正必须把虚增的成本降下来");
    assert!(c_peak > c_off, "加峰谷后成本必须高于纯空闲价");
    for m in ["deepseek-v4.1-flash", "deepseek-v4-pro"] {
        let b = models_before.get(m).copied().unwrap_or(0.0);
        let o = models_off.get(m).copied().unwrap_or(0.0);
        let p = models_peak.get(m).copied().unwrap_or(0.0);
        println!("  {m}: 修正前 {b:.2} → 平价 {o:.2} → 含峰谷 {p:.2}  (峰谷倍数 x{:.4})", if o > 0.0 { p / o } else { 0.0 });
    }
    let flash_before = models_before.get("deepseek-v4.1-flash").copied().unwrap_or(0.0);
    let flash_after = models_off.get("deepseek-v4.1-flash").copied().unwrap_or(0.0);
    println!("A6: deepseek-v4.1-flash {flash_before:.2} -> {flash_after:.2} ({:.3}x)", flash_after / flash_before);

    // A7:峰值只影响成本、不影响 token 数
    let base = CostBasis::new(&after.pricing, after.exchange_rate, &cur);
    let peaks = store.peak_shares(None, from, to).unwrap();
    let r_off = store.range_summary(None, from, to, &base, None, None).unwrap();
    let r_on = store.range_summary(None, from, to, &base.with_peaks(&peaks), None, Some(&peaks)).unwrap();
    assert_eq!(r_off.totals.total_tokens, r_on.totals.total_tokens, "峰谷不能改变 token 数");
    assert_eq!(r_off.totals.input_tokens, r_on.totals.input_tokens);
    assert_eq!(r_off.totals.output_tokens, r_on.totals.output_tokens);
    assert_eq!(r_off.totals.cache_read_tokens, r_on.totals.cache_read_tokens);
    assert_eq!(r_off.totals.calls, r_on.totals.calls);
    assert_eq!(r_off.by_model.len(), r_on.by_model.len());
    println!("A7: token 数逐位一致 = {}", r_off.totals.total_tokens);

    // 用户自填的非已知错误值不被覆盖
    let mut user = after.clone();
    user.pricing.insert("deepseek-v4.1-flash".into(), PriceEntry {
        input: 9.99, output: 8.88, cache_read: 0.5, cache_write: 0.0, peak: None,
    });
    assert!(!pricing::migrate(&mut user), "用户自填值必须原样保留");
    println!("A6: 用户自填 9.99/8.88/0.5 未被覆盖");
    let _ = std::fs::remove_dir_all(db.parent().unwrap());
    let _ = PeakTier::default();
}
