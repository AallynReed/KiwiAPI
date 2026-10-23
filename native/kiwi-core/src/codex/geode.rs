//! `app/trove/codexes/geode.py`: geode companion membership and level bonuses.

use std::{collections::HashMap, sync::LazyLock};

use regex::bytes::Regex;
use serde_json::{Map, Value, json};

use super::{bonuses::decode_le_float, float_value, py};
use crate::binfab::harvest_strings;

static UPGRADE_TREE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[A-Za-z0-9_/]+_upgrade_tree").expect("static regex"));
static LEVEL_RE: LazyLock<Regex> = LazyLock::new(|| py::bytes_re(r"([A-Za-z0-9_/]+)_level_([0-9]{2})"));
static EXPLORATION_RE: LazyLock<Regex> = LazyLock::new(|| py::bytes_re(r"(?s)\x10\x02\x24(....)\x38"));
static NAME_KEY_RE: LazyLock<Regex> = LazyLock::new(|| py::bytes_re(r"\$[A-Za-z0-9_]+_name"));
static COMPANION_ABILITY_RE: LazyLock<Regex> =
    LazyLock::new(|| py::bytes_re(r"abilities/discovery/companion/[A-Za-z0-9_/.]+"));

/// `find_upgrade_tree_ref`.
pub fn find_upgrade_tree_ref(content: &[u8]) -> Option<String> {
    harvest_strings(content, 2, 512)
        .into_iter()
        .find_map(|h| UPGRADE_TREE_RE.find(&h.text).map(|m| py::last_part(m.as_str(), "/").to_owned()))
}

/// `geode_companion_members`: `item/companion/<key>` -> rarity group.
pub fn geode_companion_members(content: &[u8]) -> HashMap<String, String> {
    let mut members = HashMap::new();
    let mut group = String::new();
    let mut prev_bare = String::new();
    for h in harvest_strings(content, 2, 512) {
        let s = h.text;
        if s.starts_with("$CollectionName") {
            group = if prev_bare.is_empty() {
                s.strip_prefix("$CollectionName_").unwrap_or(&s).to_owned()
            } else {
                prev_bare.clone()
            };
        } else if s.starts_with("item/companion/") {
            let key = s.replace('\\', "/");
            let key = py::lower(key.strip_suffix(".binfab").unwrap_or(&key));
            members.entry(key).or_insert_with(|| group.clone());
        } else if !s.contains('/') && !s.contains('$') {
            prev_bare = s;
        }
    }
    members
}

/// One level: `(level, stats, abilities)`; abilities carry `key` (or null).
pub struct Level {
    pub level: i64,
    pub stats: Vec<Map<String, Value>>,
    pub abilities: Vec<Map<String, Value>>,
}

type Rows = Vec<Map<String, Value>>;

fn parse_level_chunk(chunk: &[u8]) -> (Rows, Rows) {
    let mut stats = Vec::new();
    for c in EXPLORATION_RE.captures_iter(chunk) {
        if let Some(amount) = decode_le_float(&c[1])
            && amount.abs() <= 1_000_000.0
        {
            let mut m = Map::new();
            m.insert("stat".into(), json!("$Stat_MaxExploration"));
            m.insert("stat_id".into(), json!(0x24));
            m.insert("amount".into(), float_value(amount));
            m.insert("value".into(), float_value(amount));
            m.insert("is_percent".into(), json!(false));
            stats.push(m);
        }
    }
    let refs: Vec<String> = COMPANION_ABILITY_RE.find_iter(chunk).map(|m| py::ascii_ignore(m.as_bytes())).collect();
    let mut abilities = Vec::new();
    if !refs.is_empty() {
        let names: Vec<String> = NAME_KEY_RE.find_iter(chunk).map(|m| py::ascii_ignore(m.as_bytes())).collect();
        for (i, r) in refs.into_iter().enumerate() {
            let mut m = Map::new();
            m.insert("ref".into(), json!(r));
            m.insert("key".into(), names.get(i).map_or(Value::Null, |k| json!(k)));
            m.insert("amount".into(), json!(0));
            abilities.push(m);
        }
    }
    (stats, abilities)
}

/// `parse_upgrade_tree`: levels with any bonus, in source order.
pub fn parse_upgrade_tree(content: &[u8]) -> Vec<Level> {
    let bounds: Vec<(usize, i64)> = LEVEL_RE
        .captures_iter(content)
        .map(|c| {
            let level = std::str::from_utf8(&c[2]).expect("digits").parse().expect("two digits");
            (c.get(0).expect("match").start(), level)
        })
        .collect();
    let mut levels = Vec::new();
    for (i, &(start, level)) in bounds.iter().enumerate() {
        let end = bounds.get(i + 1).map_or(content.len(), |b| b.0);
        let (stats, abilities) = parse_level_chunk(&content[start..end]);
        if !stats.is_empty() || !abilities.is_empty() {
            levels.push(Level { level, stats, abilities });
        }
    }
    levels
}
