//! `app/trove/codexes/styles.py`: equipment appearance identity and family.

use serde_json::{Map, Value, json};

use super::py;

const FAMILY_TOKENS: [(&str, &str); 16] = [
    ("banner", "Banner"),
    ("helm", "Hat"),
    ("hat", "Hat"),
    ("face", "Face"),
    ("hair", "Hair"),
    ("mask", "Face"),
    ("weapon", "Weapon"),
    ("sword", "Weapon"),
    ("staff", "Weapon"),
    ("bow", "Weapon"),
    ("gun", "Weapon"),
    ("pistol", "Weapon"),
    ("spear", "Weapon"),
    ("fist", "Weapon"),
    ("axe", "Weapon"),
    ("lance", "Weapon"),
];

/// `equipment_id`: the prefab stem.
pub fn equipment_id(rel: &str) -> String {
    let s = rel.replace('\\', "/");
    let last = py::last_part(&s, "/");
    last.strip_suffix(".binfab").unwrap_or(last).to_owned()
}

/// `style_family`: best-effort slot family from stem tokens, or "".
pub fn style_family(rel: &str) -> &'static str {
    let stem = py::lower(&equipment_id(rel));
    FAMILY_TOKENS.iter().find(|(t, _)| stem.contains(t)).map_or("", |(_, l)| l)
}

/// `style_identity`.
pub fn style_identity(rel: &str) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("source_id".into(), json!(rel));
    m.insert("equipment_ref".into(), json!(equipment_id(rel)));
    m.insert("resolved_id".into(), json!(rel));
    m.insert("confidence".into(), json!("high"));
    m
}
