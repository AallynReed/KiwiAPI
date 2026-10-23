//! `StarChartParser` of `app/trove/gems/builds.py`: a star-chart build code
//! (`SC:`/`v2:` compact or legacy base64) to the chart's passive stats.

use std::{
    collections::{BTreeSet, HashMap},
    sync::LazyLock,
};

use kiwi_core::gems::builds::Chart;
use regex::Regex;
use serde_json::Value;

use super::data::truthy;
use crate::auth::pyutil::strip;

const BOUNTY_HUNT_NODES: [&str; 2] = ["Bounty Hunt Boon", "Bounty Hunt"];

struct NodeStat {
    name: Option<String>,
    value: f64,
    scales: bool,
}

struct Node {
    root: bool,
    stats: Vec<NodeStat>,
    overwrites: Option<Vec<String>>,
    name: Option<String>,
    abilities: Vec<String>,
}

/// `parse_build_code(code)` in full, for the preview proxy.
#[derive(Debug, Default, PartialEq)]
pub struct Preview {
    pub stats: Vec<(String, f64, f64)>,
    pub abilities: Vec<String>,
    pub paths_count: usize,
    /// The Bounty Hunt node's name and its (physical, magic) values.
    pub bounty: Option<(String, f64, f64)>,
}

pub struct StarChart {
    nodes: HashMap<String, Node>,
    parents: HashMap<String, Option<String>>,
    selectable: Vec<String>,
    /// path -> the Bounty Hunt buff's (physical, magic) values.
    bounty: HashMap<String, (f64, f64)>,
}

