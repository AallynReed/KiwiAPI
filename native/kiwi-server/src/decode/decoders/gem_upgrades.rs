//! `gem_upgrades.py`: gem stat rolls, level-up odds and costs, boosters and focuses.

use std::sync::OnceLock;

use indexmap::{IndexMap, IndexSet};
use regex::Regex;

use super::pvp_powerups::all_names;
use crate::decode::{
    ability::{Prefabs, identity},
    common::{r4, resolve_stat_name, rp, rs, stat_key},
    json::{J, Map},
    registry::{Built, Ctx},
    tree::Tree,
    value::{Dict, K, W},
};
use crate::{obj, pyfloat};

const GEM: i128 = 361;
const STAT_STEP: i128 = 3;
const BOOSTER: i128 = 367;

fn item_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"^prefabs/item/gem/(small|large)/(\w+?)_(?:.*_)?t(\d+)(_concat\d+)?\.binfab$").expect("regex")
    })
}

fn dust_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^item/crafting/gemdust_(\w+)$").expect("regex"))
}

fn objs(v: Option<&W>) -> Vec<Dict> {
    match v {
        Some(x) if x.is_list() => {
            x.elems().iter().filter(|o| o.is_obj()).map(|o| Dict::new(o.leaf().items().to_vec())).collect()
        }
        _ => Vec::new(),
    }
}

fn int_of(v: Option<&W>, default: i128) -> i128 {
    match v {
        Some(W::Int(i)) => *i,
        Some(W::Float(f)) => f.trunc() as i128,
        _ => default,
    }
}

fn float_of(v: Option<&W>, default: f64) -> f64 {
    v.and_then(W::as_num).unwrap_or(default)
}

fn items(v: Option<&W>) -> Vec<J> {
    objs(v)
        .iter()
        .filter(|r| r.str(0).is_some() && r.get(1).is_some_and(W::truthy))
        .map(|r| obj! {"item" => r.str(0).unwrap_or(""), "count" => int_of(r.get(1), 0)})
        .collect()
}

fn stat(sid: Option<&W>, add: Option<&W>) -> (String, bool) {
    let sid = sid.and_then(W::as_int).unwrap_or(0);
    let k = stat_key(sid);
    let percent = add.map_or(J::Int(1), J::from) == J::Int(0);
    (k.strip_prefix("$Stat_").unwrap_or(&k).to_owned(), percent)
}

fn value(stat: &str, percent: bool, raw: f64) -> J {
    let v = if percent {
        raw * 100.0
    } else if stat == "CriticalHitChance" {
        raw * 0.1
    } else {
        raw
    };
    J::Float(r4(v))
}

fn pool(rows: Option<&W>) -> Vec<(String, bool, f64)> {
    objs(rows)
        .iter()
        .map(|r| {
            let (s, p) = stat(r.get(0), r.get(1));
            (s, p, float_of(r.get(2), 0.0))
        })
        .collect()
}

type Pool = Vec<(String, bool, f64)>;
type Items = IndexMap<i128, Vec<J>>;
type Pools = IndexMap<i128, Vec<J>>;

fn gem_items(tree: &Tree, prefabs: &Prefabs) -> (Items, Pools) {
    let mut items: IndexMap<i128, IndexSet<(String, String, i128)>> = IndexMap::new();
    let mut pools: IndexMap<i128, Vec<(Pool, usize)>> = IndexMap::new();
    for path in tree.files("prefabs/item/gem/", ".binfab") {
        let lower = path.to_lowercase();
        let Some(m) = item_re().captures(&lower) else { continue };
        let pf = prefabs.get(rp(&path, "prefabs/"));
        let comp = pf
            .as_ref()
            .and_then(|p| p.component(GEM))
            .map(|c| Dict::new(c.leaf().items().to_vec()))
            .unwrap_or_default();
        let Some(key) = comp.int(0) else { continue };
        let tier: i128 = m[3].parse().unwrap_or(0);
        items.entry(key).or_default().insert((m[1].to_owned(), m[2].to_owned(), tier));
        let class_gem = comp.get(2).map_or(J::Int(-1), J::from) != J::Int(-1);
        if m.get(4).is_some_and(|c| !c.as_str().is_empty()) || class_gem {
            continue;
        }
        let p = pool(comp.get(3).map(W::leaf).and_then(|l| l.get(1)));
        let counter = pools.entry(key).or_default();
        let same = |a: &Vec<(String, bool, f64)>| {
            a.len() == p.len() && a.iter().zip(&p).all(|(x, y)| x.0 == y.0 && x.1 == y.1 && x.2 == y.2)
        };
        match counter.iter_mut().find(|(k, _)| same(k)) {
            Some((_, n)) => *n += 1,
            None => counter.push((p, 1)),
        }
    }
    let out_items: Items = items
        .into_iter()
        .map(|(k, v)| {
            let mut v: Vec<_> = v.into_iter().collect();
            v.sort();
            (k, v.into_iter().map(|(s, c, t)| obj! {"size" => s, "color" => c, "tier" => t}).collect())
        })
        .collect();
    let out_pools: Pools = pools
        .into_iter()
        .map(|(k, c)| {
            let mut best: Option<&(Pool, usize)> = None;
            for e in &c {
                if best.is_none_or(|b| e.1 > b.1) {
                    best = Some(e);
                }
            }
            let rows = best
                .map(|b| {
                    b.0.iter().map(|(s, p, w)| obj! {"stat" => s.as_str(), "percent" => *p, "weight" => *w}).collect()
                })
                .unwrap_or_default();
            (k, rows)
        })
        .collect();
    (out_items, out_pools)
}

