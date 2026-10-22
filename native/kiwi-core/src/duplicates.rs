//! Duplicate-name detection for `app/trove/leaderboards/duplicates.py`: the
//! series splitter (one board's rows for one name into per-identity lines),
//! the per-series summary, the verdict and the summary text. Never decides
//! which identity is the real one.

use std::collections::BTreeMap;

use crate::gems::pynum::{py_fsum, py_max};
use crate::renames::float_cmp;

pub const VERDICT_ONE_LIVE: &str = "one_live";
pub const VERDICT_MULTI_LIVE: &str = "multi_live";
pub const VERDICT_ALL_IDLE: &str = "all_idle";
pub const VERDICT_CASE_ONLY: &str = "case_only";
pub const VERDICT_NOT_ANALYSED: &str = "not_analysed";

const DROP_PENALTY: f64 = 8.0;
const BRUTE_FORCE_MAX: usize = 5;
const VELOCITY_WINDOW: usize = 4;

/// One entry row: `{"created_at", "rank", "score"}`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub created_at: i64,
    pub rank: i64,
    pub score: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    pub slot: usize,
    pub points: Vec<Point>,
}

/// `_velocity`: recent score change per capture, floored at 0.
fn velocity(points: &[Point]) -> f64 {
    if points.len() < 2 {
        return 0.0;
    }
    let tail = &points[points.len().saturating_sub(VELOCITY_WINDOW)..];
    let span = tail.len() - 1;
    if span == 0 {
        return 0.0;
    }
    py_max(0.0, (tail[span].score - tail[0].score) / span as f64)
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
    fn go(n: usize, r: usize, cur: &mut Vec<usize>, used: &mut [bool], out: &mut Vec<Vec<usize>>) {
        if cur.len() == r {
            out.push(cur.clone());
            return;
        }
        for i in 0..n {
            if !used[i] {
                used[i] = true;
                cur.push(i);
                go(n, r, cur, used, out);
                cur.pop();
                used[i] = false;
            }
        }
    }
    let mut out = Vec::new();
    if r <= n {
        go(n, r, &mut Vec::with_capacity(r), &mut vec![false; n], &mut out);
    }
    out
}

