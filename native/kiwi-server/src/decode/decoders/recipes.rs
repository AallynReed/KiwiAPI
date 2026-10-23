//! `recipes.py`: every recipe, and the crafting stations that offer them,
//! grouped the way each station's crafting window groups them.

use std::{cell::RefCell, collections::HashMap};

use indexmap::{IndexMap, IndexSet};

use super::{cosmetics::CLASSES, mastery::COLLECTION_TYPES};
use crate::decode::{
    common::{ID_DESCRIPTION, ID_NAME, IDENTITY, Locale, locale_at, r4, rp, rs, stem},
    json::{J, Map, dumps},
    registry::{Built, Ctx},
    sections::COMPONENT_SECTIONS,
    tree::Tree,
    value::{Dict, W, parse, read_object, uvar, zz},
};
use crate::obj;

const STATION_ROOT: &str = "placeable/crafting/";
const CRAFTING: i128 = 61;
const STATION: i128 = 76;
const PLACER: i128 = 50;
const BLUEPRINT: i128 = 37;
const UNLOCKER: i128 = 223;
const ITEM_KINDS: [&str; 4] = ["Prefab", "Claim", "Collectable", "Title"];
const QUANTITY_OPS: [&str; 4] = ["None", "Sum", "Substract", "Multiply"];
const RESETS: [&str; 9] = [
    "NoReset",
    "DailyReset",
    "WeeklyReset",
    "MonthlyReset",
    "SeasonReset",
    "NeverReset",
    "HourlyReset",
    "ThreeHoursReset",
    "SixHoursReset",
];
const SEASONS: [&str; 5] = ["None", "Winter", "Spring", "Summer", "Fall"];
pub const PROFESSIONS: [&str; 7] =
    ["Gardening", "Ringcrafting", "Runecrafting", "Crystallogy", "Mysticism", "MartialArts", "Gearcrafting"];
const RECIPE_TYPE: i128 = 1;
const MATCH: [&str; 3] = ["any", "all", "none"];
const LEVEL_KINDS: [&str; 5] = ["classlevel", "powerrank", "metalevel", "zonelevel", "highestpowerlevel"];
const SETS: [&str; 3] = ["setall", "setany", "setnone"];

fn level_text(k: &str, mode: &J) -> Option<&'static str> {
    match (k, mode.as_int()?) {
        ("metalevel", 0) => Some("$MoreMasteryRequirement"),
        ("metalevel", 1) => Some("$LessMasteryRequirement"),
        ("classlevel", 0) => Some("$HigherLevelRequirement"),
        ("classlevel", 1) => Some("$LowerLevelRequirement"),
        _ => None,
    }
}

fn literal_text(k: &str, mode: &J) -> Option<&'static str> {
    match (k, mode.as_int()?) {
        ("highestpowerlevel", 0) => Some("Highest class power rank {0} or more"),
        ("highestpowerlevel", 1) => Some("Highest class power rank {0} or less"),
        _ => None,
    }
}

