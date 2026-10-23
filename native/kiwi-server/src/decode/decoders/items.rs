//! `items.py`: every named item and piece of gear: what it is and does.

use std::{cell::RefCell, collections::HashMap};

use indexmap::{IndexMap, IndexSet};

use super::{mastery::COLLECTION_TYPES, mementos::decay, pvp_powerups::all_names};
use crate::decode::{
    ability::{Prefabs, describe, identity},
    common::{Locale, collection_members, lookup, r4, rp, rs, stat_rows, walk_values},
    json::{J, Map},
    registry::{Built, Ctx},
    value::{Dict, Prefab, W, parse},
};
use crate::obj;

const ROOTS: [&str; 2] = ["prefabs/item/", "prefabs/equipment/"];
const BLUEPRINT: i128 = 37;
const TAGS: i128 = 106;
const TAG_VERSION: i128 = 311;
const UNLOCKER: i128 = 223;
const FOOD: i128 = 65;
const LOCKBOX: i128 = 241;
const TITLES: i128 = 1033;
const ADVENTURES: i128 = 411;
const XP: i128 = 368;
const USE: [(i128, i128); 3] = [(398, 0), (135, 2), (57, 0)];
const FOOD_KIND: i128 = 50;
const STATIONS: [(i128, &str); 2] = [(2, "Loot Collector"), (8, "Compost")];
const SKIN: i128 = 4;
const SECONDARY_SKIN: i128 = 17;
const STYLE: i128 = 0;
const RECIPE: i128 = 1;
const EMPTY_EFFECT: &str = "effects/emptyeffect";

fn rarity_table() -> Vec<String> {
    let mut out: Vec<String> = ["Common", "Uncommon", "Rare", "Epic", "Legendary", "Relic", "Resplendent"]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
    out.extend((1..6).map(|i| format!("Shadow {i}")));
    for i in 1..6 {
        out.push(format!("Radiant {i}"));
        out.push(format!("Stellar {i}"));
    }
    out.extend((1..6).map(|i| format!("Crystal {i}")));
    out.extend((1..6).map(|i| format!("Mystic {i}")));
    out
}

fn rows(v: Option<&W>) -> Vec<&Dict> {
    v.map(W::elems).unwrap_or_default().iter().filter(|r| r.is_obj()).map(W::leaf).collect()
}

fn own(pf: &Prefab, cid: i128) -> &Dict {
    pf.component(cid).map_or(&crate::decode::value::EMPTY, W::leaf)
}

fn rarity_name(ident: &Dict, table: &[String]) -> String {
    let mut r = ident.get(9).cloned();
    for i in [17, 20] {
        if let Some(W::Int(v)) = ident.get(i)
            && *v != -1
        {
            r = Some(W::Int(*v));
        }
    }
    match r {
        Some(W::Int(i)) if (0..table.len() as i128).contains(&i) => table[i as usize].clone(),
        _ => String::new(),
    }
}

fn unlock_ref(kind: i128, r: &str) -> String {
    let r = rp(rs(r, ".binfab"), "prefabs/");
    if kind == SKIN && !r.contains('/') {
        return format!("skins/{r}");
    }
    if kind == SECONDARY_SKIN && !r.contains('/') {
        return format!("skins/secondary/{r}");
    }
    if kind == STYLE && !r.starts_with("equipment/") {
        return format!("bp:{}", rs(&r.rsplit('/').next().unwrap_or(r).to_lowercase(), ".blueprint"));
    }
    r.to_owned()
}

type Table = Vec<(IndexSet<String>, Vec<(String, J)>)>;

struct Deconstruct<'a> {
    prefabs: &'a Prefabs,
    names: &'a Locale,
    by: [Vec<((J, J), W)>; 3],
    tables: RefCell<HashMap<String, std::rc::Rc<Table>>>,
}

impl<'a> Deconstruct<'a> {
    fn new(prefabs: &'a Prefabs, names: &'a Locale) -> Self {
        let master = prefabs.get("crafting/deconstruct");
        let root = master.as_ref().and_then(|m| m.root.as_ref()).filter(|r| !r.is_empty()).map(W::leaf);
        let by = [2, 0, 1].map(|i| {
            let mut v: Vec<((J, J), W)> = Vec::new();
            for r in rows(root.and_then(|d| d.get(i))) {
                let key = (r.get(0).map_or(J::Null, J::from), r.get(2).map_or(J::Null, J::from));
                let val = r.get(1).cloned().unwrap_or(W::None);
                match v.iter_mut().find(|(k, _)| *k == key) {
                    Some(slot) => slot.1 = val,
                    None => v.push((key, val)),
                }
            }
            v
        });
        Deconstruct { prefabs, names, by, tables: RefCell::default() }
    }

