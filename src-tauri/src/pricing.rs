//! DeepSeek 定价修正:内置 curated 表 + 一次性迁移。
//!
//! OTR 自己没有任何内置定价表(`Settings::default().pricing` 是空的),全靠用户手填或
//! models.dev 同步。DeepSeek 的历史价格几次调整、又分峰谷,于是本机积累了一批**错的**
//! 单价,而且错法各不相同:
//!
//! - 旧价:flash 族 `0.14/0.28/0.0028`、pro 族 `0.435/0.87/0.003625`;
//! - **把高峰价当成平价**:`deepseek-v4-pro-0813` 存的 `1.32/3.96/0.044` 正好是 pro 的
//!   高峰价,于是它被算成了 2 倍;
//! - 手填错值:`deepseek-v4.1-flash` 的 `2.0/1.0/0.02`(输入比输出还贵),它一个人就虚增
//!   了 ¥3,594 —— 本机最贵的一条。
//!
//! 迁移只做两件事,且**只在模型命中 curated 表时**动手:
//!
//! 1. 当前值**逐位等于**一个已知错误值集合 → 换成 curated 平价;
//! 2. 平价已经等于 curated 但缺 `peak` → 补上峰谷档。
//!
//! 其余条目一律不动:用户自填的、不在已知错误集合里的值是他的选择,程序没有资格覆盖。
//! 迁移过的条目标上 `curated:deepseek`,models.dev 同步时据此跳过冲突询问。

use crate::settings::{PeakTier, PriceEntry, Settings};

/// curated 表里的一档单价(美元 / 百万 tokens)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CuratedPrice {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

impl CuratedPrice {
    /// 高峰价 = 平价 × 2(DeepSeek 峰谷档位)
    pub fn peak(self) -> PeakTier {
        PeakTier {
            input: self.input * 2.0,
            output: self.output * 2.0,
            cache_read: self.cache_read * 2.0,
            cache_write: self.cache_write * 2.0,
        }
    }

    /// 判定"用户的平价就是 curated 平价"。
    ///
    /// 只比 input/output/cache_read 三项:**cache_write 不比**。DeepSeek 的缓存写入按
    /// 输入价计费,而历史配置里一律是 0.0(用户填价时通常不管这一栏),拿它当判据会把
    /// "已经填对、只差峰谷档"的条目误判成"用户自定义值"而跳过。cache_write 随后由
    /// curated 补齐。
    fn matches_core(&self, e: &PriceEntry) -> bool {
        same(self.input, e.input)
            && same(self.output, e.output)
            && same(self.cache_read, e.cache_read)
    }
}

/// 浮点逐位比较:这些值是配置里存下来的十进制字面量,round-trip 后应当完全一致。
/// 用 epsilon 反而危险 —— "接近但不等"意味着用户改过,不该被当成已知错误值覆盖。
fn same(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits()
}

const FLASH: CuratedPrice = CuratedPrice {
    input: 0.15,
    output: 0.6,
    cache_read: 0.003,
    cache_write: 0.15,
};
const PRO: CuratedPrice = CuratedPrice {
    input: 0.66,
    output: 1.98,
    cache_read: 0.022,
    cache_write: 0.66,
};

/// 模型 → 现行官方价。**精确匹配**,不做前缀/模糊:
/// `deepseek-v4-flash-free` 与 `deepseek-v4-flash` 只差一个后缀,价格却天差地别。
pub fn curated(model: &str) -> Option<CuratedPrice> {
    let m = model.trim().to_ascii_lowercase();
    match m.as_str() {
        "deepseek-flash"
        | "deepseek-v4-flash"
        | "deepseek-v4-flash-0731"
        | "deepseek-v4-flash-vision-exp"
        | "deepseek-v4.1-flash"
        | "deepseek-v4.1-flash-expires-on-0910" => Some(FLASH),
        "deepseek-v4-pro" | "deepseek-v4-pro-0813" => Some(PRO),
        _ => None,
    }
}

/// 历史上出现过的错误单价集合(input, output, cache_read)。
/// 只有**逐位命中**其中一组才认为是"已知错误值",才允许被 curated 覆盖。
const KNOWN_WRONG: &[(f64, f64, f64)] = &[
    // flash 族旧价
    (0.14, 0.28, 0.0028),
    // pro 族旧价
    (0.435, 0.87, 0.003625),
    // pro-0813:高峰价被当成了平价
    (1.32, 3.96, 0.044),
    // deepseek-v4.1-flash:手填错值
    (2.0, 1.0, 0.02),
];

fn is_known_wrong(e: &PriceEntry) -> bool {
    KNOWN_WRONG.iter().any(|(i, o, c)| {
        same(*i, e.input) && same(*o, e.output) && same(*c, e.cache_read)
    })
}

/// curated 条目的 pricingSource 标记;models.dev 同步看到它就不再问冲突。
pub const CURATED_SOURCE: &str = "curated:deepseek";

