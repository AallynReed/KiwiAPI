//! `cosmetics.py`: costumes, bomb skins and styles.

use indexmap::{IndexMap, IndexSet};

use super::pvp_powerups::all_names;
use crate::decode::{
    ability::Prefabs,
    common::{Locale, lookup},
    json::{J, Map},
    registry::{Built, Ctx},
    value::W,
};
use crate::obj;

pub const CLASSES: [&str; 18] = [
    "Knight",
    "GunSlinger",
    "FaeTrickster",
    "Dracolyte",
    "NeonNinja",
    "CandyBarbarian",
    "IceMage",
    "ShadowHunter",
    "PirateLord",
    "Adventurer",
    "TombRaiser",
    "LunarLancer",
    "SpiritTank",
    "Chloromancer",
    "DinoTamer",
    "CrimeFighter",
    "Bard",
    "Solarion",
];
const SLOTS: [(&str, &str); 9] = [
    ("hat", "Hats"),
    ("face", "Faces"),
    ("weapon_melee", "Melee weapons"),
    ("weapon_pistol", "Guns"),
    ("weapon_staff", "Staffs"),
    ("weapon_bow", "Bows"),
    ("weapon_spear", "Spears"),
    ("weapon_fist", "Fists"),
    ("pvpbanner", "Banners"),
];

fn working(gid: &str) -> bool {
    ["InProgress", "ReadyForGame", "Hidden"].iter().any(|p| gid.starts_with(p))
}

fn split_camel(gid: &str) -> String {
    let chars: Vec<char> = gid.chars().collect();
    let mut out = String::new();
    for (i, c) in chars.iter().enumerate() {
        if i > 0 && chars[i - 1].is_ascii_lowercase() && c.is_ascii_uppercase() {
            out.push(' ');
        }
        out.push(*c);
    }
    out
}

fn groups_of(prefabs: &Prefabs, collection: &str) -> Vec<W> {
    prefabs
        .get(&format!("collections/collection_{collection}"))
        .and_then(|pf| pf.root.as_ref().map(|r| r.leaf().iter_of(0)))
        .unwrap_or_default()
}

fn group_names(prefabs: &Prefabs, collection: &str, names: &Locale) -> IndexMap<String, String> {
    let mut out = IndexMap::new();
    for group in groups_of(prefabs, collection) {
        let g = group.leaf();
        let gid = g.str(0).unwrap_or("");
        if !gid.is_empty() && !out.contains_key(gid) {
            let label = lookup(names, g.get(1).filter(|v| v.truthy()));
            let v = if working(gid) {
                String::new()
            } else if label.is_empty() {
                split_camel(gid)
            } else {
                label
            };
            out.insert(gid.to_owned(), v);
        }
    }
    out
}

fn group_label(g: &crate::decode::value::Dict, names: &Locale) -> String {
    let gid = g.str(0).unwrap_or("");
    if working(gid) {
        return String::new();
    }
    let label = lookup(names, g.get(1).filter(|v| v.truthy()));
    if label.is_empty() { gid.to_owned() } else { label }
}

fn costumes(prefabs: &Prefabs, names: &Locale) -> Vec<J> {
    let mut out: Vec<Map> = Vec::new();
    let mut seen: IndexSet<String> = IndexSet::new();
    for group in groups_of(prefabs, "skin") {
        let g = group.leaf();
        let label = group_label(g, names);
        for row in g.iter_of(3) {
            let Some(sid) = row.leaf().str(0) else { continue };
            if !seen.insert(sid.to_owned()) {
                continue;
            }
            let skin = prefabs.get(&format!("skins/{sid}"));
            let leaf = skin.as_ref().and_then(|s| s.root.as_ref()).map(W::leaf).unwrap_or(&crate::decode::value::EMPTY);
            let name = lookup(names, leaf.get(5).filter(|v| v.truthy()));
            if name.is_empty() {
                continue;
            }
            let mut e = Map::new();
            e.insert("slug".into(), J::from(sid));
            e.insert("name".into(), J::from(name));
            e.insert("description".into(), J::from(lookup(names, leaf.get(6).filter(|v| v.truthy()))));
            e.insert("prefab".into(), J::from(format!("skins/{sid}")));
            if let Some(c) = leaf.int(0).and_then(|c| usize::try_from(c).ok()).and_then(|c| CLASSES.get(c)) {
                e.insert("class".into(), J::from(*c));
            }
            if let Some(bp) = leaf.str(4).filter(|s| !s.is_empty()) {
                e.insert("blueprint".into(), J::from(bp));
            }
            if !label.is_empty() {
                e.insert("group".into(), J::from(label.as_str()));
            }
            out.push(e);
        }
    }
    out.sort_by_cached_key(|e| e["name"].as_str().unwrap_or("").to_lowercase());
    out.into_iter().map(J::Dict).collect()
}

