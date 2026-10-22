//! Pure parts of the leaderboards player reads: reset cadences and trove-day
//! math (`service.py`, `pg_store.py`, `models.py`), the chart reset cliffs,
//! `duplicates.split_series`, the mastery curve (`mastery.py`) and the
//! profile's class pairing and sigil inputs.

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::gems::pynum::{py_fsum, py_max};

pub const DAY: i64 = 86_400;
pub const RESET_HOUR: i64 = 11 * 3600;

/// `models._NON_PLAYER_BOARDS`, sorted (`non_player_boards()`).
pub const NON_PLAYER_BOARDS: [i64; 2] = [1100, 21012];

pub const TROVE_MASTERY_UUID: i64 = 1;
pub const GEODE_MASTERY_UUID: i64 = 20;
pub const GEODE_LEVEL_CAP: i64 = 100;

const DAILY_RESET_UUIDS: [i64; 1] = [32000];

#[rustfmt::skip]
const WEEKLY_RESET_UUIDS: [i64; 95] = [
    2001, 2002, 2004, 2011, 2013, 2014, 2021, 2024,
    2300, 2301, 2302, 2303, 2304, 2305, 2306, 2307, 2308, 2309, 2310, 2311, 2312, 2313, 2314, 2315, 2316, 2317,
    2400, 2401, 2402, 2403, 2404, 2405, 2406, 2407, 2408, 2409, 2410, 2411, 2412, 2413, 2414, 2415, 2416, 2417,
    4000, 4001, 4002, 4003, 4004, 4005, 4006, 4007, 4008, 4009, 4010, 4011, 4012, 4013, 4014, 4015, 4016, 4017,
    5000, 5001, 5002, 5003, 5004, 5005, 5006, 5007, 5008, 5009, 5010, 5011, 5012, 5013, 5014, 5015, 5016, 5017,
    10004, 10009, 10012, 10019, 21004, 21005, 21012, 30001, 30002, 30003, 30004, 30005, 33001, 33002, 50000,
];

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

/// `pg_store._effective_reset_kind`: a valid override, else the hardcoded cadence.
pub fn effective_reset_kind(override_: Option<&str>, uuid: i64) -> String {
    match override_ {
        Some(o @ ("daily" | "weekly" | "none")) => o.to_owned(),
        _ => reset_kind(uuid).to_owned(),
    }
}

/// `trove_day_start`: the 11:00-UTC reset opening `anchor`'s trove-day.
pub fn trove_day_start(anchor: i64) -> i64 {
    (anchor - RESET_HOUR).div_euclid(DAY) * DAY + RESET_HOUR
}

/// `pg_store.cold_keep_from`: the oldest trove-day start that stays hot.
pub fn cold_keep_from(now: i64, after_days: i64) -> i64 {
    trove_day_start(now).saturating_sub((after_days.max(1) - 1).saturating_mul(DAY))
}

/// Monday = 0, like `datetime.weekday()`.
fn weekday(ts: i64) -> i64 {
    (ts.div_euclid(DAY) + 3).rem_euclid(7)
}

