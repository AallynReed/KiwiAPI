//! `app/trove/codexes/providers.py`: which benches and professions craft a recipe.

use std::{
    collections::{HashMap, HashSet},
    sync::LazyLock,
};

use regex::bytes::Regex;
use serde_json::{Map, Value, json};

use super::{PyErr, PyResult, py};
use crate::binfab::harvest_strings;

/// The Mongo `$regex` (with `$options: "i"`) that prefilters provider prefabs.
pub const PROVIDER_PATH_RE: &str = "(interact|profession)";

static RECIPE_TOKEN_RE: LazyLock<Regex> = LazyLock::new(|| py::bytes_re(r"recipe_[a-z0-9_]+"));

pub fn lane_for(path: &str) -> &'static str {
    let low = py::lower(path);
    if low.contains("profession") {
        "profession"
    } else if low.contains("workbench") || low.contains("/crafting/") || low.contains("forge") {
        "bench"
    } else if low.contains("vendor") {
        "vendor"
    } else if low.contains("guideui") {
        "guide"
    } else if low.contains("interact") {
        "interactive"
    } else {
        "other"
    }
}

pub fn provider_id_from_path(path: &str) -> String {
    let p = path.replace('\\', "/");
    let p = p.strip_prefix("prefabs/").unwrap_or(&p);
    p.strip_suffix(".binfab").unwrap_or(p).to_owned()
}

/// Every `recipe_*` token in ASCII-lowered text, trimmed to a known recipe id.
pub fn trimmed_tokens<'a>(lowered: &'a [u8], known: &'a HashSet<String>) -> impl Iterator<Item = String> + 'a {
    RECIPE_TOKEN_RE.find_iter(lowered).filter_map(move |m| {
        let mut tok = std::str::from_utf8(m.as_bytes()).expect("ascii token");
        while !tok.is_empty() && !known.contains(tok) {
            tok = &tok[..tok.len() - 1];
        }
        (!tok.is_empty()).then(|| tok.to_owned())
    })
}

pub fn extract_provider_refs(content: &[u8], known: &HashSet<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    for h in harvest_strings(content, 2, 512) {
        let lowered = h.text.to_ascii_lowercase();
        for tok in trimmed_tokens(lowered.as_bytes(), known) {
            if seen.insert(tok.clone()) {
                out.push(tok);
            }
        }
    }
    out
}

const CATEGORY_COUNT_MARKER: [u8; 3] = [0xbe, 0x0e, 0xae];

fn slot_pattern(index: usize) -> PyResult<Vec<u8>> {
    let b = u8::try_from(16 * index + 4).map_err(|_| PyErr("bytes must be in range(0, 256)".into()))?;
    let mut out = vec![if index == 0 { 0x00 } else { 0x1e }, b];
    if index > 7 {
        out.push(0x01);
    }
    out.push(0x08);
    Ok(out)
}

/// A bench's tab: `(category, order, recipe ids)`.
pub type Category = (String, usize, Vec<String>);

pub fn extract_categories(content: &[u8], known: &HashSet<String>) -> PyResult<Vec<Category>> {
    let Some(at) = py::find(content, &CATEGORY_COUNT_MARKER, 0) else { return Ok(Vec::new()) };
    if at + 3 >= content.len() {
        return Ok(Vec::new());
    }
    let count = content[at + 3] as usize;
    if count == 0 || count > 80 {
        return Ok(Vec::new());
    }
    let mut groups: Vec<Category> = Vec::new();
    let mut cursor = 0;
    for index in 0..count {
        let pattern = slot_pattern(index)?;
        let Some(found) = py::find(content, &pattern, cursor) else { continue };
        let name_at = found + pattern.len();
        if name_at >= content.len() {
            continue;
        }
        let length = content[name_at] as usize;
        if length == 0 || name_at + 1 + length > content.len() {
            continue;
        }
        let category = py::latin1(&content[name_at + 1..name_at + 1 + length]);
        let end = py::find(content, &slot_pattern(index + 1)?, name_at).unwrap_or(content.len());
        let body = content.get(name_at + 1 + length..end).unwrap_or_default().to_ascii_lowercase();
        let mut recipes: Vec<String> = Vec::new();
        for tok in trimmed_tokens(&body, known) {
            if !recipes.contains(&tok) {
                recipes.push(tok);
            }
        }
        if !recipes.is_empty() {
            groups.push((category, groups.len(), recipes));
        }
        cursor = end;
    }
    Ok(groups)
}

/// `build_provider_map`: recipe id -> provider rows, in provider order.
pub fn build_provider_map(
    prefabs: &[(String, Vec<u8>)],
    known: &HashSet<String>,
) -> PyResult<HashMap<String, Vec<Value>>> {
    let mut out: HashMap<String, Vec<Value>> = HashMap::new();
    for (path, content) in prefabs {
        let refs = extract_provider_refs(content, known);
        if refs.is_empty() {
            continue;
        }
        let pid = provider_id_from_path(path);
        let lane = lane_for(path);
        let mut category_of: HashMap<String, (String, usize)> = HashMap::new();
        for (category, order, recipes) in extract_categories(content, known)? {
            for rid in recipes {
                category_of.entry(rid).or_insert_with(|| (category.clone(), order));
            }
        }
        for rid in refs {
            let rows = out.entry(rid.clone()).or_default();
            if rows.iter().any(|r| r["provider"] == pid.as_str()) {
                continue;
            }
            let (category, order) = match category_of.get(&rid) {
                Some((c, o)) => (c.clone(), json!(o)),
                None => (String::new(), Value::Null),
            };
            let mut row = Map::new();
            row.insert("provider".into(), json!(pid));
            row.insert("provider_path".into(), json!(path));
            row.insert("lane".into(), json!(lane));
            row.insert("category".into(), json!(category));
            row.insert("category_order".into(), order);
            rows.push(Value::Object(row));
        }
    }
    Ok(out)
}
