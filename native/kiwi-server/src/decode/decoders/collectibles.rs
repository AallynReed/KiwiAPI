//! `collectibles.py`: wings, boats, sails, auras, magriders, flasks, tomes and
//! fishing poles: what each grants and does.

use indexmap::IndexMap;

use super::{
    ally_abilities::{abilities_text, detail},
    mount_abilities::{MOUNT, ability_refs, stat_groups},
};
use crate::decode::{
    ability::{Prefabs, describe, identity, text_keys},
    common::{Locale, is_hidden, locale_at, lookup, rs, stat_rows, stem, walk_values},
    json::{J, Map},
    registry::{Built, Ctx},
    value::{Prefab, W},
};
use crate::obj;

const PARTICLES: &str = "particles/VFX/Particles/";
const KINDS: [(&str, &str, &[&str]); 8] = [
    ("wings", "wings", &["wings"]),
    ("boats", "boat", &["boat"]),
    ("sails", "sail", &["sail"]),
    ("auras", "aura", &["aura"]),
    ("magriders", "magrider", &["cart"]),
    ("flasks", "flask", &["flask", "flaskeffect"]),
    ("tomes", "tome", &["tome"]),
    ("fishing_poles", "fishing", &["fishingpole"]),
];
const BLUEPRINT: i128 = 37;
const STATS: i128 = 65;
const TAGS: i128 = 106;
const DRINK: i128 = 135;
const ALWAYS_ON: i128 = 149;
const LIQUIDS: [&str; 4] = ["water", "lava", "chocolate", "plasma"];
const AURA_WEAPONS: [(&str, &str); 6] = [
    ("melee", "Melee"),
    ("pistol", "Pistol"),
    ("bow", "Bow"),
    ("staff", "Staff"),
    ("spear", "Spear"),
    ("fist", "Fist"),
];

fn working(gid: &str) -> bool {
    ["InProgress", "ReadyForGame", "Hidden"].iter().any(|p| gid.starts_with(p))
}

fn members(prefabs: &Prefabs, folder: &str, collection: &str, names: &Locale) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let Some(pf) = prefabs.get(&format!("collections/collection_{collection}")) else { return out };
    let want = format!("collections/{folder}/");
    for group in pf.root.as_ref().map(|r| r.leaf().iter_of(0)).unwrap_or_default() {
        let g = group.leaf();
        let gid = g.str(0).unwrap_or("").to_owned();
        let label = if working(&gid) {
            String::new()
        } else {
            let n = lookup(names, g.get(1).filter(|v| v.truthy()));
            if n.is_empty() { gid.clone() } else { n }
        };
        for row in g.iter_of(3) {
            if let Some(r) = row.leaf().str(0)
                && r.starts_with(&want)
            {
                let r = rs(r, ".binfab").to_owned();
                if !out.iter().any(|(x, _)| *x == r) {
                    out.push((r, label.clone()));
                }
            }
        }
    }
    out
}

fn mount_effects(mount: &W) -> Vec<J> {
    let mut out = Vec::new();
    for row in mount.leaf().iter_of(2) {
        let r = row.leaf();
        if let Some(p) = r.str(0)
            && p.to_lowercase().ends_with(".pkfx")
        {
            out.push(obj! {"key" => r.str(1).unwrap_or(""), "pkfx" => p});
        }
    }
    out
}

fn aura_effects(slug: &str, particles: &IndexMap<String, String>) -> Vec<J> {
    let wanted: Vec<(String, String)> = if slug.starts_with("aura_stellar_") {
        vec![("Hat".into(), format!("character_{slug}"))]
    } else {
        let tail = slug.strip_prefix("aura_").unwrap_or(slug);
        AURA_WEAPONS.iter().map(|(w, label)| ((*label).to_owned(), format!("weapon_aura_{w}_{tail}"))).collect()
    };
    wanted
        .into_iter()
        .filter_map(|(label, name)| particles.get(&name).map(|p| obj! {"key" => label, "pkfx" => p.as_str()}))
        .collect()
}

fn part(prefabs: &Prefabs, r: &str) -> Map {
    let info = describe(prefabs, r, &[], &["collections/boat/"]);
    let stages: Vec<J> = info
        .get("stages")
        .and_then(J::as_list)
        .into_iter()
        .flatten()
        .map(|row| {
            J::Dict(
                row.as_dict()
                    .map(|m| m.iter().filter(|(k, _)| *k != "_depth").map(|(k, v)| (k.clone(), v.clone())).collect())
                    .unwrap_or_default(),
            )
        })
        .collect();
    let mut out = Map::new();
    if !stages.is_empty() {
        out.insert("stages".into(), J::List(stages));
    }
    out
}

fn drink(prefabs: &Prefabs, pf: &Prefab, text: &Locale) -> Option<J> {
    let mut rfs: Vec<W> = pf.component(DRINK).map(|c| c.leaf().iter_of(2)).unwrap_or_default();
    rfs.extend(pf.component(ALWAYS_ON).map(|c| c.leaf().iter_of(0)).unwrap_or_default());
    let mut ordered: Vec<String> = Vec::new();
    for r in rfs.iter().filter_map(W::as_str) {
        let r = rs(r, ".binfab").to_owned();
        if !ordered.contains(&r) {
            ordered.push(r);
        }
    }
    let mut out = Map::new();
    for r in ordered {
        if is_hidden(&r) {
            continue;
        }
        let Some(apf) = prefabs.get(&r) else { continue };
        let line = text.get(text_keys(Some(&apf)).desc()).cloned().unwrap_or_default();
        if !line.is_empty() && !out.contains_key("text") {
            out.insert("text".into(), J::from(line));
        }
        for (key, value) in detail(prefabs, &r, text) {
            if let J::List(items) = value {
                let slot = out.entry(key).or_insert_with(|| J::List(Vec::new()));
                if let Some(list) = slot.as_list_mut() {
                    for v in items {
                        if !list.contains(&v) {
                            list.push(v);
                        }
                    }
                }
            } else {
                out.entry(key).or_insert(value);
            }
        }
    }
    if out.is_empty() {
        return None;
    }
    let mut row = Map::new();
    row.insert("name".into(), J::from("Drink"));
    row.extend(out);
    Some(J::Dict(row))
}

