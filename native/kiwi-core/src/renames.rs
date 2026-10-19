//! Rename fingerprints for `app/trove/leaderboards/renames.py`: per name key,
//! the lifetime-board scores at one capture, and the names that vanished or
//! appeared between two captures. The matching itself stays in Python; it only
//! ever sees those few names.

use std::collections::BTreeMap;

use rustc_hash::FxHashMap;

use crate::activity::Capture;

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
