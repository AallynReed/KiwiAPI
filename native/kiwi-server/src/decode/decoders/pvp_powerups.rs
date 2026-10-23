//! `pvp_powerups.py`: Bomber Royale swaps and the PvP arena consumables.

use std::sync::OnceLock;

use regex::Regex;

use super::ally_abilities::detail_with;
use crate::decode::{
    ability::{Prefabs, identity},
    common::{Locale, locale_at},
    json::{J, Map},
    registry::{Built, Ctx},
};

const GROUPS: [(&str, &str); 5] = [
    ("weaponswap", "Weapons"),
    ("bombswap", "Bombs"),
    ("mobility", "Mobility"),
    ("terraswap", "Terrain"),
    ("consumable", "Consumables"),
];

fn tier_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"_t(\d)$").expect("regex"))
}

/// Every `languages/en/` table, first key wins.
pub fn all_names(tree: &crate::decode::tree::Tree) -> Locale {
    let mut names = Locale::new();
    for path in tree.files("languages/en/", ".binfab") {
        for (k, v) in locale_at(tree, &path) {
            names.entry(k).or_insert(v);
        }
    }
    names
}

pub fn build(ctx: &Ctx) -> Built {
    let tree = &ctx.tree;
    let prefabs = Prefabs::new(tree);
    let names = all_names(tree);
    let mut rows: Vec<Map> = Vec::new();
    for path in tree.files("prefabs/pvp/", ".binfab") {
        let rel = &path["prefabs/".len()..path.len() - ".binfab".len()];
        let stem = rel.rsplit('/').next().unwrap_or(rel);
        let mode = if rel.contains("/battleroyale/") {
            "battleroyale"
        } else if stem.starts_with("pvp_consumable_") {
            "arena"
        } else {
            continue;
        };
        let ident = identity(prefabs.get(rel).as_deref());
        let Some(name) = names.get(ident.name()).filter(|n| !n.is_empty()).cloned() else { continue };
        let padded = format!("_{stem}_");
        let kind = GROUPS.iter().find(|(g, _)| padded.contains(&format!("_{g}_"))).map_or("consumable", |(g, _)| g);
        let group = GROUPS.iter().find(|(g, _)| *g == kind).map_or("", |(_, n)| n);
        let tier = tier_re().captures(stem).and_then(|m| m[1].parse::<i128>().ok()).unwrap_or(1);
        let mut row = Map::new();
        row.insert("slug".into(), J::from(stem));
        row.insert("name".into(), J::from(name));
        row.insert("description".into(), J::from(names.get(ident.desc()).cloned().unwrap_or_default()));
        row.insert("prefab".into(), J::from(rel));
        row.insert("mode".into(), J::from(mode));
        row.insert("group".into(), J::from(group));
        row.insert("base".into(), J::from(tier_re().replace(stem, "").into_owned()));
        row.insert("tier".into(), J::Int(tier));
        if let Some(icon) = &ident.icon {
            row.insert("icon".into(), J::from(icon.as_str()));
        }
        row.extend(detail_with(&prefabs, rel, &names, &["abilities/", "pvp/", "effects/"], false));
        rows.push(row);
    }
    rows.sort_by_cached_key(|r| {
        (
            r["mode"].as_str().unwrap_or("").to_owned(),
            r["group"].as_str().unwrap_or("").to_owned(),
            r["base"].as_str().unwrap_or("").to_owned(),
            r["tier"].as_int().unwrap_or(0),
        )
    });
    Ok(crate::obj! {"powerups" => J::List(rows.into_iter().map(J::Dict).collect())})
}
