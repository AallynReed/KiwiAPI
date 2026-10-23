//! `app/trove/codexes/recipe.py`: recipe output, ingredients, requirements, mastery.

use std::{collections::HashSet, sync::LazyLock};

use regex::bytes::Regex;
use serde_json::{Map, Value, json};

use super::{
    PyResult, int_value,
    mastery::Multipliers,
    py,
    wire::{clean_localized_text, read_varint, unzig},
};
use crate::binfab::harvest_strings;

static ASCII_RE: LazyLock<Regex> = LazyLock::new(|| py::bytes_re(r"[ -~]{3,}"));

const PATH_PREFIXES: [&str; 5] = ["item/", "placeable/", "block/", "collections/", "effects/"];
const OUTPUT_PREFIXES: [&str; 7] =
    ["item/", "placeable/", "block/", "collections/", "effects/", "equipment/", "abilities/"];
const TOKEN_PRODUCT_ROOTS: [&str; 5] = ["item/costume/", "item/skin/", "skins/", "collections/costume/", "item/"];

const REQUIREMENT_LABELS: [(&str, &str); 4] = [
    ("activeclass", "Active Class"),
    ("powerrank", "Power Rank"),
    ("hascollection", "Requires Collection"),
    ("hastitle", "Requires Title"),
];

const CATEGORY_BY_OUTPUT: [(&str, &str); 11] = [
    ("item/mount/", "Mount"),
    ("item/pet/", "Ally"),
    ("collections/pet/", "Ally"),
    ("item/unlocker/", "Memento"),
    ("item/plantseed/", "Seed"),
    ("item/consumable/titles/", "Title"),
    ("placeable/", "Placeable"),
    ("block/", "Block"),
    ("item/lootbox/newrings/", "Ring"),
    ("item/consumable/gearcrafting/", "Gear"),
    ("item/costume/", "Costume"),
];

fn starts_any(s: &str, prefixes: &[&str]) -> bool {
    prefixes.iter().any(|p| s.starts_with(p))
}

/// `_normalize`.
pub fn normalize(text: &str) -> String {
    let t = clean_localized_text(py::strip(text));
    let t = py::rstrip_chars(&t, "(");
    let t = py::strip_chars(t, "\"");
    let t = py::strip_chars(t, "'");
    py::strip(t).to_owned()
}

/// `_pretty_name`.
pub fn pretty_name(path: &str) -> String {
    let mut stem = py::path_stem(&path.replace('\\', "/"));
    for suffix in ["_notrade", "_trade"] {
        if let Some(s) = stem.strip_suffix(suffix) {
            stem = s.to_owned();
            break;
        }
    }
    py::title(py::strip(&stem.replace('_', " ")))
}

fn looks_like_path(text: &str) -> bool {
    starts_any(&py::lower(py::strip(text)), &PATH_PREFIXES)
}

fn is_material(path: &str) -> bool {
    starts_any(&py::lower(path), &["item/crafting/", "item/currency/", "item/dragon/"])
}

fn is_output_candidate(path: &str) -> bool {
    let p = py::lower(path);
    if p.is_empty() || p.starts_with("effects/") || p.starts_with("collections/") {
        return false;
    }
    starts_any(&p, &["item/", "placeable/", "block/"])
}

fn scan_strings(content: &[u8]) -> Vec<String> {
    ASCII_RE.find_iter(content).map(|m| normalize(&py::ascii_ignore(m.as_bytes()))).filter(|t| !t.is_empty()).collect()
}

/// `decode_quantified_paths`: `(path, amount)` records.
pub fn decode_quantified_paths(content: &[u8]) -> Vec<(String, i128)> {
    let mut rows = Vec::new();
    let mut pos = 0usize;
    let n = content.len();
    while pos + 3 < n {
        if content[pos] != 0x08 {
            pos += 1;
            continue;
        }
        let (length, cursor) = read_varint(content, pos + 1);
        let length = match length {
            Some(l) if l > 0 && cursor as u128 + l <= n as u128 => l as usize,
            _ => {
                pos += 1;
                continue;
            }
        };
        let raw = &content[cursor..cursor + length];
        if raw.iter().any(|&b| !(32..=126).contains(&b)) {
            pos += 1;
            continue;
        }
        let text = normalize(&py::ascii_ignore(raw));
        if !looks_like_path(&text) {
            pos += 1;
            continue;
        }
        let nxt = cursor + length;
        if nxt >= n || content[nxt] != 0x10 {
            pos = nxt;
            continue;
        }
        let (Some(amount), end) = read_varint(content, nxt + 1) else {
            pos += 1;
            continue;
        };
        rows.push((text, unzig(amount)));
        pos = end;
    }
    rows
}