/// `reset_boundaries_for_kind`: daily/weekly reset moments in `(t_start, t_end]`.
pub fn reset_boundaries_for_kind(kind: &str, t_start: i64, t_end: i64) -> Vec<i64> {
    let weekly = kind == "weekly";
    if !(weekly || kind == "daily") || t_end <= t_start {
        return Vec::new();
    }
    let mut candidate = t_start.div_euclid(DAY) * DAY + RESET_HOUR;
    if candidate <= t_start {
        candidate += DAY;
    }
    let step = if weekly {
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

/// One `{created_at, rank, score}` capture of a player on a board.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub created_at: i64,
    pub rank: i64,
    pub score: f64,
}

/// `_inject_reset_zeros`: each point with its `synthetic` flag, plus the
/// hold-flat (`R-1`) and zero (`R`) pair at every reset between two captures.
pub fn inject_reset_zeros(points: &[Point], kind: &str) -> Vec<(Point, bool)> {
    if !matches!(kind, "daily" | "weekly") || points.len() < 2 {
        return points.iter().map(|p| (*p, false)).collect();
    }
    let mut out = Vec::with_capacity(points.len());
    for (i, p) in points.iter().enumerate() {
        out.push((*p, false));
        let Some(nxt) = points.get(i + 1) else { continue };
        let mut last_score = p.score;
        let mut last_rank = p.rank;
        for r in reset_boundaries_for_kind(kind, p.created_at, nxt.created_at) {
            if last_score > 0.0 {
                out.push((Point { created_at: r - 1, rank: last_rank, score: last_score }, true));
                if r < nxt.created_at {
                    out.push((Point { created_at: r, rank: 0, score: 0.0 }, true));
                }
            }
            last_score = 0.0;
            last_rank = 0;
        }
    }
    out
}

const DROP_PENALTY: f64 = 8.0;
const BRUTE_FORCE_MAX: usize = 5;
const VELOCITY_WINDOW: usize = 4;

/// One identity's line out of [`split_series`]: indices into the input,
/// anchor-ascending.
#[derive(Clone, Debug, PartialEq)]
pub struct Split {
    pub slot: usize,
    pub points: Vec<usize>,
}

fn py_cmp(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or(Ordering::Equal)
}

/// `_velocity`: recent score change per capture, floored at 0.
fn velocity(points: &[Point], series: &[usize]) -> f64 {
    if series.len() < 2 {
        return 0.0;
    }
    let tail = &series[series.len().saturating_sub(VELOCITY_WINDOW)..];
    let span = tail.len() - 1;
    py_max(0.0, (points[tail[span]].score - points[tail[0]].score) / span as f64)
}

/// `_pair_cost`.
fn pair_cost(score: f64, last: f64, predicted: f64) -> f64 {
    let mut cost = (score - predicted).abs();
    if score < last {
        cost += DROP_PENALTY * (last - score);
    }
    cost
}

/// `itertools.permutations(range(n), r)`, in its (lexicographic) order.
fn permutations(n: usize, r: usize) -> Vec<Vec<usize>> {
    fn walk(n: usize, r: usize, cur: &mut Vec<usize>, used: &mut [bool], out: &mut Vec<Vec<usize>>) {
        if cur.len() == r {
            out.push(cur.clone());
            return;
        }
        for i in 0..n {
            if !used[i] {
                used[i] = true;
                cur.push(i);
                walk(n, r, cur, used, out);
                cur.pop();
                used[i] = false;
            }
        }
    }
    let mut out = Vec::new();
    walk(n, r, &mut Vec::with_capacity(r), &mut vec![false; n], &mut out);
    out
}

/// `_assign`: the series each row continues (`None` opens a new one).
fn assign(scores: &[f64], lasts: &[f64], predicted: &[f64]) -> Vec<Option<usize>> {
    let (n, m) = (scores.len(), lasts.len());
    if n == 1 && m == 1 {
        return vec![Some(0)];
    }
    if n.max(m) <= BRUTE_FORCE_MAX {
        let mut best_cost = f64::INFINITY;
        let mut best: Option<Vec<usize>> = None;
        if m <= n {
            for rows_for_series in permutations(n, m) {
                let cost = py_fsum(
                    rows_for_series.iter().enumerate().map(|(s, &r)| pair_cost(scores[r], lasts[s], predicted[s])),
                );
                if cost < best_cost {
                    best_cost = cost;
                    best = Some(rows_for_series);
                }
            }
            let mut out = vec![None; n];
            for (s, &r) in best.iter().flatten().enumerate() {
                out[r] = Some(s);
            }
            return out;
        }
        for series_for_rows in permutations(m, n) {
            let cost =
                py_fsum(series_for_rows.iter().enumerate().map(|(i, &s)| pair_cost(scores[i], lasts[s], predicted[s])));
            if cost < best_cost {
                best_cost = cost;
                best = Some(series_for_rows);
            }
        }
        return match best {
            Some(b) => b.into_iter().map(Some).collect(),
            None => vec![None; n],
        };
    }
    let mut used = vec![false; m];
    scores
        .iter()
        .map(|&score| {
            let mut pick = None;
            let mut pick_cost = f64::INFINITY;
            for i in (0..m).filter(|&i| !used[i]) {
                let cost = pair_cost(score, lasts[i], predicted[i]);
                if cost < pick_cost {
                    pick = Some(i);
                    pick_cost = cost;
                }
            }
            if let Some(i) = pick {
                used[i] = true;
            }
            pick
        })
        .collect()
}

/// `duplicates.split_series`: one board's points for one name, split into
/// continuous per-identity series. Rows equal in rank and score at one anchor
/// count once (the later copies belong to no series).
pub fn split_series(points: &[Point]) -> Vec<Split> {
    if points.is_empty() {
        return Vec::new();
    }
    let mut by_anchor: HashMap<i64, Vec<usize>> = HashMap::new();
    for (i, p) in points.iter().enumerate() {
        let bucket = by_anchor.entry(p.created_at).or_default();
        if bucket.iter().any(|&j| points[j].rank == p.rank && points[j].score == p.score) {
            continue;
        }
        bucket.push(i);
    }
    let mut anchors: Vec<i64> = by_anchor.keys().copied().collect();
    anchors.sort_unstable();
    if by_anchor.values().all(|v| v.len() == 1) {
        return vec![Split { slot: 0, points: anchors.iter().map(|a| by_anchor[a][0]).collect() }];
    }
    let mut series: Vec<Vec<usize>> = Vec::new();
    for anchor in &anchors {
        let mut rows = by_anchor[anchor].clone();
        rows.sort_by(|&a, &b| py_cmp(-points[a].score, -points[b].score).then(points[a].rank.cmp(&points[b].rank)));
        if series.is_empty() {
            series = rows.iter().map(|&r| vec![r]).collect();
            continue;
        }
        let scores: Vec<f64> = rows.iter().map(|&r| points[r].score).collect();
        let lasts: Vec<f64> = series.iter().map(|s| points[s[s.len() - 1]].score).collect();
        let predicted: Vec<f64> = lasts.iter().zip(&series).map(|(last, s)| last + velocity(points, s)).collect();
        for (row, slot) in rows.iter().zip(assign(&scores, &lasts, &predicted)) {
            match slot {
                Some(s) => series[s].push(*row),
                None => series.push(vec![*row]),
            }
        }
    }
    let mut order: Vec<usize> = (0..series.len()).collect();
    order.sort_by(|&a, &b| {
        let (pa, pb) = (&points[series[a][0]], &points[series[b][0]]);
        pa.created_at.cmp(&pb.created_at).then(py_cmp(-pa.score, -pb.score))
    });
    order.into_iter().enumerate().map(|(slot, old)| Split { slot, points: std::mem::take(&mut series[old]) }).collect()
}

/// `mastery.points_to_mr`: `(level, points_into_level, next_level_cost)`.
pub fn points_to_mr(mut points: i128) -> (i64, i128, i128) {
    let mut i: i64 = 1;
    let mut increment: i128 = 25;
    while i != 1001 {
        i += 1;
        increment = match i {
            ..=5 => 25,
            6..=10 => 50,
            11..=20 => 75,
            21..=300 => 100,
            _ => 150 + ((i - 300 + 1) / 2) as i128,
        };
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
pub fn level_from_points(points: i128) -> (i64, i128, i128) {
    let (level, into, next) = points_to_mr(points.max(0));
    (level, into, (next - into).max(0))
}

/// `level_from_points(round(score))[0]`; `None` where `round` raises.
pub fn level_from_score(score: f64) -> Option<i64> {
    let r = score.round_ties_even();
    if !r.is_finite() {
        return None;
    }
    // Far past level 1001 every total reads the same.
    let points = if r.abs() >= 1e30 { r.signum() as i128 * 10i128.pow(30) } else { r as i128 };
    Some(level_from_points(points).0)
}

/// `service._parse_board_csv`: every token `int()` accepts, sorted.
pub fn parse_board_csv(csv: &str) -> Vec<i128> {
    let mut out: Vec<i128> = csv
        .split(',')
        .map(crate::pystr::strip)
        .filter(|t| !t.is_empty())
        .filter_map(|t| crate::feeds::py::int(t, 10))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

pub const POWER_RANK_BASE: i64 = crate::stats::POWER_RANK_BASE;
pub const EFFORT_BASE: i64 = crate::stats::EFFORT_BASE;
pub const PARAGON_BASE: i64 = crate::stats::PARAGON_BASE;

/// The number of classes (`stats.class_count()`).
pub fn class_count() -> i64 {
    crate::stats::BOARD_CLASS_ORDER.len() as i64
}

/// One class row of `_profile_classes` before its name and icon.
#[derive(Clone, Debug, PartialEq)]
pub struct ClassBoards {
    pub index: i64,
    pub power_rank: Option<i64>,
    pub effort: Option<i64>,
    pub paragon: Option<i64>,
}

/// `_profile_classes` without the names: `scores` is `{board: latest_score or 0}`.
pub fn profile_classes(scores: &HashMap<i64, f64>) -> Vec<ClassBoards> {
    let present = |u: i64| scores.contains_key(&u).then_some(u);
    let mut out: Vec<ClassBoards> = (0..class_count())
        .filter_map(|i| {
            let ids = [POWER_RANK_BASE + i, EFFORT_BASE + i, PARAGON_BASE + i];
            ids.iter().any(|u| scores.contains_key(u)).then(|| ClassBoards {
                index: i,
                power_rank: present(ids[0]),
                effort: present(ids[1]),
                paragon: present(ids[2]),
            })
        })
        .collect();
    let val = |u: Option<i64>| u.and_then(|u| scores.get(&u).copied()).unwrap_or(0.0);
    out.sort_by(|a, b| {
        a.power_rank
            .is_none()
            .cmp(&b.power_rank.is_none())
            .then(py_cmp(-val(a.power_rank), -val(b.power_rank)))
            .then(py_cmp(-val(a.effort), -val(b.effort)))
            .then(py_cmp(-val(a.paragon), -val(b.paragon)))
    });
    out
}

/// `_profile_sigil` before the class name.
#[derive(Clone, Debug, PartialEq)]
pub struct Sigil {
    pub power_rank: i64,
    pub power_rank_board: i64,
    pub trove_mastery: i64,
    pub geode_mastery: i64,
}

/// A score `round()` raises on.
#[derive(Debug, PartialEq)]
pub struct NonFinite;

/// `_profile_sigil`: `scores` is each board's `latest_score`, in board order.
/// `Err` where Python raises (a non-finite score).
pub fn profile_sigil(scores: &[(i64, f64)]) -> Result<Option<Sigil>, NonFinite> {
    let get = |u: i64| scores.iter().rev().find(|(b, _)| *b == u).map(|(_, s)| *s);
    let mut pr_board: Option<(i64, f64)> = None;
    for u in (0..class_count()).map(|i| POWER_RANK_BASE + i) {
        if let Some(s) = get(u)
            && pr_board.is_none_or(|(_, best)| s > best)
        {
            pr_board = Some((u, s));
        }
    }
    let (Some((board, pr)), Some(trove), Some(geode)) = (pr_board, get(TROVE_MASTERY_UUID), get(GEODE_MASTERY_UUID))
    else {
        return Ok(None);
    };
    let trove = level_from_score(trove).ok_or(NonFinite)?;
    let geode = level_from_score(geode).ok_or(NonFinite)?.min(GEODE_LEVEL_CAP);
    let r = pr.round_ties_even();
    if !(r.is_finite() && r.abs() < 9.0e18) {
        return Err(NonFinite);
    }
    Ok(Some(Sigil { power_rank: r as i64, power_rank_board: board, trove_mastery: trove, geode_mastery: geode }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(created_at: i64, rank: i64, score: f64) -> Point {
        Point { created_at, rank, score }
    }

    #[test]
    fn trove_days() {
        // 2026-10-20 15:30 UTC (a Tuesday).
        let now = 1_792_510_200;
        assert_eq!(trove_day_start(now), 1_792_494_000);
        assert_eq!(trove_day_start(1_792_494_000), 1_792_494_000);
        assert_eq!(trove_day_start(1_792_493_999), 1_792_494_000 - DAY);
        assert_eq!(cold_keep_from(now, 7), 1_792_494_000 - 6 * DAY);
        assert_eq!(cold_keep_from(now, 0), 1_792_494_000);
        assert_eq!(weekday(1_792_494_000), 1);
    }

    #[test]
    fn boundaries() {
        let tue = 1_792_494_000;
        assert_eq!(reset_boundaries_for_kind("daily", tue - 1, tue), vec![tue]);
        assert_eq!(reset_boundaries_for_kind("daily", tue, tue + DAY - 1), Vec::<i64>::new());
        assert_eq!(reset_boundaries_for_kind("weekly", tue - 3 * DAY, tue), vec![tue - DAY]);
        assert!(reset_boundaries_for_kind("default", 0, tue).is_empty());
        assert!(reset_boundaries_for_kind("daily", tue, tue).is_empty());
    }

    #[test]
    fn reset_cliffs() {
        let tue = 1_792_494_000;
        let pts = [p(tue - 3600, 4, 120.0), p(tue + 3600, 2, 10.0)];
        let out = inject_reset_zeros(&pts, "daily");
        assert_eq!(out, vec![(pts[0], false), (p(tue - 1, 4, 120.0), true), (p(tue, 0, 0.0), true), (pts[1], false)]);
        assert_eq!(inject_reset_zeros(&pts, "none").len(), 2);
    }

    #[test]
    fn split_two_identities() {
        let pts = [p(1, 1, 100.0), p(1, 2, 50.0), p(2, 1, 110.0), p(2, 2, 50.0), p(3, 2, 50.0), p(3, 1, 120.0)];
        let split = split_series(&pts);
        assert_eq!(split, vec![Split { slot: 0, points: vec![0, 2, 5] }, Split { slot: 1, points: vec![1, 3, 4] }]);
        assert_eq!(split_series(&[p(5, 1, 1.0), p(5, 1, 1.0)]), vec![Split { slot: 0, points: vec![0] }]);
    }

    #[test]
    fn permutation_order() {
        assert_eq!(permutations(3, 2), vec![vec![0, 1], vec![0, 2], vec![1, 0], vec![1, 2], vec![2, 0], vec![2, 1]]);
    }

    #[test]
    fn mastery_levels() {
        assert_eq!(level_from_points(0), (1, 0, 25));
        assert_eq!(level_from_points(25), (2, 0, 25));
        assert_eq!(level_from_points(26), (2, 1, 24));
        assert_eq!(level_from_points(i128::MAX / 2).0, 1001);
        assert_eq!(level_from_score(f64::NAN), None);
    }

    #[test]
    fn board_csv() {
        assert_eq!(parse_board_csv(" 4003, abc,1_000 ,\u{661}\u{660},,4003"), vec![10, 1000, 4003]);
    }
}
