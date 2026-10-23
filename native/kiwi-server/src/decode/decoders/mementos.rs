//! `mementos.py`: Delve mementos, where each files, what it gives.

use std::{cell::RefCell, collections::HashMap, sync::Arc};

use indexmap::{IndexMap, IndexSet};

use crate::decode::{
    ability::identity,
    common::{IDENTITY, Locale, locale_at, rs},
    json::{J, Map},
    registry::{Built, Ctx},
    tree::Tree,
    value::{Dict, Prefab, W, parse},
};
use crate::obj;

const ROOT: &str = "item/unlocker/delve/";
const LANG: &str = "languages/en/";
const MEMENTO: i128 = 18;
const RECIPE: i128 = 1;
const MASTERY_SOURCE: i128 = 21;
const DECONSTRUCTOR: i128 = 2;
const ID_RARITY: i128 = 9;
const BLUEPRINT: i128 = 37;
const TAGS: i128 = 106;
const UNLOCKER: i128 = 223;
const DECAY: i128 = 472;
const ADVENTURE: &str = "adv_daily_delve_memento";

fn rarity(v: Option<&W>) -> Option<&'static str> {
    match v? {
        W::Int(0) => Some("Common"),
        W::Int(1) => Some("Uncommon"),
        W::Int(2) => Some("Rare"),
        W::Int(3) => Some("Epic"),
        W::Int(4) => Some("Legendary"),
        _ => None,
    }
}

fn decay_trigger(v: Option<&W>) -> J {
    match v {
        Some(W::Int(i)) => match i {
            0 => J::from("Offline"),
            1 => J::from("Online"),
            2 => J::from("LastLogin"),
            3 => J::from("Equipped"),
            4 => J::from("DailyReset"),
            5 => J::from("WeeklyReset"),
            6 => J::from("Event"),
            _ => J::Int(*i),
        },
        Some(v) => J::from(v),
        None => J::Null,
    }
}

/// `_rows`: the leaf sections of the objects in a value.
pub fn rows(value: Option<&W>) -> Vec<Arc<Dict>> {
    value
        .map(W::iter)
        .unwrap_or_default()
        .iter()
        .filter(|r| r.is_obj())
        .map(|r| Arc::new(Dict::new(r.leaf().items().to_vec())))
        .collect()
}

/// Parsed prefabs and locale text, loaded on demand.
pub struct Game {
    pub tree: Tree,
    pf: RefCell<HashMap<String, Option<Arc<Prefab>>>>,
    tables: RefCell<HashMap<String, Arc<Locale>>>,
    stems: Vec<String>,
}

impl Game {
    pub fn new(tree: &Tree) -> Game {
        let mut stems: Vec<String> =
            tree.files(LANG, ".binfab").iter().map(|p| p[LANG.len()..p.len() - ".binfab".len()].to_owned()).collect();
        stems.sort_by_key(|s| std::cmp::Reverse(s.chars().count()));
        Game { tree: tree.clone(), pf: RefCell::default(), tables: RefCell::default(), stems }
    }

    pub fn prefab(&self, rel: &str) -> Option<Arc<Prefab>> {
        let rel = rs(rel, ".binfab");
        if let Some(hit) = self.pf.borrow().get(rel) {
            return hit.clone();
        }
        let parsed = self
            .tree
            .read(&format!("prefabs/{rel}.binfab"))
            .filter(|d| !d.is_empty())
            .and_then(|d| parse(&d).ok())
            .map(Arc::new);
        self.pf.borrow_mut().insert(rel.to_owned(), parsed.clone());
        parsed
    }

    pub fn table(&self, stem: &str) -> Arc<Locale> {
        if let Some(t) = self.tables.borrow().get(stem) {
            return t.clone();
        }
        let t = Arc::new(locale_at(&self.tree, &format!("{LANG}{stem}.binfab")));
        self.tables.borrow_mut().insert(stem.to_owned(), t.clone());
        t
    }

    pub fn text(&self, key: Option<&W>, stem: Option<&str>, rel: &str) -> String {
        let Some(key) = key.and_then(W::as_str) else { return String::new() };
        self.text_str(key, stem, rel)
    }

