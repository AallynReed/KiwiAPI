//! `class_levels.py` (classes.json): each class's base sheet, class bonuses and
//! per-level stats, onto the hand-maintained repo copy.

use std::sync::OnceLock;

use indexmap::IndexMap;
use regex::bytes::Regex;

use crate::{
    decode::{
        common::{operation, r4, read_uleb, stat_keys, unzig},
        json::{J, Map},
        registry::{Built, Ctx, DecodeError},
    },
    obj,
};

const MAX_LEVEL: i128 = 30;

/// `$Stat_` key -> (sheet name, wire-to-sheet scale, shown as a percentage).
fn stats(key: &str) -> Option<(&'static str, Scale, bool)> {
    Some(match key {
        "PhysicalDamage" => ("Physical Damage", Scale::I(1), false),
        "SpellDamage" => ("Magic Damage", Scale::I(1), false),
        "MaxHealth" => ("Maximum Health", Scale::I(1), false),
        "MaxEnergy" => ("Energy", Scale::I(1), false),
        "HealthRegen_controller" => ("Health Regen", Scale::I(1), false),
        "EnergyRegen_controller" => ("Energy Regen", Scale::I(1), false),
        "Stability" => ("Stability", Scale::I(1), false),
        "MovementSpeed" => ("Movement Speed", Scale::I(1), false),
        "AttackSpeed" => ("Attack Speed", Scale::I(1), true),
        "Jump" => ("Jump", Scale::I(1), false),
        "CriticalHitChance" => ("Critical Hit", Scale::F(0.1), true),
        "CriticalHitDamage" => ("Critical Damage", Scale::I(1), true),
        "Light" => ("Light", Scale::I(1), false),
        "JumpSpeedMultiplier" => ("Jump Speed", Scale::I(100), true),
        _ => return None,
    })
}

#[derive(Clone, Copy)]
enum Scale {
    I(i32),
    F(f64),
}

impl Scale {
    fn apply(self, v: f64) -> f64 {
        match self {
            Scale::I(i) => v * f64::from(i),
            Scale::F(f) => v * f,
        }
    }
}

const BASE: [(&str, i32, bool); 13] = [
    ("Physical Damage", 100, false),
    ("Magic Damage", 100, false),
    ("Maximum Health", 150, false),
    ("Maximum Health %", 0, true),
    ("Energy", 100, false),
    ("Health Regen", 25, false),
    ("Energy Regen", 100, false),
    ("Movement Speed", 40, false),
    ("Attack Speed", 100, true),
    ("Jump", 3, false),
    ("Critical Hit", 2, true),
    ("Critical Damage", 50, true),
    ("Light", 0, false),
];
const BONUS_NAMES: [&str; 3] = ["Physical Damage", "Magic Damage", "Health Regen"];

fn level_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?s-u)[\x00\x1e]([\x80-\xff]*[\x00-\x7f])\x38").expect("regex"))
}

fn stat_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?s-u)\x10(.)\x24(.{4})\x38").expect("regex"))
}

/// `_num`: round to 4 places, an int when whole.
pub fn num(v: f64) -> J {
    let v = r4(v);
    if v == v.trunc() && v.is_finite() { J::Int(v as i128) } else { J::Float(v) }
}

fn index_error() -> DecodeError {
    DecodeError { kind: "IndexError", msg: "index out of range".into() }
}

fn level_records(data: &[u8]) -> Result<Vec<(usize, usize)>, DecodeError> {
    let mut found: IndexMap<i128, usize> = IndexMap::new();
    for m in level_re().find_iter(data) {
        let (key, pos) = read_uleb(data, m.start() + 1).ok_or_else(index_error)?;
        let level = key >> 4;
        if key & 0xF != 2 || !(1..=MAX_LEVEL).contains(&level) || found.contains_key(&level) {
            continue;
        }
        if level > 1 && !found.contains_key(&(level - 1)) {
            continue;
        }
        let (length, body) = read_uleb(data, pos + 1).ok_or_else(index_error)?;
        let at = body as i128 + length;
        if at >= 0 && data.get(at as usize) == Some(&0x3e) {
            found.insert(level, m.start());
        }
    }
    let mut levels: Vec<i128> = found.keys().copied().collect();
    levels.sort();
    if levels != (1..=MAX_LEVEL).collect::<Vec<_>>() {
        let list: Vec<String> = levels.iter().map(i128::to_string).collect();
        return Err(DecodeError::value(format!("level list incomplete: [{}]", list.join(", "))));
    }
    let from = found[&MAX_LEVEL];
    let end = data[from..]
        .windows(11)
        .position(|w| w == b"ui/classes/")
        .map(|p| p + from)
        .ok_or_else(|| DecodeError::value("no class icon after the level list"))?;
    let mut starts: Vec<usize> = (1..=MAX_LEVEL).map(|lv| found[&lv]).collect();
    starts.push(end);
    Ok(starts.windows(2).map(|w| (w[0], w[1])).collect())
}

