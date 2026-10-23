//! `mastery.py`: the Trove, Geode and PvP Mastery ladders, and what each
//! collectible is worth.

use std::sync::Arc;

use indexmap::IndexMap;

use super::pvp_powerups::all_names;
use crate::decode::{
    ability::{Prefabs, identity},
    common::{Locale, lookup, rs, stat_rows},
    json::{J, Map},
    registry::{Built, Ctx},
    tree::Tree,
    value::{Dict, K, W, WireError, parse, uvar, zz},
};
use crate::obj;

pub const COLLECTION_TYPES: [&str; 19] = [
    "EquipmentAppearance",
    "Recipe",
    "Mount",
    "Pet",
    "Skin",
    "Cart",
    "Flask",
    "Wings",
    "Tome",
    "Boat",
    "Sail",
    "FishingPole",
    "Fish",
    "FlaskEffect",
    "Badge",
    "Aura",
    "GeodeCompanion",
    "SecondarySkin",
    "Memento",
];
const LADDERS: [(&str, &str, &str, Option<&str>); 3] = [
    ("trove", "Trove Mastery", "meta", Some("meta/multipliers")),
    ("geode", "Geode Mastery", "geodemeta", Some("meta/geode_multipliers")),
    ("pvp", "PvP Mastery", "pvpmeta", None),
];
const SKIN: i128 = 4;
const SECONDARY_SKIN: i128 = 17;
const STYLE: i128 = 0;

fn source(ctype: &str) -> Option<i128> {
    Some(match ctype {
        "EquipmentAppearance" => 0,
        "Recipe" => 2,
        "Mount" => 3,
        "Pet" => 4,
        "Skin" => 5,
        "Cart" => 7,
        "Flask" => 12,
        "Wings" => 13,
        "Tome" => 14,
        "Boat" => 15,
        "Sail" => 16,
        "FishingPole" => 17,
        "Fish" => 18,
        "FlaskEffect" => 19,
        "Badge" => 20,
        "Aura" => 22,
        "GeodeCompanion" => 29,
        "Memento" => 21,
        _ => return None,
    })
}

const SOURCE_NAMES: [(i128, &str); 23] = [
    (0, "Style"),
    (1, "Hat style"),
    (2, "Recipe"),
    (3, "Mount"),
    (4, "Ally"),
    (5, "Costume"),
    (7, "Mag Rider"),
    (8, "Class level"),
    (9, "Every 20th class level"),
    (10, "Class at max level"),
    (11, "Profession tier"),
    (12, "Flask"),
    (13, "Wings"),
    (14, "Tome"),
    (15, "Boat"),
    (16, "Sail"),
    (17, "Fishing pole"),
    (18, "Fish"),
    (19, "Emblem"),
    (20, "Badge rank"),
    (21, "Memento"),
    (22, "Aura"),
    (29, "Geode companion"),
];

const END: i128 = 0x0F;
const OBJ: i128 = 0x17;
const ARR: i128 = 0x1F;
const MAP: i128 = 0x27;
const CUSTOM: i128 = 0x2F;
const BIGMAP: i128 = 0x37;
const STRMAP: i128 = 0x3F;

struct Single<'a> {
    d: &'a [u8],
}

fn round6(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.6}").parse().unwrap_or(x)
}

