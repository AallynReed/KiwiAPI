//! `fish.py`: every fish (weights, trophy sizes, trophies, deconstruction) and
//! the lure tables.

use std::{cell::RefCell, collections::HashMap};

use indexmap::{IndexMap, IndexSet};

use crate::decode::{
    ability::{Prefabs, identity},
    common::{EFFECT_DURATION, ID_DESCRIPTION, ID_NAME, IDENTITY, Locale, locale_at, rp, rs},
    json::{J, Map},
    registry::{Built, Ctx},
    tree::Tree,
    value::{Dict, Prefab, W},
};
use crate::obj;

const ID_RARITY: i128 = 9;
const ID_ITEM_TYPE: i128 = 10;
const TAGS: i128 = 106;
const BLUEPRINT: i128 = 37;
const USE_EFFECTS: i128 = 398;
const SPLITTER: i128 = 46;
const BAIT: i128 = 1017;
const CONDITIONS: i128 = 239;
const ITEM_RARITY: [&str; 7] = ["Common", "Uncommon", "Rare", "Epic", "Legendary", "Relic", "Resplendent"];
const SIZE_KEYS: [&str; 4] = ["$FishSize_Small", "$FishSize_Medium", "$FishSize_Large", "$FishSize_ExtraLarge"];
const SIZE_SUFFIX: [&str; 4] = ["", "_Basic", "_Silver", "_Gold"];
const RARITY_TYPES: [&str; 9] =
    ["Default", "Good", "Great", "Epic", "Legendary", "RarityBait1", "RarityBait2", "RarityBait3", "AlwaysRare"];
const TROPHY_TYPES: [&str; 8] =
    ["Default", "Good", "Great", "Epic", "TrophyBait1", "TrophyBait2", "TrophyBait3", "AlwaysTrophy"];
const SPEED_TYPES: [&str; 8] =
    ["Default", "Optimal", "Nonoptimal", "DefaultBait", "SpeedBait1", "SpeedBait2", "SpeedBait3", "Debug"];
const FISH_VERSION: i32 = 1;

fn rows(pf: Option<&Prefab>, idx: i128) -> Vec<Dict> {
    let Some(root) = pf.and_then(|p| p.root.as_ref()) else { return Vec::new() };
    root.leaf().iter_of(idx).iter().filter(|r| r.is_obj()).map(|r| clone_dict(r.leaf())).collect()
}

fn clone_dict(d: &Dict) -> Dict {
    Dict::new(d.items().to_vec())
}

fn enum_of<'a>(names: &[&'a str], v: Option<&W>) -> Option<&'a str> {
    let i = v?.as_int()?;
    usize::try_from(i).ok().and_then(|i| names.get(i)).copied()
}

/// Locale lookups that follow where the game files each key.
pub struct Text {
    tree: Tree,
    tables: RefCell<HashMap<String, Locale>>,
}

impl Text {
    pub fn new(tree: &Tree) -> Text {
        Text { tree: tree.clone(), tables: RefCell::default() }
    }

    pub fn table(&self, name: &str) -> Locale {
        if let Some(t) = self.tables.borrow().get(name) {
            return t.clone();
        }
        let t = locale_at(&self.tree, &format!("languages/en/{name}.binfab"));
        self.tables.borrow_mut().insert(name.to_owned(), t.clone());
        t
    }

    fn lookup(&self, name: &str, key: &str) -> Option<String> {
        if let Some(t) = self.tables.borrow().get(name) {
            return t.get(key).cloned();
        }
        self.table(name).get(key).cloned()
    }

