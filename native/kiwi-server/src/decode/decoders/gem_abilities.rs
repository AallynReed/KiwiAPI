//! `gem_abilities.py`: empowered gems and the numbers behind their abilities.

use std::sync::OnceLock;

use indexmap::IndexMap;
use regex::Regex;

use crate::decode::{
    ability::{ABILITIES, Prefabs, describe, refs},
    common::{locale_at, stem},
    json::{J, Map},
    registry::{Built, Ctx},
    tree::Tree,
    value::strings_of,
};

const ELEMENTS: [(&str, &str); 5] =
    [("blue", "Water"), ("red", "Fire"), ("yellow", "Air"), ("opal", "Cosmic"), ("prismatic", "Prismatic")];

fn item_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^([a-z]+)_(.+)_t(\d+)$").expect("regex"))
}

/// Every ability a class prefab names directly: the base kit a swap replaces.
pub fn class_kit(prefabs: &Prefabs, tree: &Tree) -> Vec<String> {
    let mut kit: Vec<String> = Vec::new();
    for path in tree.files("prefabs/class/", ".binfab") {
        let rel = &path["prefabs/".len()..path.len() - ".binfab".len()];
        if let Some(pf) = prefabs.get(rel)
            && let Some(root) = &pf.root
        {
            for s in root.strings() {
                if s.starts_with("abilities/") && !kit.iter().any(|k| **k == *s) {
                    kit.push(s.to_string());
                }
            }
        }
    }
    kit
}

fn list_mut<'a>(entry: &'a mut Map, key: &str) -> &'a mut Vec<J> {
    if !matches!(entry.get(key), Some(J::List(_))) {
        entry.insert(key.into(), J::List(Vec::new()));
    }
    entry.get_mut(key).and_then(J::as_list_mut).expect("list")
}

fn strip(row: &J, key: &str) -> J {
    J::Dict(
        row.as_dict()
            .map(|m| m.iter().filter(|(k, _)| *k != key).map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default(),
    )
}

pub fn merge_rows(entry: &mut Map, info: &Map) {
    let rows = |key: &str| info.get(key).and_then(J::as_list).cloned().unwrap_or_default();
    for key in ["stages", "healing", "effects"] {
        for row in rows(key) {
            let row = strip(&row, "_depth");
            let list = list_mut(entry, key);
            if !list.contains(&row) {
                list.push(row);
            }
        }
    }
    for eff in rows("effects") {
        for stat in eff.get("stats").and_then(J::as_list).cloned().unwrap_or_default() {
            let mut row = stat.as_dict().cloned().unwrap_or_default();
            row.insert("prefab".into(), eff.get("prefab").cloned().unwrap_or(J::Null));
            let row = J::Dict(row);
            let list = list_mut(entry, "stats");
            if !list.contains(&row) {
                list.push(row);
            }
        }
    }
    for v in rows("vfx") {
        let list = list_mut(entry, "vfx");
        if !list.contains(&v) {
            list.push(v);
        }
    }
    for act in rows("actions") {
        let row = strip(&act, "_depth");
        let list = list_mut(entry, "actions");
        if !list.contains(&row) {
            list.push(row);
        }
    }
    let mut prefabs: Vec<String> = entry
        .get("prefabs")
        .and_then(J::as_list)
        .into_iter()
        .flatten()
        .filter_map(|p| p.as_str().map(str::to_owned))
        .collect();
    prefabs.extend(rows("prefabs").iter().filter_map(|p| p.as_str().map(str::to_owned)));
    prefabs.sort();
    prefabs.dedup();
    entry.insert("prefabs".into(), J::from(prefabs));
    for key in ["energy", "cooldown", "proc_cooldown"] {
        if let Some(v) = info.get(key).filter(|v| v.truthy())
            && !entry.contains_key(key)
        {
            entry.insert(key.into(), v.clone());
        }
    }
}

type Entry = (Map, Vec<i128>, Vec<String>);

pub fn build(ctx: &Ctx) -> Built {
    let tree = &ctx.tree;
    let prefabs = Prefabs::new(tree);
    let kit = class_kit(&prefabs, tree);
    let aloc = locale_at(tree, "languages/en/prefabs_item_gem_large.binfab");
    let mut gems: IndexMap<(String, String), Entry> = IndexMap::new();
    for path in tree.files("prefabs/item/gem/large/", ".binfab") {
        let name = stem(&path);
        let Some(m) = item_re().captures(&name) else { continue };
        if path.matches('/').count() != 4 {
            continue;
        }
        let (colour, slug, tier) = (m[1].to_owned(), m[2].to_owned(), m[3].to_owned());
        let (entry, tiers, rfs) = gems.entry((colour.clone(), slug.clone())).or_insert_with(|| {
            let element = ELEMENTS.iter().find(|(c, _)| *c == colour).map_or(colour.as_str(), |(_, e)| e);
            let mut e = Map::new();
            e.insert("element".into(), J::from(element));
            e.insert("slug".into(), J::from(slug.as_str()));
            e.insert("name".into(), J::from(""));
            e.insert("description".into(), J::from(""));
            for k in ["tiers", "prefabs", "stats", "stages", "healing", "effects", "vfx", "actions"] {
                e.insert(k.into(), J::List(Vec::new()));
            }
            (e, Vec::new(), Vec::new())
        });
        tiers.push(tier.parse().unwrap_or(0));
        let item = prefabs.get(&path["prefabs/".len()..path.len() - ".binfab".len()]);
        if let Some(item) = &item {
            for r in refs(item, ABILITIES) {
                if !rfs.contains(&r) {
                    rfs.push(r);
                }
            }
        }
        let mut keys: Vec<String> = item
            .as_ref()
            .map(|i| strings_of(i.components.iter().map(|(_, o)| o)))
            .unwrap_or_default()
            .iter()
            .filter(|t| t.starts_with("$prefabs_item_gem_large") && t.ends_with("_name"))
            .map(|t| t[..t.len() - "_name".len()].to_owned())
            .collect();
        keys.push(format!("$prefabs_item_gem_large_{name}"));
        keys.extend(
            ELEMENTS
                .iter()
                .filter(|(c, _)| *c != colour)
                .map(|(c, _)| format!("$prefabs_item_gem_large_{c}_{slug}_t{tier}")),
        );
        for base in keys {
            for (field, suffix) in [("name", "_name"), ("description", "_description")] {
                if !entry[field].truthy() {
                    let v = aloc.get(&format!("{base}{suffix}")).cloned().unwrap_or_default();
                    entry.insert(field.into(), J::from(v));
                }
            }
        }
    }
    let mut out = Vec::new();
    for (_, (mut entry, mut tiers, rfs)) in gems {
        tiers.sort();
        tiers.dedup();
        entry.insert("tiers".into(), J::List(tiers.into_iter().map(J::Int).collect()));
        for r in rfs {
            merge_rows(&mut entry, &describe(&prefabs, &r, &kit, ABILITIES));
        }
        for key in ["healing", "effects", "vfx", "actions"] {
            if !entry[key].truthy() {
                entry.shift_remove(key);
            }
        }
        out.push(entry);
    }
    out.sort_by_cached_key(|g| {
        let name = g["name"].as_str().unwrap_or("");
        let name = if name.is_empty() { g["slug"].as_str().unwrap_or("") } else { name };
        (g["element"].as_str().unwrap_or("").to_owned(), name.to_owned())
    });
    Ok(J::List(out.into_iter().map(J::Dict).collect()))
}