fn stat_records(data: &[u8], start: usize, end: usize) -> Vec<(&'static str, &'static str, f64)> {
    let mut out = Vec::new();
    let end = end.min(data.len());
    if start >= end {
        return out;
    }
    for m in stat_re().captures_iter(&data[start..end]) {
        let at = start + m.get(0).expect("match").start();
        let Some(prev) = at.checked_sub(1).map(|i| data[i]) else { continue };
        let (stat, op) = (unzig(i128::from(prev)), i128::from(data[at + 1]));
        if let (Some(key), Some(op)) = (stat_keys(stat), operation(op)) {
            let raw = &m[2];
            out.push((&key[6..], op, f64::from(f32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]]))));
        }
    }
    out
}

struct Row {
    name: String,
    value: f64,
    pct: bool,
}

fn decode(data: &[u8]) -> Result<Map, DecodeError> {
    let mut totals: IndexMap<&str, f64> = BASE.iter().map(|(n, v, _)| (*n, f64::from(*v))).collect();
    let mut fixed: IndexMap<String, Option<f64>> = IndexMap::new();
    let mut bonuses: IndexMap<&str, f64> = BONUS_NAMES.iter().map(|n| (*n, 0.0)).collect();
    let mut levels = Map::new();
    for (i, (start, end)) in level_records(data)?.into_iter().enumerate() {
        let level = i + 1;
        let mut rows: IndexMap<String, Row> = IndexMap::new();
        for (key, op, value) in stat_records(data, start, end) {
            let Some((name, scale, pct)) = stats(key) else {
                return Err(DecodeError::value(format!("level {level}: unmapped stat {key}")));
            };
            match op {
                "Add" => {
                    rows.entry(name.to_owned()).or_insert(Row { name: name.into(), value: 0.0, pct }).value +=
                        scale.apply(value);
                }
                "MultiplySum" => {
                    let n = format!("{name} %");
                    rows.entry(n.clone()).or_insert(Row { name: n, value: 0.0, pct: true }).value += value * 100.0;
                }
                "Multiply" if value == 0.0 => {
                    fixed.insert(name.into(), None);
                }
                "Multiply" => {
                    let Some(b) = bonuses.get_mut(name) else {
                        return Err(DecodeError::value(format!("level {level}: unexpected multiplier on {name}")));
                    };
                    *b += (value - 1.0) * 100.0;
                }
                "Set" => {
                    fixed.insert(name.into(), Some(scale.apply(value)));
                }
                _ => return Err(DecodeError::value(format!("level {level}: unexpected {op} on {name}"))),
            }
        }
        let mut list = Vec::new();
        for row in rows.values() {
            let v = num(row.value);
            if fixed.contains_key(&row.name) {
                return Err(DecodeError::value(format!("level {level}: {} grows but is fixed", row.name)));
            }
            if let Some(t) = totals.get_mut(row.name.as_str()) {
                *t += match v {
                    J::Int(i) => i as f64,
                    J::Float(f) => f,
                    _ => 0.0,
                };
            }
            list.push(obj! {"name" => row.name.as_str(), "value" => v, "percentage" => row.pct});
        }
        if !list.is_empty() {
            levels.insert(level.to_string(), J::List(list));
        }
    }
    let mut out_stats = Vec::new();
    for (name, _, pct) in BASE {
        let value = match fixed.get(name) {
            Some(v) => *v,
            None => Some(totals[name]),
        };
        out_stats.push(obj! {"name" => name, "value" => value.map_or(J::Null, num), "percentage" => pct});
    }
    let bonus_rows: Vec<J> =
        bonuses.iter().map(|(n, v)| obj! {"name" => *n, "value" => num(*v), "percentage" => true}).collect();
    let mut out = Map::new();
    out.insert("stats".into(), J::List(out_stats));
    out.insert("bonuses".into(), J::List(bonus_rows));
    out.insert("levels".into(), J::Dict(levels));
    Ok(out)
}

pub fn build(ctx: &Ctx) -> Built {
    let mut classes = ctx.baseline("classes.json")?;
    for c in classes.as_list_mut().into_iter().flatten() {
        let qn = c.get("qualified_name").and_then(J::as_str).unwrap_or("").to_owned();
        let data = ctx
            .tree
            .read(&format!("prefabs/class/{qn}.binfab"))
            .ok_or_else(|| DecodeError::value(format!("no class prefab for {qn}")))?;
        let decoded = decode(&data)?;
        if let Some(m) = c.as_dict_mut() {
            m.extend(decoded);
        }
    }
    Ok(classes)
}
