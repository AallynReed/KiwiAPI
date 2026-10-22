//! `player_board_agg` rebuilt in memory: every capture, oldest first, folded by
//! exactly the rule of `pg_store._fold_anchor_into_agg`, so the result equals a
//! replay of the ingest fold without thousands of SQL upserts.

use rustc_hash::FxHashMap;

use crate::activity::{SIGNATURE, be_8, be_i16, be_i32};

/// One `(player_id, board_uuid)` row of the aggregate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Row {
    pub best_rank: i32,
    pub appearances: i64,
    pub first_seen: i64,
    pub last_seen: i64,
    pub latest_rank: i32,
    pub latest_score: f64,
    pub last_folded: i64,
    pub last_rise: Option<i64>,
}

#[derive(Default)]
pub struct Agg {
    pub rows: FxHashMap<(i64, i32), Row>,
}

impl Agg {
    /// Fold one capture's binary COPY of `(board_uuid int4, player_id int8,
    /// rank int4, score float8)`. None (nothing folded) if it isn't that COPY.
    pub fn fold_copy(&mut self, anchor: i64, data: &[u8]) -> Option<usize> {
        if data.get(..11)? != SIGNATURE {
            return None;
        }
        let ext = be_i32(data, 15)?;
        let mut at = 19 + usize::try_from(ext).ok()?;
        // DISTINCT ON (player, board) ORDER BY rank ASC, score DESC.
        let mut best: FxHashMap<(i64, i32), (i32, f64)> = FxHashMap::default();
        loop {
            let fields = be_i16(data, at)?;
            at += 2;
            if fields == -1 {
                break;
            }
            if fields != 4 || be_i32(data, at)? != 4 {
                return None;
            }
            let board = be_i32(data, at + 4)?;
            at += 8;
            if be_i32(data, at)? != 8 {
                return None;
            }
            let player = i64::from_be_bytes(be_8(data, at + 4)?);
            at += 12;
            if be_i32(data, at)? != 4 {
                return None;
            }
            let rank = be_i32(data, at + 4)?;
            at += 8;
            if be_i32(data, at)? != 8 {
                return None;
            }
            let score = f64::from_be_bytes(be_8(data, at + 4)?);
            at += 12;
            let e = best.entry((player, board)).or_insert((rank, score));
            if rank < e.0 || (rank == e.0 && score > e.1) {
                *e = (rank, score);
            }
        }
        let n = best.len();
        for (key, (rank, score)) in best {
            self.fold(key, anchor, rank, score);
        }
        Some(n)
    }

    /// The ON CONFLICT rule; every `a.*` is the row before this capture.
    fn fold(&mut self, key: (i64, i32), x: i64, rank: i32, score: f64) {
        let Some(a) = self.rows.get(&key).copied() else {
            self.rows.insert(
                key,
                Row {
                    best_rank: rank,
                    appearances: 1,
                    first_seen: x,
                    last_seen: x,
                    latest_rank: rank,
                    latest_score: score,
                    last_folded: x,
                    last_rise: None,
                },
            );
            return;
        };
        let newer = x >= a.last_seen;
        self.rows.insert(
            key,
            Row {
                best_rank: a.best_rank.min(rank),
                appearances: a.appearances + i64::from(x > a.last_folded),
                first_seen: a.first_seen.min(x),
                last_seen: a.last_seen.max(x),
                latest_rank: if newer { rank } else { a.latest_rank },
                latest_score: if newer { score } else { a.latest_score },
                last_folded: a.last_folded.max(x),
                last_rise: if x > a.last_seen && score > a.latest_score { Some(x) } else { a.last_rise },
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_like_the_ingest_rule() {
        let mut agg = Agg::default();
        agg.fold((1, 7), 100, 5, 10.0);
        agg.fold((1, 7), 200, 3, 9.0);
        agg.fold((1, 7), 300, 4, 12.0);
        let r = agg.rows[&(1, 7)];
        assert_eq!((r.best_rank, r.appearances, r.first_seen, r.last_seen), (3, 3, 100, 300));
        assert_eq!((r.latest_rank, r.latest_score, r.last_rise), (4, 12.0, Some(300)));
    }
}
