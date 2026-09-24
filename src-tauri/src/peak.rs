//! 峰谷计价:把按天成本按**真实发生的小时**拆成平价段与高峰段。
//!
//! DeepSeek 的高峰时段是北京时间周一至周五 09:00-12:00、14:00-18:00,高峰单价是
//! 平价的 2 倍。按天表没有小时信息,所以要从按小时表里取出每个
//! (date, hour, model, provider) 的四类 token 数,判出各自是否落在高峰,再聚合成
//! "该 (date, model, provider) 桶里每类 token 的高峰占比"。
//!
//! 两条设计约束:
//!
//! 1. **token 数仍以按天表为准**。本机实测按小时表与按天表差 8%(按小时表缺近期日),
//!    拿按小时表出总额会让日总额跳变。峰谷只改变**单价**,不改变 token 数。
//! 2. **时区换算在 Rust 里做,不在 SQL 里**。库里存的 (date, hour) 是**本地**时间,
//!    高峰定义是北京时间;直接拿本地 hour 判高峰在 UTC+8 本机碰巧等价,换个时区就错,
//!    跨日边界也会错。这里用 chrono 把"本地 date+hour"转成北京时间再判。

use std::collections::HashMap;

/// 每类 token 落在高峰时段的比例(0.0 = 全平价,1.0 = 全高峰)
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PeakShare {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

impl PeakShare {
    /// 按类型加权合并:同桶内多行(不同小时)按各自的 token 量合成一个占比
    fn weighted(&self, other: &PeakShare, w: f64, ow: f64) -> PeakShare {
        let total = w + ow;
        if total <= 0.0 {
            return PeakShare::default();
        }
        let mix = |a: f64, b: f64| (a * w + b * ow) / total;
        PeakShare {
            input: mix(self.input, other.input),
            output: mix(self.output, other.output),
            cache_read: mix(self.cache_read, other.cache_read),
            cache_write: mix(self.cache_write, other.cache_write),
        }
    }
}

/// 按 (date, model, provider) 索引的峰值占比表
#[derive(Debug, Default)]
pub struct PeakShares {
    by_bucket: HashMap<(String, String, String), PeakShare>,
    bucket_weight: HashMap<(String, String, String), f64>,
    /// 兜底:某模型在整个查询区间里的整体占比(该桶没有小时数据时用)
    by_model: HashMap<String, PeakShare>,
    model_weight: HashMap<String, f64>,
}

impl PeakShares {
    /// 某桶的峰值占比。桶没有小时数据 → 回退到该模型的区间整体占比;
    /// 该模型完全没有小时数据 → 全平价(p=0),与加峰谷之前逐位一致。
    pub fn get(&self, date: &str, model: &str, provider: &str) -> PeakShare {
        if let Some(s) = self
            .by_bucket
            .get(&(date.to_string(), model.to_string(), provider.to_string()))
        {
            return *s;
        }
        self.by_model.get(model).copied().unwrap_or_default()
    }

    /// 该模型在整个区间里的整体峰值占比(明细表按会话聚合、拿不到逐日桶时用)。
    /// 与逐桶取值同源同口径,只是分辨率粗一档。
    pub fn model_share(&self, model: &str) -> PeakShare {
        self.by_model.get(model).copied().unwrap_or_default()
    }

    /// 并入一个小时的观测(供 Store 从按小时表构建)
    pub fn add_public(&mut self, date: &str, model: &str, provider: &str, share: PeakShare, weight: f64) {
        self.add(date, model, provider, share, weight);
    }

    /// 并入一个小时的观测:share 是该小时的峰值占比(0 或 1),weight 是它的 token 量
    fn add(&mut self, date: &str, model: &str, provider: &str, share: PeakShare, weight: f64) {
        let bucket = (date.to_string(), model.to_string(), provider.to_string());
        let prev = self.bucket_weight.get(&bucket).copied().unwrap_or(0.0);
        let entry = self.by_bucket.entry(bucket.clone()).or_default();
        *entry = entry.weighted(&share, prev, weight);
        self.bucket_weight.insert(bucket, prev + weight);

        let mw = self.model_weight.get(model).copied().unwrap_or(0.0);
        let m = self.by_model.entry(model.to_string()).or_default();
        *m = m.weighted(&share, mw, weight);
        self.model_weight.insert(model.to_string(), mw + weight);
    }
}

/// 某个"本地 date + hour"是否落在 DeepSeek 高峰时段(北京时间,周一至周五)。
///
/// 节假日不建模:官方口径排除法定节假日,但影响 <1%,而且需要一张会过期的日历。
pub fn is_peak_hour(date: &str, hour: i64) -> bool {
    use chrono::{Datelike, FixedOffset, NaiveDate, Timelike, Weekday};
    let Ok(naive) = NaiveDate::parse_from_str(date, "%Y-%m-%d") else {
        return false;
    };
    let Some(naive_time) = naive.and_hms_opt(hour.clamp(0, 23) as u32, 0, 0) else {
        return false;
    };
    // 库里是**本地**墙上时间:先按本地时区落到一个瞬时,再换算到北京时间。
    // 直接拼北京时间会在别的时区算错日期边界(跨日的小时会被归到错误的星期)。
    let Some(local) = naive_time.and_local_timezone(chrono::Local).single() else {
        return false;
    };
    let beijing = local.with_timezone(&FixedOffset::east_opt(8 * 3600).unwrap());
    if matches!(beijing.weekday(), Weekday::Sat | Weekday::Sun) {
        return false;
    }
    let h = beijing.hour();
    (9..12).contains(&h) || (14..18).contains(&h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_hours_are_beijing_weekday_morning_and_afternoon() {
        // 2026-09-21 是周一
        for h in [9, 10, 11, 14, 15, 16, 17] {
            assert!(is_peak_hour("2026-09-21", h), "{h} 点应当是高峰");
        }
        for h in [0, 8, 12, 13, 18, 19, 23] {
            assert!(!is_peak_hour("2026-09-21", h), "{h} 点不该是高峰");
        }
    }

    #[test]
    fn weekends_have_no_peak() {
        // 2026-09-19 周六、09-20 周日
        for h in [9, 10, 14, 15] {
            assert!(!is_peak_hour("2026-09-19", h));
            assert!(!is_peak_hour("2026-09-20", h));
        }
    }

    #[test]
    fn malformed_dates_are_off_peak() {
        assert!(!is_peak_hour("", 10));
        assert!(!is_peak_hour("not-a-date", 10));
        assert!(!is_peak_hour("2026-09-21", 99), "越界小时按 23 处理,不是高峰");
    }

    /// 加权合并:3/4 的量在高峰 → 占比 0.75
    #[test]
    fn shares_merge_weighted_by_token_volume() {
        let mut s = PeakShares::default();
        let peak = PeakShare { input: 1.0, output: 1.0, cache_read: 1.0, cache_write: 1.0 };
        let off = PeakShare::default();
        s.add("2026-09-21", "m", "p", peak, 30.0);
        s.add("2026-09-21", "m", "p", off, 10.0);
        let got = s.get("2026-09-21", "m", "p");
        assert!((got.input - 0.75).abs() < 1e-9, "got {got:?}");
        assert!((got.output - 0.75).abs() < 1e-9);
    }

    /// 桶没有小时数据 → 回退到该模型的区间整体占比;模型完全没有 → 全平价
    #[test]
    fn missing_buckets_fall_back_then_to_zero() {
        let mut s = PeakShares::default();
        let peak = PeakShare { input: 1.0, output: 1.0, cache_read: 1.0, cache_write: 1.0 };
        s.add("2026-09-21", "m", "p", peak, 10.0);
        assert_eq!(
            s.get("2026-09-21", "m", "p").input,
            1.0,
            "有小时数据的桶按实际算"
        );
        assert_eq!(
            s.get("2026-09-20", "m", "p").input,
            1.0,
            "缺这个桶 → 回退到模型区间占比"
        );
        assert_eq!(
            s.get("2026-09-21", "other", "p").input,
            0.0,
            "模型完全没有小时数据 → 全平价"
        );
    }

    /// 同一个桶里既有高峰又有平价的多个小时:按各小时 token 量加权
    #[test]
    fn same_bucket_merges_across_hours() {
        let mut s = PeakShares::default();
        let peak = PeakShare { input: 1.0, output: 1.0, cache_read: 1.0, cache_write: 1.0 };
        let off = PeakShare::default();
        s.add("2026-09-21", "m", "p", peak, 3.0);
        s.add("2026-09-21", "m", "p", off, 1.0);
        s.add("2026-09-21", "m", "p", peak, 4.0);
        // 高峰 7 / 总 8
        let got = s.get("2026-09-21", "m", "p");
        assert!((got.input - 0.875).abs() < 1e-9, "got {got:?}");
    }
}
