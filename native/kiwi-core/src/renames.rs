//! Rename detection for `app/trove/leaderboards/renames.py`: per name key, the
//! lifetime-board scores at one capture, the names that vanished or appeared
//! between two captures, and the conservative matcher (`_match`,
//! `_confidence`, `_summary`) over them.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};

use rustc_hash::FxHashMap;

use crate::activity::Capture;
use crate::gems::py_round;
use crate::gems::pynum::{py_fsum, py_max};

/// One name key's fingerprint: best score per board, and the player ids behind it.
#[derive(Default)]
pub struct Fp {
    pub boards: BTreeMap<i32, f64>,
    pub ids: Vec<i64>,
}

pub type Fps = FxHashMap<i64, Fp>;

/// `_fingerprints`: `keys` maps each player id to its name key (`name.strip().lower()`
/// numbered by the caller). None when a player id has no key yet.
pub fn fingerprint(cap: &Capture, min_score: f64, keys: &FxHashMap<i64, i64>) -> Option<Fps> {
    let mut out: Fps = FxHashMap::default();
    for (board, scores) in &cap.boards {
        for (pid, &score) in scores {
            if score < min_score {
                continue;
            }
            let fp = out.entry(*keys.get(pid)?).or_default();
            let best = fp.boards.entry(*board).or_insert(score);
            if score > *best {
                *best = score;
            }
            if !fp.ids.contains(pid) {
                fp.ids.push(*pid);
            }
        }
    }
    Some(out)
}

/// [`fingerprint`] that leaves out players without a key instead of failing
/// (what the Python fallback's `JOIN player` does).
pub fn fingerprint_known(cap: &Capture, min_score: f64, key_of: impl Fn(i64) -> Option<i64>) -> Fps {
    let mut out: Fps = FxHashMap::default();
    for (board, scores) in &cap.boards {
        for (pid, &score) in scores {
            if score < min_score {
                continue;
            }
            let Some(key) = key_of(*pid) else { continue };
            let fp = out.entry(key).or_default();
            let best = fp.boards.entry(*board).or_insert(score);
            if score > *best {
                *best = score;
            }
            if !fp.ids.contains(pid) {
                fp.ids.push(*pid);
            }
        }
    }
    out
}

/// A side of the diff: (key, [(board, score)], player ids).
pub type Side = Vec<(i64, Vec<(i32, f64)>, Vec<i64>)>;

/// Keys only in `a` and only in `b`, each with at least `min_boards` boards.
pub fn diff(a: &Fps, b: &Fps, min_boards: usize) -> (Side, Side) {
    let only = |x: &Fps, y: &Fps| -> Side {
        let mut v: Side = x
            .iter()
            .filter(|(k, fp)| !y.contains_key(k) && fp.boards.len() >= min_boards)
            .map(|(k, fp)| (*k, fp.boards.iter().map(|(b, s)| (*b, *s)).collect(), fp.ids.clone()))
            .collect();
        v.sort_unstable_by_key(|e| e.0);
        v
    };
    (only(a, b), only(b, a))
}

/// One side of a pair as `_match` takes it: `{"display": .., "boards": {uuid: score}}`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Named {
    pub display: String,
    pub boards: BTreeMap<i32, f64>,
}

/// The `renames_*` knobs the matcher reads.
#[derive(Clone, Copy, Debug)]
pub struct MatchCfg {
    pub min_boards: i64,
    pub drift: f64,
    pub min_conf: f64,
}