fn split_sections(strings: &[String]) -> (&[String], &[String]) {
    let nulls: Vec<usize> =
        strings.iter().enumerate().filter(|(_, s)| py::lower(s) == "null").map(|(i, _)| i).collect();
    match nulls.as_slice() {
        [first, second, ..] => (&strings[..*first], &strings[second + 1..]),
        [first] => (&strings[..*first], &strings[first + 1..]),
        [] => (strings, &[]),
    }
}

fn token_ok(s: &str) -> bool {
    s.len() >= 4 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn structural_output(content: &[u8], quantified: &[(String, i128)], prefab_exists: &dyn Fn(&str) -> bool) -> String {
    let (mut best16, mut best1): (Option<String>, Option<String>) = (None, None);
    let mut loose = String::new();
    let mut token = String::new();
    let ingredients: HashSet<String> = quantified.iter().map(|q| normalize(&q.0)).collect();
    for h in harvest_strings(content, 2, 512) {
        let norm = normalize(&h.text);
        if norm.is_empty() || py::lower(&norm) == "null" {
            continue;
        }
        if starts_any(&py::lower(&norm), &OUTPUT_PREFIXES) {
            match h.field {
                16 => {
                    best16.get_or_insert(norm);
                }
                1 => {
                    best1.get_or_insert(norm);
                }
                0 if !ingredients.contains(&norm) => loose = norm,
                _ => {}
            }
        } else if (h.field == 1 || h.field == 2) && token.is_empty() && token_ok(&norm) {
            token = norm;
        }
    }
    let named = best16.filter(|s| !s.is_empty()).or(best1.filter(|s| !s.is_empty())).unwrap_or(loose);
    if !named.is_empty() {
        return named;
    }
    if !token.is_empty() {
        for root in TOKEN_PRODUCT_ROOTS {
            let cand = format!("{root}{token}");
            if prefab_exists(&cand) {
                return cand;
            }
        }
    }
    String::new()
}

fn quantified_output(quantified: &[(String, i128)]) -> String {
    quantified
        .iter()
        .rev()
        .map(|q| normalize(&q.0))
        .find(|p| !p.is_empty() && !is_material(p) && !py::lower(p).starts_with("placeable/block/"))
        .unwrap_or_default()
}

fn choose_output(
    content: &[u8],
    strings: &[String],
    quantified: &[(String, i128)],
    exists: &dyn Fn(&str) -> bool,
) -> String {
    let structural = structural_output(content, quantified, exists);
    if !structural.is_empty() {
        return structural;
    }
    let from_quantified = quantified_output(quantified);
    if !from_quantified.is_empty() {
        return from_quantified;
    }
    let (head, tail) = split_sections(strings);
    for section in [tail, head] {
        if let Some(t) = section.iter().find(|t| is_output_candidate(t) && !is_material(t)) {
            return t.clone();
        }
    }
    tail.iter().chain(head).find(|t| t.starts_with("collections/")).cloned().unwrap_or_default()
}

fn output_amount(output: &str, quantified: &[(String, i128)]) -> i128 {
    let target = normalize(output);
    quantified.iter().rev().find(|q| normalize(&q.0) == target).map_or(1, |q| if q.1 > 0 { q.1 } else { 1 })
}

fn requirements(strings: &[String], ingredient_paths: &HashSet<String>, output_path: &str) -> Vec<String> {
    let norm: Vec<String> = strings.iter().map(|s| normalize(s)).collect();
    let output_path = normalize(output_path);
    let mut out: Vec<String> = Vec::new();
    for (i, text) in norm.iter().enumerate() {
        let low = py::lower(text);
        if text.is_empty() || low == "null" {
            continue;
        }
        if ingredient_paths.contains(text)
            || *text == output_path
            || looks_like_path(text)
            || low.starts_with("$prefabs_")
        {
            continue;
        }
        let label = if low == "zonetag" {
            norm.get(i + 1).filter(|n| !n.is_empty() && py::lower(n) != "zonetag").map(|n| format!("Zone: {n}"))
        } else {
            REQUIREMENT_LABELS.iter().find(|(k, _)| *k == low).map(|(_, v)| (*v).to_owned())
        };
        if let Some(l) = label
            && !out.contains(&l)
        {
            out.push(l);
        }
    }
    out
}

pub fn category_from_output(output: &str) -> &'static str {
    let low = py::lower(output);
    CATEGORY_BY_OUTPUT.iter().find(|(p, _)| low.starts_with(p)).map_or("Recipe", |(_, l)| l)
}