impl Single<'_> {
    fn key(&self, p: usize) -> Result<(i128, i128, usize), WireError> {
        let (k, p) = zz(self.d, p, 35)?;
        if k < 0 {
            return Err(WireError);
        }
        let (mut wt, mut idx) = (k & 7, k >> 3);
        if wt == 7 {
            (wt, idx) = (k & 0x3F, k >> 6);
        }
        Ok((idx, wt, p))
    }

    fn take(&self, p: usize, n: i128) -> Result<&[u8], WireError> {
        if n < 0 || p as i128 + n > self.d.len() as i128 {
            return Err(WireError);
        }
        Ok(&self.d[p..p + n as usize])
    }

    fn scalar(&self, p: usize, wt: i128) -> Result<(W, usize), WireError> {
        match wt {
            0 => zz(self.d, p, 35).map(|(v, q)| (W::Int(v), q)),
            1 => uvar(self.d, p, 35).map(|(v, q)| (W::Int(v), q)),
            2 => {
                let b = self.take(p, 4)?;
                Ok((W::Float(round6(f64::from(f32::from_le_bytes([b[0], b[1], b[2], b[3]])))), p + 4))
            }
            3 => {
                let b: [u8; 8] = self.take(p, 8)?.try_into().map_err(|_| WireError)?;
                if matches!(b[7], 0 | 0xFF) && matches!(b[6], 0 | 0xFF) {
                    Ok((W::Int(i128::from(i64::from_le_bytes(b))), p + 8))
                } else {
                    Ok((W::Float(f64::from_le_bytes(b)), p + 8))
                }
            }
            4 => {
                let (n, q) = uvar(self.d, p, 35)?;
                let t = self.take(q, n)?;
                Ok((W::Str(Arc::from(String::from_utf8_lossy(t).as_ref())), q + n as usize))
            }
            6 => {
                let (n, ewt, mut q) = self.key(p)?;
                if n > 256 || !(0..=3).contains(&ewt) {
                    return Err(WireError);
                }
                let mut out = Vec::new();
                for _ in 0..n {
                    let (v, r) = self.scalar(q, ewt)?;
                    out.push(v);
                    q = r;
                }
                Ok((W::Tuple(out.into()), q))
            }
            CUSTOM => {
                let (n, q) = uvar(self.d, p, 35)?;
                let t = self.take(q, n)?;
                Ok((W::Bytes(Arc::from(t)), q + n as usize))
            }
            _ => Err(WireError),
        }
    }

    fn value(&self, p: usize, wt: i128) -> Result<(W, usize), WireError> {
        match wt {
            OBJ => self.section(p),
            ARR => {
                let (_, ewt, mut q) = self.key(p)?;
                let mut slots: IndexMap<i128, W> = IndexMap::new();
                loop {
                    let (k, r) = zz(self.d, q, 70)?;
                    q = r;
                    let (op, slot) = (k & 7, k >> 3);
                    if op == 4 {
                        let n = slots.keys().copied().max().map_or(0, |m| m + 1);
                        let list: Vec<W> = (0..n).map(|i| slots.get(&i).cloned().unwrap_or(W::None)).collect();
                        return Ok((W::List(list.into()), q));
                    }
                    if op == 0 {
                        slots.insert(slot, W::None);
                        continue;
                    }
                    if op != 2 {
                        return Err(WireError);
                    }
                    let (v, r) = self.value(q, ewt)?;
                    slots.insert(slot, v);
                    q = r;
                }
            }
            MAP | BIGMAP | STRMAP => {
                let (_, vwt, mut q) = self.key(p)?;
                let mut out: IndexMap<K, W> = IndexMap::new();
                loop {
                    let (op, mk) = if wt == MAP {
                        let (k, r) = zz(self.d, q, 70)?;
                        q = r;
                        (k & 7, K::Int(k >> 3))
                    } else {
                        let op = i128::from(*self.d.get(q).ok_or(WireError)?);
                        q += 1;
                        if wt == BIGMAP {
                            let (mk, r) = zz(self.d, q, 70)?;
                            q = r;
                            (op, K::Int(mk))
                        } else {
                            let (n, r) = uvar(self.d, q, 35)?;
                            let t = self.d.get(r..(r + n as usize).min(self.d.len())).ok_or(WireError)?;
                            q = r + n as usize;
                            (op, K::Str(Arc::from(String::from_utf8_lossy(t).as_ref())))
                        }
                    };
                    if op == 4 {
                        return Ok((W::Dict(Arc::new(Dict::new(out.into_iter().collect()))), q));
                    }
                    if op == 0 || op == 3 {
                        out.insert(mk, W::None);
                        continue;
                    }
                    let (v, r) = self.value(q, vwt)?;
                    out.insert(mk, v);
                    q = r;
                }
            }
            _ => self.scalar(p, wt),
        }
    }

    fn section(&self, mut p: usize) -> Result<(W, usize), WireError> {
        let mut out: IndexMap<i128, W> = IndexMap::new();
        loop {
            let (idx, wt, q) = self.key(p)?;
            if wt == END {
                return Ok((W::Dict(Arc::new(Dict::new(out.into_iter().map(|(k, v)| (K::Int(k), v)).collect()))), q));
            }
            let (v, r) = self.value(q, wt)?;
            out.insert(idx, v);
            p = r;
        }
    }
}

