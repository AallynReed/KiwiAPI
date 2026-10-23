//! `app/trove/codexes/powerrank.py`: the `_powerrank` component joined to the tier table.

use std::collections::HashMap;

use super::{
    py,
    wire::{read_uleb, unzig},
};

pub type PowerRankTable = HashMap<u128, i128>;

const BY_TIER: [(u128, i128); 6] = [(0, 0), (600, 50), (620, 50), (800, 5), (802, 20), (804, 75)];
const SPECIAL_30: [u8; 10] = [0x2E, 0x00, 0x08, 0x30, 0x00, 0x40, 0x01, 0x1E, 0xE2, 0x05];

/// `parse_power_rank_table`: `1E <idx> 00 <tier> 10 <value>` rows.
pub fn parse_power_rank_table(data: &[u8]) -> PowerRankTable {
    let mut table = PowerRankTable::new();
    let n = data.len();
    let mut pos = 0;
    while pos < n {
        let Some(marker_at) = py::find(data, &[0x1e], pos) else { break };
        let row = (|| {
            let (_, mut cursor) = read_uleb(data, marker_at + 1).ok()?;
            if cursor >= n || data[cursor] != 0x00 {
                return Some(None);
            }
            let (tier, c) = read_uleb(data, cursor + 1).ok()?;
            cursor = c;
            if cursor >= n || data[cursor] != 0x10 {
                return Some(None);
            }
            let (value, c) = read_uleb(data, cursor + 1).ok()?;
            Some(Some((tier, value, c)))
        })();
        match row.flatten() {
            Some((tier, value, cursor)) => {
                let rank = unzig(value);
                if rank > 0 {
                    table.entry(tier).or_insert(rank);
                }
                pos = cursor;
            }
            None => pos = marker_at + 1,
        }
    }
    table
}

/// `decode_power_rank`.
pub fn decode_power_rank(content: &[u8], table: &PowerRankTable) -> Option<i128> {
    if py::contains(content, &SPECIAL_30) {
        return Some(30);
    }
    let lookup = |v: u128| table.get(&v).copied().or_else(|| BY_TIER.iter().find(|(t, _)| *t == v).map(|(_, r)| *r));
    let n = content.len();
    let mut i = 0;
    while i + 2 < n {
        if content[i] == 0x30 {
            let Ok((value, j)) = read_uleb(content, i + 1) else {
                i += 1;
                continue;
            };
            if j + 1 < n
                && content[j] == 0x40
                && content[j + 1] == 0x01
                && let Some(pr) = lookup(value)
            {
                return Some(pr);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    None
}