/// 就地迁移定价表;返回是否改动过(调用方据此决定要不要落盘)。
pub fn migrate(settings: &mut Settings) -> bool {
    let mut changed = false;
    let models: Vec<String> = settings.pricing.keys().cloned().collect();
    for model in models {
        let Some(want) = curated(&model) else {
            continue; // 不在 curated 表里的模型一律不动
        };
        let Some(entry) = settings.pricing.get(&model).cloned() else {
            continue;
        };
        let target = PriceEntry {
            input: want.input,
            output: want.output,
            cache_read: want.cache_read,
            cache_write: want.cache_write,
            peak: Some(want.peak()),
        };
        // 用户手填的、不在已知错误集合里的值:保留。但若三项平价恰好等于 curated
        // (说明他填对了),只补 cache_write 与 peak,输入/输出/缓存读一位不动。
        let fixed = if is_known_wrong(&entry) {
            target
        } else if want.matches_core(&entry) {
            PriceEntry {
                cache_write: want.cache_write,
                peak: Some(want.peak()),
                ..entry
            }
        } else {
            continue;
        };
        if fixed == entry {
            continue;
        }
        settings.pricing.insert(model.clone(), fixed);
        settings
            .pricing_source
            .insert(model, CURATED_SOURCE.to_string());
        changed = true;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn settings_with(pairs: &[(&str, f64, f64, f64)]) -> Settings {
        let mut s = Settings::default();
        s.pricing = pairs
            .iter()
            .map(|(m, i, o, c)| {
                (
                    (*m).to_string(),
                    PriceEntry {
                        input: *i,
                        output: *o,
                        cache_read: *c,
                        cache_write: 0.0,
                        peak: None,
                    },
                )
            })
            .collect();
        s.pricing_source = HashMap::new();
        s
    }

    /// 用户现场的四类错误值都被修正成 curated 平价,并补上峰谷档。
    #[test]
    fn known_wrong_values_are_replaced_by_curated_offpeak() {
        let mut s = settings_with(&[
            ("deepseek-v4.1-flash", 2.0, 1.0, 0.02),
            ("deepseek-v4-pro-0813", 1.32, 3.96, 0.044),
            ("deepseek-v4-flash", 0.14, 0.28, 0.0028),
            ("deepseek-v4-pro", 0.435, 0.87, 0.003625),
        ]);
        assert!(migrate(&mut s));
        let flash = &s.pricing["deepseek-v4.1-flash"];
        assert_eq!((flash.input, flash.output, flash.cache_read), (0.15, 0.6, 0.003));
        assert_eq!(
            flash.peak.as_ref().map(|p| p.output),
            Some(1.2),
            "高峰价必须是平价的 2 倍"
        );
        let pro = &s.pricing["deepseek-v4-pro"];
        assert_eq!((pro.input, pro.output, pro.cache_read), (0.66, 1.98, 0.022));
        // pro-0813 存的是高峰价,迁移后平价必须减半
        assert_eq!(s.pricing["deepseek-v4-pro-0813"].input, 0.66);
        assert_eq!(s.pricing["deepseek-v4-pro-0813"].output, 1.98);
        assert_eq!(s.pricing_source["deepseek-v4-pro"], CURATED_SOURCE);
    }

    /// 用户自填的、不在已知错误集合里的值**不被覆盖** —— 那是他的选择。
    #[test]
    fn user_supplied_non_known_values_are_left_alone() {
        let mut s = settings_with(&[("deepseek-v4.1-flash", 9.99, 8.88, 0.5)]);
        assert!(!migrate(&mut s), "不该有任何改动");
        let e = &s.pricing["deepseek-v4.1-flash"];
        assert_eq!((e.input, e.output, e.cache_read), (9.99, 8.88, 0.5));
        assert!(e.peak.is_none());
        assert!(!s.pricing_source.contains_key("deepseek-v4.1-flash"));
    }

    /// 平价已经填对、只缺峰谷档:只补 peak,数字一位不动。
    #[test]
    fn correct_offpeak_only_gains_the_peak_tier() {
        let mut s = settings_with(&[("deepseek-flash", 0.15, 0.6, 0.003)]);
        assert!(migrate(&mut s));
        let e = &s.pricing["deepseek-flash"];
        assert_eq!(
            (e.input, e.output, e.cache_read),
            (0.15, 0.6, 0.003),
            "平价必须一位不动"
        );
        assert_eq!(e.cache_write, 0.15, "缓存写按输入价补齐");
        assert_eq!(e.peak.as_ref().map(|p| p.input), Some(0.3));
    }

    /// 幂等:跑第二遍不该再改任何东西。
    #[test]
    fn migration_is_idempotent() {
        let mut s = settings_with(&[
            ("deepseek-v4.1-flash", 2.0, 1.0, 0.02),
            ("deepseek-flash", 0.15, 0.6, 0.003),
        ]);
        assert!(migrate(&mut s));
        assert!(!migrate(&mut s));
    }

    /// curated 之外一律不碰:免费模型、别的厂商、未知模型。
    #[test]
    fn models_outside_the_curated_table_are_untouched() {
        let mut s = settings_with(&[
            ("deepseek-v4-flash-free", 0.0, 0.0, 0.0),
            ("deepseek-v4-flash-vision-exp-2", 0.14, 0.28, 0.0028),
            ("gpt-5.9", 1.0, 2.0, 0.1),
        ]);
        assert!(!migrate(&mut s));
        assert_eq!(s.pricing["deepseek-v4-flash-free"].input, 0.0);
        assert_eq!(s.pricing["deepseek-v4-flash-vision-exp-2"].input, 0.14);
        assert_eq!(s.pricing["gpt-5.9"].input, 1.0);
    }

    /// 精确匹配:带 -free / 别的后缀的模型不能蹭到 flash 族的价。
    #[test]
    fn curated_matching_is_exact_not_prefix() {
        assert!(curated("deepseek-v4.1-flash").is_some());
        assert!(curated("DeepSeek-V4.1-Flash").is_some(), "大小写不敏感");
        assert!(curated("deepseek-v4-flash-free").is_none());
        assert!(curated("deepseek-v4-flash-0731").is_some());
        assert!(curated("deepseek-v4-flash-0731-extra").is_none());
    }
}