/// `_single`: a bare object read on the rule that every object has one section.
pub fn single(data: &[u8]) -> Result<W, WireError> {
    let r = Single { d: data };
    let (root, end) = r.section(0)?;
    if end != data.len() {
        return Err(WireError);
    }
    Ok(root)
}

fn leaf_of(v: Option<&W>) -> Dict {
    v.map(|x| Dict::new(x.leaf().items().to_vec())).unwrap_or_default()
}

fn claims(prefabs: &Prefabs, names: &Locale, stem: &str) -> IndexMap<String, Map> {
    let pf = prefabs.get(&format!("claim/{stem}"));
    let table = pf.as_ref().and_then(|p| p.root.as_ref()).and_then(|r| r.leaf().get(0).cloned());
    let mut out = IndexMap::new();
    if let Some(W::Dict(t)) = table {
        for (cid, node) in t.items() {
            if node.is_obj() {
                let key = match cid {
                    K::Str(s) => s.to_string(),
                    K::Int(i) => i.to_string(),
                };
                out.insert(key, claim(node.section(0).get(1), prefabs, names));
            }
        }
    }
    out
}

/// `_claim`: what a claim hands out.
pub fn claim(c: Option<&W>, prefabs: &Prefabs, names: &Locale) -> Map {
    let mut out = Map::new();
    let Some(c) = c.filter(|c| c.is_obj() && !c.is_empty()) else { return out };
    let (kind, body) = (c.section(0).get(0), c.section(0).get(1));
    let Some(body) = body.filter(|b| b.is_obj() && !b.is_empty()) else { return out };
    let own = if body.sections().len() > 1 { body.leaf() } else { &crate::decode::value::EMPTY };
    let title = lookup(names, body.section(0).get(3).filter(|v| v.truthy()));
    if !title.is_empty() {
        out.insert("name".into(), J::from(title));
    }
    let mut grants: Vec<J> = Vec::new();
    match kind.and_then(W::as_str) {
        Some("ClaimCollectable") => {
            for row in own.iter_of(0) {
                let r = row.leaf();
                if let (Some(t), Some(rf)) = (r.int(0), r.str(1)) {
                    grants.push(obj! {"ref" => reference(t, rf)});
                }
            }
        }
        Some("ClaimPrefab") => {
            for row in own.iter_of(0) {
                let r = row.leaf();
                if let Some(item) = r.str(0) {
                    let item = rs(item, ".binfab");
                    let count = r.get(1).filter(|v| v.truthy()).map_or(J::Int(1), J::from);
                    let name = names.get(identity(prefabs.get(item).as_deref()).name()).cloned().unwrap_or_default();
                    grants.push(obj! {"ref" => item, "count" => count, "name" => name});
                }
            }
        }
        Some("ClaimCompound") => {
            for sub in own.iter_of(0) {
                if let Some(J::List(g)) = claim(Some(&sub), prefabs, names).get("grants") {
                    grants.extend(g.iter().cloned());
                }
            }
        }
        _ => {}
    }
    if !grants.is_empty() {
        out.insert("grants".into(), J::List(grants));
    }
    out
}

pub fn reference(ctype: i128, r: &str) -> String {
    let r = rs(r, ".binfab");
    if ctype == SKIN && !r.contains('/') {
        return format!("skins/{r}");
    }
    if ctype == SECONDARY_SKIN && !r.contains('/') {
        return format!("skins/secondary/{r}");
    }
    r.to_owned()
}

fn dict_values(v: Option<&W>) -> Vec<W> {
    v.map(W::iter).unwrap_or_default()
}