    pub fn get(&self, rel: &str, key: Option<&str>) -> String {
        let Some(key) = key.filter(|k| !k.is_empty()) else { return String::new() };
        if let Some(lit) = key.strip_prefix('@') {
            return lit.to_owned();
        }
        let parts: Vec<&str> = key.trim_start_matches('$').split('_').collect();
        let dirs: Vec<&str> = {
            let mut d: Vec<&str> = rel.split('/').collect();
            d.pop();
            d
        };
        let mut names: Vec<String> = (1..parts.len()).rev().map(|n| parts[..n].join("_")).collect();
        names.extend((1..=dirs.len()).rev().map(|n| format!("prefabs_{}", dirs[..n].join("_"))));
        for name in names {
            if self.tree.exists(&format!("languages/en/{name}.binfab"))
                && let Some(t) = self.lookup(&name, key).filter(|t| !t.is_empty())
            {
                return t;
            }
        }
        let sibling = format!("languages/en/prefabs_{}", dirs[..dirs.len().min(2)].join("_"));
        for path in self.tree.files(&sibling, ".binfab") {
            let name = &path["languages/en/".len()..path.len() - ".binfab".len()];
            if let Some(t) = self.lookup(name, key).filter(|t| !t.is_empty()) {
                return t;
            }
        }
        String::new()
    }
}

fn named(prefabs: &Prefabs, text: &Text, rel: &str) -> Map {
    let ident = identity(prefabs.get(rel).as_deref());
    let mut m = Map::new();
    m.insert("name".into(), J::from(text.get(rel, ident.name_key.as_deref())));
    m.insert("prefab".into(), J::from(rel));
    m
}

type Decon = Vec<(IndexSet<String>, Vec<(String, i128)>)>;

fn deconstruct_rows(prefabs: &Prefabs, item_type: i128) -> Decon {
    let mut out = Vec::new();
    for idx in rows(prefabs.get("crafting/deconstruct").as_deref(), 0) {
        if idx.get(0).and_then(W::as_int) != Some(item_type) {
            continue;
        }
        let Some(table) = idx.str(1) else { continue };
        for row in rows(prefabs.get(rp(table, "prefabs/")).as_deref(), 0) {
            let keys: IndexSet<String> = row.iter_of(0).iter().filter_map(|k| k.as_str().map(str::to_owned)).collect();
            let outs: Vec<(String, i128)> = row
                .iter_of(1)
                .iter()
                .filter(|o| o.is_obj())
                .filter_map(|o| {
                    let (p, n) = (o.leaf().get(0), o.leaf().get(1));
                    match (p.and_then(W::as_str), n) {
                        (Some(p), Some(W::Int(n))) if *n != 0 => Some((p.to_owned(), *n)),
                        _ => None,
                    }
                })
                .collect();
            out.push((keys, outs));
        }
    }
    out
}

fn yields(rows: &Decon, keys: &IndexSet<String>, prefabs: &Prefabs, text: &Text) -> Vec<J> {
    let mut total: IndexMap<String, i128> = IndexMap::new();
    for (need, outs) in rows {
        if need.iter().all(|k| keys.contains(k)) {
            for (rel, n) in outs {
                *total.entry(rel.clone()).or_insert(0) += n;
            }
        }
    }
    total
        .into_iter()
        .map(|(rel, n)| {
            let mut m = named(prefabs, text, &rel);
            m.insert("count".into(), J::Int(n));
            J::Dict(m)
        })
        .collect()
}

fn conditions(rows: &[W], negate: bool, req: &mut IndexSet<String>, exc: &mut IndexSet<String>) -> bool {
    for row in rows {
        let leaf = row.leaf();
        let op = match leaf.get(0) {
            Some(W::Str(s)) => s.to_lowercase(),
            Some(W::Int(i)) => i.to_string(),
            None => String::new(),
            Some(_) => "?".into(),
        };
        let body = leaf.get(1);
        let args = body.filter(|b| b.is_obj()).and_then(|b| b.leaf().get(0)).cloned();
        if op == "tags" {
            let target = if negate { &mut *exc } else { &mut *req };
            for a in args.as_ref().map(W::iter).unwrap_or_default() {
                if let W::Str(s) = a {
                    target.insert(s.to_string());
                }
            }
        } else if op == "setall" || op == "setnone" {
            let inner = args.as_ref().map(W::iter).unwrap_or_default();
            if !conditions(&inner, negate ^ (op == "setnone"), req, exc) {
                return false;
            }
        } else {
            return false;
        }
    }
    true
}

