//! `app/trove/codexes/mastery.py`: per-type mastery base x the multipliers tables.

use std::collections::HashMap;

use super::{py, wire::read_uleb};

const BASE_RULES: [(&str, i128); 21] = [
    ("collections/wings", 100),
    ("collections/mount", 50),
    ("collections/boat", 50),
    ("collections/magrider", 50),
    ("collections/aura", 50),
    ("collections/flask", 25),
    ("collections/badge", 20),
    ("collections/tome", 20),
    ("collections/fishingpole", 20),
    ("collections/fishing", 20),
    ("collections/pet", 10),
    ("collections/geodecompanion", 10),
    ("item/companion", 10),
    ("collections/sail", 10),
    ("item/fish", 5),
    ("item/unlocker", 5),
    ("collections/memento", 5),
    ("recipe_", 2),
    ("equipment/", 1),
    ("equipment_", 1),
    (".blueprint", 1),
];

const MARKER: [u8; 3] = [0xBE, 0x01, 0xAE];
const GROUP_MULTIPLIERS: [i128; 4] = [0, 2, 3, 5];

/// One multipliers-file row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MultRow {
    pub multiplier: i128,
    pub base: i128,
    pub predicted: i128,
}

pub type Multipliers = HashMap<String, MultRow>;

/// `infer_mastery_base`: (normalized identifier, base).
pub fn infer_mastery_base(identifier: &str, geode: bool) -> (String, i128) {
    let mut normalized = identifier.replace('\\', "/");
    if let Some(rest) = normalized.strip_prefix("skins/") {
        normalized = format!("collections/skin/{rest}");
    }
    if geode && normalized.contains("item/companion") {
        return (normalized, 50);
    }
    if let Some((_, base)) = BASE_RULES.iter().find(|(f, _)| normalized.contains(f)) {
        return (normalized, *base);
    }
    if !normalized.contains('/') {
        return (format!("collections/skin/{normalized}"), 35);
    }
    (normalized, 0)
}

fn encode_varint(mut value: u128) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            out.push(byte | 0x80);
        } else {
            out.push(byte);
            return out;
        }
    }
}

fn parse_file(content: &[u8], geode: bool) -> Multipliers {
    let mut rows = Multipliers::new();
    let mut position = 9usize;
    'groups: for multiplier in GROUP_MULTIPLIERS {
        let Some(marker_pos) = py::find(content, &MARKER, position) else { break };
        position = marker_pos + MARKER.len();
        let Ok((count, p)) = read_uleb(content, position) else { break };
        position = p;
        let mut index: u128 = 1;
        while index <= count {
            let mut pattern = encode_varint(4 + 16 * (index - 1));
            pattern.push(0);
            let Some(next_pos) = py::find(content, &pattern, position) else { break };
            position = next_pos + pattern.len() + 2;
            let Ok((length, p)) = read_uleb(content, position) else { break 'groups };
            position = p;
            let end = position.saturating_add(usize::try_from(length).unwrap_or(usize::MAX));
            let raw = py::ascii_ignore(content.get(position..end.min(content.len())).unwrap_or_default());
            position = end;
            let (identifier, base) = infer_mastery_base(&raw, geode);
            let predicted = if geode && multiplier == 0 {
                if identifier.starts_with("collections/") || identifier.starts_with("item/") { base } else { 0 }
            } else {
                base * multiplier
            };
            rows.insert(identifier, MultRow { multiplier, base, predicted });
            index += 1;
        }
    }
    rows
}

/// `parse_multipliers`.
pub fn parse_multipliers(content: &[u8]) -> Multipliers {
    parse_file(content, false)
}

/// `parse_geode_multipliers`.
pub fn parse_geode_multipliers(content: &[u8]) -> Multipliers {
    parse_file(content, true)
}

/// `mastery_for`.
pub fn mastery_for(identifier: &str, multipliers: &Multipliers) -> Option<i128> {
    let (normalized, base) = infer_mastery_base(identifier, false);
    match multipliers.get(&normalized) {
        Some(row) => Some(row.predicted),
        None => (base > 0).then_some(base),
    }
}

/// `geode_mastery_for`.
pub fn geode_mastery_for(identifier: &str, geode: &Multipliers) -> Option<i128> {
    let (normalized, _) = infer_mastery_base(identifier, true);
    geode.get(&normalized).map(|r| r.predicted)
}