fn levels(levels: Option<&W>) -> Vec<Map> {
    let Some(W::Dict(d)) = levels.filter(|v| v.truthy()) else { return Vec::new() };
    let mut keys: Vec<i128> = d
        .items()
        .iter()
        .filter(|(k, v)| matches!(k, K::Int(_)) && v.is_obj())
        .filter_map(|(k, _)| match k {
            K::Int(i) => Some(*i),
            K::Str(_) => None,
        })
        .collect();
    keys.sort();
    let mut out = Vec::new();
    for level in keys {
        if level < 2 {
            continue;
        }
        let e = d.get(level).map(W::leaf).expect("level");
        let odds = d.get(level - 1).map(W::leaf).unwrap_or(&crate::decode::value::EMPTY);
        let rewards: Vec<i128> = e.iter_of(5).iter().filter_map(W::as_int).collect();
        let kind = e.get(10).map_or(J::Int(0), J::from);
        let dust = if kind == J::Int(0) || kind == J::Int(2) { int_of(e.get(7), 0) } else { 0 };
        let amber = if kind == J::Int(1) || kind == J::Int(2) { int_of(e.get(11), 0) } else { 0 };
        let mut m = Map::new();
        m.insert("level".into(), J::Int(level));
        m.insert("chance".into(), J::Float(pyfloat::round(float_of(odds.get(4), 0.0), 6)));
        m.insert("double_chance".into(), J::Float(pyfloat::round(float_of(odds.get(3), 0.0), 6)));
        m.insert("stat_steps".into(), J::from(rewards.iter().filter(|r| **r == STAT_STEP).count()));
        m.insert("boost".into(), J::Bool(rewards.iter().any(|r| *r == 1 || *r == 2)));
        m.insert("cost".into(), J::List(items(e.get(0))));
        m.insert("dust".into(), J::Int(dust));
        m.insert("amber".into(), J::Int(amber));
        out.push(m);
    }
    out
}

fn gem(key: i128, rec: &Dict, gem_items: Vec<J>, pool: Option<Vec<J>>) -> Map {
    let mut stats = Vec::new();
    for r in objs(rec.get(0)) {
        let (s, percent) = stat(r.get(0), r.get(1));
        let rng = r.get(3).map(W::leaf).unwrap_or(&crate::decode::value::EMPTY);
        let mut m = Map::new();
        m.insert("stat".into(), J::from(s.as_str()));
        m.insert("name".into(), J::from(resolve_stat_name(&format!("$Stat_{s}"))));
        m.insert("percent".into(), J::Bool(percent));
        for (i, k) in ["min", "max", "step"].iter().enumerate() {
            m.insert((*k).into(), value(&s, percent, float_of(rng.get(i as i128), 0.0)));
        }
        stats.push(J::Dict(m));
    }
    let lv = levels(rec.get(2));
    let tier_up = rec.get(8).map(W::leaf).unwrap_or(&crate::decode::value::EMPTY);
    let up_item = items(rec.get(9));
    let max_level = lv.iter().filter_map(|l| l.get("level").and_then(J::as_int)).max().unwrap_or(1);
    let tier_upgrade = if tier_up.get(0).map(J::from) == Some(J::Int(2)) && !up_item.is_empty() {
        obj! {"level" => tier_up.get(1).map_or(J::Null, J::from), "item" => up_item[0].get("item").cloned().unwrap_or(J::Null)}
    } else {
        J::Null
    };
    let mut out = Map::new();
    out.insert("key".into(), J::Int(key));
    out.insert("items".into(), J::List(gem_items));
    out.insert("max_level".into(), J::Int(max_level));
    out.insert("stats".into(), J::List(stats));
    out.insert("pool".into(), J::List(pool.unwrap_or_default()));
    out.insert("levels".into(), J::List(lv.into_iter().map(J::Dict).collect()));
    out.insert("reselect_cost".into(), J::List(items(rec.get(6))));
    out.insert("tier_upgrade".into(), tier_upgrade);
    out
}