pub fn enum_of(table: &[&'static str], v: Option<&W>) -> Option<&'static str> {
    let i = v?.as_int()?;
    usize::try_from(i).ok().and_then(|i| table.get(i)).copied()
}

/// `_rel`: a prefab reference as a logical path, or "" when it is not one.
pub fn rel(path: Option<&W>) -> String {
    match path {
        Some(W::Str(s)) if !s.is_empty() && !s.ends_with(".blueprint") => {
            rs(rp(s.trim(), "prefabs/"), ".binfab").to_owned()
        }
        _ => String::new(),
    }
}

/// `_components`: the wanted components of an entity prefab, parsing only their bytes.
pub fn components(data: &[u8], wanted: &[i128]) -> IndexMap<i128, W> {
    let read = || -> Result<IndexMap<i128, W>, crate::decode::value::WireError> {
        let (_, p) = zz(data, 0, 35)?;
        let (_, p) = zz(data, p, 70)?;
        let (size, mut p) = uvar(data, p, 35)?;
        if p as i128 + size != data.len() as i128 {
            return Ok(IndexMap::new());
        }
        let mut spans: IndexMap<i128, (usize, usize)> = IndexMap::new();
        while p < data.len() {
            let (cid, q) = zz(data, p, 35)?;
            let (n, q) = uvar(data, q, 35)?;
            let n = usize::try_from(n).map_err(|_| crate::decode::value::WireError)?;
            if wanted.contains(&cid) {
                spans.entry(cid).or_insert((q, q + n));
            }
            p = q + n;
        }
        let mut out = IndexMap::new();
        for (cid, (a, b)) in spans {
            let sections = COMPONENT_SECTIONS.iter().find(|(c, _)| *c == cid).map(|(_, s)| *s);
            out.insert(cid, read_object(data, a, b, sections)?);
        }
        Ok(out)
    };
    read().unwrap_or_default()
}

/// Identity name/description keys of referenced prefabs, each file read once.
pub struct Names {
    tree: Tree,
    keys: RefCell<HashMap<String, (String, String)>>,
}

impl Names {
    pub fn new(tree: &Tree) -> Names {
        Names { tree: tree.clone(), keys: RefCell::default() }
    }

    pub fn get(&self, r: &str) -> (String, String) {
        if let Some(hit) = self.keys.borrow().get(r) {
            return hit.clone();
        }
        let comps =
            self.tree.read(&format!("prefabs/{r}.binfab")).map(|d| components(&d, &[IDENTITY])).unwrap_or_default();
        let leaf = comps.get(&IDENTITY).map_or(&crate::decode::value::EMPTY, W::leaf);
        let out = (leaf.str(ID_NAME).unwrap_or("").to_owned(), leaf.str(ID_DESCRIPTION).unwrap_or("").to_owned());
        self.keys.borrow_mut().insert(r.to_owned(), out.clone());
        out
    }
}

/// `_collectable`.
pub fn collectable(obj: &W) -> Map {
    let leaf = obj.leaf();
    let mut out = Map::new();
    let Some(r) = leaf.str(1).filter(|s| !s.is_empty()) else { return out };
    out.insert("ref".into(), J::from(r));
    if let Some(kind) = enum_of(&COLLECTION_TYPES, leaf.get(0)) {
        out.insert("type".into(), J::from(kind));
    }
    out
}

fn item(obj: &W, names: &Names, keys: &mut IndexSet<String>, result: bool) -> Option<J> {
    let leaf = obj.leaf();
    if leaf.is_empty() {
        return None;
    }
    let mut out = Map::new();
    if result {
        let kind = enum_of(&ITEM_KINDS, Some(leaf.get(4).unwrap_or(&W::Int(0))))
            .map(J::from)
            .unwrap_or_else(|| leaf.get(4).map_or(J::Null, J::from));
        out.insert("kind".into(), kind);
    }
    let path = rel(leaf.get(0));
    if !path.is_empty() {
        out.insert("path".into(), J::from(path.as_str()));
    }
    if let Some(c) = leaf.str(2).filter(|s| !s.is_empty()) {
        out.insert("claim".into(), J::from(c));
    }
    let coll = leaf.get(3).map(collectable).unwrap_or_default();
    let r = coll.get("ref").and_then(J::as_str).unwrap_or("").to_owned();
    if !coll.is_empty() {
        out.insert("collectable".into(), J::Dict(coll));
    }
    let named = if !path.is_empty() {
        path
    } else if r.contains('/') {
        rel(Some(&W::Str(r.as_str().into())))
    } else {
        String::new()
    };
    if !named.is_empty() {
        let key = names.get(&named).0;
        if !key.is_empty() {
            out.insert("name".into(), J::from(key.as_str()));
            keys.insert(key);
        }
    }
    if let Some(a) = leaf.int(1) {
        out.insert("amount".into(), J::Int(a));
    }
    let op = leaf.get(7).cloned().unwrap_or(W::Int(0));
    if op.truthy() {
        let label = enum_of(&QUANTITY_OPS, Some(&op)).map(J::from).unwrap_or_else(|| J::from(&op));
        out.insert(
            "per_craft".into(),
            obj! {"op" => label, "factor" => r4(leaf.get(6).and_then(W::as_num).unwrap_or(0.0))},
        );
    }
    out.keys().any(|k| k != "kind" && k != "amount").then_some(J::Dict(out))
}

fn bounds(own: &Dict, lo: i128, hi: i128, mode_i: i128) -> Vec<(&'static str, J)> {
    let mode = own.get(mode_i).map_or(J::Int(0), J::from);
    let mut out = Vec::new();
    if mode == J::Int(0) || mode == J::Int(2) {
        out.push(("min", own.get(lo).map_or(J::Int(0), J::from)));
    }
    if mode == J::Int(1) || mode == J::Int(2) {
        out.push(("max", own.get(hi).map_or(J::Int(0), J::from)));
    }
    out
}

/// `str()` of a scalar.
fn py_str(v: &J) -> String {
    match v {
        J::Int(i) => i.to_string(),
        J::Float(f) => crate::pyfloat::repr(*f),
        J::Str(s) => s.clone(),
        J::Bool(b) => if *b { "True" } else { "False" }.into(),
        J::Null => "None".into(),
        other => dumps(other, None),
    }
}

/// `_requirement`: a requirement record; `"text"` may be a `[fmt, value]` pair
/// (Python's tuple) for the caller to fill in.
pub fn requirement(node: Option<&W>, keys: &mut IndexSet<String>) -> Option<Map> {
    let leaf = node?.leaf();
    let kind = leaf.str(0)?;
    let k = kind.to_lowercase();
    if matches!(k.as_str(), "" | "null" | "none") {
        return None;
    }
    let mut out = Map::new();
    out.insert("kind".into(), J::from(k.as_str()));
    let mut own = &crate::decode::value::EMPTY;
    if let Some(params) = leaf.get(1).filter(|p| p.is_obj() && !p.is_empty()) {
        if let Some(msg) = params.section(0).str(0).filter(|m| m.starts_with('$')) {
            out.insert("message".into(), J::from(msg));
            keys.insert(msg.to_owned());
        }
        if params.sections().len() > 1 {
            own = params.leaf();
        }
    }
    if LEVEL_KINDS.contains(&k.as_str()) {
        for (b, v) in bounds(own, 0, 1, 2) {
            out.insert(b.into(), v);
        }
        let mode = own.get(2).map_or(J::Int(0), J::from);
        let at = if mode == J::Int(0) { 0 } else { 1 };
        let value = own.get(at).map_or(J::Int(0), J::from);
        if let Some(fmt) = level_text(&k, &mode) {
            out.insert("text".into(), J::List(vec![J::from(fmt), value]));
            keys.insert(fmt.to_owned());
        } else if let Some(lit) = literal_text(&k, &mode) {
            out.insert("text".into(), J::from(lit.replace("{0}", &py_str(&value))));
        }
    } else if k == "profession" {
        if let Some(p) = enum_of(&PROFESSIONS, own.get(0)) {
            out.insert("profession".into(), J::from(p));
        }
        for (b, v) in bounds(own, 1, 2, 3) {
            let f = match v {
                J::Int(i) => i as f64,
                J::Float(f) => f,
                _ => 0.0,
            };
            out.insert(b.into(), J::Float(r4(f)));
        }
    } else if k == "hastitle" || k == "hastitleequipped" {
        out.insert("titles".into(), J::List(own.iter_of(0).iter().filter_map(W::as_str).map(J::from).collect()));
        if k == "hastitle" {
            out.insert("match".into(), J::from(enum_of(&MATCH, Some(own.get(1).unwrap_or(&W::Int(0))))));
        }
    } else if k == "hascollection" {
        let cs: Vec<J> = own.iter_of(0).iter().map(collectable).filter(|c| !c.is_empty()).map(J::Dict).collect();
        out.insert("collectables".into(), J::List(cs));
        out.insert("match".into(), J::from(enum_of(&MATCH, Some(own.get(1).unwrap_or(&W::Int(0))))));
    } else if k == "activeclass" {
        let classes: Vec<J> = own
            .iter_of(0)
            .iter()
            .filter(|c| matches!(c, W::Int(_)))
            .map(|c| enum_of(&CLASSES, Some(c)).map(J::from).unwrap_or_else(|| J::from(c)))
            .collect();
        out.insert("classes".into(), J::List(classes));
    } else if k == "tags" || k == "zonetag" {
        out.insert("tags".into(), J::List(own.iter_of(0).iter().filter_map(W::as_str).map(J::from).collect()));
    } else if SETS.contains(&k.as_str()) {
        let of: Vec<J> = own
            .iter_of(0)
            .iter()
            .filter_map(|c| requirement(Some(c), keys))
            .filter(|r| !r.is_empty())
            .map(J::Dict)
            .collect();
        out.insert("of".into(), J::List(of));
    } else if k == "recipe" {
        let applies: Vec<J> = [("player", 0), ("workbench", 1)]
            .iter()
            .filter(|(_, i)| own.get(*i).is_some_and(W::truthy))
            .map(|(w, _)| J::from(*w))
            .collect();
        out.insert("applies_to".into(), J::List(applies));
        if let Some(child) = requirement(own.get(2), keys).filter(|c| !c.is_empty()) {
            out.insert("condition".into(), J::Dict(child));
        }
    }
    Some(out)
}

fn recipe(root: &W, names: &Names, keys: &mut IndexSet<String>) -> Map {
    let leaf = root.leaf();
    let mut out = Map::new();
    let results: Vec<J> = leaf.iter_of(1).iter().filter_map(|o| item(o, names, keys, true)).collect();
    out.insert("results".into(), J::List(results));
    let ingredients: Vec<J> = leaf.iter_of(0).iter().filter_map(|o| item(o, names, keys, false)).collect();
    out.insert("ingredients".into(), J::List(ingredients));
    if let Some(t) = leaf.get(2).and_then(W::as_float).filter(|t| *t > 0.0) {
        out.insert("craft_time".into(), J::Float(r4(t)));
    }
    if let Some(req) = requirement(leaf.get(10), keys).filter(|r| !r.is_empty()) {
        out.insert("requirement".into(), J::Dict(req));
    }
    let reset = leaf.get(13).cloned().unwrap_or(W::Int(0));
    if let Some(limit) = leaf.get(14).map_or(Some(0), W::as_int)
        && limit > 0
        && reset.truthy()
    {
        let r = enum_of(&RESETS, Some(&reset)).map(J::from).unwrap_or_else(|| J::from(&reset));
        out.insert("limit".into(), obj! {"count" => limit, "reset" => r});
    }
    let season = leaf.get(20).cloned().unwrap_or(W::Int(0));
    if season.truthy() {
        out.insert("season".into(), enum_of(&SEASONS, Some(&season)).map(J::from).unwrap_or_else(|| J::from(&season)));
    }
    out
}

/// `_tabs`: a crafting component's tabs.
pub fn tabs(crafting: &W) -> Vec<(J, Vec<String>)> {
    crafting
        .leaf()
        .iter_of(14)
        .iter()
        .map(|tab| {
            let leaf = tab.leaf();
            let ids: Vec<String> = leaf
                .iter_of(1)
                .iter()
                .filter_map(|r| r.as_str().filter(|s| !s.is_empty()).map(str::to_owned))
                .collect();
            (J::from(leaf.str(0).unwrap_or("")), ids)
        })
        .collect()
}

/// `_tiers`: a profession's tiers.
pub fn tiers(tree: &Tree, profession: &str) -> Vec<(J, J, Vec<String>)> {
    let Ok(pf) =
        parse(&tree.read(&format!("prefabs/professions/{}.binfab", profession.to_lowercase())).unwrap_or_default())
    else {
        return Vec::new();
    };
    let Some(root) = &pf.root else { return Vec::new() };
    root.leaf()
        .iter_of(0)
        .iter()
        .map(|tier| {
            let leaf = tier.leaf();
            let ids: Vec<String> = leaf
                .iter_of(2)
                .iter()
                .filter_map(|r| r.as_str().filter(|s| !s.is_empty()).map(str::to_owned))
                .collect();
            let name = leaf.get(0).filter(|v| v.truthy()).map_or(J::from(""), J::from);
            (name, leaf.get(1).map_or(J::Int(0), J::from), ids)
        })
        .collect()
}

/// `_label`: a locale key, an `@literal`, or plain text.
pub fn label(key: &J, text: &Locale) -> String {
    let Some(k) = key.as_str().filter(|k| !k.is_empty()) else { return String::new() };
    if k.starts_with('$') {
        return text.get(k).cloned().unwrap_or_default();
    }
    rp(k, "@").to_owned()
}

fn texts(tree: &Tree, keys: &IndexSet<String>) -> Locale {
    let mut out = Locale::new();
    for path in tree.files("languages/en/", ".binfab") {
        for (k, v) in locale_at(tree, &path) {
            if keys.contains(&k) {
                out.entry(k).or_insert(v);
            }
        }
    }
    out
}

struct Group {
    name: J,
    unlock_skill: Option<J>,
    recipes: Vec<String>,
}

pub fn build(ctx: &Ctx) -> Built {
    let tree = &ctx.tree;
    let names = Names::new(tree);
    let mut keys: IndexSet<String> = IndexSet::new();
    let mut known: IndexMap<String, String> = IndexMap::new();
    for p in tree.files("prefabs/recipes/", ".binfab") {
        let s = stem(&p);
        known.insert(s.to_lowercase(), s);
    }
    let mut recipes: IndexMap<String, Map> = IndexMap::new();
    for rid in known.values() {
        let Ok(pf) = parse(&tree.read(&format!("prefabs/recipes/{rid}.binfab")).unwrap_or_default()) else { continue };
        if let Some(root) = &pf.root {
            let r = recipe(root, &names, &mut keys);
            recipes.insert(rid.clone(), r);
        }
    }
    let resolve = |ids: &[String], recipes: &IndexMap<String, Map>| -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for i in ids {
            if let Some(k) = known.get(&i.to_lowercase())
                && recipes.contains_key(k)
                && !out.contains(k)
            {
                out.push(k.clone());
            }
        }
        out
    };

    let mut placer: IndexMap<String, String> = IndexMap::new();
    let mut teaches: IndexMap<String, Vec<String>> = IndexMap::new();
    let mut collection_unlockers: IndexMap<String, Vec<String>> = IndexMap::new();
    let mut stations_raw: Vec<(String, IndexMap<i128, W>)> = Vec::new();
    let mut others: Vec<Map> = Vec::new();
    for path in tree.files("prefabs/", ".binfab") {
        let r = rel(Some(&W::Str(path.as_str().into())));
        let mut wanted = vec![CRAFTING, STATION, UNLOCKER];
        if r.starts_with(STATION_ROOT) {
            wanted.extend([PLACER, BLUEPRINT]);
        }
        let comps = components(&tree.read(&path).unwrap_or_default(), &wanted);
        if r.starts_with(STATION_ROOT)
            && let Some(p) = comps.get(&PLACER)
        {
            for s in p.strings() {
                let t = rel(Some(&W::Str(s.clone())));
                if t.starts_with(STATION_ROOT) {
                    placer.entry(t).or_insert_with(|| r.clone());
                }
            }
        }
        if let Some(u) = comps.get(&UNLOCKER) {
            let u = u.leaf();
            for c in u.iter_of(8).iter().map(collectable) {
                if c.get("type").and_then(J::as_str) == Some("Recipe") {
                    let k = c.get("ref").and_then(J::as_str).unwrap_or("").to_lowercase();
                    teaches.entry(k).or_default().push(r.clone());
                }
            }
            if u.iter_of(7).iter().any(|t| J::from(t) == J::Int(RECIPE_TYPE)) {
                for cat in u.iter_of(6) {
                    if let Some(c) = cat.as_str().filter(|c| !c.is_empty()) {
                        collection_unlockers.entry(c.to_owned()).or_default().push(r.clone());
                    }
                }
            }
        }
        if let Some(crafting) = comps.get(&CRAFTING) {
            if r.starts_with(STATION_ROOT) {
                stations_raw.push((r.clone(), comps.clone()));
            } else {
                let all: Vec<String> = tabs(crafting).into_iter().flat_map(|(_, ids)| ids).collect();
                let ids = resolve(&all, &recipes);
                if !ids.is_empty() {
                    let title = comps.get(&STATION).and_then(|s| s.leaf().get(0)).filter(|t| t.truthy());
                    let name = title.map_or(J::from(""), J::from);
                    let mut o = Map::new();
                    o.insert("prefab".into(), J::from(r.as_str()));
                    o.insert("name".into(), name.clone());
                    o.insert("recipes".into(), J::from(ids));
                    others.push(o);
                    if let Some(s) = name.as_str() {
                        keys.insert(s.to_owned());
                    }
                }
            }
        }
    }
    let teach_rows: Vec<(String, String)> = recipes
        .iter()
        .flat_map(|(other, orec)| {
            orec.get("results").and_then(J::as_list).into_iter().flatten().filter_map(move |res| {
                let c = res.get("collectable")?;
                (c.get("type").and_then(J::as_str) == Some("Recipe"))
                    .then(|| (c.str_or("ref", "").to_lowercase(), format!("recipes/{other}")))
            })
        })
        .collect();
    for (k, v) in teach_rows {
        teaches.entry(k).or_default().push(v);
    }
    for (rid, rec) in recipes.iter_mut() {
        if let Some(by) = teaches.get(&rid.to_lowercase()).filter(|b| !b.is_empty()) {
            let mut seen: IndexSet<String> = IndexSet::new();
            let list: Vec<String> = by.iter().filter(|b| seen.insert((*b).clone())).cloned().collect();
            rec.insert("unlocked_by".into(), J::from(list));
        }
    }

    let mut collections: IndexMap<String, Map> = IndexMap::new();
    let cats = parse(&tree.read("prefabs/collections/collection_recipe.binfab").unwrap_or_default())
        .ok()
        .and_then(|pf| pf.root.as_ref().and_then(|r| r.leaf().get(0).filter(|v| v.truthy()).cloned()))
        .map(|v| v.iter())
        .unwrap_or_default();
    for cat in cats {
        let leaf = cat.leaf();
        let Some(cname) = leaf.str(0) else { continue };
        let name = leaf.get(1).filter(|v| v.truthy()).map_or(J::from(cname), J::from);
        if let Some(s) = name.as_str() {
            keys.insert(s.to_owned());
        }
        let mut entry = Map::new();
        entry.insert("name".into(), name);
        if let Some(u) = collection_unlockers.get(cname).filter(|u| !u.is_empty()) {
            entry.insert("unlockers".into(), J::from(u.clone()));
        }
        collections.insert(cname.to_owned(), entry);
        for e in leaf.iter_of(3) {
            if !e.is_obj() {
                continue;
            }
            if let Some(rid) = e.leaf().str(0)
                && let Some(k) = known.get(&rid.to_lowercase())
                && let Some(rec) = recipes.get_mut(k)
            {
                rec.entry("collection".to_owned()).or_insert_with(|| J::from(cname));
            }
        }
    }

    let mut stations: Vec<(Map, Vec<Group>)> = Vec::new();
    for (r, comps) in &stations_raw {
        let st = comps.get(&STATION).map_or(&crate::decode::value::EMPTY, W::leaf);
        let prof = enum_of(&PROFESSIONS, Some(st.get(2).unwrap_or(&W::Int(-1))));
        let mut groups: Vec<Group> = match prof {
            Some(p) => tiers(tree, p)
                .into_iter()
                .map(|(n, u, ids)| Group { name: n, unlock_skill: Some(u), recipes: ids })
                .collect(),
            None => tabs(&comps[&CRAFTING])
                .into_iter()
                .map(|(n, ids)| Group { name: n, unlock_skill: None, recipes: ids })
                .collect(),
        };
        for g in groups.iter_mut() {
            g.recipes = resolve(&g.recipes, &recipes);
            if let Some(s) = g.name.as_str() {
                keys.insert(s.to_owned());
            }
        }
        groups.retain(|g| !g.recipes.is_empty());
        let item = placer.get(r).cloned().unwrap_or_default();
        let (name_key, desc_key) = if item.is_empty() { (String::new(), String::new()) } else { names.get(&item) };
        let title = st.get(0).filter(|v| v.truthy()).map_or(J::from(name_key.as_str()), J::from);
        let mut row = Map::new();
        row.insert("slug".into(), J::from(rp(r, STATION_ROOT).replace('/', "_")));
        row.insert("name".into(), title.clone());
        row.insert("description".into(), J::from(desc_key.as_str()));
        row.insert("prefab".into(), J::from(r.as_str()));
        if let Some(s) = title.as_str() {
            keys.insert(s.to_owned());
        }
        keys.insert(desc_key);
        if !item.is_empty() {
            row.insert("item".into(), J::from(item));
        }
        if let Some(bp) = comps.get(&BLUEPRINT).and_then(|b| b.leaf().str(0)).filter(|s| !s.is_empty()) {
            row.insert("blueprint".into(), J::from(bp));
        }
        if let Some(p) = prof {
            row.insert("profession".into(), J::from(p));
            keys.insert(format!("$Profession_DisplayName_{p}"));
        }
        stations.push((row, groups));
    }
    keys.shift_remove("");
    let text = texts(tree, &keys);
    Ok(finish(recipes, stations, others, collections, &text))
}

fn named(rows: &mut [J], text: &Locale) {
    for r in rows.iter_mut() {
        let Some(m) = r.as_dict_mut() else { continue };
        if let Some(n) = m.get("name") {
            let label = n.as_str().and_then(|k| text.get(k)).cloned().unwrap_or_default();
            if label.is_empty() {
                m.shift_remove("name");
            } else {
                m.insert("name".into(), J::from(label));
            }
        }
    }
}

fn req_text(req: &mut Map, text: &Locale) {
    if let Some(msg) = req.get("message") {
        let t = msg.as_str().and_then(|k| text.get(k)).cloned().unwrap_or_default();
        if t.is_empty() {
            req.shift_remove("message");
        } else {
            req.insert("message".into(), J::from(t));
        }
    }
    if let Some(J::List(pair)) = req.get("text").cloned() {
        let fmt = pair.first().and_then(J::as_str).unwrap_or("");
        match text.get(fmt).filter(|t| !t.is_empty()) {
            Some(t) => {
                let v = pair.get(1).map(py_str).unwrap_or_default();
                req.insert("text".into(), J::from(t.replace("{0}", &v)));
            }
            None => {
                req.shift_remove("text");
            }
        }
    }
    if let Some(J::List(of)) = req.get_mut("of") {
        for child in of.iter_mut() {
            if let Some(c) = child.as_dict_mut() {
                req_text(c, text);
            }
        }
    }
    if let Some(J::Dict(c)) = req.get_mut("condition") {
        req_text(c, text);
    }
}

type Groups = Vec<(String, Vec<String>)>;

fn finish(
    mut recipes: IndexMap<String, Map>,
    stations: Vec<(Map, Vec<Group>)>,
    mut others: Vec<Map>,
    mut collections: IndexMap<String, Map>,
    text: &Locale,
) -> J {
    for rec in recipes.values_mut() {
        for key in ["results", "ingredients"] {
            if let Some(J::List(rows)) = rec.get_mut(key) {
                named(rows, text);
            }
        }
        if let Some(J::Dict(req)) = rec.get_mut("requirement") {
            req_text(req, text);
        }
    }
    for c in collections.values_mut() {
        let name = c.get("name").cloned().unwrap_or(J::Null);
        let l = label(&name, text);
        let v = if l.is_empty() { J::from(rp(name.as_str().unwrap_or(""), "$")) } else { J::from(l) };
        c.insert("name".into(), v);
    }
    let mut offered: IndexMap<String, Vec<String>> = IndexMap::new();
    let mut kept: Vec<(Map, Groups)> = Vec::new();
    for (mut st, groups) in stations {
        let name = label(st.get("name").unwrap_or(&J::Null), text);
        st.insert("name".into(), J::from(name.as_str()));
        let desc = label(st.get("description").unwrap_or(&J::Null), text);
        st.insert("description".into(), J::from(desc.as_str()));
        let groups: Vec<(String, Option<J>, Vec<String>)> =
            groups.into_iter().map(|g| (label(&g.name, text), g.unlock_skill, g.recipes)).collect();
        let mut glist = Vec::new();
        for (n, u, ids) in &groups {
            let mut m = Map::new();
            m.insert("name".into(), J::from(n.as_str()));
            if let Some(u) = u {
                m.insert("unlock_skill".into(), u.clone());
            }
            m.insert("recipes".into(), J::from(ids.clone()));
            glist.push(J::Dict(m));
        }
        st.insert("groups".into(), J::List(glist));
        if let Some(p) = st.get("profession").and_then(J::as_str).map(str::to_owned) {
            let pn = text.get(&format!("$Profession_DisplayName_{p}")).cloned().unwrap_or(p);
            st.insert("profession_name".into(), J::from(pn));
        }
        if name.is_empty() || groups.is_empty() {
            continue;
        }
        if desc.is_empty() {
            st.shift_remove("description");
        }
        let slug = st["slug"].as_str().unwrap_or("").to_owned();
        for (_, _, ids) in &groups {
            for rid in ids {
                let list = offered.entry(rid.clone()).or_default();
                if !list.contains(&slug) {
                    list.push(slug.clone());
                }
            }
        }
        kept.push((st, groups.into_iter().map(|(n, _, ids)| (n, ids)).collect()));
    }
    let sig = |groups: &[(String, Vec<String>)]| {
        dumps(
            &J::List(groups.iter().map(|(n, ids)| J::List(vec![J::from(n.as_str()), J::from(ids.clone())])).collect()),
            None,
        )
    };
    let mut shape: IndexMap<String, Vec<String>> = IndexMap::new();
    for (st, groups) in &kept {
        shape.entry(sig(groups)).or_default().push(st["slug"].as_str().unwrap_or("").to_owned());
    }
    let mut kept: Vec<Map> = kept
        .into_iter()
        .map(|(mut st, groups)| {
            let slug = st["slug"].as_str().unwrap_or("").to_owned();
            let twins: Vec<String> = shape[&sig(&groups)].iter().filter(|s| **s != slug).cloned().collect();
            if !twins.is_empty() {
                st.insert("same_as".into(), J::from(twins));
            }
            st
        })
        .collect();
    let mut elsewhere: IndexSet<String> = IndexSet::new();
    for o in others.iter_mut() {
        let n = label(o.get("name").unwrap_or(&J::Null), text);
        if n.is_empty() {
            o.shift_remove("name");
        } else {
            o.insert("name".into(), J::from(n));
        }
        for r in o.get("recipes").and_then(J::as_list).into_iter().flatten() {
            if let Some(s) = r.as_str() {
                elsewhere.insert(s.to_owned());
            }
        }
    }
    for (rid, rec) in recipes.iter_mut() {
        if let Some(st) = offered.get(rid) {
            rec.insert("stations".into(), J::from(st.clone()));
        }
    }
    kept.sort_by_cached_key(|s| {
        (s["name"].as_str().unwrap_or("").to_lowercase(), s["slug"].as_str().unwrap_or("").to_owned())
    });
    let mut not_offered: Vec<String> =
        recipes.keys().filter(|r| !offered.contains_key(*r) && !elsewhere.contains(*r)).cloned().collect();
    not_offered.sort();
    recipes.sort_keys();
    obj! {
        "stations" => J::List(kept.into_iter().map(J::Dict).collect()),
        "other_offerers" => J::List(others.into_iter().map(J::Dict).collect()),
        "collections" => J::Dict(collections.into_iter().map(|(k, v)| (k, J::Dict(v))).collect()),
        "recipes" => J::Dict(recipes.into_iter().map(|(k, v)| (k, J::Dict(v))).collect()),
        "not_offered" => not_offered,
    }
}