/// `_assign`: the series slot for each row (`None` opens a new series).
fn assign(scores: &[f64], lasts: &[f64], predicted: &[f64]) -> Vec<Option<usize>> {
    let (n, m) = (scores.len(), lasts.len());
    if n == 1 && m == 1 {
        return vec![Some(0)];
    }
    if n.max(m) <= BRUTE_FORCE_MAX {
        let mut best_cost = f64::INFINITY;
        let mut out = vec![None; n];
        if m <= n {
            let mut best: Option<Vec<usize>> = None;
            for rows_for_series in permutations(n, m) {
                let cost = py_fsum(
                    rows_for_series.iter().enumerate().map(|(s, &r)| pair_cost(scores[r], lasts[s], predicted[s])),
                );
                if cost < best_cost {
                    best_cost = cost;
                    best = Some(rows_for_series);
                }
            }
            if let Some(best) = best {
                for (s, r) in best.into_iter().enumerate() {
                    out[r] = Some(s);
                }
            }
            return out;
        }
        let mut best_rows: Option<Vec<usize>> = None;
        for series_for_rows in permutations(m, n) {
            let cost =
                py_fsum(series_for_rows.iter().enumerate().map(|(i, &s)| pair_cost(scores[i], lasts[s], predicted[s])));
            if cost < best_cost {
                best_cost = cost;
                best_rows = Some(series_for_rows);
            }
        }
        return best_rows.map_or(out, |b| b.into_iter().map(Some).collect());
    }
    let mut used = vec![false; m];
    scores
        .iter()
        .map(|&score| {
            let mut pick: Option<usize> = None;
            let mut pick_cost = f64::INFINITY;
            for i in 0..m {
                if used[i] {
                    continue;
                }
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

/// `split_series`: one board's rows for one name, as continuous per-identity
/// series with stable slots (earliest start first, then higher score).
pub fn split_series(points: &[Point]) -> Vec<Series> {
    if points.is_empty() {
        return Vec::new();
    }
    let mut by_anchor: BTreeMap<i64, Vec<Point>> = BTreeMap::new();
    for p in points {
        let bucket = by_anchor.entry(p.created_at).or_default();
        if bucket.iter().any(|q| q.rank == p.rank && q.score == p.score) {
            continue;
        }
        bucket.push(*p);
    }
    if by_anchor.values().all(|v| v.len() == 1) {
        return vec![Series { slot: 0, points: by_anchor.values().map(|v| v[0]).collect() }];
    }

    let mut series: Vec<Vec<Point>> = Vec::new();
    for rows in by_anchor.into_values() {
        let mut rows = rows;
        rows.sort_by(|a, b| float_cmp(-a.score, -b.score).then(a.rank.cmp(&b.rank)));
        if series.is_empty() {
            series = rows.into_iter().map(|r| vec![r]).collect();
            continue;
        }
        let scores: Vec<f64> = rows.iter().map(|r| r.score).collect();
        let lasts: Vec<f64> = series.iter().map(|s| s[s.len() - 1].score).collect();
        let predicted: Vec<f64> = series.iter().zip(&lasts).map(|(s, last)| last + velocity(s)).collect();
        for (row, slot) in rows.into_iter().zip(assign(&scores, &lasts, &predicted)) {
            match slot {
                None => series.push(vec![row]),
                Some(s) => series[s].push(row),
            }
        }
    }

    let mut order: Vec<usize> = (0..series.len()).collect();
    order.sort_by(|&a, &b| {
        let (pa, pb) = (series[a][0], series[b][0]);
        pa.created_at.cmp(&pb.created_at).then(float_cmp(-pa.score, -pb.score))
    });
    let mut taken: Vec<Option<Vec<Point>>> = series.into_iter().map(Some).collect();
    order
        .into_iter()
        .enumerate()
        .map(|(slot, old)| Series { slot, points: taken[old].take().unwrap_or_default() })
        .collect()
}

/// One series as `summarise_series` reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct SeriesSummary {
    pub slot: usize,
    pub captures: usize,
    pub first_anchor: i64,
    pub last_anchor: i64,
    pub first_score: f64,
    pub last_score: f64,
    pub last_rank: i64,
    pub moved: bool,
    pub frozen: bool,
}

/// Python `max(xs) != min(xs)` over floats.
fn spread(xs: &[f64]) -> bool {
    let mut hi = xs[0];
    let mut lo = xs[0];
    for &x in &xs[1..] {
        if x > hi {
            hi = x;
        }
        if x < lo {
            lo = x;
        }
    }
    hi != lo
}

/// `summarise_series`.
pub fn summarise_series(split: &[Series], frozen_min_captures: i64) -> Vec<SeriesSummary> {
    split
        .iter()
        .map(|s| {
            let pts = &s.points;
            let scores: Vec<f64> = pts.iter().map(|p| p.score).collect();
            let moved = scores.len() > 1 && spread(&scores);
            let (first, last) = (pts[0], pts[pts.len() - 1]);
            SeriesSummary {
                slot: s.slot,
                captures: pts.len(),
                first_anchor: first.created_at,
                last_anchor: last.created_at,
                first_score: first.score,
                last_score: last.score,
                last_rank: last.rank,
                moved,
                frozen: !moved && i64::try_from(pts.len()).unwrap_or(i64::MAX) >= frozen_min_captures,
            }
        })
        .collect()
}

/// `_verdict` over each board's series reports.
pub fn verdict<'a>(boards: impl IntoIterator<Item = &'a [SeriesSummary]>) -> &'static str {
    let mut live = std::collections::BTreeSet::new();
    let mut seen = std::collections::BTreeSet::new();
    for board in boards {
        for s in board {
            seen.insert(s.slot);
            if s.moved {
                live.insert(s.slot);
            }
        }
    }
    if live.is_empty() {
        return VERDICT_ALL_IDLE;
    }
    if live.len() >= 2 {
        return VERDICT_MULTI_LIVE;
    }
    if seen.len() > live.len() { VERDICT_ONE_LIVE } else { VERDICT_ALL_IDLE }
}

/// `_summary_text`.
pub fn summary_text(name: &str, boards: i64, verdict: &str, series_count: i64) -> String {
    let plural = if boards != 1 { "s" } else { "" };
    if verdict == VERDICT_CASE_ONLY {
        return format!(
            "Two or more spellings of \u{201c}{name}\u{201d} differ only in capitalisation, so Trove's leaderboards \
             treat them as one name here and their scores are merged into a single profile."
        );
    }
    let head = format!(
        "Trove's own capture lists \u{201c}{name}\u{201d} {series_count} times on the same board across {boards} \
         board{plural}"
    );
    match verdict {
        VERDICT_MULTI_LIVE => format!(
            "{head}, and more than one of those score lines is still moving - they look like separate active \
             identities sharing a name."
        ),
        VERDICT_ONE_LIVE => format!(
            "{head}, and only one of those score lines is still moving; the other has not changed all window, which \
             is what a leftover leaderboard row looks like."
        ),
        _ => format!(
            "{head}. None of the score lines moved in this window, so there is no way to tell them apart from activity alone."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(created_at: i64, rank: i64, score: f64) -> Point {
        Point { created_at, rank, score }
    }

    #[test]
    fn live_line_keeps_its_slot_when_it_overtakes_a_ghost() {
        let pts = [
            p(1, 1, 7.0e6),
            p(1, 5, 2.0e6),
            p(2, 1, 7.0e6),
            p(2, 4, 4.0e6),
            p(3, 1, 7.0e6),
            p(3, 3, 6.0e6),
            p(4, 1, 8.0e6),
            p(4, 2, 7.0e6),
        ];
        let split = split_series(&pts);
        assert_eq!(split.len(), 2);
        assert_eq!(split[1].points.iter().map(|p| p.score).collect::<Vec<_>>(), [2.0e6, 4.0e6, 6.0e6, 8.0e6]);
        let sums = summarise_series(&split, 4);
        assert!(sums[0].frozen && sums[1].moved);
        assert_eq!(verdict([sums.as_slice()]), VERDICT_ONE_LIVE);
    }

    #[test]
    fn identical_repeat_is_one_row() {
        let split = split_series(&[p(1, 3, 5.0), p(1, 3, 5.0), p(2, 3, 6.0)]);
        assert_eq!(split, vec![Series { slot: 0, points: vec![p(1, 3, 5.0), p(2, 3, 6.0)] }]);
    }

    #[test]
    fn permutations_in_itertools_order() {
        assert_eq!(permutations(3, 2), [[0, 1], [0, 2], [1, 0], [1, 2], [2, 0], [2, 1]]);
        assert!(permutations(1, 2).is_empty());
    }
}