fn ladder(slug: &str, name: &str, stem: &str, tree: &Tree, prefabs: &Prefabs, names: &Locale) -> Option<Map> {
    let root = single(&tree.read(&format!("prefabs/meta/{stem}.binfab")).unwrap_or_default()).ok()?;
    let root = root.as_dict()?;
    let claims = claims(prefabs, names, if stem == "pvpmeta" { "pvp" } else { stem });
    let mut levels = Vec::new();
    if let Some(W::Dict(lv)) = root.get(1).filter(|v| v.truthy()) {
        let mut items: Vec<&(K, W)> = lv.items().iter().collect();
        items.sort_by(|a, b| match (&a.0, &b.0) {
            (K::Int(x), K::Int(y)) => x.cmp(y),
            (K::Str(x), K::Str(y)) => x.cmp(y),
            (K::Int(_), K::Str(_)) => std::cmp::Ordering::Less,
            (K::Str(_), K::Int(_)) => std::cmp::Ordering::Greater,
        });
        for (k, row) in items {
            let K::Int(l) = k else { continue };
            let Some(row) = row.as_dict().filter(|_| *l >= 1) else { continue };
            let mut entry = Map::new();
            entry.insert("level".into(), J::Int(*l));
            entry.insert("points".into(), row.get(0).filter(|v| v.truthy()).map_or(J::Int(0), J::from));
            let ids: Vec<J> = row.iter_of(1).iter().filter_map(W::as_int).map(J::Int).collect();
            if !ids.is_empty() {
                entry.insert("rewards".into(), J::List(ids));
            }
            levels.push(J::Dict(entry));
        }
    }
    let mut rewards = Vec::new();
    for rw in dict_values(root.get(3).filter(|v| v.truthy())) {
        let Some(rw) = rw.as_dict() else { continue };
        let Some(id) = rw.int(0) else { continue };
        let ckey = match rw.get(4).filter(|v| v.truthy()) {
            Some(W::Str(s)) => s.to_string(),
            _ => String::new(),
        };
        let claim = claims.get(&ckey).cloned().unwrap_or_default();
        let t = lookup(names, rw.get(1).filter(|v| v.truthy()));
        let text = if t.is_empty() { claim.get("name").cloned().unwrap_or(J::from("")) } else { J::from(t) };
        let mut out = Map::new();
        out.insert("id".into(), J::Int(id));
        out.insert("text".into(), text);
        let stats = stat_rows(&dict_values(rw.get(5).filter(|v| v.truthy())));
        if !stats.is_empty() {
            out.insert("stats".into(), J::List(stats));
        }
        if let Some(g) = claim.get("grants").filter(|g| g.truthy()) {
            out.insert("grants".into(), g.clone());
        }
        if let Some(icon) = rw.str(2).filter(|s| !s.is_empty()) {
            out.insert("icon".into(), J::from(icon));
        }
        rewards.push(J::Dict(out));
    }
    let every = |rows: Option<&W>| -> Vec<J> {
        let mut out = Vec::new();
        for r in dict_values(rows.filter(|v| v.truthy())) {
            if let Some(d) = r.as_dict()
                && d.get(3).and_then(W::as_int) == Some(1)
            {
                out.extend(stat_rows(&dict_values(d.get(5).filter(|v| v.truthy()))));
            }
        }
        out
    };
    let sources: Vec<J> = dict_values(root.get(0).filter(|v| v.truthy()))
        .iter()
        .filter_map(|r| {
            let d = r.as_dict()?;
            let (s, p) = (d.int(0)?, d.int(1)?);
            Some(obj! {"source" => s, "points" => p})
        })
        .collect();
    let mut m = Map::new();
    m.insert("slug".into(), J::from(slug));
    m.insert("name".into(), J::from(name));
    m.insert("sources".into(), J::List(sources));
    m.insert("levels".into(), J::List(levels));
    m.insert("rewards".into(), J::List(rewards));
    m.insert("every_level".into(), J::List(every(root.get(2))));
    m.insert("cap".into(), root.get(5).filter(|v| v.truthy()).map_or(J::Int(0), J::from));
    m.insert("past_cap".into(), J::List(every(root.get(6))));
    Some(m)
}