    pub fn text_str(&self, key: &str, stem: Option<&str>, rel: &str) -> String {
        if let Some(lit) = key.strip_prefix('@') {
            return lit.to_owned();
        }
        if !key.starts_with('$') {
            return String::new();
        }
        if let Some(stem) = stem.filter(|s| !s.is_empty()) {
            return self.table(stem).get(key).cloned().unwrap_or_default();
        }
        let low = key[1..].to_lowercase();
        let folder = if rel.contains('/') {
            format!("prefabs_{}", rel.rsplit_once('/').map_or("", |(a, _)| a).replace('/', "_").to_lowercase())
        } else {
            String::new()
        };
        let likely: Vec<&String> = self
            .stems
            .iter()
            .filter(|s| low.starts_with(&format!("{}_", s.to_lowercase())) || folder.starts_with(&s.to_lowercase()))
            .collect();
        let rest = self.stems.iter().filter(|s| !likely.contains(s));
        for s in likely.iter().copied().chain(rest) {
            if let Some(v) = self.table(s).get(key) {
                return v.clone();
            }
        }
        String::new()
    }

    pub fn name(&self, rel: &str) -> String {
        self.text_str(identity(self.prefab(rel).as_deref()).name(), None, rel)
    }

    pub fn recipe(&self, rid: &str) -> Option<Arc<Prefab>> {
        self.prefab(&format!("recipes/{rid}"))
    }
}

fn unlock(row: &Dict) -> Option<(i128, String)> {
    let u = row.get(3)?;
    let leaf = u.leaf();
    let kind = match leaf.get(0) {
        None => -1,
        Some(W::Int(i)) => *i,
        Some(_) => return None,
    };
    let path = leaf.str(1).filter(|p| !p.is_empty())?;
    (kind >= 0).then(|| (kind, path.to_owned()))
}

fn int_or(v: Option<&W>, default: i128) -> J {
    v.map_or(J::Int(default), J::from)
}

fn root_rows(pf: Option<&Prefab>, idx: i128) -> Vec<Arc<Dict>> {
    rows(pf.and_then(|p| p.root.as_ref()).filter(|r| !r.is_empty()).and_then(|r| r.leaf().get(idx)))
}

fn recipe_results(g: &Game, pf: Option<&Prefab>) -> Vec<Map> {
    let mut out = Vec::new();
    for row in root_rows(pf, 1) {
        if let Some((kind, path)) = unlock(&row) {
            let mut m = Map::new();
            m.insert("unlock".into(), J::from(path.as_str()));
            m.insert("type".into(), J::Int(kind));
            m.insert("name".into(), J::from(g.name(&path)));
            out.push(m);
        } else if let Some(item) = row.str(0).filter(|s| !s.is_empty()) {
            let mut m = Map::new();
            m.insert("item".into(), J::from(item));
            m.insert("count".into(), int_or(row.get(1), 1));
            m.insert("name".into(), J::from(g.name(item)));
            out.push(m);
        }
    }
    out
}

fn recipe_name(g: &Game, rid: &str) -> String {
    recipe_results(g, g.recipe(rid).as_deref())
        .iter()
        .find_map(|r| r.get("name").and_then(J::as_str).filter(|n| !n.is_empty()).map(str::to_owned))
        .unwrap_or_default()
}

fn ingredients(g: &Game, pf: &Prefab) -> Vec<J> {
    root_rows(Some(pf), 0)
        .iter()
        .filter_map(|r| {
            let item = r.str(0).filter(|s| !s.is_empty())?;
            Some(obj! {"item" => item, "count" => int_or(r.get(1), 1), "name" => g.name(item)})
        })
        .collect()
}

struct Group {
    id: J,
    name: J,
    counted: bool,
}

fn collection(g: &Game) -> (IndexMap<String, Arc<Group>>, Vec<J>) {
    let pf = g.prefab("collections/collection_memento");
    let mut entries: IndexMap<String, Arc<Group>> = IndexMap::new();
    let mut groups = Vec::new();
    for grp in root_rows(pf.as_deref(), 0) {
        let rws = rows(grp.get(3));
        if rws.is_empty() {
            continue;
        }
        let id = grp.get(0).map_or(J::from(""), J::from);
        let text = g.text(grp.get(1), Some("prefabs_collections"), "");
        let name = if text.is_empty() { id.clone() } else { J::from(text) };
        let info = Arc::new(Group { id, name, counted: grp.get(1 + 1).is_some_and(W::truthy) });
        groups.push(obj! {"id" => info.id.clone(), "name" => info.name.clone(), "counted" => info.counted, "entries" => rws.len()});
        for r in &rws {
            if let Some(k) = r.str(0) {
                entries.entry(k.to_owned()).or_insert_with(|| info.clone());
            }
        }
    }
    (entries, groups)
}

