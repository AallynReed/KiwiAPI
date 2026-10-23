//! `app/trove/codexes/types.py`: codex types and path classification.

pub const PREFABS_ROOT: &str = "prefabs/";
pub const LOCALE_ROOT: &str = "languages/en/";

/// `(type, prefix under prefabs/, excluded stem suffixes)`, most specific first.
pub const CODEX_TYPES: [(&str, &str, &[&str]); 17] = [
    ("fish", "item/fish/", &[]),
    ("memento", "item/unlocker/", &[]),
    ("ally", "collections/pet/", &["_npc"]),
    ("mount", "collections/mount/", &[]),
    ("badge", "collections/badge/", &[]),
    ("wings", "collections/wings/", &[]),
    ("aura", "collections/aura/", &[]),
    ("boat", "collections/boat/", &[]),
    ("sail", "collections/sail/", &[]),
    ("flask", "collections/flask/", &[]),
    ("tome", "collections/tome/", &[]),
    ("magrider", "collections/magrider/", &[]),
    ("fishingpole", "collections/fishing/", &[]),
    ("skin", "skins/", &[]),
    ("style", "equipment/", &[]),
    ("recipe", "recipes/", &[]),
    ("item", "item/", &[]),
];

/// Every served type: the path-classified ones plus `dragon`.
pub const ALL_TYPES: [&str; 18] = [
    "fish",
    "memento",
    "ally",
    "mount",
    "badge",
    "wings",
    "aura",
    "boat",
    "sail",
    "flask",
    "tome",
    "magrider",
    "fishingpole",
    "skin",
    "style",
    "recipe",
    "item",
    "dragon",
];

/// `classify`: the codex type of a prefab path, or `None`.
pub fn classify(path: &str) -> Option<&'static str> {
    let rel = path.strip_prefix(PREFABS_ROOT)?;
    if !path.ends_with(".binfab") {
        return None;
    }
    let last = rel.rsplit_once('/').map_or(rel, |(_, t)| t);
    let stem = last.strip_suffix(".binfab").unwrap_or(last);
    for (ctype, prefix, excludes) in CODEX_TYPES {
        if rel.starts_with(prefix) {
            if excludes.iter().any(|s| stem.ends_with(s)) {
                return None;
            }
            return Some(ctype);
        }
    }
    None
}