fn values(
    prefabs: &Prefabs,
    stem: Option<&str>,
    bases: &IndexMap<i128, i128>,
    members: &IndexMap<String, (i128, bool)>,
) -> IndexMap<String, i128> {
    let pf = stem.and_then(|s| prefabs.get(s));
    let root = leaf_of(pf.as_ref().and_then(|p| p.root.as_ref()));
    let groups: Vec<Dict> = root.iter_of(0).iter().map(|g| Dict::new(g.leaf().items().to_vec())).collect();
    let default = root.get(1);
    let mult = |g: &Dict| g.get(0).filter(|v| v.truthy()).and_then(W::as_num).unwrap_or(0.0);
    let mut listed: IndexMap<String, f64> = IndexMap::new();
    for g in &groups {
        for row in g.iter_of(1) {
            let r = row.leaf();
            if let (Some(t), Some(rf)) = (r.int(0), r.str(1)) {
                listed.entry(reference(t, rf)).or_insert_with(|| mult(g));
            }
        }
    }
    let fallback = match default {
        Some(W::Int(d)) if (0..groups.len() as i128).contains(d) => mult(&groups[*d as usize]),
        _ => 0.0,
    };
    let mut out = IndexMap::new();
    for (r, (ctype, counted)) in members {
        let Some(src) = usize::try_from(*ctype).ok().and_then(|c| COLLECTION_TYPES.get(c)).and_then(|c| source(c))
        else {
            continue;
        };
        let Some(base) = bases.get(&src) else { continue };
        if !counted {
            continue;
        }
        let v = (*base as f64 * listed.get(r).copied().unwrap_or(fallback)).round_ties_even() as i128;
        if v != 0 {
            out.insert(r.clone(), v);
        }
    }
    out
}

fn members(tree: &Tree) -> IndexMap<String, (i128, bool)> {
    let mut out: IndexMap<String, (i128, bool)> = IndexMap::new();
    for path in tree.files("prefabs/collections/", ".binfab") {
        let stem = rs(path.rsplit('/').next().unwrap_or(&path), ".binfab").to_owned();
        let Some(tail) = stem.strip_prefix("collection_") else { continue };
        let Some(ctype) = COLLECTION_TYPES.iter().position(|t| t.to_lowercase() == tail) else { continue };
        let ctype = ctype as i128;
        if ctype == STYLE {
            continue;
        }
        let Ok(pf) = parse(&tree.read(&path).unwrap_or_default()) else { continue };
        let root = leaf_of(pf.root.as_ref());
        for group in root.iter_of(0) {
            let g = group.leaf();
            for row in g.iter_of(3) {
                if let Some(r) = row.leaf().str(0) {
                    let r = reference(ctype, r);
                    let counted = g.get(2).is_none_or(|v| J::from(v) != J::Int(0));
                    let prev = out.get(&r).is_some_and(|x| x.1);
                    out.insert(r, (ctype, prev || counted));
                }
            }
        }
    }
    out
}

pub fn build(ctx: &Ctx) -> Built {
    let tree = &ctx.tree;
    let prefabs = Prefabs::new(tree);
    let names = all_names(tree);
    let ladders: Vec<Map> =
        LADDERS.iter().filter_map(|(slug, name, stem, _)| ladder(slug, name, stem, tree, &prefabs, &names)).collect();
    let members = members(tree);
    let mut vals: IndexMap<String, Map> = IndexMap::new();
    for (slug, _, _, mult) in LADDERS {
        let Some(l) = ladders.iter().find(|x| x.get("slug").and_then(J::as_str) == Some(slug)) else { continue };
        let Some(mult) = mult else { continue };
        let mut bases: IndexMap<i128, i128> = IndexMap::new();
        for s in l.get("sources").and_then(J::as_list).into_iter().flatten() {
            if let (Some(a), Some(b)) = (s.get("source").and_then(J::as_int), s.get("points").and_then(J::as_int)) {
                bases.insert(a, b);
            }
        }
        for (r, v) in values(&prefabs, Some(mult), &bases, &members) {
            vals.entry(r).or_default().insert(slug.into(), J::Int(v));
        }
    }
    let mut sorted: Vec<(String, Map)> = vals.into_iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let source_names: Map = SOURCE_NAMES.iter().map(|(k, v)| (k.to_string(), J::from(*v))).collect();
    Ok(obj! {
        "ladders" => J::List(ladders.into_iter().map(J::Dict).collect()),
        "source_names" => source_names,
        "collectibles" => J::Dict(sorted.into_iter().map(|(k, v)| (k, J::Dict(v))).collect()),
    })
}