/// `_confidence`'s sub-terms, in the evidence key order.
#[derive(Clone, Debug, PartialEq)]
pub struct Terms {
    pub matched_boards: usize,
    pub runnerup_overlap: usize,
    pub board_term: f64,
    pub tightness: f64,
    pub exclusivity: f64,
    pub rarity: f64,
    pub mean_drift_pct: f64,
    pub confidence: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BoardMatch {
    pub uuid: i32,
    pub score_from: f64,
    pub score_to: f64,
    pub drift_pct: f64,
}

/// One detected rename, before the caller adds anchors, board names and stamps.
#[derive(Clone, Debug, PartialEq)]
pub struct Rename {
    pub from_name: String,
    pub to_name: String,
    pub confidence: f64,
    pub matched_boards: usize,
    pub boards: Vec<BoardMatch>,
    pub terms: Terms,
    pub summary: String,
}

const CONFIDENCE_CEILING: f64 = 0.97;

/// Python's float ordering as a sort key (-0.0 equals 0.0); NaN sorts last.
pub(crate) fn float_cmp(a: f64, b: f64) -> Ordering {
    a.partial_cmp(&b).unwrap_or_else(|| a.is_nan().cmp(&b.is_nan()))
}

/// `bisect.bisect_left`.
fn bisect_left(a: &[f64], x: f64) -> usize {
    let (mut lo, mut hi) = (0, a.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        if a[mid] < x { lo = mid + 1 } else { hi = mid }
    }
    lo
}

/// `bisect.bisect_right`.
fn bisect_right(a: &[f64], x: f64) -> usize {
    let (mut lo, mut hi) = (0, a.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        if x < a[mid] { hi = mid } else { lo = mid + 1 }
    }
    lo
}

/// `(len(m), -sum(drifts))`.
type Rank = (usize, f64);

/// Python tuple `a > b`.
fn rank_gt(a: Rank, b: Rank) -> bool {
    if a.0 != b.0 {
        a.0 > b.0
    } else if a.1 != b.1 {
        a.1 > b.1
    } else {
        false
    }
}

/// One matched board: (uuid, drift fraction, appeared names in that score cell).
type Cell = (i32, f64, usize);

/// An appeared name and the boards it matches a disappeared one on.
type Candidate = (usize, Vec<Cell>);

fn rank(m: &[Cell]) -> Rank {
    (m.len(), -py_fsum(m.iter().map(|c| c.1)))
}

/// `_match`: disappeared (A-only) to appeared (B-only) names, mutual and
/// unambiguous best matches only, in disappeared-name order.
pub fn match_pair(fp_a: &BTreeMap<String, Named>, fp_b: &BTreeMap<String, Named>, cfg: &MatchCfg) -> Vec<Rename> {
    let min_boards = usize::try_from(cfg.min_boards.max(1)).unwrap_or(usize::MAX);
    let disappeared: Vec<&Named> =
        fp_a.iter().filter(|(k, v)| !fp_b.contains_key(*k) && v.boards.len() >= min_boards).map(|(_, v)| v).collect();
    let appeared: Vec<&Named> =
        fp_b.iter().filter(|(k, v)| !fp_a.contains_key(*k) && v.boards.len() >= min_boards).map(|(_, v)| v).collect();
    if disappeared.is_empty() || appeared.is_empty() {
        return Vec::new();
    }

    let mut index: HashMap<i32, (Vec<f64>, Vec<usize>)> = HashMap::new();
    {
        let mut tmp: HashMap<i32, Vec<(f64, usize)>> = HashMap::new();
        for (ay, fp) in appeared.iter().enumerate() {
            for (&uuid, &score) in &fp.boards {
                tmp.entry(uuid).or_default().push((score, ay));
            }
        }
        for (uuid, mut pairs) in tmp {
            pairs.sort_by(|a, b| float_cmp(a.0, b.0));
            index.insert(uuid, pairs.into_iter().unzip());
        }
    }
    let candidates = |uuid: i32, s_from: f64| -> &[usize] {
        let Some((scores, names)) = index.get(&uuid) else { return &[] };
        let i = bisect_left(scores, s_from * (1.0 - 1e-9));
        let j = bisect_right(scores, s_from * (1.0 + cfg.drift));
        if j > i { &names[i..j] } else { &[] }
    };

    // matches: [(dx, [(ay, cells)])], both in insertion order.
    let mut matches: Vec<(usize, Vec<Candidate>)> = Vec::new();
    for (dx, xfp) in disappeared.iter().enumerate() {
        let mut per_cand: Vec<Candidate> = Vec::new();
        let mut pos: HashMap<usize, usize> = HashMap::new();
        for (&uuid, &s_from) in &xfp.boards {
            let cands = candidates(uuid, s_from);
            for &ay in cands {
                let s_to = appeared[ay].boards[&uuid];
                let drift = if s_from > 0.0 { (s_to - s_from) / s_from } else { 0.0 };
                let at = *pos.entry(ay).or_insert_with(|| {
                    per_cand.push((ay, Vec::new()));
                    per_cand.len() - 1
                });
                per_cand[at].1.push((uuid, drift, cands.len()));
            }
        }
        per_cand.retain(|(_, m)| m.len() >= min_boards);
        if !per_cand.is_empty() {
            matches.push((dx, per_cand));
        }
    }
    if matches.is_empty() {
        return Vec::new();
    }

    // Best candidate and runner-up overlap per disappeared name.
    let mut best_x: Vec<(usize, usize)> = Vec::with_capacity(matches.len());
    let mut runnerup: Vec<usize> = Vec::with_capacity(matches.len());
    for (_, cands) in &matches {
        // The first two of a stable descending sort by rank.
        let ranks: Vec<Rank> = cands.iter().map(|(_, m)| rank(m)).collect();
        let first = (1..ranks.len()).fold(0, |b, i| if rank_gt(ranks[i], ranks[b]) { i } else { b });
        let second =
            (0..ranks.len()).filter(|&i| i != first).reduce(|b, i| if rank_gt(ranks[i], ranks[b]) { i } else { b });
        best_x.push((cands[first].0, first));
        runnerup.push(second.map_or(0, |i| cands[i].1.len()));
    }

    // Best disappeared name per appeared name, for the mutual check.
    let mut best_y: HashMap<usize, (usize, Rank)> = HashMap::new();
    for (dx, cands) in &matches {
        for (ay, m) in cands {
            let r = rank(m);
            match best_y.get(ay) {
                Some(cur) if !rank_gt(r, cur.1) => {}
                _ => {
                    best_y.insert(*ay, (*dx, r));
                }
            }
        }
    }

    let mut out = Vec::new();
    for (i, (dx, cands)) in matches.iter().enumerate() {
        let (ay, at) = best_x[i];
        if best_y.get(&ay).map(|b| b.0) != Some(*dx) {
            continue;
        }
        let best_m = &cands[at].1;
        let overlap = best_m.len();
        if runnerup[i] >= overlap {
            continue;
        }
        let terms = confidence(best_m, overlap, runnerup[i], cfg.drift);
        if terms.confidence < cfg.min_conf {
            continue;
        }
        let (xfp, yfp) = (disappeared[*dx], appeared[ay]);
        let mut cells = best_m.clone();
        cells.sort_by_key(|c| c.0);
        let boards = cells
            .iter()
            .map(|&(uuid, drift, _)| BoardMatch {
                uuid,
                score_from: py_round(xfp.boards[&uuid], 2),
                score_to: py_round(yfp.boards[&uuid], 2),
                drift_pct: py_round(drift * 100.0, 3),
            })
            .collect();
        out.push(Rename {
            from_name: xfp.display.clone(),
            to_name: yfp.display.clone(),
            confidence: terms.confidence,
            matched_boards: overlap,
            boards,
            summary: summary(&xfp.display, &yfp.display, overlap, &terms),
            terms,
        });
    }
    out
}

/// `_confidence`: board count, score tightness, exclusivity and cell rarity
/// folded into a confidence in [0, 0.97], with its sub-terms.
fn confidence(matched: &[Cell], overlap: usize, runnerup: usize, drift: f64) -> Terms {
    let n = matched.len() as f64;
    let tightness = if drift > 0.0 {
        py_fsum(matched.iter().map(|c| py_max(0.0, 1.0 - (c.1 / drift)))) / n
    } else if matched.iter().all(|c| c.1 <= 0.0) {
        1.0
    } else {
        0.0
    };
    let board_term = 1.0 - 0.5f64.powi(i32::try_from(overlap).unwrap_or(i32::MAX));
    let exclusivity = if overlap > 0 { (overlap as f64 - runnerup as f64) / overlap as f64 } else { 0.0 };
    let rarity = matched.iter().filter(|c| c.2 <= 1).count() as f64 / n;
    let conf = board_term * (0.55 + 0.45 * tightness) * (0.60 + 0.40 * exclusivity) * (0.70 + 0.30 * rarity);
    let rounded = py_round(conf, 4);
    let confidence = if rounded < CONFIDENCE_CEILING { rounded } else { CONFIDENCE_CEILING };
    Terms {
        matched_boards: overlap,
        runnerup_overlap: runnerup,
        board_term: py_round(board_term, 4),
        tightness: py_round(tightness, 4),
        exclusivity: py_round(exclusivity, 4),
        rarity: py_round(rarity, 4),
        mean_drift_pct: py_round(py_fsum(matched.iter().map(|c| c.1)) / n * 100.0, 3),
        confidence,
    }
}

/// `_summary`.
pub fn summary(from_name: &str, to_name: &str, overlap: usize, terms: &Terms) -> String {
    let carry = if terms.tightness >= 0.999 { "carried over unchanged" } else { "carried over near-identically" };
    let plural = if overlap != 1 { "s" } else { "" };
    format!(
        "\u{201c}{from_name}\u{201d} vanished and \u{201c}{to_name}\u{201d} appeared in the same capture with the \
         same lifetime score fingerprint {carry} across {overlap} board{plural} (Trove/Geode Mastery, Power Rank - \
         boards that never reset and survive a rename). Mutual, unambiguous best match."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(display: &str, boards: &[(i32, f64)]) -> Named {
        Named { display: display.into(), boards: boards.iter().copied().collect() }
    }

    #[test]
    fn mutual_unambiguous_match() {
        let cfg = MatchCfg { min_boards: 2, drift: 0.02, min_conf: 0.5 };
        let a = BTreeMap::from([
            ("old".to_string(), named("Old", &[(1, 5000.0), (20, 8000.0)])),
            ("stay".to_string(), named("Stay", &[(1, 100.0), (20, 200.0)])),
        ]);
        let b = BTreeMap::from([
            ("new".to_string(), named("New", &[(1, 5000.0), (20, 8040.0)])),
            ("stay".to_string(), named("Stay", &[(1, 100.0), (20, 200.0)])),
        ]);
        let got = match_pair(&a, &b, &cfg);
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].from_name.as_str(), got[0].to_name.as_str()), ("Old", "New"));
        assert_eq!(got[0].boards[1].drift_pct, 0.5);
        assert_eq!(got[0].terms.tightness, 0.875);
        assert_eq!(got[0].terms.confidence, 0.7078);
    }

    #[test]
    fn bisect_like_python() {
        let a = [1.0, 2.0, 2.0, 3.0];
        assert_eq!((bisect_left(&a, 2.0), bisect_right(&a, 2.0)), (1, 3));
        assert_eq!((bisect_left(&a, 0.0), bisect_right(&a, 9.0)), (0, 4));
    }
}