fn powers(prefabs: &Prefabs, rfs: &[String], text: &Locale) -> Vec<J> {
    let mut out = Vec::new();
    for r in rfs {
        let Some(apf) = prefabs.get(r) else { continue };
        let ident = identity(Some(&apf));
        let mut row = Map::new();
        row.insert("ref".into(), J::from(r.as_str()));
        let title = text.get(ident.name()).cloned().unwrap_or_default();
        let desc = text.get(ident.desc()).cloned().unwrap_or_default();
        if !title.is_empty() {
            row.insert("name".into(), J::from(title.as_str()));
        }
        if !desc.is_empty() {
            row.insert("text".into(), J::from(desc.as_str()));
        }
        let d = if r.starts_with("collections/") { part(prefabs, r) } else { detail(prefabs, r, text) };
        if !d.is_empty() || !title.is_empty() || !desc.is_empty() {
            row.extend(d);
            out.push(J::Dict(row));
        }
    }
    out
}

fn entry(
    prefabs: &Prefabs,
    r: &str,
    group: &str,
    names: &Locale,
    text: &Locale,
    particles: &IndexMap<String, String>,
) -> Option<J> {
    let pf = prefabs.get(r)?;
    let ident = identity(Some(&pf));
    let name = names.get(ident.name()).filter(|n| !n.is_empty())?.clone();
    let mut e = Map::new();
    let slug = stem(r);
    e.insert("slug".into(), J::from(slug.as_str()));
    e.insert("name".into(), J::from(name));
    e.insert("description".into(), J::from(names.get(ident.desc()).cloned().unwrap_or_default()));
    e.insert("prefab".into(), J::from(r));
    if let Some(bp) = pf.component(BLUEPRINT).and_then(|c| c.leaf().str(0)).filter(|s| !s.is_empty()) {
        e.insert("blueprint".into(), J::from(bp));
    }
    if !group.is_empty() {
        e.insert("group".into(), J::from(group));
    }
    let stats = pf.component(STATS).map(|c| stat_rows(&walk_values(c))).unwrap_or_default();
    if !stats.is_empty() {
        e.insert("equip_stats".into(), J::List(stats));
    }
    let tags: Vec<W> = pf.component(TAGS).map(|c| c.leaf().iter_of(0)).unwrap_or_default();
    let liquids: Vec<J> =
        LIQUIDS.iter().filter(|l| tags.iter().any(|t| t.as_str() == Some(**l))).map(|l| J::from(*l)).collect();
    if !liquids.is_empty() {
        e.insert("liquids".into(), J::List(liquids));
    }
    if r.starts_with("collections/flask/")
        && let Some(d) = drink(prefabs, &pf, text)
    {
        e.insert("abilities".into(), J::List(vec![d]));
    }
    if let Some(mount) = pf.component(MOUNT) {
        let groups = stat_groups(&pf, mount);
        if !groups.is_empty() {
            e.insert("stats".into(), J::List(groups));
        }
        let p = powers(prefabs, &ability_refs(mount), text);
        if !p.is_empty() {
            e.insert("abilities".into(), J::List(p));
        }
        let fx = mount_effects(mount);
        if !fx.is_empty() {
            e.insert("vfx".into(), J::List(fx));
        }
    }
    if r.starts_with("collections/aura/") {
        let fx = aura_effects(&slug, particles);
        if !fx.is_empty() {
            e.insert("vfx".into(), J::List(fx));
        }
    }
    Some(J::Dict(e))
}

pub fn build(ctx: &Ctx) -> Built {
    let tree = &ctx.tree;
    let prefabs = Prefabs::new(tree);
    let names = locale_at(tree, "languages/en/prefabs_collections.binfab");
    let text = abilities_text(tree);
    let mut particles: IndexMap<String, String> = IndexMap::new();
    for path in tree.files(PARTICLES, ".pkfx") {
        let name = path.rsplit('/').next().unwrap_or(&path);
        let chars: Vec<char> = name.chars().collect();
        let base: String = chars[..chars.len().saturating_sub(5)].iter().collect();
        particles.insert(base.to_lowercase(), format!("Particles/{name}"));
    }
    let mut out = Map::new();
    for (key, folder, collections) in KINDS {
        let mut own = names.clone();
        own.extend(locale_at(tree, &format!("languages/en/prefabs_collections_{folder}.binfab")));
        let mut both = own.clone();
        both.extend(text.clone());
        let mut rows = Vec::new();
        for c in collections {
            for (r, group) in members(&prefabs, folder, c, &own) {
                if let Some(e) = entry(&prefabs, &r, &group, &own, &both, &particles) {
                    rows.push(e);
                }
            }
        }
        rows.sort_by_cached_key(|e| e.str_or("name", "").to_lowercase());
        out.insert(key.into(), J::List(rows));
    }
    Ok(J::Dict(out))
}
