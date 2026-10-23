//! `app/trove/codexes/abilities.py`: ability display text from its own prefab.

use super::{localize::Loc, py};
use crate::binfab::harvest_strings;

/// `normalize_ref`: an ability ref -> `abilities/...` (lowercased, no suffix).
pub fn normalize_ref(r: &str) -> String {
    let text = py::strip_chars(py::strip(&r.replace('\\', "/")), "/").to_owned();
    let text = text.strip_suffix(".binfab").unwrap_or(&text);
    if text.is_empty() {
        return String::new();
    }
    let text = text.strip_prefix("prefabs/").unwrap_or(text);
    let text = if text.starts_with("abilities/") { text.to_owned() } else { format!("abilities/{text}") };
    py::lower(&text)
}

/// `extract_keys` + `display`: `(name_key, desc_key, name, description)`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AbilityMeta {
    pub name_key: String,
    pub desc_key: String,
    pub name: String,
    pub description: String,
}

pub fn extract_keys(content: &[u8]) -> (String, String) {
    let (mut name_key, mut desc_key) = (String::new(), String::new());
    for h in harvest_strings(content, 2, 512) {
        let text = h.text;
        if !text.starts_with('$') {
            continue;
        }
        if desc_key.is_empty() && text.ends_with("_description") {
            desc_key = text;
        } else if name_key.is_empty() && text.ends_with("_name") {
            name_key = text;
        }
        if !name_key.is_empty() && !desc_key.is_empty() {
            break;
        }
    }
    (name_key, desc_key)
}

pub fn display(keys: (String, String), loc: &Loc) -> AbilityMeta {
    let (name_key, desc_key) = keys;
    let look = |k: &str| if k.is_empty() { String::new() } else { loc.get(k).cloned().unwrap_or_default() };
    AbilityMeta { name: look(&name_key), description: look(&desc_key), name_key, desc_key }
}
