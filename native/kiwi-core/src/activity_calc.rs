//! Pure parts of `app/trove/leaderboards/activity.py` and `class_activity.py`:
//! rollup windows, the weekly-reset test, chart bucketing and the per-class
//! headcounts. Store access and JSON shapes live in kiwi-server.

use std::collections::{BTreeMap, HashMap, HashSet};

use rustc_hash::FxHashMap;

use crate::{activity::Capture, gems::py_round};

pub const DAY: i64 = 86_400;
pub const ROLLUP_SLACK: f64 = 1.5;
/// `activity._ACTIVE_RETENTION_SECONDS`.
pub const ACTIVE_RETENTION: i64 = 11 * DAY;
/// `class_activity._ACTIVE_RETENTION_SECONDS`.
pub const CLASS_ACTIVE_RETENTION: i64 = 3 * DAY;
/// `_SHORT_RESET_WINDOW_SECONDS`.
pub const SHORT_RESET_WINDOW: i64 = 2 * 3600;
/// `_BACKFILL_ANCHOR_MARGIN`.
pub const BACKFILL_ANCHOR_MARGIN: i64 = 2 * DAY;
pub const XP_BOARD: i32 = 21005;
pub const CLASS_COUNT: i64 = crate::stats::BOARD_CLASS_ORDER.len() as i64;
pub const EFFORT_BASE: i32 = 4000;
pub const POWER_RANK_BASE: i32 = 1000;
/// `models._NON_PLAYER_BOARDS`.
pub const NON_PLAYER_BOARDS: [i32; 2] = [1100, 21012];

pub fn is_player_board(uuid: i32) -> bool {
    !NON_PLAYER_BOARDS.contains(&uuid)
}

pub fn effort_board_uuids() -> Vec<i32> {
    (0..CLASS_COUNT as i32).map(|i| EFFORT_BASE + i).collect()
}

pub fn pr_board_uuids() -> Vec<i32> {
    (0..CLASS_COUNT as i32).map(|i| POWER_RANK_BASE + i).collect()
}

/// `_median`: over the truthy, positive values.
pub fn median(values: &[f64]) -> Option<f64> {
    let mut vals: Vec<f64> = values.iter().copied().filter(|v| *v != 0.0 && *v > 0.0).collect();
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = vals.len();
    if n == 0 {
        return None;
    }
    let mid = n / 2;
    Some(if n % 2 == 1 { vals[mid] } else { (vals[mid - 1] + vals[mid]) / 2.0 })
}

/// `_intervals_hours`.
pub fn intervals_hours(anchors: &[i64]) -> Vec<f64> {
    let mut a = anchors.to_vec();
    a.sort_unstable();
    a.windows(2).map(|w| (w[1] - w[0]) as f64 / 3600.0).collect()
}

/// `_pick_early_anchor` over newest-first stamps.
pub fn pick_early_anchor(stamps_desc: &[i64], late: i64, seconds_back: i64) -> Option<i64> {
    let target = late - seconds_back;
    let mut oldest_before = None;
    for &s in stamps_desc {
        if s >= late {
            continue;
        }
        if s <= target {
            return Some(s);
        }
        oldest_before = Some(s);
    }
    oldest_before
}

/// `_rollup_measurable`.
pub fn rollup_measurable(early: i64, late: i64, period: i64) -> bool {
    ((late - early) as f64) <= period as f64 * ROLLUP_SLACK
}

