//! Pure leaderboards read-side logic from `app/trove/leaderboards/`
//! (`service.py`, `models.py`, `pg_store.py`, `cache.py`, `mastery.py`).

use std::collections::HashMap;

use crate::gems::{py_round, pynum::py_fsum};

pub const RESET_HOUR_UTC: i64 = 11;
pub const TROVE_DAY_OFFSET: i64 = RESET_HOUR_UTC * 3600;
pub const DAY: i64 = 86_400;
/// `datetime(9999, 12, 31, 23, 59, 59, tzinfo=UTC).timestamp()`.
pub const PY_MAX_TIMESTAMP: i64 = 253_402_300_799;

const DAILY_RESET_UUIDS: [i64; 1] = [32000];
const WEEKLY_RESET_UUIDS: &[i64] = &[
    2001, 2002, 2004, 2011, 2013, 2014, 2021, 2024, 2300, 2301, 2302, 2303, 2304, 2305, 2306, 2307, 2308, 2309, 2310,
    2311, 2312, 2313, 2314, 2315, 2316, 2317, 2400, 2401, 2402, 2403, 2404, 2405, 2406, 2407, 2408, 2409, 2410, 2411,
    2412, 2413, 2414, 2415, 2416, 2417, 4000, 4001, 4002, 4003, 4004, 4005, 4006, 4007, 4008, 4009, 4010, 4011, 4012,
    4013, 4014, 4015, 4016, 4017, 5000, 5001, 5002, 5003, 5004, 5005, 5006, 5007, 5008, 5009, 5010, 5011, 5012, 5013,
    5014, 5015, 5016, 5017, 10004, 10009, 10012, 10019, 21004, 21005, 21012, 30001, 30002, 30003, 30004, 30005, 33001,
    33002, 50000,
];
pub const NON_PLAYER_BOARDS: [i64; 2] = [1100, 21012];

/// `pg_store.trove_day_start`: the 11:00-UTC reset opening `anchor`'s trove-day.
pub fn trove_day_start(anchor: i64) -> i64 {
    (anchor - TROVE_DAY_OFFSET).div_euclid(DAY) * DAY + TROVE_DAY_OFFSET
}

/// `models.reset_kind`: the hardcoded cadence.
pub fn reset_kind(uuid: i64) -> &'static str {
    if DAILY_RESET_UUIDS.contains(&uuid) {
        "daily"
    } else if WEEKLY_RESET_UUIDS.contains(&uuid) {
        "weekly"
    } else {
        "default"
    }
}

/// `pg_store._effective_reset_kind`: a valid admin override, else the hardcoded cadence.
pub fn effective_reset_kind(override_kind: Option<&str>, uuid: i64) -> &'static str {
    match override_kind {
        Some("daily") => "daily",
        Some("weekly") => "weekly",
        Some("none") => "none",
        _ => reset_kind(uuid),
    }
}

pub fn is_player_board(uuid: i64) -> bool {
    !NON_PLAYER_BOARDS.contains(&uuid)
}

/// Python `date.weekday()` of a unix timestamp (Monday = 0).
fn weekday(ts: i64) -> i64 {
    (ts.div_euclid(DAY) + 3).rem_euclid(7)
}