fn upgrade_names(prefabs: &Prefabs, text: &Text) -> IndexMap<String, String> {
    let mut names: IndexMap<String, String> = IndexMap::new();
    let mut progression = Locale::new();
    for path in text.tree.files("languages/en/prefabs_progression", ".binfab") {
        let name = rs(path.rsplit('/').next().unwrap_or(&path), ".binfab").to_owned();
        progression.extend(text.table(&name));
    }
    for path in prefabs.tree.files("prefabs/upgrade/upgrades/", ".binfab") {
        let Some(pf) = prefabs.get(rp(&path, "prefabs/")) else { continue };
        let Some(entries) = pf.root.as_ref().and_then(|r| r.get(0)).and_then(W::as_dict) else { continue };
        for entry in entries.values() {
            let data = entry.is_obj().then(|| entry.get(1)).flatten();
            let Some(data) = data.filter(|d| d.is_obj() && d.sections().len() >= 2) else { continue };
            let title = data.section(0).str(0).and_then(|k| progression.get(k)).cloned().unwrap_or_default();
            for tag in data.leaf().iter_of(0) {
                if let W::Str(t) = tag
                    && !title.is_empty()
                {
                    names.entry(t.to_string()).or_insert_with(|| title.clone());
                }
            }
        }
    }
    names
}

fn bait(prefabs: &Prefabs, r: &str, upgrades: &IndexMap<String, String>) -> Vec<J> {
    let Some(pf) = prefabs.get(r) else { return Vec::new() };
    if let Some(splitter) = pf.component(SPLITTER).filter(|s| s.sections().len() > 1) {
        return splitter
            .leaf()
            .iter_of(0)
            .iter()
            .filter_map(W::as_str)
            .flat_map(|sub| bait(prefabs, sub, upgrades))
            .collect();
    }
    let Some(b) = pf.component(BAIT).filter(|b| b.sections().len() >= 3) else { return Vec::new() };
    let leaf = b.leaf();
    let mut row = Map::new();
    row.insert("catch".into(), J::from(enum_of(&RARITY_TYPES, Some(leaf.get(0).unwrap_or(&W::Int(0))))));
    row.insert("size".into(), J::from(enum_of(&TROPHY_TYPES, Some(leaf.get(1).unwrap_or(&W::Int(0))))));
    row.insert("bite".into(), J::from(enum_of(&SPEED_TYPES, Some(leaf.get(2).unwrap_or(&W::Int(0))))));
    row.insert("duration".into(), b.section(0).get(EFFECT_DURATION).map_or(J::Null, J::from));
    if let Some(cond) = pf.component(CONDITIONS) {
        let (mut req, mut exc) = (IndexSet::new(), IndexSet::new());
        if conditions(&cond.leaf().iter_of(0), false, &mut req, &mut exc) {
            for (key, tags) in [("requires", req), ("excludes", exc)] {
                if !tags.is_empty() {
                    let mut list: Vec<String> =
                        tags.iter().map(|t| upgrades.get(t).cloned().unwrap_or_else(|| t.clone())).collect();
                    list.sort();
                    row.insert(key.into(), J::from(list));
                }
            }
        }
    }
    vec![J::Dict(row)]
}

fn lures(prefabs: &Prefabs, text: &Text) -> Vec<J> {
    let upgrades = upgrade_names(prefabs, text);
    let mut out = Vec::new();
    for path in prefabs.tree.files("prefabs/item/consumable/fishingbait/", ".binfab") {
        let rel = rs(rp(&path, "prefabs/"), ".binfab");
        let pf = prefabs.get(rel);
        let ident = identity(pf.as_deref());
        let name = text.get(rel, ident.name_key.as_deref());
        let Some(usef) = pf.as_ref().and_then(|p| p.component(USE_EFFECTS)) else { continue };
        if name.is_empty() || usef.sections().len() < 2 {
            continue;
        }
        let effects: Vec<J> =
            usef.leaf().iter_of(0).iter().filter_map(W::as_str).flat_map(|r| bait(prefabs, r, &upgrades)).collect();
        if !effects.is_empty() {
            out.push(obj! {
                "name" => name, "description" => text.get(rel, ident.description_key.as_deref()),
                "prefab" => rel, "effects" => effects,
            });
        }
    }
    out
}

