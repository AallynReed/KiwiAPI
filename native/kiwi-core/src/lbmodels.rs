//! `app/trove/leaderboards/models.py`: board reset cadences and the cold-tier
//! cutoff.

/// `_DAILY_RESET_UUIDS`.
const DAILY: [i64; 1] = [32000];

/// `_WEEKLY_RESET_UUIDS`.
#[rustfmt::skip]
const WEEKLY: [i64; 95] = [
    2001, 2002, 2004, 2011, 2013, 2014, 2021, 2024,
    2300, 2301, 2302, 2303, 2304, 2305, 2306, 2307, 2308, 2309, 2310, 2311, 2312, 2313, 2314, 2315, 2316, 2317,
    2400, 2401, 2402, 2403, 2404, 2405, 2406, 2407, 2408, 2409, 2410, 2411, 2412, 2413, 2414, 2415, 2416, 2417,
    4000, 4001, 4002, 4003, 4004, 4005, 4006, 4007, 4008, 4009, 4010, 4011, 4012, 4013, 4014, 4015, 4016, 4017,
    5000, 5001, 5002, 5003, 5004, 5005, 5006, 5007, 5008, 5009, 5010, 5011, 5012, 5013, 5014, 5015, 5016, 5017,
    10004, 10009, 10012, 10019, 21004, 21005, 21012, 30001, 30002, 30003, 30004, 30005, 33001, 33002, 50000,
];

/// `RESET_KIND_VALUES`.
pub const RESET_KIND_VALUES: [&str; 3] = ["daily", "weekly", "none"];

/// `reset_kind(uuid)`: the hardcoded fallback cadence.
pub fn reset_kind(uuid: i64) -> &'static str {
    if DAILY.contains(&uuid) {
        "daily"
    } else if WEEKLY.contains(&uuid) {
        "weekly"
    } else {
        "default"
    }
}

/// `pg_store._effective_reset_kind`.
pub fn effective_reset_kind(over: Option<&str>, uuid: i64) -> &str {
    match over {
        Some(o) if RESET_KIND_VALUES.contains(&o) => o,
        _ => reset_kind(uuid),
    }
}

/// `is_lifetime_kind`.
pub fn is_lifetime_kind(kind: &str) -> bool {
    matches!(kind, "default" | "none")
}

/// `cold_keep_from(now, after_days)`: the oldest trove-day start kept hot.
pub fn cold_keep_from(now: i64, after_days: i64) -> i64 {
    crate::lbparse::trove_day_start(now) - (after_days.max(1) - 1) * crate::lbparse::DAY
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cadences() {
        assert_eq!(reset_kind(32000), "daily");
        assert_eq!(reset_kind(2001), "weekly");
        assert_eq!(reset_kind(5000), "weekly");
        assert_eq!(reset_kind(999_999), "default");
        assert_eq!(effective_reset_kind(Some("none"), 2001), "none");
        assert_eq!(effective_reset_kind(Some("bogus"), 2001), "weekly");
        assert!(is_lifetime_kind("none") && !is_lifetime_kind("weekly"));
        assert_eq!(cold_keep_from(1_761_130_800, 7), 1_761_130_800 - 6 * 86400);
        assert_eq!(cold_keep_from(1_761_130_800, 0), 1_761_130_800);
    }
}