fn mastery(g: &Game) -> Option<IndexMap<String, i128>> {
    let (meta, mult) = (g.prefab("meta/meta")?, g.prefab("meta/multipliers")?);
    let (mroot, uroot) = (meta.root.as_ref().filter(|r| !r.is_empty())?, mult.root.as_ref().filter(|r| !r.is_empty())?);
    let base = rows(mroot.leaf().get(0))
        .iter()
        .find(|r| r.get(0).and_then(W::as_int) == Some(MASTERY_SOURCE))
        .map(|r| r.get(1).cloned());
    let base = match base {
        Some(Some(W::Int(b))) => b,
        _ => return None,
    };
    let groups = rows(uroot.leaf().get(0));
    let default = uroot.leaf().int(1)?;
    if !(0..groups.len() as i128).contains(&default) {
        return None;
    }
    let times = |grp: &Dict| -> i128 {
        let m = grp.get(0).and_then(W::as_num).unwrap_or(0.0);
        match grp.get(0) {
            Some(W::Int(i)) => base * i,
            _ => (base as f64 * m).round_ties_even() as i128,
        }
    };
    let mut out = IndexMap::new();
    out.insert(String::new(), times(&groups[default as usize]));
    for grp in &groups {
        for r in rows(grp.get(1)) {
            if r.get(0).and_then(W::as_int) == Some(MEMENTO)
                && let Some(p) = r.str(1)
            {
                out.insert(p.to_owned(), times(grp));
            }
        }
    }
    Some(out)
}

fn deconstruct_rows(g: &Game) -> Vec<(IndexSet<String>, Vec<J>)> {
    let master = g.prefab("crafting/deconstruct");
    let table = root_rows(master.as_deref(), 1)
        .iter()
        .find(|r| r.get(0).and_then(W::as_int) == Some(MEMENTO) && r.get(2).and_then(W::as_int) == Some(DECONSTRUCTOR))
        .and_then(|r| r.get(1).cloned());
    let pf = table.as_ref().and_then(W::as_str).and_then(|t| g.prefab(t.strip_prefix("prefabs/").unwrap_or(t)));
    let mut out = Vec::new();
    for r in root_rows(pf.as_deref(), 0) {
        let tags: IndexSet<String> = r.iter_of(0).iter().filter_map(|t| t.as_str().map(str::to_owned)).collect();
        let outs: Vec<J> = rows(r.get(1))
            .iter()
            .filter_map(|o| {
                let item = o.str(0)?;
                Some(obj! {"item" => item, "count" => int_or(o.get(1), 1), "name" => g.name(item)})
            })
            .collect();
        out.push((tags, outs));
    }
    out
}

fn collect_unlocks(g: &Game) -> IndexMap<String, Vec<String>> {
    let pf = g.prefab("collections/unlocks");
    let mut out: IndexMap<String, Vec<String>> = IndexMap::new();
    for r in root_rows(pf.as_deref(), 0) {
        let src = r.get(0).map(|h| h.leaf()).cloned_dict();
        if src.get(0).and_then(W::as_int) != Some(MEMENTO) {
            continue;
        }
        let Some(key) = src.str(1) else { continue };
        let list = out.entry(key.to_owned()).or_default();
        for o in rows(r.get(1)) {
            if o.get(0).and_then(W::as_int) == Some(RECIPE)
                && let Some(p) = o.str(1)
            {
                list.push(p.to_owned());
            }
        }
    }
    out
}

trait ClonedDict {
    fn cloned_dict(self) -> Dict;
}

impl ClonedDict for Option<&Dict> {
    fn cloned_dict(self) -> Dict {
        self.map(|d| Dict::new(d.items().to_vec())).unwrap_or_default()
    }
}

