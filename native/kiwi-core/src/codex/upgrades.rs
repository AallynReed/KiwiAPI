//! `app/trove/codexes/upgrades.py`: progression trees (costs) and node effects.

use std::{
    collections::{BTreeSet, HashMap},
    sync::LazyLock,
};

use regex::bytes::Regex;
use serde_json::{Map, Value, json};

use super::{
    int_value, py,
    wire::{read_varint, unzig},
};

pub const TREES_ROOT: &str = "prefabs/upgrade/trees/";
pub const UPGRADES_ROOT: &str = "prefabs/upgrade/upgrades/";

static COST_RE: LazyLock<Regex> = LazyLock::new(|| py::bytes_re(r"item/(?:crafting|currency)/[A-Za-z0-9_\-/]+"));
static TOKEN_RE: LazyLock<Regex> = LazyLock::new(|| py::bytes_re(r"[a-z0-9][a-z0-9_]{2,63}"));
static ABILITY_REF_RE: LazyLock<Regex> = LazyLock::new(|| py::bytes_re(r"abilities/[A-Za-z0-9_/.\-]+"));
static NAME_KEY_RE: LazyLock<Regex> = LazyLock::new(|| py::bytes_re(r"\$[A-Za-z0-9_]+_name"));
static DESC_KEY_RE: LazyLock<Regex> = LazyLock::new(|| py::bytes_re(r"\$[A-Za-z0-9_]+_description"));

fn ident_byte(b: u8) -> bool {
    b == 0x5F || b.is_ascii_alphanumeric()
}

pub fn system_key_from_path(path: &str) -> String {
    let p = path.replace('\\', "/");
    let last = py::last_part(&p, "/");
    py::lower(last.strip_suffix(".binfab").unwrap_or(last))
}

fn system_kind(keys: &[String]) -> &'static str {
    if keys.iter().any(|k| k.contains("_level_")) {
        "geode_companion"
    } else if keys.iter().any(|k| k.contains("_root_") || k.contains("_branch_")) {
        "class_prestige"
    } else {
        "geode_module"
    }
}

fn costs(data: &[u8], start: usize, end: usize) -> Vec<Value> {
    let mut out = Vec::new();
    let hay = &data[..end];
    let mut pos = start;
    while let Some(m) = COST_RE.find_at(hay, pos) {
        pos = m.end();
        let tag = m.end();
        if tag >= end || data[tag] != 0x10 {
            continue;
        }
        let (Some(raw), _) = read_varint(data, tag + 1) else { continue };
        let quantity = unzig(raw);
        if quantity <= 0 {
            continue;
        }
        let mut row = Map::new();
        row.insert("item".into(), json!(py::ascii_ignore(m.as_bytes())));
        row.insert("quantity".into(), int_value(quantity));
        out.push(Value::Object(row));
    }
    out
}

fn node_spans(data: &[u8]) -> Vec<(String, usize, usize)> {
    let cost_starts: Vec<usize> = COST_RE.find_iter(data).map(|m| m.start()).collect();
    if cost_starts.is_empty() {
        return Vec::new();
    }
    let mut candidates: Vec<(usize, String)> = Vec::new();
    for m in TOKEN_RE.find_iter(data) {
        let start = m.start();
        if start > 0 && (data[start - 1] == 0x2F || ident_byte(data[start - 1])) {
            continue;
        }
        let token = py::ascii_ignore(m.as_bytes());
        if token.ends_with("_upgrade_tree") || token.starts_with("item") {
            continue;
        }
        candidates.push((start, token));
    }
    let mut opened: Vec<(String, usize)> = Vec::new();
    let mut have: HashMap<String, usize> = HashMap::new();
    for (index, (offset, token)) in candidates.iter().enumerate() {
        if have.contains_key(token) {
            continue;
        }
        let nxt = candidates.get(index + 1).map_or(data.len(), |c| c.0);
        let first_after = cost_starts.partition_point(|&c| c <= *offset);
        if cost_starts.get(first_after).is_some_and(|&c| c < nxt) {
            have.insert(token.clone(), *offset);
            opened.push((token.clone(), *offset));
        }
    }
    opened.sort_by_key(|(_, o)| *o);
    opened
        .iter()
        .enumerate()
        .map(|(i, (token, offset))| (token.clone(), *offset, opened.get(i + 1).map_or(data.len(), |n| n.1)))
        .collect()
}

/// One cost-bearing node.
#[derive(Clone, Debug)]
pub struct Node {
    pub node_key: String,
    pub rank: Option<i64>,
    pub costs: Vec<Value>,
    pub requires: Vec<String>,
}

/// `parse_upgrade_costs`.
#[derive(Clone, Debug)]
pub struct Tree {
    pub system_kind: &'static str,
    pub system_key: String,
    pub nodes: Vec<Node>,
}

fn rank_of(key: &str) -> Option<i64> {
    let b = key.as_bytes();
    let n = b.len();
    (n >= 2 && b[n - 2].is_ascii_digit() && b[n - 1].is_ascii_digit()).then(|| key[n - 2..].parse().expect("digits"))
}

pub fn parse_upgrade_costs(data: &[u8], path: &str) -> Tree {
    let spans = node_spans(data);
    let system_key = system_key_from_path(path);
    if spans.is_empty() {
        return Tree { system_kind: "", system_key, nodes: Vec::new() };
    }
    let keys: Vec<String> = spans.iter().map(|s| s.0.clone()).collect();
    let mut nodes = Vec::new();
    for (node_key, start, end) in &spans {
        let c = costs(data, *start, *end);
        if c.is_empty() {
            continue;
        }
        let chunk = &data[*start..*end];
        let requires: BTreeSet<String> =
            keys.iter().filter(|o| *o != node_key && py::contains(chunk, o.as_bytes())).cloned().collect();
        nodes.push(Node {
            node_key: node_key.clone(),
            rank: rank_of(node_key),
            costs: c,
            requires: requires.into_iter().collect(),
        });
    }
    let system_kind = if nodes.is_empty() { "" } else { system_kind(&keys) };
    Tree { system_kind, system_key, nodes }
}

/// `parse_upgrade_effects`: node key -> (name key, desc key, ability refs).
pub fn parse_upgrade_effects(data: &[u8], node_keys: &[String]) -> HashMap<String, (String, String, Vec<String>)> {
    let mut starts: Vec<(String, usize)> = Vec::new();
    for key in node_keys {
        if let Some(at) = py::find(data, key.as_bytes(), 0)
            && !starts.iter().any(|(k, _)| k == key)
        {
            starts.push((key.clone(), at));
        }
    }
    starts.sort_by_key(|(_, at)| *at);
    let mut out = HashMap::new();
    for (i, (key, start)) in starts.iter().enumerate() {
        let end = starts.get(i + 1).map_or(data.len(), |s| s.1);
        let chunk = &data[*start..end];
        let mut abilities: Vec<String> = Vec::new();
        for m in ABILITY_REF_RE.find_iter(chunk) {
            let r = py::rstrip_chars(&py::ascii_ignore(m.as_bytes()), "./").to_owned();
            if !abilities.contains(&r) {
                abilities.push(r);
            }
        }
        let name_key = NAME_KEY_RE.find(chunk).map(|m| py::ascii_ignore(m.as_bytes())).unwrap_or_default();
        let desc_key = DESC_KEY_RE.find(chunk).map(|m| py::ascii_ignore(m.as_bytes())).unwrap_or_default();
        if !name_key.is_empty() || !desc_key.is_empty() || !abilities.is_empty() {
            out.insert(key.clone(), (name_key, desc_key, abilities));
        }
    }
    out
}