/// `float(stat.get("value", 0) or 0)`, 0.0 where Python's `float()` fails.
fn float_of(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        Some(Value::String(s)) => strip(s).replace('_', "").parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// `_scales`: the binfab op decides; rows without one fall back to `percentage`.
fn scales(stat: &Value) -> bool {
    match stat.get("op") {
        Some(op) if truthy(Some(op)) => matches!(op.as_str(), Some("MultiplySum" | "Multiply")),
        _ => truthy(stat.get("percentage")),
    }
}

/// `_stat_value` over `Ability_Values`.
fn ability_value(values: &[Value], name: &str) -> f64 {
    let bonus = format!("{name} Bonus");
    let mut total = 0.0;
    for v in values {
        if v.get("name").and_then(Value::as_str).is_some_and(|n| n == name || n == bonus) {
            total += match v.get("value") {
                Some(x) if truthy(Some(x)) => x.as_f64().unwrap_or(0.0),
                _ => 0.0,
            };
        }
    }
    total
}

impl StarChart {
    pub fn new(data: Option<&Value>) -> Self {
        let mut chart = StarChart {
            nodes: HashMap::new(),
            parents: HashMap::new(),
            selectable: Vec::new(),
            bounty: HashMap::new(),
        };
        if let Some(Value::Object(roots)) = data {
            for root in roots.values() {
                chart.flatten(root, None);
            }
        }
        let mut selectable: Vec<String> = chart.nodes.iter().filter(|(_, n)| !n.root).map(|(p, _)| p.clone()).collect();
        selectable.sort();
        chart.selectable = selectable;
        chart
    }

    fn flatten(&mut self, node: &Value, parent: Option<&str>) {
        let path = node.get("Path").and_then(Value::as_str);
        if let Some(path) = path {
            let stats = node
                .get("Stats")
                .and_then(Value::as_array)
                .map(|rows| {
                    rows.iter()
                        .map(|s| NodeStat {
                            name: s.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()).map(str::to_owned),
                            value: if truthy(s.get("value")) { float_of(s.get("value")) } else { 0.0 },
                            scales: scales(s),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let overwrites = node.get("Overwrites").map(|o| {
                o.as_array().map_or_else(Vec::new, |a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
            });
            let name = node.get("Name").and_then(Value::as_str);
            match node.get("Ability_Values").and_then(Value::as_array) {
                Some(values) if name.is_some_and(|n| BOUNTY_HUNT_NODES.contains(&n)) && !values.is_empty() => {
                    let pair = (ability_value(values, "Physical Damage"), ability_value(values, "Magic Damage"));
                    self.bounty.insert(path.to_owned(), pair);
                }
                _ => {
                    self.bounty.remove(path);
                }
            }
            let root = node.get("Type").and_then(Value::as_str) == Some("Root");
            let abilities = node
                .get("Abilities")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
                .unwrap_or_default();
            let name = name.map(str::to_owned);
            self.nodes.insert(path.to_owned(), Node { root, stats, overwrites, name, abilities });
            self.parents.insert(path.to_owned(), parent.map(str::to_owned));
        }
        if let Some(children) = node.get("Stars").and_then(Value::as_array) {
            for child in children {
                self.flatten(child, path);
            }
        }
    }

    fn expand(&self, path: &str, into: &mut BTreeSet<String>) {
        let mut expanded = BTreeSet::new();
        let mut current = path.to_owned();
        while let Some(node) = self.nodes.get(&current) {
            if current.is_empty() || node.root || expanded.contains(&current) {
                break;
            }
            expanded.insert(current.clone());
            match self.parents.get(&current).cloned().flatten() {
                Some(parent) if !parent.is_empty() && !self.nodes.get(&parent).is_some_and(|n| n.root) => {
                    current = parent;
                }
                _ => break,
            }
        }
        into.extend(expanded);
    }

    fn compact_path(&self, token: &str) -> Option<String> {
        static SEGMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[a-z]+|\d+").expect("valid regex"));
        let compact = strip(token).to_lowercase();
        let mut chars = compact.chars();
        let root = match chars.next()? {
            'c' => "combat",
            'g' => "gathering",
            'p' => "pve",
            _ => return None,
        };
        let mut path = root.to_owned();
        for m in SEGMENT.find_iter(chars.as_str()) {
            path.push('.');
            path.push_str(m.as_str());
        }
        self.nodes.get(&path).filter(|n| !n.root).map(|_| path)
    }

    fn decode(&self, code: &str) -> Option<BTreeSet<String>> {
        let compact = strip(code);
        let mut selected = BTreeSet::new();
        if compact.is_empty() {
            return Some(selected);
        }
        if compact.starts_with("SC:") || compact.starts_with("v2:") {
            let payload = compact.split_once(':').map_or("", |p| p.1);
            if payload.contains('|') {
                for token in payload.split('|') {
                    if let Some(path) = self.compact_path(token) {
                        self.expand(&path, &mut selected);
                    }
                }
                return Some(selected);
            }
            let pad = (4 - payload.chars().count() % 4) % 4;
            let padded: Vec<u8> = payload
                .bytes()
                .chain(std::iter::repeat_n(b'=', pad))
                .map(|b| match b {
                    b'-' => b'+',
                    b'_' => b'/',
                    b => b,
                })
                .collect();
            for id in a2b_base64(&padded)? {
                if let Some(path) = self.selectable.get(id as usize) {
                    self.expand(path, &mut selected);
                }
            }
            return Some(selected);
        }
        if !compact.is_ascii() {
            return None;
        }
        let decoded = String::from_utf8(a2b_base64(compact.as_bytes())?).ok()?;
        Some(decoded.split('$').filter(|p| self.nodes.contains_key(*p)).map(str::to_owned).collect())
    }

    /// Everything `parse_build_code` returns; the active paths and the
    /// abilities set are walked in sorted order (Python's is hash order).
    pub fn preview(&self, code: &str) -> Preview {
        let mut out = Preview::default();
        if code.is_empty() || self.nodes.is_empty() {
            return out;
        }
        let Some(selected) = self.decode(code) else { return out };
        out.paths_count = selected.len();
        let chart = self.parse(code);
        out.stats = chart.stats;
        let mut overwrites = BTreeSet::new();
        for path in &selected {
            if let Some(o) = self.nodes.get(path).and_then(|n| n.overwrites.as_ref()) {
                overwrites.extend(o.iter().cloned());
            }
        }
        let mut abilities = BTreeSet::new();
        for path in selected.iter().filter(|p| !overwrites.contains(*p)) {
            let Some(node) = self.nodes.get(path) else { continue };
            if out.bounty.is_none()
                && let Some(&(physical, magic)) = self.bounty.get(path)
            {
                out.bounty = Some((node.name.clone().unwrap_or_default(), physical, magic));
            }
            abilities.extend(node.abilities.iter().cloned());
        }
        out.abilities = abilities.into_iter().collect();
        out
    }

    /// `parse_build_code(code)`: the chart's stats (empty for an empty or
    /// undecodable code). Python walks the active paths in hash order; sorted
    /// order here.
    pub fn parse(&self, code: &str) -> Chart {
        let mut chart = Chart::default();
        if code.is_empty() || self.nodes.is_empty() {
            return chart;
        }
        let Some(selected) = self.decode(code) else { return chart };
        let mut overwrites = BTreeSet::new();
        for path in &selected {
            if let Some(o) = self.nodes.get(path).and_then(|n| n.overwrites.as_ref()) {
                overwrites.extend(o.iter().cloned());
            }
        }
        let active: Vec<&String> = selected.iter().filter(|p| !overwrites.contains(*p)).collect();
        chart.bounty = active.iter().find_map(|p| self.bounty.get(*p).copied());
        for path in active {
            let Some(node) = self.nodes.get(path) else { continue };
            for s in &node.stats {
                let Some(name) = &s.name else { continue };
                let i = match chart.stats.iter().position(|e| &e.0 == name) {
                    Some(i) => i,
                    None => {
                        chart.stats.push((name.clone(), 0.0, 0.0));
                        chart.stats.len() - 1
                    }
                };
                if s.scales {
                    chart.stats[i].2 += s.value;
                } else {
                    chart.stats[i].1 += s.value;
                }
            }
        }
        chart
    }
}

/// `binascii.a2b_base64(data, strict_mode=False)`; `None` is its `binascii.Error`.
pub fn a2b_base64(data: &[u8]) -> Option<Vec<u8>> {
    fn value(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(data.len() / 4 * 3 + 3);
    let (mut quad, mut left, mut pads) = (0u8, 0u8, 0u8);
    for &c in data {
        if c == b'=' {
            if quad >= 2 {
                pads += 1;
                if quad + pads >= 4 {
                    return Some(out);
                }
            }
            continue;
        }
        let Some(v) = value(c) else { continue };
        pads = 0;
        match quad {
            0 => {
                quad = 1;
                left = v;
            }
            1 => {
                quad = 2;
                out.push((left << 2) | (v >> 4));
                left = v & 0x0f;
            }
            2 => {
                quad = 3;
                out.push((left << 4) | (v >> 2));
                left = v & 0x03;
            }
            _ => {
                quad = 0;
                out.push((left << 6) | v);
                left = 0;
            }
        }
    }
    (quad == 0).then_some(out)
}
