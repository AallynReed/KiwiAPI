//! `app/trove/codexes/unlocks.py`: what owning or consuming a thing grants.

use std::{collections::HashSet, sync::LazyLock};

use regex::Regex;

use super::{py, wire::real_fields};

static RECIPE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^recipe_[a-z0-9_]+$").expect("static regex"));
static PATH_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9_]+(?:/[A-Za-z0-9_\-.\[\]]+)+$").expect("static regex"));

const SOURCE_ROOTS: [&str; 5] = ["collections/", "item/", "equipment/", "recipes/", "placeable/"];

fn is_target_only(text: &str) -> bool {
    let lowered = py::lower(text);
    lowered.ends_with(".blueprint") || RECIPE_RE.is_match(&lowered) || lowered.starts_with("collections/badge/")
}

fn is_source(text: &str) -> bool {
    let lowered = py::lower(text);
    if lowered.starts_with('$') || is_target_only(&lowered) {
        return false;
    }
    if SOURCE_ROOTS.iter().any(|r| lowered.starts_with(r)) && PATH_RE.is_match(&lowered) {
        return true;
    }
    !lowered.contains('/') && !lowered.contains('.') && lowered.chars().count() > 3
}

/// `parse_unlocks`: `(source, target)` pairs in file order.
pub fn parse_unlocks(data: &[u8]) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut seen = HashSet::new();
    let mut source: Option<String> = None;
    for f in real_fields(data) {
        let text = py::rstrip_chars(py::strip(&f.text), "./").to_owned();
        if text.is_empty() {
            continue;
        }
        if is_target_only(&text) {
            let Some(src) = &source else { continue };
            let key = (py::lower(src), py::lower(&text));
            if key.0 != key.1 && seen.insert(key) {
                pairs.push((src.clone(), text));
            }
        } else if is_source(&text) {
            source = Some(text);
        }
    }
    pairs
}
