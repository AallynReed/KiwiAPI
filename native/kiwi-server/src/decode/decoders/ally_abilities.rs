//! `ally_abilities.py`: allies with the stats and abilities they grant.

use crate::decode::{
    ability::{ABILITIES, Prefabs, component_values, describe, identity, refs, text_keys},
    common::{Locale, collection_members, is_hidden, locale_at, stat_rows, stem},
    json::{J, Map},
    registry::{Built, Ctx},
    value::{W, parse},
};

fn without(row: &J, keys: &[&str]) -> Map {
    row.as_dict()
        .map(|m| m.iter().filter(|(k, _)| !keys.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}

/// `_detail`: what an ability ref does, trimmed to what a reader needs.
pub fn detail_with(prefabs: &Prefabs, r: &str, text: &Locale, prefix: &[&str], vfx: bool) -> Map {
    let info = describe(prefabs, r, &[], prefix);
    let mut out = Map::new();
    for key in ["energy", "cooldown", "proc_cooldown"] {
        if let Some(v) = info.get(key).filter(|v| v.truthy()) {
            out.insert(key.into(), v.clone());
        }
    }
    for key in ["stages", "healing"] {
        let rows: Vec<J> = info
            .get(key)
            .and_then(J::as_list)
            .into_iter()
            .flatten()
            .map(|r| J::Dict(without(r, &["_depth"])))
            .collect();
        if !rows.is_empty() {
            out.insert(key.into(), J::List(rows));
        }
    }
    let mut effects = Vec::new();
    for eff in info.get("effects").and_then(J::as_list).into_iter().flatten() {
        let mut row = without(eff, &["_depth", "name_key", "description_key"]);
        if let Some(t) = text.get(eff.str_or("name_key", "")).filter(|t| !t.is_empty()) {
            row.insert("title".into(), J::from(t.as_str()));
        }
        effects.push(J::Dict(row));
    }
    if !effects.is_empty() {
        out.insert("effects".into(), J::List(effects));
    }
    if vfx && let Some(v) = info.get("vfx").filter(|v| v.truthy()) {
        out.insert("vfx".into(), v.clone());
    }
    out
}

pub fn detail(prefabs: &Prefabs, r: &str, text: &Locale) -> Map {
    detail_with(prefabs, r, text, ABILITIES, true)
}

/// Every `languages/en/prefabs_abilities*` table merged.
pub fn abilities_text(tree: &crate::decode::tree::Tree) -> Locale {
    let mut out = Locale::new();
    for path in tree.files("languages/en/prefabs_abilities", ".binfab") {
        out.extend(locale_at(tree, &path));
    }
    out
}

pub fn build(ctx: &Ctx) -> Built {
    let tree = &ctx.tree;
    let prefabs = Prefabs::new(tree);
    let names = locale_at(tree, "languages/en/prefabs_collections_pet.binfab");
    let text = abilities_text(tree);
    let mut merged = locale_at(tree, "languages/en/prefabs_collections.binfab");
    merged.extend(names.clone());
    let owned = collection_members(prefabs.get("collections/collection_pet").as_deref(), &merged);
    let mut allies: Vec<Map> = Vec::new();
    for path in tree.files("prefabs/collections/pet/", ".binfab") {
        if path.matches('/').count() != 3 {
            continue;
        }
        let Ok(pf) = parse(&tree.read(&path).unwrap_or_default()) else { continue };
        let mut values: Vec<W> = Vec::new();
        component_values(&pf, &mut |v| values.push(v.clone()));
        let stats = stat_rows(&values);
        let slug = stem(&path);
        let member = owned.get(&format!("collections/pet/{slug}"));
        if stats.is_empty() && member.is_none() {
            continue;
        }
        let ident = identity(Some(&pf));
        let name = names.get(ident.name()).cloned().unwrap_or_default();
        let description = names.get(ident.desc()).cloned().unwrap_or_default();
        let mut powers: Vec<Map> = Vec::new();
        for r in refs(&pf, ABILITIES) {
            if is_hidden(&r) {
                continue;
            }
            let Some(apf) = prefabs.get(&r) else { continue };
            let t = text.get(text_keys(Some(&apf)).desc()).cloned().unwrap_or_default();
            if t.is_empty()
                || powers.iter().any(|p| {
                    p.get("ref").and_then(J::as_str) == Some(&r) && p.get("text").and_then(J::as_str) == Some(&t)
                })
            {
                continue;
            }
            let mut row = Map::new();
            row.insert("ref".into(), J::from(r.as_str()));
            row.insert("text".into(), J::from(t));
            row.extend(detail(&prefabs, &r, &text));
            powers.push(row);
        }
        let mut ally = Map::new();
        ally.insert("slug".into(), J::from(slug.as_str()));
        ally.insert("name".into(), J::from(if name.is_empty() { slug.clone() } else { name }));
        ally.insert("description".into(), J::from(description));
        ally.insert("prefab".into(), J::from(format!("collections/pet/{slug}")));
        ally.insert("stats".into(), J::List(stats));
        ally.insert("abilities".into(), J::List(powers.into_iter().map(J::Dict).collect()));
        if let Some((_, group)) = member
            && !group.is_empty()
        {
            ally.insert("group".into(), J::from(group.as_str()));
        }
        allies.push(ally);
    }
    allies.sort_by_cached_key(|a| a["name"].as_str().unwrap_or("").to_lowercase());
    Ok(J::List(allies.into_iter().map(J::Dict).collect()))
}