fn odds(pf: Option<&Prefab>, names: &[&str], labels: &[String]) -> Map {
    let mut out = Map::new();
    for row in rows(pf, 0) {
        let key = enum_of(names, Some(row.get(0).unwrap_or(&W::Int(0))));
        let values = row.iter_of(1);
        if let Some(key) = key
            && values.len() == labels.len()
        {
            out.insert(key.into(), J::Dict(labels.iter().cloned().zip(values.iter().map(J::from)).collect()));
        }
    }
    out
}

fn f(v: Option<&W>) -> f64 {
    v.and_then(W::as_num).unwrap_or(0.0)
}

pub fn build(ctx: &Ctx) -> Built {
    let tree = &ctx.tree;
    let prefabs = Prefabs::new(tree);
    let text = Text::new(tree);
    let ui = text.table("ui");
    let sizes: Vec<String> = SIZE_KEYS.iter().map(|k| ui.get(*k).cloned().unwrap_or_default()).collect();

    let weight_pf = prefabs.get("meta/fishing/fishweightdata");
    let fractions = weight_pf.as_ref().and_then(|p| p.root.as_ref()).and_then(|r| r.get(0)).cloned();
    let mut weights: HashMap<i128, (f64, f64)> = HashMap::new();
    for r in rows(weight_pf.as_deref(), 1) {
        weights.insert(r.int(0).unwrap_or(0), (f(r.get(1)), f(r.get(2))));
    }
    let mut fish_data: HashMap<String, Dict> = HashMap::new();
    for r in rows(prefabs.get("fish/fish").as_deref(), 0) {
        if let Some(k) = r.str(0).map(str::to_owned) {
            fish_data.insert(k, r);
        }
    }
    let mut categories: IndexMap<String, String> = IndexMap::new();
    for cat in rows(prefabs.get("collections/collection_fish").as_deref(), 0) {
        let t = text.get("collections/collection_fish", cat.str(1));
        let title = if t.is_empty() {
            match cat.get(0) {
                Some(W::Str(s)) => s.to_string(),
                Some(W::Int(i)) => i.to_string(),
                _ => String::new(),
            }
        } else {
            t
        };
        for entry in cat.iter_of(3) {
            if entry.is_obj()
                && let Some(k) = entry.get(0).and_then(W::as_str)
            {
                categories.entry(k.to_owned()).or_insert_with(|| title.clone());
            }
        }
    }

    let mut decon_cache: HashMap<i128, Decon> = HashMap::new();
    let mut fish: Vec<Map> = Vec::new();
    for path in tree.files("prefabs/item/fish/", ".binfab") {
        let rel = rs(rp(&path, "prefabs/"), ".binfab");
        let Some(pf) = prefabs.get(rel) else { continue };
        let Some(obj) = pf.component(IDENTITY) else { continue };
        let name = text.get(rel, obj.get(ID_NAME).and_then(W::as_str));
        if name.is_empty() {
            continue;
        }
        let rarity = enum_of(&ITEM_RARITY, obj.get(ID_RARITY)).unwrap_or("");
        let tags: Vec<String> = pf
            .component(TAGS)
            .map(|t| t.leaf().iter_of(0))
            .unwrap_or_default()
            .iter()
            .filter_map(|t| t.as_str().map(str::to_owned))
            .collect();
        let mut entry = Map::new();
        entry.insert("slug".into(), J::from(rp(rel, "item/fish/").replace('/', "_")));
        entry.insert("name".into(), J::from(name));
        entry.insert("description".into(), J::from(text.get(rel, obj.get(ID_DESCRIPTION).and_then(W::as_str))));
        entry.insert("prefab".into(), J::from(rel));
        entry.insert("blueprint".into(), pf.component(BLUEPRINT).and_then(|b| b.get(0)).map_or(J::Null, J::from));
        entry.insert("liquid".into(), J::from(rel.split('/').nth(2).unwrap_or("")));
        entry.insert("rarity".into(), J::from(rarity));
        if let Some(c) = categories.get(rel) {
            entry.insert("collection".into(), J::from(c.as_str()));
        }
        if let Some(pool) = tags
            .iter()
            .find(|t| t.starts_with("pools_") && t.matches('_').count() == 2)
            .map(|t| t.split('_').nth(1).unwrap_or(""))
            .filter(|p| !p.is_empty())
        {
            entry.insert("pool".into(), J::from(pool));
        }
        entry.insert("tags".into(), J::from(tags.clone()));

        let item_type = obj.get(ID_ITEM_TYPE).and_then(W::as_int);
        if let Some(t) = item_type
            && !decon_cache.contains_key(&t)
        {
            decon_cache.insert(t, deconstruct_rows(&prefabs, t));
        }
        let empty: Decon = Vec::new();
        let drows = item_type.and_then(|t| decon_cache.get(&t)).unwrap_or(&empty);
        let keys: IndexSet<String> = tags.iter().map(|t| format!("{t}_ver{FISH_VERSION}")).collect();

        let data = fish_data.get(rel);
        let span = data.and_then(|d| d.get(4)).and_then(|k| match k {
            W::Int(i) => weights.get(i).copied(),
            _ => None,
        });
        if let Some((lo, hi)) = span {
            entry.insert("weight".into(), obj! {"min" => lo, "max" => hi});
        }
        let mut lo = span.map_or(0.0, |s| s.0);
        let mut bands = Vec::new();
        for (i, label) in sizes.iter().enumerate() {
            let mut band = Map::new();
            band.insert("size".into(), J::from(label.as_str()));
            if let Some((min, max)) = span
                && let Some(W::List(fr)) | Some(W::Rep(fr)) | Some(W::Obj(fr)) = fractions.as_ref()
                && i < fr.len()
            {
                let hi = if i == sizes.len() - 1 {
                    max
                } else {
                    let frac = fr[i].as_num().unwrap_or(0.0);
                    max.min(lo + frac * (max - min))
                };
                band.insert("min".into(), J::Float(crate::pyfloat::round(lo, 3)));
                band.insert("max".into(), J::Float(crate::pyfloat::round(hi, 3)));
                lo = hi;
            }
            if i > 0
                && let Some(t) = data.and_then(|d| d.str(i as i128))
            {
                band.insert("trophy".into(), J::Dict(named(&prefabs, &text, t)));
            }
            let mut k = keys.clone();
            k.insert(format!("{rarity}{}", SIZE_SUFFIX[i]));
            band.insert("deconstruct".into(), J::List(yields(drows, &k, &prefabs, &text)));
            bands.push(J::Dict(band));
        }
        entry.insert("sizes".into(), J::List(bands));
        fish.push(entry);
    }
    let liquid_order = |l: &str| match l {
        "water" => 0,
        "lava" => 1,
        "chocolate" => 2,
        "plasma" => 3,
        _ => 9,
    };
    fish.sort_by_cached_key(|f| {
        let liquid = f["liquid"].as_str().unwrap_or("").to_owned();
        let rarity = f["rarity"].as_str().unwrap_or("");
        let ri = ITEM_RARITY.iter().position(|r| *r == rarity).unwrap_or(9);
        (liquid_order(&liquid), liquid, ri, f["name"].as_str().unwrap_or("").to_lowercase())
    });
    let catch_labels: Vec<String> = ITEM_RARITY[..3].iter().map(|r| r.to_lowercase()).collect();
    let mut bite = Map::new();
    for r in rows(prefabs.get("meta/fishing/fishingspeeddata").as_deref(), 0) {
        if let Some(k) = enum_of(&SPEED_TYPES, Some(r.get(0).unwrap_or(&W::Int(0)))) {
            bite.insert(
                k.into(),
                obj! {"min" => r.get(1).map_or(J::Int(0), J::from), "max" => r.get(2).map_or(J::Int(0), J::from)},
            );
        }
    }
    Ok(obj! {
        "fish" => J::List(fish.into_iter().map(J::Dict).collect()),
        "catch_odds" => odds(prefabs.get("meta/fishing/fishingraritydata").as_deref(), &RARITY_TYPES, &catch_labels),
        "size_odds" => odds(prefabs.get("meta/fishing/fishtrophydata").as_deref(), &TROPHY_TYPES, &sizes),
        "bite_time" => bite,
        "lures" => lures(&prefabs, &text),
    })
}