fn adventure(g: &Game) -> (J, IndexSet<String>) {
    let pf = g.prefab("meta/activities/npc_adventures");
    for adv in root_rows(pf.as_deref(), 1) {
        if adv.str(0) != Some(ADVENTURE) {
            continue;
        }
        let mut items = IndexSet::new();
        let mut stack: Vec<W> = vec![adv.get(3).cloned().unwrap_or(W::None)];
        while let Some(v) = stack.pop() {
            match &v {
                W::Str(s) if s.starts_with(ROOT) => {
                    items.insert(s.to_lowercase());
                }
                W::Dict(d) => stack.extend(d.values().cloned()),
                W::Tuple(x) | W::Obj(x) | W::Rep(x) | W::List(x) => stack.extend(x.iter().cloned()),
                _ => {}
            }
        }
        let info = obj! {"name" => g.text(adv.get(1), Some("adventures"), ""), "description" => g.text(adv.get(2), Some("adventures"), "")};
        return (info, items);
    }
    (J::Dict(Map::new()), IndexSet::new())
}

/// `_decay`: what destroys the item over time.
pub fn decay(pf: &Prefab) -> Option<J> {
    let comp = pf.component(DECAY)?;
    for row in rows(comp.get(5)) {
        let Some(timer) = row.get(1).filter(|t| t.is_obj() && !t.is_empty()) else { continue };
        if row.str(0) != Some("quantitydecay") {
            continue;
        }
        let base = timer.section(0);
        let own = if timer.sections().len() > 1 { timer.leaf() } else { &crate::decode::value::EMPTY };
        let seconds = match base.get(1) {
            Some(W::Float(f)) if f.is_finite() && *f == f.trunc() => J::Int(*f as i128),
            Some(v) => J::from(v),
            None => J::Null,
        };
        let mut out = Map::new();
        out.insert("trigger".into(), decay_trigger(base.get(3)));
        out.insert("seconds".into(), seconds);
        if let Some(n) = own.int(0) {
            out.insert("destroys".into(), if n == -1 { J::from("all") } else { J::Int(n) });
        }
        return Some(J::Dict(out));
    }
    None
}

