//! The game's levelling table: EXP needed to get from level N to N+1, for levels 1 to 99 (index N - 1).
//! With the EXP number and the percentage on the bar, exactly one level's size fits, which tells the current level.

#[rustfmt::skip]
pub const EXP_TO_NEXT: [u64; 99] = [
    15, 34, 57, 92, 135, 372, 560, 840, 1_242, 1_716, // 1-10
    2_360, 3_216, 4_200, 5_460, 7_050, 8_840, 11_040, 13_716, 16_680, 20_216, // 11-20
    24_402, 28_980, 34_320, 40_512, 47_216, 54_900, 63_666, 73_080, 83_720, 95_700, // 21-30
    108_480, 122_760, 138_666, 155_540, 174_216, 194_832, 216_600, 240_500, 266_682, 294_216, // 31-40
    324_240, 356_916, 391_160, 428_280, 468_450, 510_420, 555_680, 604_416, 655_200, 709_716, // 41-50
    748_608, 789_631, 832_902, 878_545, 926_689, 977_471, 1_031_036, 1_087_536, 1_147_132, 1_209_994, // 51-60
    1_276_301, 1_346_242, 1_420_016, 1_497_832, 1_579_913, 1_666_492, 1_757_815, 1_854_143, 1_955_750, 2_062_925, // 61-70
    2_175_973, 2_295_216, 2_420_993, 2_553_663, 2_693_603, 2_841_212, 2_996_910, 3_161_140, 3_334_370, 3_517_093, // 71-80
    3_709_829, 3_913_127, 4_127_566, 4_353_756, 4_592_341, 4_844_001, 5_109_452, 5_389_449, 5_684_790, 11_938_059, // 81-90
    25_069_923, 52_646_838, 110_558_359, 232_172_553, 487_562_361, 1_023_880_958, 2_150_150_011, 4_515_315_023, 9_482_161_548, // 91-99
];

/// EXP needed to finish `level` (None above the table).
pub fn exp_to_next(level: u32) -> Option<u64> {
    EXP_TO_NEXT.get((level as usize).checked_sub(1)?).copied()
}

/// The level whose size fits an EXP number and percentage read off the bar: the percentage is the number over the
/// level's size, shown to two decimals. None if no level fits, or (for small numbers, where many levels would give the
/// same two decimals) more than one does.
pub fn level_for(exp: u64, pct: f64) -> Option<u32> {
    let mut found = None;
    for (i, &need) in EXP_TO_NEXT.iter().enumerate() {
        if exp > need {
            continue;
        }
        let shown = exp as f64 / need as f64 * 100.0;
        if (shown - pct).abs() <= 0.0051 {
            if found.is_some() {
                return None;
            }
            found = Some(i as u32 + 1);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_matches_the_published_one() {
        assert_eq!(exp_to_next(1), Some(15));
        assert_eq!(exp_to_next(32), Some(122_760));
        assert_eq!(exp_to_next(99), Some(9_482_161_548));
        assert_eq!(exp_to_next(100), None);
        assert_eq!(exp_to_next(0), None);
        // the published "total from level 1" column: level 32 starts at 752,817 and level 33 at 875,577
        assert_eq!(EXP_TO_NEXT[..31].iter().sum::<u64>(), 752_817);
        assert_eq!(EXP_TO_NEXT[..32].iter().sum::<u64>(), 875_577);
        assert_eq!(EXP_TO_NEXT[..98].iter().sum::<u64>(), 8_712_616_849);
        assert_eq!(EXP_TO_NEXT.iter().sum::<u64>(), 18_194_778_397);
    }

    #[test]
    fn finds_the_level_from_the_bar() {
        // the sample frame: 68,601 [71.68%] is level 40 territory? check against the table instead of assuming
        let l = level_for(68_601, 71.68).unwrap();
        assert_eq!(exp_to_next(l).map(|n| (68_601.0 / n as f64 * 100.0 * 100.0).round() / 100.0), Some(71.68));
        // 26,772 at 21.81% is level 32 (122,760): 26,772 / 122,760 = 21.81%
        assert_eq!(level_for(26_772, 21.81), Some(32));
        assert_eq!(level_for(53_000, 43.17), Some(32));
        // a tiny number fits many levels: no answer
        assert_eq!(level_for(1, 0.00), None);
    }
}
