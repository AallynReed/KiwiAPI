//! `app/trove/codexes/loot.py`: the style catalogues under `prefabs/loot/`.

use std::{collections::HashMap, sync::LazyLock};

use regex::bytes::Regex;

use super::{PyResult, py, wire::read_uleb};

pub const LOOT_ROOT: &str = "prefabs/loot/";

const CATALOGUES: [(&str, &str); 9] = [
    ("hat", "Hat"),
    ("face", "Face"),
    ("weapon_bow", "Weapon"),
    ("weapon_fist", "Weapon"),
    ("weapon_melee", "Weapon"),
    ("weapon_pistol", "Weapon"),
    ("weapon_spear", "Weapon"),
    ("weapon_staff", "Weapon"),
    ("pvpbanner", "Banner"),
];

static EQUIPMENT_REF_RE: LazyLock<Regex> = LazyLock::new(|| py::bytes_re(r"equipment/[A-Za-z0-9_/.\-\[\]]+"));

/// One catalogue row (plus the `source` path once indexed).
#[derive(Clone, Debug, PartialEq)]
pub struct StyleRow {
    pub equipment: String,
    pub family: &'static str,
    pub name_key: String,
    pub raw_category: Option<u8>,
    pub base_mastery: Option<i64>,
    pub source: String,
}

pub fn catalogue_stem(path: &str) -> String {
    let p = path.replace('\\', "/");
    let last = py::last_part(&p, "/");
    py::lower(last.strip_suffix(".binfab").unwrap_or(last))
}

fn family(stem: &str) -> Option<&'static str> {
    CATALOGUES.iter().find(|(s, _)| *s == stem).map(|(_, f)| *f)
}

pub fn is_style_catalogue(path: &str) -> bool {
    let p = py::lower(&path.replace('\\', "/"));
    p.starts_with(LOOT_ROOT) && family(&catalogue_stem(&p)).is_some()
}

fn index_pattern(index: u128) -> Vec<u8> {
    let mut value = index * 16 - 12;
    let mut out = Vec::new();
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        out.push(if value != 0 { byte | 0x80 } else { byte });
        if value == 0 {
            break;
        }
    }
    out.push(0x08);
    out
}

fn read_len_string(data: &[u8], pos: usize) -> (String, usize) {
    if pos >= data.len() {
        return (String::new(), pos);
    }
    let length = data[pos] as usize;
    let pos = pos + 1;
    if length == 0 || pos + length > data.len() {
        return (String::new(), pos);
    }
    (py::latin1(&data[pos..pos + length]), pos + length)
}

fn base_mastery(stem: &str, raw: Option<u8>) -> Option<i64> {
    if stem == "hat" {
        return match raw? {
            0x04 => Some(10),
            0x01 | 0x02 => Some(1),
            _ => None,
        };
    }
    Some(1)
}

fn parse_pvpbanner(data: &[u8]) -> Vec<StyleRow> {
    let mut rows = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for m in EQUIPMENT_REF_RE.find_iter(data) {
        let r = py::lower(py::rstrip_chars(&py::latin1(m.as_bytes()).replace('\\', "/"), "./"));
        if seen.insert(r.clone()) {
            rows.push(StyleRow {
                equipment: r,
                family: "Banner",
                name_key: String::new(),
                raw_category: None,
                base_mastery: Some(1),
                source: String::new(),
            });
        }
    }
    rows
}

/// `parse_style_catalogue` (raises where Python's `read_uleb` does).
pub fn parse_style_catalogue(data: &[u8], path: &str) -> PyResult<Vec<StyleRow>> {
    let stem = catalogue_stem(path);
    let fam = family(&stem).unwrap_or("");
    if stem == "pvpbanner" {
        return Ok(parse_pvpbanner(data));
    }
    let mut rows = Vec::new();
    let n = data.len();
    let mut pos = 2;
    if pos >= n {
        return Ok(rows);
    }
    let category_count = data[pos];
    pos += 2;
    for category_index in 1..=u128::from(category_count) {
        let pattern = index_pattern(category_index);
        let Some(found) = py::find(data, &pattern, pos) else { break };
        pos = found + pattern.len();
        let (_category, p) = read_len_string(data, pos);
        pos = p + 3;
        let (count, p) = read_uleb(data, pos)?;
        pos = p + 1;
        if count > 100_000 {
            break;
        }
        for row_index in 1..=count {
            let row_pattern = index_pattern(row_index);
            let Some(row_found) = py::find(data, &row_pattern, pos) else { break };
            pos = row_found + row_pattern.len();
            let (blueprint, p) = read_len_string(data, pos);
            pos = p + 1;
            let (name_key, p) = read_len_string(data, pos);
            pos = p + 1;
            let (_desc_key, p) = read_len_string(data, pos);
            pos = p;
            if blueprint.is_empty() {
                continue;
            }
            let raw_category = (pos + 1 < n && data[pos] == 0x30).then(|| data[pos + 1]);
            let r = py::lower(&blueprint.replace('\\', "/"));
            let last = py::last_part(&r, "/");
            rows.push(StyleRow {
                equipment: last.strip_suffix(".blueprint").unwrap_or(last).to_owned(),
                family: fam,
                name_key: if name_key.starts_with('$') { name_key } else { String::new() },
                raw_category,
                base_mastery: base_mastery(&stem, raw_category),
                source: String::new(),
            });
        }
    }
    Ok(rows)
}

/// `style_index`: equipment id -> row, the first catalogue (by path) wins.
pub fn style_index(catalogues: &[(String, Vec<u8>)]) -> PyResult<HashMap<String, StyleRow>> {
    let mut sorted: Vec<&(String, Vec<u8>)> = catalogues.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    let mut out = HashMap::new();
    for (path, data) in sorted {
        for mut row in parse_style_catalogue(data, path)? {
            if !row.equipment.is_empty() && !out.contains_key(&row.equipment) {
                row.source = path.clone();
                out.insert(row.equipment.clone(), row);
            }
        }
    }
    Ok(out)
}