    fn lookup(&self, i: usize, key: &(J, J)) -> Option<W> {
        self.by[i].iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    fn table(&self, r: Option<&W>) -> std::rc::Rc<Table> {
        let Some(W::Str(s)) = r else { return std::rc::Rc::new(Vec::new()) };
        let r = rs(rp(s, "prefabs/"), ".binfab").to_owned();
        if let Some(t) = self.tables.borrow().get(&r) {
            return t.clone();
        }
        let pf = self.prefabs.get(&r);
        let mut out = Vec::new();
        let root = pf.as_ref().and_then(|p| p.root.as_ref()).filter(|x| !x.is_empty());
        for row in rows(root.and_then(|x| x.leaf().get(0))) {
            let need: IndexSet<String> = row.iter_of(0).iter().filter_map(|t| t.as_str().map(str::to_owned)).collect();
            let outs: Vec<(String, J)> = rows(row.get(1))
                .into_iter()
                .filter_map(|o| {
                    let item = o.str(0)?.to_owned();
                    let n = o.get(1).filter(|v| v.truthy()).map_or(J::Int(1), J::from);
                    Some((item, n))
                })
                .collect();
            out.push((need, outs));
        }
        let out = std::rc::Rc::new(out);
        self.tables.borrow_mut().insert(r, out.clone());
        out
    }

    fn call(&self, rel: &str, item_type: Option<&W>, collections: &IndexSet<i128>, keys: &IndexSet<String>) -> Vec<J> {
        let mut out = Vec::new();
        let it = item_type.map_or(J::Null, J::from);
        for (station, label) in STATIONS {
            let st = J::Int(station);
            let truthy = |w: &Option<W>| w.as_ref().is_some_and(W::truthy);
            let mut r = self.lookup(0, &(J::from(rel), st.clone()));
            if !truthy(&r) {
                r = self.lookup(1, &(it.clone(), st.clone()));
            }
            if !truthy(&r) {
                let mut cs: Vec<i128> = collections.iter().copied().collect();
                cs.sort();
                r = cs.iter().find_map(|c| self.lookup(2, &(J::Int(*c), st.clone())));
            }
            let mut totals: IndexMap<String, J> = IndexMap::new();
            for (need, outs) in self.table(r.as_ref()).iter() {
                if need.iter().all(|n| keys.contains(n)) {
                    for (item, n) in outs {
                        let item = rs(rp(item, "prefabs/"), ".binfab").to_owned();
                        let prev = totals.get(&item).cloned().unwrap_or(J::Int(0));
                        totals.insert(item, add(&prev, n));
                    }
                }
            }
            if !totals.is_empty() {
                let yields: Vec<J> = totals
                    .into_iter()
                    .map(|(k, v)| {
                        let name = self
                            .names
                            .get(identity(self.prefabs.get(&k).as_deref()).name())
                            .cloned()
                            .unwrap_or_default();
                        obj! {"item" => k, "name" => name, "count" => v}
                    })
                    .collect();
                out.push(obj! {"station" => label, "yields" => yields});
            }
        }
        out
    }
}

fn add(a: &J, b: &J) -> J {
    match (a, b) {
        (J::Int(x), J::Int(y)) => J::Int(x + y),
        (J::Int(x), J::Float(y)) => J::Float(*x as f64 + y),
        (J::Float(x), J::Int(y)) => J::Float(x + *y as f64),
        (J::Float(x), J::Float(y)) => J::Float(x + y),
        (_, b) => b.clone(),
    }
}

fn sprouts(prefabs: &Prefabs) -> IndexMap<String, J> {
    let mut templates: IndexMap<String, W> = IndexMap::new();
    for stem in ["blocks/plants", "blocks/gardening"] {
        let pf = prefabs.get(stem);
        let root = pf.as_ref().and_then(|p| p.root.as_ref()).filter(|r| !r.is_empty());
        for row in root.and_then(|r| r.leaf().get(1)).map(W::iter).unwrap_or_default() {
            if row.is_obj()
                && row.sections().len() > 1
                && let Some(k) = row.section(0).str(0)
                && let Some(bp) = row.section(1).get(0).filter(|b| b.as_str().is_some_and(|s| !s.is_empty()))
            {
                templates.insert(k.to_owned(), bp.clone());
            }
        }
    }
    let pf = prefabs.get("blocks/sprouts");
    let root = pf.as_ref().and_then(|p| p.root.as_ref()).filter(|r| !r.is_empty());
    let mut out = IndexMap::new();
    for row in root.and_then(|r| r.leaf().get(1)).map(W::iter).unwrap_or_default() {
        if !row.is_obj() || row.sections().len() < 2 {
            continue;
        }
        let Some(seed) = row.section(0).str(0) else { continue };
        let body = row.section(1);
        let outs: Vec<(String, f64)> = rows(body.get(2))
            .into_iter()
            .filter_map(|o| {
                Some((o.str(3)?.to_owned(), o.get(4).filter(|v| v.truthy()).and_then(W::as_num).unwrap_or(0.0)))
            })
            .collect();
        let sum: f64 = outs.iter().fold(0.0, |a, (_, w)| a + w);
        let total = if sum == 0.0 { 1.0 } else { sum };
        let mut grown = Vec::new();
        for (r, w) in &outs {
            let mut entry = Map::new();
            entry.insert("chance".into(), J::Float(r4(w / total)));
            if r.starts_with("plant/") {
                let Some(t) = templates.get(r) else { continue };
                entry.insert("blueprint".into(), J::from(t));
            } else {
                entry.insert("ref".into(), J::from(rs(r, ".binfab")));
            }
            grown.push(J::Dict(entry));
        }
        if let Some(secs) = body.get(1).filter(|v| matches!(v, W::Int(_) | W::Float(_)))
            && !grown.is_empty()
        {
            out.insert(rs(seed, ".binfab").to_lowercase(), obj! {"seconds" => J::from(secs), "outcomes" => grown});
        }
    }
    out
}

fn without(row: &J, keys: &[&str]) -> Map {
    row.as_dict()
        .map(|m| m.iter().filter(|(k, _)| !keys.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}

fn use_of(prefabs: &Prefabs, pf: &Prefab, names: &Locale) -> Map {
    let mut roots: Vec<String> = Vec::new();
    for (cid, idx) in USE {
        match own(pf, cid).get(idx) {
            Some(W::Str(s)) => roots.push(s.to_string()),
            Some(v) => roots.extend(v.iter().iter().filter_map(|x| x.as_str().map(str::to_owned))),
            None => {}
        }
    }
    let mut out = Map::new();
    for root in roots {
        let root = rs(&root, ".binfab");
        if root.is_empty() || root == EMPTY_EFFECT {
            continue;
        }
        let info = describe(prefabs, root, &[], &["abilities/", "effects/", "item/"]);
        for key in ["stages", "healing"] {
            let rws: Vec<J> = info
                .get(key)
                .and_then(J::as_list)
                .into_iter()
                .flatten()
                .map(|r| J::Dict(without(r, &["_depth"])))
                .collect();
            if !rws.is_empty() {
                out.entry(key.to_owned())
                    .or_insert_with(|| J::List(Vec::new()))
                    .as_list_mut()
                    .expect("list")
                    .extend(rws);
            }
        }
        for eff in info.get("effects").and_then(J::as_list).into_iter().flatten() {
            let mut row = without(eff, &["_depth", "name_key", "description_key"]);
            if let Some(t) = names.get(eff.str_or("name_key", "")).filter(|t| !t.is_empty()) {
                row.insert("title".into(), J::from(t.as_str()));
            }
            out.entry("effects".to_owned())
                .or_insert_with(|| J::List(Vec::new()))
                .as_list_mut()
                .expect("list")
                .push(J::Dict(row));
        }
    }
    out
}

fn cost(v: Option<&W>, prefabs: &Prefabs, names: &Locale) -> Vec<J> {
    rows(v)
        .into_iter()
        .filter_map(|r| {
            let item = r.str(0)?;
            let n = r.int(1).filter(|n| *n > 0)?;
            let item = rs(item, ".binfab");
            let name = names.get(identity(prefabs.get(item).as_deref()).name()).cloned().unwrap_or_default();
            Some(obj! {"item" => item, "name" => name, "count" => n})
        })
        .collect()
}

/// `str()` of a scalar in an f-string.
fn fmt_scalar(v: &W) -> String {
    match v {
        W::Int(i) => i.to_string(),
        W::Float(f) => crate::pyfloat::repr(*f),
        W::Str(s) => s.to_string(),
        W::None => "None".into(),
        other => format!("{other:?}"),
    }
}

type Members = IndexMap<String, (String, String)>;

pub fn build(ctx: &Ctx) -> Built {
    let tree = &ctx.tree;
    let prefabs = Prefabs::new(tree);
    let names = all_names(tree);
    let mut guides: IndexMap<String, String> = IndexMap::new();
    for (k, v) in &names {
        if let Some(rest) = k.strip_prefix("$GuideUI_generic_") {
            guides.insert(rest.to_lowercase(), v.clone());
        }
    }
    let pools: RefCell<HashMap<i128, Members>> = RefCell::default();
    let pool = |ctype: i128| -> IndexMap<String, (String, String)> {
        if let Some(p) = pools.borrow().get(&ctype) {
            return p.clone();
        }
        let name = usize::try_from(ctype)
            .ok()
            .and_then(|c| COLLECTION_TYPES.get(c))
            .map(|c| c.to_lowercase())
            .unwrap_or_default();
        let p = if name.is_empty() {
            IndexMap::new()
        } else {
            collection_members(prefabs.get(&format!("collections/collection_{name}")).as_deref(), &names)
        };
        pools.borrow_mut().insert(ctype, p.clone());
        p
    };
    let rarity = rarity_table();
    let decon = Deconstruct::new(&prefabs, &names);
    let sprouts = sprouts(&prefabs);
    let mut items = Vec::new();
    for path in ROOTS.iter().flat_map(|r| tree.files(r, ".binfab")) {
        let rel = &path["prefabs/".len()..path.len() - ".binfab".len()];
        let Ok(pf) = parse(&tree.read(&path).unwrap_or_default()) else { continue };
        let ident = pf.component(49).map_or(&crate::decode::value::EMPTY, W::leaf);
        let name = lookup(&names, ident.get(1).filter(|v| v.truthy()));
        if name.is_empty() {
            continue;
        }
        let mut folder =
            if rel.matches('/').count() > 1 { rel.split('/').nth(1).unwrap_or("").to_owned() } else { String::new() };
        let mut slug = rp(rel, "item/").replace('/', "_");
        if rel.starts_with("equipment/") {
            folder = format!("gear_{folder}").trim_end_matches('_').to_owned();
            slug = format!("gear_{}", rp(rel, "equipment/").replace('/', "_"));
        }
        let mut item = Map::new();
        item.insert("slug".into(), J::from(slug));
        item.insert("name".into(), J::from(name));
        item.insert("description".into(), J::from(lookup(&names, ident.get(5).filter(|v| v.truthy()))));
        item.insert("prefab".into(), J::from(rel));
        item.insert("folder".into(), J::from(folder));
        let rname = rarity_name(ident, &rarity);
        if !rname.is_empty() {
            item.insert("rarity".into(), J::from(rname.as_str()));
        }
        if ident.get(14).is_some_and(|t| J::from(t) == J::Int(0)) {
            item.insert("tradable".into(), J::Bool(false));
        }
        if let Some(bp) = pf.component(BLUEPRINT).and_then(|c| c.leaf().str(0)).filter(|s| !s.is_empty()) {
            item.insert("blueprint".into(), J::from(bp));
        }
        let guide = guides
            .get(&rel.replace('/', "_").to_lowercase())
            .filter(|g| !g.is_empty())
            .or_else(|| guides.get(&rp(rel, "item/").replace('/', "_").to_lowercase()));
        if let Some(g) = guide.filter(|g| !g.is_empty()) {
            item.insert("guide".into(), J::from(g.as_str()));
        }
        let unl = own(&pf, UNLOCKER);
        let mut ctypes: IndexSet<i128> = IndexSet::new();
        let mut unlocks: Vec<String> = Vec::new();
        for r in rows(unl.get(8)) {
            if let (Some(kind), Some(rf)) = (r.int(0), r.str(1).filter(|s| !s.is_empty())) {
                ctypes.insert(kind);
                if kind != RECIPE {
                    let u = unlock_ref(kind, rf);
                    if !unlocks.contains(&u) {
                        unlocks.push(u);
                    }
                }
            }
        }
        if !unlocks.is_empty() {
            item.insert("unlocks".into(), J::from(unlocks));
        }
        if unl.get(5).is_some_and(|v| J::from(v) == J::Int(1))
            && unl.get(6).is_some_and(W::truthy)
            && unl.get(7).is_some_and(W::truthy)
        {
            let groups: IndexSet<String> =
                unl.iter_of(6).iter().filter_map(|g| g.as_str().map(str::to_owned)).collect();
            let mut members: Vec<String> = Vec::new();
            for ctype in unl.iter_of(7).iter().filter_map(W::as_int) {
                ctypes.insert(ctype);
                for (rf, (gid, _)) in pool(ctype) {
                    if groups.contains(&gid) {
                        members.push(unlock_ref(ctype, &rf));
                    }
                }
            }
            if !members.is_empty() {
                let mut g: Vec<String> = groups.into_iter().collect();
                g.sort();
                let mut seen = IndexSet::new();
                let members: Vec<String> = members.into_iter().filter(|m| seen.insert(m.clone())).collect();
                item.insert("random_unlock".into(), obj! {"groups" => g, "members" => members});
            }
        }
        let food = own(&pf, FOOD);
        if food.get(0).is_some_and(|v| J::from(v) == J::Int(FOOD_KIND)) {
            let stats = stat_rows(&food.get(1).map(walk_values).unwrap_or_else(|| vec![W::None]));
            if !stats.is_empty() {
                item.insert("food".into(), J::List(stats));
            }
        }
        let used = use_of(&prefabs, &pf, &names);
        if !used.is_empty() {
            item.insert("use".into(), J::Dict(used));
        }
        let titles: Vec<J> = own(&pf, TITLES).iter_of(0).iter().filter_map(W::as_str).map(J::from).collect();
        if !titles.is_empty() {
            item.insert("titles".into(), J::List(titles));
        }
        let advs: Vec<J> =
            rows(own(&pf, ADVENTURES).get(1)).into_iter().filter_map(|a| a.str(0).map(J::from)).collect();
        if !advs.is_empty() {
            item.insert("adventures".into(), J::List(advs));
        }
        if let Some(xp) = own(&pf, XP).int(0).filter(|x| *x > 0) {
            item.insert("experience".into(), J::Int(xp));
        }
        if pf.component(LOCKBOX).is_some_and(|c| !c.is_empty()) {
            let opens: Vec<J> = rows(own(&pf, LOCKBOX).get(0))
                .into_iter()
                .map(|o| obj! {"label" => o.str(4).unwrap_or(""), "cost" => cost(o.get(3), &prefabs, &names)})
                .collect();
            if !opens.is_empty() {
                item.insert("opens".into(), J::List(opens));
            }
        }
        if let Some(d) = decay(&pf) {
            item.insert("decay".into(), d);
        }
        if let Some(g) = sprouts.get(&rel.to_lowercase()) {
            item.insert("grows".into(), g.clone());
        }
        let tags: Vec<String> = pf
            .component(TAGS)
            .map(|c| c.leaf().iter_of(0))
            .unwrap_or_default()
            .iter()
            .filter_map(|t| t.as_str().filter(|s| !s.is_empty()).map(str::to_owned))
            .collect();
        let version =
            pf.component(TAG_VERSION).filter(|c| !c.is_empty()).map(|c| c.leaf().get(2).cloned().unwrap_or(W::Int(0)));
        let mut keys: IndexSet<String> = match &version {
            Some(v) => tags.iter().map(|t| format!("{t}_ver{}", fmt_scalar(v))).collect(),
            None => tags.iter().cloned().collect(),
        };
        keys.insert(rname.replace(' ', ""));
        let dec = decon.call(rel, ident.get(10), &ctypes, &keys);
        if !dec.is_empty() {
            item.insert("deconstruct".into(), J::List(dec));
        }
        items.push(J::Dict(item));
    }
    Ok(obj! {"items" => items})
}
