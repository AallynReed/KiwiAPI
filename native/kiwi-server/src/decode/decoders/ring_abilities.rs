//! `ring_abilities.py`: class rings and what their implementation chain changes.

use indexmap::IndexMap;

use super::gem_abilities::{class_kit, merge_rows};
use crate::decode::{
    ability::{ABILITIES, Prefabs, describe, identity, refs},
    common::{locale_at, stem},
    json::{J, Map},
    registry::{Built, Ctx},
    tree::Tree,
};

const MODS: &str = "prefabs/abilities/mods_01/";

fn locale_names(tree: &Tree) -> IndexMap<String, (String, String)> {
    let table = locale_at(tree, "languages/en/prefabs_abilities_mods.binfab");
    let mut out = IndexMap::new();
    let head = "$prefabs_abilities_mods_".len();
    for (key, value) in &table {
        if !key.ends_with("_name") {
            continue;
        }
        let chars: Vec<char> = key.chars().collect();
        let slug: String =
            if chars.len() >= head + 5 { chars[head..chars.len() - 5].iter().collect() } else { String::new() };
        let desc = table.get(&format!("$prefabs_abilities_mods_{slug}_description")).cloned().unwrap_or_default();
        out.insert(slug, (value.clone(), desc));
    }
    out
}

pub fn build(ctx: &Ctx) -> Built {
    let tree = &ctx.tree;
    let prefabs = Prefabs::new(tree);
    let names = locale_names(tree);
    let kit = class_kit(&prefabs, tree);
    let mut rings: Vec<Map> = Vec::new();
    for folder in tree.dirs(MODS) {
        let paths: Vec<String> = tree
            .files(&format!("{MODS}{folder}/ring_"), ".binfab")
            .into_iter()
            .filter(|p| p.matches('/').count() == 4)
            .collect();
        let others: Vec<String> = paths.iter().map(|p| format!("abilities/mods_01/{folder}/{}", stem(p))).collect();
        for path in &paths {
            let ring = stem(path);
            let rel = format!("abilities/mods_01/{folder}/{ring}");
            let pf = prefabs.get(&rel);
            let ident = identity(pf.as_deref());
            let (owner, name) = match ident.name().split_once(':') {
                Some((o, n)) => (o, n),
                None => (ident.name(), ""),
            };
            if name.trim().is_empty() || !owner.chars().next().is_some_and(char::is_alphabetic) {
                continue;
            }
            let (mut name, mut description) = (name.trim().to_owned(), ident.desc().trim().to_owned());
            let slug = ring.get("ring_".len()..).unwrap_or("").to_owned();
            for (key, (loc_name, loc_desc)) in &names {
                if *key == slug || key.ends_with(&format!("_{slug}")) {
                    if !loc_name.is_empty() {
                        name = loc_name.clone();
                    }
                    if !loc_desc.is_empty() {
                        description = loc_desc.clone();
                    }
                    break;
                }
            }
            let mut entry = Map::new();
            entry.insert("class".into(), J::from(owner.trim()));
            entry.insert("game_folder".into(), J::from(folder.as_str()));
            entry.insert("name".into(), J::from(name));
            entry.insert("description".into(), J::from(description));
            entry.insert("slug".into(), J::from(slug));
            entry.insert("prefab".into(), J::from(rel.as_str()));
            for k in ["prefabs", "stats", "stages", "healing", "effects", "vfx", "actions"] {
                entry.insert(k.into(), J::List(Vec::new()));
            }
            let mut stop = kit.clone();
            stop.extend(others.iter().filter(|o| **o != rel).cloned());
            for r in pf.as_ref().map(|p| refs(p, ABILITIES)).unwrap_or_default() {
                merge_rows(&mut entry, &describe(&prefabs, &r, &stop, ABILITIES));
            }
            for key in ["healing", "effects", "vfx", "actions"] {
                if !entry[key].truthy() {
                    entry.shift_remove(key);
                }
            }
            rings.push(entry);
        }
    }
    rings.sort_by_cached_key(|r| {
        (r["class"].as_str().unwrap_or("").to_owned(), r["name"].as_str().unwrap_or("").to_owned())
    });
    Ok(J::List(rings.into_iter().map(J::Dict).collect()))
}