fn bomb_skins(prefabs: &Prefabs, names: &Locale) -> Vec<J> {
    let mut out: Vec<Map> = Vec::new();
    let mut seen: IndexSet<String> = IndexSet::new();
    for group in groups_of(prefabs, "secondaryskin") {
        let g = group.leaf();
        let label = group_label(g, names);
        for row in g.iter_of(3) {
            let Some(sid) = row.leaf().str(0) else { continue };
            if !seen.insert(sid.to_owned()) {
                continue;
            }
            let skin = prefabs.get(&format!("skins/secondary/{sid}"));
            let leaf = skin.as_ref().and_then(|s| s.root.as_ref()).map(W::leaf).unwrap_or(&crate::decode::value::EMPTY);
            let name = lookup(names, leaf.get(3).filter(|v| v.truthy()));
            if name.is_empty() {
                continue;
            }
            let mut e = Map::new();
            e.insert("slug".into(), J::from(sid));
            e.insert("name".into(), J::from(name));
            e.insert("description".into(), J::from(lookup(names, leaf.get(4).filter(|v| v.truthy()))));
            e.insert("prefab".into(), J::from(format!("skins/secondary/{sid}")));
            if let Some(bp) = leaf.str(2).filter(|s| !s.is_empty()) {
                e.insert("blueprint".into(), J::from(bp));
            }
            if !label.is_empty() {
                e.insert("group".into(), J::from(label.as_str()));
            }
            let effects = leaf.get(0).map(|v| v.leaf().iter_of(0)).unwrap_or_default();
            let vfx: Vec<J> = effects
                .iter()
                .filter(|r| r.leaf().str(1).is_some_and(|p| p.to_lowercase().ends_with(".pkfx")))
                .map(|r| obj! {"key" => r.leaf().get(0).map_or(J::Null, J::from), "pkfx" => r.leaf().get(1).map_or(J::Null, J::from)})
                .collect();
            if !vfx.is_empty() {
                e.insert("vfx".into(), J::List(vfx));
            }
            out.push(e);
        }
    }
    out.sort_by_cached_key(|e| e["name"].as_str().unwrap_or("").to_lowercase());
    out.into_iter().map(J::Dict).collect()
}

fn model(prefabs: &Prefabs, r: &str) -> String {
    if r.to_lowercase().ends_with(".blueprint") {
        return r.to_owned();
    }
    if r.starts_with("equipment/") {
        return prefabs
            .get(r)
            .and_then(|p| p.component(37).and_then(|c| c.leaf().str(0)).map(str::to_owned))
            .unwrap_or_default();
    }
    String::new()
}

fn styles(prefabs: &Prefabs, names: &Locale, groups: &IndexMap<String, String>) -> Vec<J> {
    let mut slots = Vec::new();
    for (table, title) in SLOTS {
        let mut rows = Vec::new();
        let pf = prefabs.get(&format!("loot/{table}"));
        for group in pf.as_ref().and_then(|p| p.root.as_ref()).map(|r| r.leaf().iter_of(0)).unwrap_or_default() {
            let g = group.leaf();
            let label = g.str(0).and_then(|gid| groups.get(gid)).cloned().unwrap_or_default();
            for row in g.iter_of(1) {
                let r = row.leaf();
                let name = lookup(names, r.get(1).filter(|v| v.truthy()));
                if name.is_empty() {
                    continue;
                }
                let mut style = Map::new();
                style.insert("name".into(), J::from(name));
                let desc = lookup(names, r.get(2).filter(|v| v.truthy()));
                if !desc.is_empty() {
                    style.insert("description".into(), J::from(desc));
                }
                let m = r.str(0).map(|bp| model(prefabs, bp)).unwrap_or_default();
                if !m.is_empty() {
                    style.insert("blueprint".into(), J::from(m));
                }
                if !label.is_empty() {
                    style.insert("group".into(), J::from(label.as_str()));
                }
                rows.push(J::Dict(style));
            }
        }
        if !rows.is_empty() {
            slots.push(obj! {
                "slug" => title.to_lowercase().replace(' ', "-"), "name" => title, "prefab" => format!("loot/{table}"),
                "styles" => rows,
            });
        }
    }
    slots
}

pub fn build(ctx: &Ctx) -> Built {
    let prefabs = Prefabs::new(&ctx.tree);
    let names = all_names(&ctx.tree);
    let groups = group_names(&prefabs, "equipmentappearance", &names);
    Ok(obj! {
        "costumes" => costumes(&prefabs, &names),
        "bomb_skins" => bomb_skins(&prefabs, &names),
        "style_slots" => styles(&prefabs, &names, &groups),
    })
}

pub fn count(j: &J) -> Option<usize> {
    let n = |k: &str| j.get(k).filter(|v| v.truthy()).map_or(0, J::len);
    let styles: usize = j
        .get("style_slots")
        .filter(|v| v.truthy())
        .and_then(J::as_list)
        .map(|l| l.iter().map(|s| s.get("styles").map_or(0, J::len)).sum())
        .unwrap_or(0);
    Some(n("costumes") + n("bomb_skins") + styles)
}
