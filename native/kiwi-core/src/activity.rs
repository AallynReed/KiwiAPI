//! Player-activity kernels for `app/trove/leaderboards/activity.py`: one capture's
//! scores decoded from a Postgres binary COPY of `(board_uuid int4, player_id
//! int8, score float8)`, and the "who rose since the previous capture" rule.

use std::collections::HashMap;

use rustc_hash::{FxHashMap, FxHashSet};

/// One capture: per board, each player's score. A player listed twice on a
/// board keeps their highest score.
#[derive(Default)]
pub struct Capture {
    pub boards: FxHashMap<i32, FxHashMap<i64, f64>>,
}

fn be_i32(b: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn be_i16(b: &[u8], at: usize) -> Option<i16> {
    Some(i16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn be_8(b: &[u8], at: usize) -> Option<[u8; 8]> {
    b.get(at..at + 8)?.try_into().ok()
}

const SIGNATURE: &[u8; 11] = b"PGCOPY\n\xff\r\n\0";

/// Decode a binary COPY of `(int4, int8, float8)` rows. None if it isn't one.
pub fn decode_copy(data: &[u8]) -> Option<Capture> {
    if data.get(..11)? != SIGNATURE {
        return None;
    }
    let ext = be_i32(data, 15)?;
    let mut at = 19 + usize::try_from(ext).ok()?;
    let mut cap = Capture::default();
    loop {
        let fields = be_i16(data, at)?;
        at += 2;
        if fields == -1 {
            return Some(cap);
        }
        if fields != 3 {
            return None;
        }
        if be_i32(data, at)? != 4 {
            return None;
        }
        let board = be_i32(data, at + 4)?;
        at += 8;
        if be_i32(data, at)? != 8 {
            return None;
        }
        let player = i64::from_be_bytes(be_8(data, at + 4)?);
        at += 12;
        if be_i32(data, at)? != 8 {
            return None;
        }
        let score = f64::from_be_bytes(be_8(data, at + 4)?);
        at += 12;
        let slot = cap.boards.entry(board).or_default().entry(player).or_insert(score);
        if score > *slot {
            *slot = score;
        }
    }
}

/// `_board_active_names` for one board: rose since `early`, or newly listed
/// above `early`'s lowest score.
fn board_active(late: &FxHashMap<i64, f64>, early: &FxHashMap<i64, f64>, out: &mut Vec<i64>) {
    let floor = early.values().copied().fold(f64::INFINITY, f64::min);
    for (pid, &score) in late {
        match early.get(pid) {
            None if score > floor => out.push(*pid),
            Some(&prev) if score > prev => out.push(*pid),
            _ => {}
        }
    }
}

pub struct Pair {
    /// Distinct active player ids across every board.
    pub active: Vec<i64>,
    /// Boards with entries in both captures.
    pub boards: usize,
    /// Active players per board.
    pub per_board: HashMap<i32, usize>,
}

/// `_pair_estimate` over `boards` (the player boards).
pub fn pair(early: &Capture, late: &Capture, boards: &[i32]) -> Pair {
    let mut union: FxHashSet<i64> = FxHashSet::default();
    let mut per_board = HashMap::new();
    let mut counted = 0;
    let mut buf = Vec::new();
    for uuid in boards {
        let (Some(l), Some(e)) = (late.boards.get(uuid), early.boards.get(uuid)) else { continue };
        if l.is_empty() || e.is_empty() {
            continue;
        }
        counted += 1;
        buf.clear();
        board_active(l, e, &mut buf);
        if !buf.is_empty() {
            per_board.insert(*uuid, buf.len());
        }
        union.extend(buf.iter().copied());
    }
    let mut active: Vec<i64> = union.into_iter().collect();
    active.sort_unstable();
    Pair { active, boards: counted, per_board }
}

/// The weekly-reset situation of one capture window (decided by the caller).
pub struct ResetWindow {
    /// A reset fell inside the window.
    pub crossed: bool,
    /// ...and the window is short enough to count as measured but empty.
    pub short: bool,
}

/// "Established" floors for the clean view; a floor of 0 is a no-op.
pub struct Floors {
    pub power_rank: f64,
    pub effort: f64,
    pub xp: f64,
    pub xp_board: i32,
}

/// One class's window: (class index, active player ids, clean ids or None).
pub type ClassSet = (i64, Vec<i64>, Option<Vec<i64>>);

/// `class_activity._class_active_sets` by player id. `late`/`early` hold the
/// Effort boards; `gates` the Power Rank boards (+ XP) at the window end.
pub fn class_sets(early: &Capture, late: &Capture, gates: &Capture, reset: &ResetWindow, f: &Floors) -> Vec<ClassSet> {
    let mut by_class: std::collections::BTreeMap<i64, FxHashSet<i64>> = Default::default();
    let mut buf = Vec::new();
    for (uuid, l) in &late.boards {
        if l.is_empty() {
            continue;
        }
        let class = i64::from(*uuid).rem_euclid(1000);
        let e = early.boards.get(uuid).filter(|e| !e.is_empty());
        if reset.short {
            if e.is_some() {
                by_class.entry(class).or_default();
            }
            continue;
        }
        let Some(e) = e else { continue };
        if reset.crossed {
            continue;
        }
        buf.clear();
        board_active(l, e, &mut buf);
        by_class.entry(class).or_default().extend(buf.iter().copied());
    }
    let xp = (f.xp > 0.0).then(|| gates.boards.get(&f.xp_board)).flatten();
    let empty = FxHashMap::default();
    by_class
        .into_iter()
        .map(|(class, players)| {
            let pr = gates.boards.get(&(1000 + class as i32));
            let clean = pr.map(|pr| {
                let effort = late.boards.get(&(4000 + class as i32)).unwrap_or(&empty);
                let mut c: Vec<i64> = players
                    .iter()
                    .copied()
                    .filter(|p| {
                        pr.get(p).copied().unwrap_or(0.0) >= f.power_rank
                            && effort.get(p).copied().unwrap_or(0.0) >= f.effort
                            && xp.is_none_or(|x| x.get(p).copied().unwrap_or(0.0) >= f.xp)
                    })
                    .collect();
                c.sort_unstable();
                c
            });
            let mut active: Vec<i64> = players.into_iter().collect();
            active.sort_unstable();
            (class, active, clean)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn copy(rows: &[(i32, i64, f64)]) -> Vec<u8> {
        let mut b = SIGNATURE.to_vec();
        b.extend(0i32.to_be_bytes());
        b.extend(0i32.to_be_bytes());
        for (u, p, s) in rows {
            b.extend(3i16.to_be_bytes());
            b.extend(4i32.to_be_bytes());
            b.extend(u.to_be_bytes());
            b.extend(8i32.to_be_bytes());
            b.extend(p.to_be_bytes());
            b.extend(8i32.to_be_bytes());
            b.extend(s.to_be_bytes());
        }
        b.extend((-1i16).to_be_bytes());
        b
    }

    #[test]
    fn rise_and_new_above_floor() {
        let early = decode_copy(&copy(&[(1, 10, 5.0), (1, 11, 3.0), (1, 11, 2.0)])).unwrap();
        let late = decode_copy(&copy(&[(1, 10, 6.0), (1, 11, 3.0), (1, 12, 4.0), (1, 13, 1.0)])).unwrap();
        let p = pair(&early, &late, &[1, 2]);
        assert_eq!((p.active, p.boards), (vec![10, 12], 1));
    }
}