pub fn build(ctx: &Ctx) -> Built {
    let tree = &ctx.tree;
    let g = Game::new(tree);
    let (entries, groups) = collection(&g);
    let mastery = mastery(&g);
    let deconstruct = deconstruct_rows(&g);
    let teaches = collect_unlocks(&g);
    let (adv, adventure_items) = adventure(&g);

    let mut made_by: IndexMap<String, Vec<J>> = IndexMap::new();
    let mut used_in: IndexMap<String, Vec<J>> = IndexMap::new();
    for path in tree.files("prefabs/recipes/", ".binfab") {
        let data = tree.read(&path).unwrap_or_default();
        if !data.windows(ROOT.len()).any(|w| w == ROOT.as_bytes()) {
            continue;
        }
        let rid = &path["prefabs/recipes/".len()..path.len() - ".binfab".len()];
        let Some(pf) = g.recipe(rid) else { continue };
        if pf.root.is_none() {
            continue;
        }
        let results = recipe_results(&g, Some(&pf));
        let ingr = ingredients(&g, &pf);
        for r in &results {
            let target =
                r.get("unlock").filter(|v| v.truthy()).or_else(|| r.get("item")).and_then(J::as_str).unwrap_or("");
            if target.starts_with(ROOT) {
                made_by
                    .entry(target.to_lowercase())
                    .or_default()
                    .push(obj! {"recipe" => rid, "ingredients" => ingr.clone()});
            }
        }
        for i in &ingr {
            let item = i.str_or("item", "");
            if item.starts_with(ROOT) {
                let makes: Vec<J> = results
                    .iter()
                    .map(|r| {
                        J::Dict(r.iter().filter(|(k, _)| *k != "type").map(|(k, v)| (k.clone(), v.clone())).collect())
                    })
                    .collect();
                used_in.entry(item.to_lowercase()).or_default().push(obj! {"recipe" => rid, "makes" => makes});
            }
        }
    }

    let mut out: Vec<Map> = Vec::new();
    for path in tree.files(&format!("prefabs/{ROOT}"), ".binfab") {
        let rel = &path["prefabs/".len()..path.len() - ".binfab".len()];
        let Some(pf) = g.prefab(rel) else { continue };
        let Some(unlocker) = pf.component(UNLOCKER) else { continue };
        let grants: Vec<(J, String)> = rows(unlocker.get(8))
            .iter()
            .filter_map(|r| r.str(1).map(|p| (r.get(0).map_or(J::Int(-1), J::from), p.to_owned())))
            .collect();
        let collectible = grants.iter().find(|(k, _)| *k == J::Int(MEMENTO)).map(|(_, p)| p.clone());
        let ident = identity(Some(&pf));
        let name = g.text_str(ident.name(), None, rel);
        let Some(collectible) = collectible else { continue };
        if name.is_empty() {
            continue;
        }
        let sub = &rel[ROOT.len()..];
        let leaf = pf.component(IDENTITY).map(|c| c.leaf()).unwrap_or(&crate::decode::value::EMPTY);
        let mut row = Map::new();
        row.insert("slug".into(), J::from(sub.replace('/', "_")));
        row.insert("name".into(), J::from(name.as_str()));
        if let Some(t) = name.strip_prefix("Memento: ") {
            row.insert("title".into(), J::from(t));
        }
        let description = g.text_str(ident.desc(), None, rel);
        if !description.is_empty() {
            row.insert("description".into(), J::from(description));
        }
        row.insert("prefab".into(), J::from(rel));
        if collectible.to_lowercase() != rel.to_lowercase() {
            row.insert("collectible".into(), J::from(collectible.as_str()));
        }
        if let Some(bp) = pf.component(BLUEPRINT).and_then(|b| b.get(0)).and_then(W::as_str).filter(|s| !s.is_empty()) {
            row.insert("blueprint".into(), J::from(rs(bp, ".blueprint")));
        }
        if let Some(r) = rarity(leaf.get(ID_RARITY)) {
            row.insert("rarity".into(), J::from(r));
        }
        let entry = entries.get(&collectible);
        if let Some(e) = entry {
            row.insert("category".into(), e.name.clone());
            row.insert("category_id".into(), e.id.clone());
        }
        if format!("/{sub}").contains("/events/") {
            row.insert("event".into(), J::Bool(true));
        }
        if let (Some(e), Some(m)) = (entry, mastery.as_ref())
            && e.counted
        {
            let v = m.get(&collectible).copied().unwrap_or(m[""]);
            row.insert("mastery".into(), J::Int(v));
        }
        if let Some(d) = decay(&pf) {
            row.insert("decay".into(), d);
        }
        let tags: IndexSet<String> = pf
            .component(TAGS)
            .map(|t| t.leaf().iter_of(0))
            .unwrap_or_default()
            .iter()
            .filter_map(|t| t.as_str().map(str::to_owned))
            .collect();
        let loot: Vec<J> = deconstruct
            .iter()
            .filter(|(t, _)| t.iter().all(|x| tags.contains(x)))
            .flat_map(|(_, o)| o.iter().cloned())
            .collect();
        if !loot.is_empty() {
            row.insert("deconstruct".into(), J::List(loot));
        }
        let mut recipes: Vec<String> =
            grants.iter().filter(|(k, _)| *k == J::Int(RECIPE)).map(|(_, p)| p.clone()).collect();
        recipes.extend(teaches.get(&collectible).cloned().unwrap_or_default());
        let mut taught = Vec::new();
        let mut seen: IndexSet<String> = IndexSet::new();
        for rid in recipes {
            if seen.insert(rid.clone()) {
                taught.push(obj! {"recipe" => rid.as_str(), "name" => recipe_name(&g, &rid)});
            }
        }
        if !taught.is_empty() {
            row.insert("teaches".into(), J::List(taught));
        }
        let made = made_by
            .get(&rel.to_lowercase())
            .filter(|m| !m.is_empty())
            .or_else(|| made_by.get(&collectible.to_lowercase()));
        if let Some(m) = made.filter(|m| !m.is_empty()) {
            row.insert("crafted_by".into(), J::List(m.clone()));
        }
        if let Some(u) = used_in.get(&rel.to_lowercase()).filter(|u| !u.is_empty()) {
            row.insert("used_in".into(), J::List(u.clone()));
        }
        if adventure_items.contains(&rel.to_lowercase()) {
            row.insert("daily_adventure".into(), J::Bool(true));
        }
        out.push(row);
    }
    let order = |p: &str| match p {
        "path" => 0,
        "boss" => 1,
        "decor" => 2,
        _ => 9,
    };
    out.sort_by_cached_key(|m| {
        let prefab = m["prefab"].as_str().unwrap_or("");
        let head = prefab.get(ROOT.len()..).unwrap_or("").split('/').next().unwrap_or("").to_owned();
        (order(&head), m["name"].as_str().unwrap_or("").to_lowercase())
    });
    Ok(obj! {"categories" => groups, "adventure" => adv, "mementos" => J::List(out.into_iter().map(J::Dict).collect())})
}