/// `service.reset_boundaries_for_kind`: reset moments strictly after `t_start`, up to `t_end`.
pub fn reset_boundaries_for_kind(kind: &str, t_start: i64, t_end: i64) -> Vec<i64> {
    if !matches!(kind, "daily" | "weekly") || t_end <= t_start {
        return Vec::new();
    }
    let mut candidate = t_start.div_euclid(DAY) * DAY + TROVE_DAY_OFFSET;
    if candidate <= t_start {
        candidate += DAY;
    }
    let step = if kind == "weekly" {
        while weekday(candidate) != 0 {
            candidate += DAY;
        }
        7 * DAY
    } else {
        DAY
    };
    let mut out = Vec::new();
    while candidate <= t_end {
        out.push(candidate);
        candidate += step;
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub created_at: i64,
    pub rank: i64,
    pub score: f64,
    pub synthetic: bool,
}

/// `service._inject_reset_zeros` over `(created_at, rank, score)` points.
pub fn inject_reset_zeros(points: &[(i64, i64, f64)], kind: &str) -> Vec<Point> {
    let annotated: Vec<Point> =
        points.iter().map(|&(created_at, rank, score)| Point { created_at, rank, score, synthetic: false }).collect();
    if !matches!(kind, "daily" | "weekly") || annotated.len() < 2 {
        return annotated;
    }
    let mut out = Vec::with_capacity(annotated.len());
    for (i, p) in annotated.iter().enumerate() {
        out.push(*p);
        let Some(next) = annotated.get(i + 1) else { continue };
        let (mut last_score, mut last_rank) = (p.score, p.rank);
        for r in reset_boundaries_for_kind(kind, p.created_at, next.created_at) {
            if last_score > 0.0 {
                out.push(Point { created_at: r - 1, rank: last_rank, score: last_score, synthetic: true });
                if r < next.created_at {
                    out.push(Point { created_at: r, rank: 0, score: 0.0, synthetic: true });
                }
            }
            last_score = 0.0;
            last_rank = 0;
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stamp {
    Anchor(i64),
    /// Python's `-1`.
    Invalid,
    /// Past `datetime`'s range, where Python raises (a 500).
    OutOfRange,
}

/// `service.normalize_timestamp` at `now` (unix seconds).
pub fn normalize_timestamp(ts: i64, now: i64, allow_backfill: bool) -> Stamp {
    if ts <= 0 {
        return Stamp::Invalid;
    }
    if ts > PY_MAX_TIMESTAMP {
        return Stamp::OutOfRange;
    }
    let parsed = ts - ts.rem_euclid(60);
    let now_min = now - now.rem_euclid(60);
    if parsed > now_min + 300 || (!allow_backfill && parsed < now_min - 14 * DAY) {
        return Stamp::Invalid;
    }
    Stamp::Anchor(if parsed.rem_euclid(DAY) == 0 { parsed + TROVE_DAY_OFFSET } else { parsed })
}

/// `cache._latest_per_day`: the newest anchor of each trove-day, newest first.
pub fn latest_per_day(anchors: &[i64], days: usize) -> Vec<i64> {
    let mut by_day: HashMap<i64, i64> = HashMap::new();
    for &a in anchors {
        let slot = by_day.entry((a - TROVE_DAY_OFFSET).div_euclid(DAY)).or_insert(-1);
        if a > *slot {
            *slot = a;
        }
    }
    let mut out: Vec<i64> = by_day.into_values().collect();
    out.sort_unstable_by(|a, b| b.cmp(a));
    out.truncate(days);
    out
}

/// `cache._cap_to_ready`.
pub fn cap_to_ready(timestamps: Vec<i64>, ready: Option<i64>) -> Vec<i64> {
    match ready {
        None => timestamps,
        Some(r) => timestamps.into_iter().filter(|&t| t <= r).collect(),
    }
}

// --- mastery.py -------------------------------------------------------------

pub const MAX_LEVEL: i64 = 1001;

fn level_cost(i: i64) -> i64 {
    match i {
        ..=5 => 25,
        6..=10 => 50,
        11..=20 => 75,
        21..=300 => 100,
        _ => 150 + ((i - 300) as f64 * 0.5).ceil() as i64,
    }
}

/// `mastery.points_to_mr`: `(level, points_into_level, next_level_cost)`.
pub fn points_to_mr(mut points: i64) -> (i64, i64, i64) {
    let (mut i, mut increment) = (1, 25);
    while i != MAX_LEVEL {
        i += 1;
        increment = level_cost(i);
        points -= increment;
        if points <= 0 {
            if points < 0 {
                points += increment;
                i -= 1;
            }
            break;
        }
    }
    (i, points, increment)
}

/// `mastery.level_from_points`: `(level, points_into_level, points_to_next_level)`.
pub fn level_from_points(points: i64) -> (i64, i64, i64) {
    let (level, into, next) = points_to_mr(points.max(0));
    (level, into, (next - into).max(0))
}

// --- board health -----------------------------------------------------------

fn sorted(xs: impl IntoIterator<Item = f64>) -> Vec<f64> {
    let mut s: Vec<f64> = xs.into_iter().collect();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    s
}

/// `service._median`.
pub fn median(xs: &[f64]) -> f64 {
    let s = sorted(xs.iter().copied());
    let n = s.len();
    if n == 0 {
        return 0.0;
    }
    let mid = n / 2;
    if n % 2 == 1 { s[mid] } else { (s[mid - 1] + s[mid]) / 2.0 }
}

/// `service._gini` over the non-negative values.
pub fn gini(xs: &[f64]) -> f64 {
    let s = sorted(xs.iter().copied().filter(|&x| x >= 0.0));
    let n = s.len();
    let total = py_fsum(s.iter().copied());
    if n == 0 || total == 0.0 {
        return 0.0;
    }
    let cum = py_fsum(s.iter().enumerate().map(|(i, &x)| (i + 1) as f64 * x));
    (2.0 * cum) / (n as f64 * total) - (n + 1) as f64 / n as f64
}

/// One entry as `compute_board_health` reads it.
#[derive(Clone, Copy, Debug, Default)]
pub struct HealthItem {
    pub score: f64,
    pub is_new: bool,
    pub score_delta: Option<f64>,
    pub prev_score: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Health {
    pub leader_share: Option<f64>,
    pub p1_pn_ratio: Option<f64>,
    pub gini: Option<f64>,
    pub turnover_rate: Option<f64>,
    pub new_entrants: Option<i64>,
    pub median_score_gain: Option<f64>,
    pub median_score_gain_pct: Option<f64>,
}

/// `service.compute_board_health`.
pub fn compute_board_health(items: &[HealthItem], comparable: bool) -> Health {
    let mut out = Health::default();
    let scores: Vec<f64> = items.iter().map(|it| it.score).collect();
    if let (Some(&first), Some(&last)) = (scores.first(), scores.last()) {
        let total = py_fsum(scores.iter().copied());
        if total > 0.0 {
            out.leader_share = Some(py_round(first / total, 4));
        }
        if last > 0.0 {
            out.p1_pn_ratio = Some(py_round(first / last, 4));
        }
        out.gini = Some(py_round(gini(&scores), 4));
    }
    if comparable && !items.is_empty() {
        let new_entrants = items.iter().filter(|it| it.is_new).count();
        out.new_entrants = Some(new_entrants as i64);
        out.turnover_rate = Some(py_round(new_entrants as f64 / items.len() as f64, 4));
        let old = || items.iter().filter(|it| !it.is_new);
        let gains: Vec<f64> = old().filter_map(|it| it.score_delta).collect();
        if !gains.is_empty() {
            out.median_score_gain = Some(py_round(median(&gains), 2));
        }
        let pcts: Vec<f64> = old()
            .filter(|it| it.prev_score.is_some_and(|p| p != 0.0))
            .map(|it| it.score_delta.unwrap_or(f64::NAN) / it.prev_score.unwrap_or(f64::NAN))
            .collect();
        if !pcts.is_empty() {
            out.median_score_gain_pct = Some(py_round(median(&pcts) * 100.0, 2));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_start_and_kinds() {
        assert_eq!(trove_day_start(1_760_000_000), 1_759_921_200);
        assert_eq!(trove_day_start(1_759_921_200), 1_759_921_200);
        assert_eq!(trove_day_start(1_759_921_199), 1_759_834_800);
        assert_eq!(effective_reset_kind(Some("none"), 4000), "none");
        assert_eq!(effective_reset_kind(Some("monthly"), 4000), "weekly");
        assert_eq!(effective_reset_kind(None, 32000), "daily");
        assert_eq!(effective_reset_kind(None, 1), "default");
        assert!(!is_player_board(1100) && is_player_board(1));
    }

    #[test]
    fn mastery_reference() {
        assert_eq!(points_to_mr(0), (1, 0, 25));
        assert_eq!(points_to_mr(25), (2, 0, 25));
        assert_eq!(points_to_mr(26), (2, 1, 25));
        assert_eq!(level_from_points(-5), (1, 0, 25));
        assert_eq!(points_to_mr(i64::MAX / 2).0, MAX_LEVEL);
    }

    #[test]
    fn boundaries() {
        // Sunday 2025-10-19 12:00 UTC -> Monday 11:00 is the next weekly reset.
        let sun = 1_760_875_200;
        assert_eq!(reset_boundaries_for_kind("weekly", sun, sun + 7 * DAY), vec![sun + 23 * 3600]);
        assert_eq!(reset_boundaries_for_kind("daily", sun, sun + DAY), vec![sun + 23 * 3600]);
        assert!(reset_boundaries_for_kind("default", sun, sun + 9 * DAY).is_empty());
    }
}