fn boosters(tree: &Tree, prefabs: &Prefabs) -> (Vec<J>, Vec<J>) {
    let slots = prefabs.get("gems/meta/gem_insurancedata");
    let usable: Vec<J> = slots
        .as_ref()
        .and_then(|s| s.root.as_ref())
        .map(|r| r.leaf().iter_of(0))
        .unwrap_or_default()
        .iter()
        .map(|r| r.leaf().get(0).map_or(J::Null, J::from))
        .collect();
    let (mut bs, mut fs) = (Vec::new(), Vec::new());
    for path in tree.files("prefabs/item/gem/booster/", ".binfab") {
        let rel = rs(rp(&path, "prefabs/"), ".binfab");
        let pf = prefabs.get(rel);
        let b = pf
            .as_ref()
            .and_then(|p| p.component(BOOSTER))
            .map(|c| Dict::new(c.leaf().items().to_vec()))
            .unwrap_or_default();
        let kind = b.get(4).map(J::from);
        if kind == Some(J::Int(3)) && b.get(5).is_some_and(W::truthy) {
            fs.push(obj! {"item" => rel, "amount" => r4(float_of(b.get(5), 0.0) * 100.0)});
        } else if kind == Some(J::Int(2)) && usable.contains(&J::from(rel)) {
            bs.push(obj! {"item" => rel, "chance_multiplier" => float_of(b.get(2), 1.0), "double_multiplier" => float_of(b.get(1), 1.0)});
        }
    }
    (bs, fs)
}

pub fn build(ctx: &Ctx) -> Built {
    let tree = &ctx.tree;
    let prefabs = Prefabs::new(tree);
    let root = prefabs.get("gems/meta/gem_upgradedata");
    let table = root.as_ref().and_then(|r| r.root.as_ref()).and_then(|r| r.get(0).filter(|v| v.truthy()).cloned());
    let (gem_items, pools) = gem_items(tree, &prefabs);
    let mut gems = Vec::new();
    if let Some(W::Dict(t)) = &table {
        let mut entries: Vec<(i128, &W)> = t
            .items()
            .iter()
            .filter_map(|(k, v)| match k {
                K::Int(i) if v.is_obj() => Some((*i, v)),
                _ => None,
            })
            .collect();
        entries.sort_by_key(|(k, _)| *k);
        for (k, v) in entries {
            gems.push(gem(k, v.leaf(), gem_items.get(&k).cloned().unwrap_or_default(), pools.get(&k).cloned()));
        }
    }
    let mats = prefabs.get("gems/meta/gem_upgradematerialslist");
    let mut materials = Map::new();
    if let Some(r) = mats.as_ref().and_then(|m| m.root.as_ref()) {
        for row in r.leaf().values() {
            let l = row.leaf();
            let (dust, amber) = (l.str(0), l.get(1));
            if let Some(m) = dust.and_then(|d| dust_re().captures(d))
                && amber.is_some_and(W::truthy)
            {
                materials.insert(
                    m[1].to_owned(),
                    obj! {"dust" => dust.unwrap_or(""), "amber" => amber.map_or(J::Null, J::from)},
                );
            }
        }
    }
    let (bs, fs) = boosters(tree, &prefabs);
    let text = all_names(tree);
    let mut wanted: IndexSet<String> = IndexSet::new();
    let s = |j: Option<&J>| j.and_then(J::as_str).map(str::to_owned);
    for g in &gems {
        for l in g.get("levels").and_then(J::as_list).into_iter().flatten() {
            for i in l.get("cost").and_then(J::as_list).into_iter().flatten() {
                wanted.extend(s(i.get("item")));
            }
        }
        for i in g.get("reselect_cost").and_then(J::as_list).into_iter().flatten() {
            wanted.extend(s(i.get("item")));
        }
        if let Some(t) = g.get("tier_upgrade").filter(|t| t.truthy()) {
            wanted.extend(s(t.get("item")));
        }
    }
    for m in materials.values() {
        for v in m.as_dict().into_iter().flat_map(|d| d.values()) {
            wanted.extend(v.as_str().map(str::to_owned));
        }
    }
    for b in bs.iter().chain(fs.iter()) {
        wanted.extend(s(b.get("item")));
    }
    let mut wanted: Vec<String> = wanted.into_iter().collect();
    wanted.sort();
    let mut names = Map::new();
    for item in wanted {
        let key = identity(prefabs.get(&item).as_deref()).name_key;
        if let Some(t) = key.and_then(|k| text.get(&k).filter(|t| !t.is_empty()).cloned()) {
            names.insert(item, J::from(t));
        }
    }
    Ok(obj! {
        "gems" => J::List(gems.into_iter().map(J::Dict).collect()),
        "materials" => materials, "boosters" => bs, "focuses" => fs, "names" => names,
    })
}