/// The newest stamp at or before `late - DAY`
/// (`bisect_right(asc, late - DAY) - 1`).
pub fn day_start_anchor(stamps_asc: &[i64], late: i64) -> Option<i64> {
    let j = stamps_asc.partition_point(|&s| s <= late - DAY);
    (j > 0).then(|| stamps_asc[j - 1])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetKind {
    Daily,
    Weekly,
}

/// `bool(reset_boundaries_for_kind(kind, t_start, t_end))`: an 11:00 UTC
/// reset (Mondays for weekly) inside `(t_start, t_end]`.
pub fn reset_crossed(kind: ResetKind, t_start: i64, t_end: i64) -> bool {
    if t_end <= t_start {
        return false;
    }
    let mut cand = t_start.div_euclid(DAY) * DAY + 11 * 3600;
    if cand <= t_start {
        cand += DAY;
    }
    if kind == ResetKind::Weekly {
        while (cand.div_euclid(DAY) + 3).rem_euclid(7) != 0 {
            cand += DAY;
        }
    }
    cand <= t_end
}

/// Python `round(x)` to an int, half to even.
pub fn round_int(x: f64) -> i64 {
    x.round_ties_even() as i64
}

// ---------------------------------------------------------------------------
// Player activity chart

/// One stored `activity_estimate` row with a 24h value.
#[derive(Clone, Debug, PartialEq)]
pub struct SeriesRow {
    pub window_end: i64,
    pub duration_hours: f64,
    pub estimate_24h: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SeriesPoint {
    pub t: i64,
    pub active: f64,
    pub avg: f64,
    pub peak: f64,
    pub samples: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Extreme {
    pub t: i64,
    pub active: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    pub bucket: i64,
    pub points: Vec<SeriesPoint>,
    pub peak: Option<Extreme>,
    pub low: Option<Extreme>,
    pub average: Option<f64>,
    pub latest: Option<f64>,
}

struct Bucket {
    sum: f64,
    n: i64,
    t_sum: i64,
    max: f64,
    max_t: i64,
    min: f64,
    min_t: i64,
}

/// The bucketing of `activity_series` (rows ascending by window end).
pub fn activity_series(rows: &[SeriesRow], min_bucket: i64) -> Series {
    let mut bucket = min_bucket;
    if let Some(c) = median(&rows.iter().map(|r| r.duration_hours).collect::<Vec<_>>()) {
        bucket = bucket.max(round_int(c) * 3600);
    }
    let mut agg: BTreeMap<i64, Bucket> = BTreeMap::new();
    for r in rows {
        let v = r.estimate_24h as f64;
        let te = r.window_end;
        let b = te.div_euclid(bucket) * bucket;
        match agg.get_mut(&b) {
            None => {
                agg.insert(b, Bucket { sum: v, n: 1, t_sum: te, max: v, max_t: te, min: v, min_t: te });
            }
            Some(a) => {
                a.sum += v;
                a.n += 1;
                a.t_sum += te;
                if v > a.max {
                    a.max = v;
                    a.max_t = te;
                }
                if v < a.min {
                    a.min = v;
                    a.min_t = te;
                }
            }
        }
    }
    let mut peak_b: Option<i64> = None;
    let mut low_b: Option<i64> = None;
    for (&b, a) in &agg {
        if peak_b.is_none_or(|p| a.max > agg[&p].max) {
            peak_b = Some(b);
        }
        if low_b.is_none_or(|l| a.min < agg[&l].min) {
            low_b = Some(b);
        }
    }
    let mut points = Vec::with_capacity(agg.len());
    let mut avg_running = 0.0;
    for (&b, a) in &agg {
        let avg = a.sum / a.n as f64;
        avg_running += avg;
        let (active, t) = if Some(b) == peak_b {
            (py_round(a.max, 1), a.max_t)
        } else if Some(b) == low_b {
            (py_round(a.min, 1), a.min_t)
        } else {
            (py_round(avg, 1), round_int(a.t_sum as f64 / a.n as f64))
        };
        points.push(SeriesPoint { t, active, avg: py_round(avg, 1), peak: py_round(a.max, 1), samples: a.n });
    }
    let peak = peak_b.map(|b| Extreme { t: agg[&b].max_t, active: py_round(agg[&b].max, 1) });
    let low = low_b.map(|b| Extreme { t: agg[&b].min_t, active: py_round(agg[&b].min, 1) });
    let average = (!points.is_empty()).then(|| py_round(avg_running / points.len() as f64, 1));
    let latest = rows.last().map(|r| r.estimate_24h as f64);
    Series { bucket, points, peak, low, average, latest }
}

// ---------------------------------------------------------------------------
// Class activity chart

/// One stored `class_activity_estimate` row (only the fields the chart reads).
#[derive(Clone, Debug, PartialEq)]
pub struct ClassSeriesRow {
    pub class_index: i64,
    pub window_end: i64,
    pub duration_hours: f64,
    pub estimate_24h: Option<i64>,
    pub estimate_24h_clean: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClassLine {
    pub values: Vec<Option<f64>>,
    pub values_clean: Vec<Option<f64>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClassSeries {
    pub bucket: i64,
    pub buckets: Vec<i64>,
    /// One per class index `0..CLASS_COUNT`.
    pub lines: Vec<ClassLine>,
}

#[derive(Default)]
struct ClassAgg {
    raw_sum: f64,
    raw_n: i64,
    raw_max: f64,
    clean_sum: f64,
    clean_n: i64,
    clean_max: f64,
}

/// `_peak_preserve`.
pub fn peak_preserve(avgs: &[Option<f64>], maxes: &[Option<f64>]) -> Vec<Option<f64>> {
    let mut peak: Option<usize> = None;
    for (i, m) in maxes.iter().enumerate() {
        let Some(m) = m else { continue };
        if peak.is_none_or(|p| *m > maxes[p].unwrap_or(f64::NAN)) {
            peak = Some(i);
        }
    }
    let mut out = avgs.to_vec();
    if let Some(p) = peak {
        out[p] = maxes[p].map(|m| py_round(m, 1));
    }
    out
}

/// The bucketing of `class_activity_series`. `bucket` is the period's
/// minimum, or `None` for the dynamic "all" bucket over `now - window_start`.
pub fn class_series(rows: &[ClassSeriesRow], bucket: Option<i64>, now: i64, window_start: i64) -> ClassSeries {
    let rows: Vec<&ClassSeriesRow> = rows.iter().filter(|r| r.estimate_24h.is_some()).collect();
    let mut bucket = match bucket {
        Some(b) => b,
        None => {
            let span = 3600.max(now - window_start);
            3600.max(((span as f64 / 120.0) as i64).div_euclid(3600) * 3600)
        }
    };
    let mut by_end: Vec<(i64, f64)> = Vec::new();
    let mut at: HashMap<i64, usize> = HashMap::new();
    for r in &rows {
        match at.get(&r.window_end) {
            Some(&i) => by_end[i].1 = r.duration_hours,
            None => {
                at.insert(r.window_end, by_end.len());
                by_end.push((r.window_end, r.duration_hours));
            }
        }
    }
    if let Some(c) = median(&by_end.iter().map(|x| x.1).collect::<Vec<_>>()) {
        bucket = bucket.max(round_int(c) * 3600);
    }
    let mut agg: BTreeMap<i64, (i64, i64, HashMap<i64, ClassAgg>)> = BTreeMap::new();
    for r in &rows {
        let raw_v = r.estimate_24h.unwrap_or(0) as f64;
        let b = r.window_end.div_euclid(bucket) * bucket;
        let bd = agg.entry(b).or_insert_with(|| (0, 0, HashMap::new()));
        bd.0 += r.window_end;
        bd.1 += 1;
        let c = bd.2.entry(r.class_index).or_default();
        c.raw_sum += raw_v;
        c.raw_n += 1;
        if raw_v > c.raw_max {
            c.raw_max = raw_v;
        }
        if let Some(cl) = r.estimate_24h_clean {
            let v = cl as f64;
            c.clean_sum += v;
            c.clean_n += 1;
            if v > c.clean_max {
                c.clean_max = v;
            }
        }
    }
    let buckets = agg.values().map(|(t_sum, t_n, _)| round_int(*t_sum as f64 / *t_n as f64)).collect();
    let mut lines = Vec::with_capacity(CLASS_COUNT as usize);
    for i in 0..CLASS_COUNT {
        let (mut raw_avgs, mut raw_maxes, mut clean_avgs, mut clean_maxes) = (vec![], vec![], vec![], vec![]);
        for (_, _, classes) in agg.values() {
            let c = classes.get(&i);
            match c.filter(|c| c.raw_n > 0) {
                Some(c) => {
                    raw_avgs.push(Some(py_round(c.raw_sum / c.raw_n as f64, 1)));
                    raw_maxes.push(Some(c.raw_max));
                }
                None => {
                    raw_avgs.push(None);
                    raw_maxes.push(None);
                }
            }
            match c.filter(|c| c.clean_n > 0) {
                Some(c) => {
                    clean_avgs.push(Some(py_round(c.clean_sum / c.clean_n as f64, 1)));
                    clean_maxes.push(Some(c.clean_max));
                }
                None => {
                    clean_avgs.push(None);
                    clean_maxes.push(None);
                }
            }
        }
        lines.push(ClassLine {
            values: peak_preserve(&raw_avgs, &raw_maxes),
            values_clean: peak_preserve(&clean_avgs, &clean_maxes),
        });
    }
    ClassSeries { bucket, buckets, lines }
}

// ---------------------------------------------------------------------------
// Class headcounts

/// The clean-view floors as `_clean_thresholds` returns them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Thresholds {
    pub power_rank: i64,
    pub effort: i64,
    pub xp: i64,
}

impl Thresholds {
    pub fn gate_uuids(&self) -> Vec<i32> {
        let mut g = pr_board_uuids();
        if self.xp > 0 {
            g.push(XP_BOARD);
        }
        g
    }

    pub fn floors(&self) -> crate::activity::Floors {
        crate::activity::Floors {
            power_rank: self.power_rank as f64,
            effort: self.effort as f64,
            xp: self.xp as f64,
            xp_board: XP_BOARD,
        }
    }
}

/// One class's counts: raw, clean (None when its Power Rank board is absent),
/// and the donut's Effort added in the last 24h (None when unmeasured).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ClassCount {
    pub raw: i64,
    pub clean: Option<i64>,
    pub effort_raw: Option<i64>,
    pub effort_clean: Option<i64>,
}

fn passes(
    p: i64,
    pr: &FxHashMap<i64, f64>,
    effort_score: f64,
    xp: Option<&FxHashMap<i64, f64>>,
    t: &Thresholds,
) -> bool {
    pr.get(&p).copied().unwrap_or(0.0) >= t.power_rank as f64
        && effort_score >= t.effort as f64
        && xp.is_none_or(|x| x.get(&p).copied().unwrap_or(0.0) >= t.xp as f64)
}

fn xp_map<'a>(gates: &'a Capture, t: &Thresholds) -> Option<&'a FxHashMap<i64, f64>> {
    (t.xp > 0).then(|| gates.boards.get(&XP_BOARD)).flatten()
}

/// `_snapshot_counts`: per class, the players on its Effort board.
pub fn snapshot_counts(effort: &Capture, gates: &Capture, t: &Thresholds) -> BTreeMap<i64, ClassCount> {
    let xp = xp_map(gates, t);
    let mut out = BTreeMap::new();
    for i in 0..CLASS_COUNT {
        let Some(board) = effort.boards.get(&(EFFORT_BASE + i as i32)).filter(|b| !b.is_empty()) else { continue };
        let clean = gates
            .boards
            .get(&(POWER_RANK_BASE + i as i32))
            .map(|pr| board.iter().filter(|(p, score)| passes(**p, pr, **score, xp, t)).count() as i64);
        out.insert(i, ClassCount { raw: board.len() as i64, clean, ..Default::default() });
    }
    out
}

/// `_effort_deltas`: per class, the summed positive score gains of players on
/// its Effort board in both snapshots.
pub fn effort_deltas(
    late: &Capture,
    early: &Capture,
    gates: &Capture,
    t: &Thresholds,
) -> BTreeMap<i64, (i64, Option<i64>)> {
    let xp = xp_map(gates, t);
    let empty = FxHashMap::default();
    let mut out = BTreeMap::new();
    for i in 0..CLASS_COUNT {
        let uuid = EFFORT_BASE + i as i32;
        let Some(l) = late.boards.get(&uuid).filter(|b| !b.is_empty()) else { continue };
        let e = early.boards.get(&uuid).unwrap_or(&empty);
        let pr = gates.boards.get(&(POWER_RANK_BASE + i as i32));
        let mut raw = 0.0;
        let mut clean = pr.map(|_| 0.0);
        for (p, &lv) in l {
            let Some(&ev) = e.get(p) else { continue };
            let gain = lv - ev;
            if gain <= 0.0 {
                continue;
            }
            raw += gain;
            if let (Some(c), Some(pr)) = (clean.as_mut(), pr)
                && passes(*p, pr, lv, xp, t)
            {
                *c += gain;
            }
        }
        out.insert(i, (round_int(raw), clean.map(round_int)));
    }
    out
}

/// One class as `_build_current` lists it (before name/icon lookup).
#[derive(Clone, Debug, PartialEq)]
pub struct CurrentClass {
    pub class_index: i64,
    pub active_players: i64,
    pub share: f64,
    pub active_players_clean: Option<i64>,
    pub share_clean: Option<f64>,
    pub effort_added: Option<i64>,
    pub effort_added_clean: Option<i64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CurrentTotals {
    pub total_active: Option<i64>,
    pub total_active_clean: Option<i64>,
    pub total_effort_added: Option<i64>,
    pub total_effort_added_clean: Option<i64>,
    pub classes: Vec<CurrentClass>,
}

/// The totals, shares and ordering of `_build_current`.
pub fn build_current(counts: &BTreeMap<i64, ClassCount>) -> CurrentTotals {
    let raw_total: i64 = counts.values().map(|c| c.raw).sum();
    let cleans: Vec<i64> = counts.values().filter_map(|c| c.clean).collect();
    let clean_total = (!cleans.is_empty()).then(|| cleans.iter().sum::<i64>());
    let eff_raw: Vec<i64> = counts.values().filter_map(|c| c.effort_raw).collect();
    let eff_clean: Vec<i64> = counts.values().filter_map(|c| c.effort_clean).collect();
    let mut classes: Vec<CurrentClass> = counts
        .iter()
        .map(|(&i, c)| CurrentClass {
            class_index: i,
            active_players: c.raw,
            share: if raw_total != 0 { py_round(c.raw as f64 / raw_total as f64, 4) } else { 0.0 },
            active_players_clean: c.clean,
            share_clean: match (c.clean, clean_total) {
                (Some(cl), Some(tot)) if tot != 0 => Some(py_round(cl as f64 / tot as f64, 4)),
                (Some(_), _) => Some(0.0),
                (None, _) => None,
            },
            effort_added: c.effort_raw,
            effort_added_clean: c.effort_clean,
        })
        .collect();
    classes.sort_by_key(|c| {
        (c.active_players_clean.is_none(), -c.active_players_clean.unwrap_or(0), -c.active_players, c.class_index)
    });
    CurrentTotals {
        total_active: (!counts.is_empty()).then_some(raw_total),
        total_active_clean: clean_total,
        total_effort_added: (!eff_raw.is_empty()).then(|| eff_raw.iter().sum()),
        total_effort_added_clean: (!eff_clean.is_empty()).then(|| eff_clean.iter().sum()),
        classes,
    }
}

// ---------------------------------------------------------------------------
// Backfill replays

/// Contiguous replay windows, oldest first: `(early, late, item)`.
pub type Chain<T> = std::collections::VecDeque<(i64, i64, T)>;

/// Push a window, restarting the chain when it doesn't continue the last one.
pub fn chain_push<T>(chain: &mut Chain<T>, early: i64, late: i64, item: T) {
    if chain.back().is_some_and(|b| b.1 != early) {
        chain.clear();
    }
    chain.push_back((early, late, item));
}

/// Where a replay's 24h rollup at `anchor` comes from.
#[derive(Debug, PartialEq, Eq)]
pub enum RollupSource {
    /// No value.
    None,
    /// The union of the in-memory chain (already trimmed to the span).
    Chain,
    /// The materialized table over `(early, anchor]`.
    Table(i64),
}

/// The shared head of `_replay_rollup_24h` / `_replay_rollups_24h`: trims the
/// chain to the rollup span and picks the source.
pub fn replay_source<T>(chain: &mut Chain<T>, stamps_asc: &[i64], anchor: i64, active_floor: i64) -> RollupSource {
    let Some(early) = day_start_anchor(stamps_asc, anchor) else { return RollupSource::None };
    while chain.front().is_some_and(|c| c.1 <= early) {
        chain.pop_front();
    }
    if !rollup_measurable(early, anchor, DAY) {
        return RollupSource::None;
    }
    if chain.front().is_some_and(|c| c.0 <= early) {
        return RollupSource::Chain;
    }
    if early >= active_floor { RollupSource::Table(early) } else { RollupSource::None }
}

/// `len(set().union(*names))` over a player chain.
pub fn chain_union_len(chain: &Chain<HashSet<String>>) -> i64 {
    let mut all: HashSet<&str> = HashSet::new();
    for (_, _, names) in chain {
        all.extend(names.iter().map(String::as_str));
    }
    all.len() as i64
}

/// One window's per-class sets: names and clean names (None = unmeasurable).
pub type ClassSets = BTreeMap<i64, (HashSet<String>, Option<HashSet<String>>)>;

/// The chain branch of `_replay_rollups_24h`: per class measurable in every
/// window, the raw and clean union sizes.
pub fn chain_class_rollups(chain: &Chain<ClassSets>) -> BTreeMap<i64, (i64, Option<i64>)> {
    let mut out = BTreeMap::new();
    let Some(last) = chain.back() else { return out };
    for &i in last.2.keys() {
        if !chain.iter().all(|w| w.2.contains_key(&i)) {
            continue;
        }
        let mut raw: HashSet<&str> = HashSet::new();
        let mut clean: Option<HashSet<&str>> = Some(HashSet::new());
        for w in chain {
            let (names, c) = &w.2[&i];
            raw.extend(names.iter().map(String::as_str));
            match (c, clean.as_mut()) {
                (Some(c), Some(acc)) => acc.extend(c.iter().map(String::as_str)),
                _ => clean = None,
            }
        }
        out.insert(i, (raw.len() as i64, clean.map(|c| c.len() as i64)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weekly_reset_is_monday_eleven() {
        // Monday 2026-10-19 11:00 UTC.
        let reset = 1_792_407_600;
        assert!(reset_crossed(ResetKind::Weekly, reset - 1, reset));
        assert!(!reset_crossed(ResetKind::Weekly, reset, reset + 6 * DAY));
        assert!(reset_crossed(ResetKind::Daily, reset, reset + DAY));
        assert!(!reset_crossed(ResetKind::Daily, reset, reset + DAY - 1));
        assert!(!reset_crossed(ResetKind::Weekly, reset + 5, reset + 5));
    }

    #[test]
    fn early_anchor_and_rollup_start() {
        let desc = [100_000, 90_000, 10_000, 5_000];
        assert_eq!(pick_early_anchor(&desc, 100_000, DAY), Some(10_000));
        assert_eq!(pick_early_anchor(&desc, 100_000, 7 * DAY), Some(5_000));
        assert_eq!(pick_early_anchor(&[100], 100, DAY), None);
        assert_eq!(day_start_anchor(&[5_000, 10_000, 13_600, 100_000], 100_000), Some(13_600));
        assert_eq!(day_start_anchor(&[90_000], 100_000), None);
    }

    #[test]
    fn median_skips_zero() {
        assert_eq!(median(&[0.0, 3.0, 1.0]), Some(2.0));
        assert_eq!(median(&[0.0]), None);
        assert_eq!(median(&[6.0, 5.0, 7.0]), Some(6.0));
    }
}