fn match_mastery_struct(content: &[u8], pos: usize) -> Option<i128> {
    let n = content.len();
    if pos >= n || content[pos] != 0x30 {
        return None;
    }
    let (v6, cur) = read_varint(content, pos + 1);
    if !matches!(v6, Some(0 | 2)) || cur >= n || content[cur] != 0x50 {
        return None;
    }
    let (mastery, cur) = read_varint(content, cur + 1);
    let mastery = mastery?;
    if cur >= n || content[cur] != 0x70 {
        return None;
    }
    let (Some(_), cur) = read_varint(content, cur + 1) else { return None };
    if cur + 1 >= n || content[cur] != 0x80 || content[cur + 1] != 0x01 {
        return None;
    }
    let (Some(_), cur) = read_varint(content, cur + 2) else { return None };
    if cur + 1 >= n || content[cur] != 0x98 || content[cur + 1] != 0x01 {
        return None;
    }
    Some(mastery as i128)
}

/// `recipe_mastery_from_prefab`: only when every structural match agrees.
pub fn recipe_mastery_from_prefab(content: &[u8]) -> Option<i128> {
    let matches: Vec<i128> =
        (0..content.len()).filter(|&p| content[p] == 0x30).filter_map(|p| match_mastery_struct(content, p)).collect();
    let first = *matches.first()?;
    matches.iter().all(|&m| m == first).then_some(first)
}

/// `resolve_recipe_mastery`: `(value, source, prefab_byte)`.
pub fn resolve_recipe_mastery(
    rel: &str,
    content: &[u8],
    multipliers: &Multipliers,
) -> (Option<i128>, &'static str, Option<i128>) {
    let prefab_byte = recipe_mastery_from_prefab(content);
    let stem = py::last_part(&rel.replace('\\', "/"), "/").to_owned();
    if let Some(row) = multipliers.get(&stem) {
        return (Some(row.predicted), "multipliers", prefab_byte);
    }
    match prefab_byte {
        Some(b) => (Some(b), "prefab", Some(b)),
        None => (None, "review", None),
    }
}

/// What `item_meta` resolves for a referenced prefab.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemMeta {
    pub name: String,
    pub desc: String,
    pub blueprint: Option<String>,
}

/// `resolve_meta`: a referenced prefab's name, description and model.
pub type Resolve<'a> = dyn FnMut(&str) -> PyResult<ItemMeta> + 'a;

/// `parse_recipe`.
pub struct Parsed {
    pub name: String,
    pub description: String,
    pub category: &'static str,
    pub output: Option<Map<String, Value>>,
    pub ingredients: Vec<Value>,
    pub requirements: Vec<String>,
}

pub fn parse_recipe(
    content: &[u8],
    resolve_meta: Option<&mut Resolve<'_>>,
    prefab_exists: &dyn Fn(&str) -> bool,
) -> PyResult<Parsed> {
    let strings = scan_strings(content);
    let quantified = decode_quantified_paths(content);
    let output_path = choose_output(content, &strings, &quantified, prefab_exists);
    let output_norm = output_path.strip_suffix(".binfab").unwrap_or(&output_path).to_owned();

    let mut ingredient_paths = HashSet::new();
    let mut ingredients: Vec<(String, i128, String)> = Vec::new();
    for (path, amount) in &quantified {
        let p = normalize(path);
        if p.is_empty() || p == output_norm || p.starts_with("collections/") {
            continue;
        }
        ingredient_paths.insert(p.clone());
        let name = pretty_name(&p);
        ingredients.push((p, *amount, name));
    }

    let mut name = if output_norm.is_empty() { String::new() } else { pretty_name(&output_norm) };
    let mut description = String::new();
    if let Some(resolve) = resolve_meta
        && !output_norm.is_empty()
    {
        let meta = resolve(&output_norm)?;
        if !meta.name.is_empty() {
            name = meta.name;
        }
        description = meta.desc;
        for ing in &mut ingredients {
            let im = resolve(&ing.0)?;
            if !im.name.is_empty() {
                ing.2 = im.name;
            }
        }
    }

    let output = (!output_norm.is_empty()).then(|| {
        let mut m = Map::new();
        m.insert("path".into(), json!(output_norm));
        m.insert("name".into(), json!(name));
        m.insert("amount".into(), int_value(output_amount(&output_norm, &quantified)));
        m
    });
    Ok(Parsed {
        category: category_from_output(&output_norm),
        requirements: requirements(&strings, &ingredient_paths, &output_norm),
        ingredients: ingredients
            .into_iter()
            .map(|(p, a, n)| {
                let mut m = Map::new();
                m.insert("path".into(), json!(p));
                m.insert("amount".into(), int_value(a));
                m.insert("name".into(), json!(n));
                Value::Object(m)
            })
            .collect(),
        name,
        description,
        output,
    })
}
