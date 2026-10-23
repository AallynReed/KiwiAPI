//! `app/trove/decode/ability.py`: what an ability does, read structurally from
//! its prefab chain (action costs, damage and healing parameters, effects).

use std::{cell::RefCell, collections::HashMap, sync::Arc};

use indexmap::IndexMap;

use super::{
    common::*,
    json::{J, Map},
    tree::Tree,
    value::{Dict, Prefab, W, parse},
};
use crate::obj;

pub const WALK_LIMIT: usize = 80;
pub const ABILITIES: &[&str] = &["abilities/"];

/// Parsed prefabs of a tree by logical path (`abilities/x/y`), cached.
pub struct Prefabs {
    pub tree: Tree,
    cache: RefCell<HashMap<String, Option<Arc<Prefab>>>>,
    vfx: RefCell<Option<Arc<IndexMap<String, String>>>>,
    identity_only: bool,
}

impl Prefabs {
    pub fn new(tree: &Tree) -> Prefabs {
        Prefabs { tree: tree.clone(), cache: RefCell::default(), vfx: RefCell::default(), identity_only: false }
    }

    /// `quests._Identities`: each prefab reduced to its identity component.
    pub fn identities(tree: &Tree) -> Prefabs {
        Prefabs { identity_only: true, ..Prefabs::new(tree) }
    }

    pub fn get(&self, rel: &str) -> Option<Arc<Prefab>> {
        let rel = rs(rel, ".binfab");
        if let Some(hit) = self.cache.borrow().get(rel) {
            return hit.clone();
        }
        let data = self.tree.read(&format!("prefabs/{rel}.binfab")).filter(|d| !d.is_empty());
        let parsed = if self.identity_only {
            data.map(|d| Prefab {
                entity: true,
                components: super::decoders::recipes::components(&d, &[IDENTITY]).into_iter().collect(),
                ..Prefab::default()
            })
        } else {
            data.and_then(|d| parse(&d).ok())
        };
        let parsed = parsed.map(Arc::new);
        self.cache.borrow_mut().insert(rel.to_owned(), parsed.clone());
        parsed
    }

    pub fn exists(&self, rel: &str) -> bool {
        self.tree.exists(&format!("prefabs/{}.binfab", rs(rel, ".binfab")))
    }

    /// `$VFX_KEY -> Particles/....pkfx` from `prefabs/sfx/sfx.binfab`.
    pub fn vfx_table(&self) -> Arc<IndexMap<String, String>> {
        if let Some(t) = self.vfx.borrow().as_ref() {
            return t.clone();
        }
        let mut out = IndexMap::new();
        if let Some(pf) = self.get("sfx/sfx") {
            for rows in pf.root_leaf().values() {
                if !rows.is_list() {
                    continue;
                }
                for row in rows.elems() {
                    let leaf = row.leaf();
                    if let (Some(k), Some(p)) = (leaf.str(0), leaf.str(1))
                        && p.to_lowercase().ends_with(".pkfx")
                    {
                        out.entry(k.to_owned()).or_insert_with(|| p.to_owned());
                    }
                }
            }
        }
        let out = Arc::new(out);
        *self.vfx.borrow_mut() = Some(out.clone());
        out
    }
}

/// `walk_values` over every component, then the root.
pub fn component_values(pf: &Prefab, f: &mut impl FnMut(&W)) {
    for (_, obj) in &pf.components {
        obj.walk(f);
    }
    if let Some(root) = &pf.root {
        root.walk(f);
    }
}

/// Prefab paths under `prefix` this prefab names, in order, deduped.
pub fn refs(pf: &Prefab, prefix: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    component_values(pf, &mut |v| {
        if let W::Str(s) = v
            && prefix.iter().any(|p| s.starts_with(p))
        {
            let r = rs(s, ".binfab");
            if !out.iter().any(|o| o == r) {
                out.push(r.to_owned());
            }
        }
    });
    out
}

#[derive(Default, Clone, Debug)]
pub struct Ident {
    pub name_key: Option<String>,
    pub description_key: Option<String>,
    pub icon: Option<String>,
}

impl Ident {
    pub fn is_empty(&self) -> bool {
        self.name_key.is_none() && self.description_key.is_none() && self.icon.is_none()
    }

    /// `ident.get("name_key", "")`.
    pub fn name(&self) -> &str {
        self.name_key.as_deref().unwrap_or("")
    }

    pub fn desc(&self) -> &str {
        self.description_key.as_deref().unwrap_or("")
    }
}

fn nonempty(d: &Dict, i: i128) -> Option<String> {
    d.str(i).filter(|s| !s.is_empty()).map(str::to_owned)
}

/// `{name_key, description_key, icon}` from the identity component.
pub fn identity(pf: Option<&Prefab>) -> Ident {
    let Some(obj) = pf.and_then(|p| p.component(IDENTITY)) else { return Ident::default() };
    let leaf = obj.leaf();
    Ident {
        name_key: nonempty(leaf, ID_NAME),
        description_key: nonempty(leaf, ID_DESCRIPTION),
        icon: nonempty(leaf, ID_ICON),
    }
}

fn dollar(d: &Dict, i: i128) -> Option<String> {
    d.str(i).filter(|s| s.starts_with('$')).map(str::to_owned)
}

/// An ability's own name/description keys: identity, else its first effect's base.
pub fn text_keys(pf: Option<&Prefab>) -> Ident {
    let ident = identity(pf);
    let Some(pf) = pf else { return ident };
    if !ident.is_empty() {
        return ident;
    }
    for (cid, obj) in &pf.components {
        if !is_effect(*cid) || obj.is_empty() {
            continue;
        }
        let base = obj.section(0);
        let out = Ident {
            name_key: dollar(base, EFFECT_NAME),
            description_key: dollar(base, EFFECT_DESCRIPTION),
            icon: None,
        };
        if !out.is_empty() {
            return out;
        }
    }
    Ident::default()
}

fn num(v: Option<&W>) -> J {
    match v {
        Some(W::Int(i)) => J::Int(*i),
        Some(W::Float(f)) => J::Float(r4(*f)),
        _ => J::Int(0),
    }
}

/// Energy cost and cooldown from the prefab's action component.
pub fn action(pf: Option<&Prefab>) -> Option<Map> {
    for (cid, obj) in &pf?.components {
        let Some((energy, cooldown)) = action_spec(*cid) else { continue };
        let section = obj.section(ACTION_SECTION);
        let mut out = Map::new();
        out.insert("component".into(), J::Int(*cid));
        if let Some(i) = energy {
            out.insert("energy".into(), num(Some(section.get(i).unwrap_or(&W::Int(0)))));
        }
        if let Some(i) = cooldown {
            out.insert("cooldown".into(), num(Some(section.get(i).unwrap_or(&W::Int(0)))));
        }
        return Some(out);
    }
    None
}

fn labelled(leaf: &Dict, names: &[(i128, &str)]) -> Map {
    let mut out = Map::new();
    for (idx, name) in names {
        let v = match leaf.get(*idx) {
            Some(W::Int(i)) if *i != 0 => J::Int(*i),
            Some(W::Float(f)) if *f != 0.0 => J::Float(r4(*f)),
            Some(W::Str(s)) if !s.is_empty() => J::Str(s.to_string()),
            _ => continue,
        };
        out.insert((*name).into(), v);
    }
    out
}

pub fn is_damage(v: &W) -> bool {
    let Some(d) = v.as_dict() else { return false };
    DAMAGE_SIGNATURE.iter().all(|i| d.has_key(*i))
        && DAMAGE_INTS.iter().all(|i| d.get(*i).is_none_or(|x| matches!(x, W::Int(_))))
        && DAMAGE_FLOATS.iter().all(|i| d.get(*i).is_none_or(|x| matches!(x, W::Float(_))))
}

/// Labelled DamageParameters under a prefab that actually deal damage.
pub fn damage_records(pf: &Prefab) -> Vec<Map> {
    let mut out: Vec<Map> = Vec::new();
    component_values(pf, &mut |v| {
        if is_damage(v) {
            let rec = labelled(v.as_dict().expect("dict"), &DAMAGE);
            let hit = ["damage", "multiplier", "max_health_percent"].iter().any(|k| rec.get(*k).is_some_and(J::truthy));
            if hit && !out.iter().any(|o| J::Dict(o.clone()) == J::Dict(rec.clone())) {
                out.push(rec);
            }
        }
    });
    out
}

pub fn is_healing(v: &W) -> bool {
    let Some(d) = v.as_dict() else { return false };
    (1..=7).all(|i| d.has_key(i))
        && d.items().iter().all(|(k, _)| matches!(k, super::value::K::Int(i) if (0..9).contains(i)))
        && d.str(5).is_some()
        && d.str(6).is_some()
        && matches!(d.get(1), Some(W::Float(_)))
        && matches!(d.get(3), Some(W::Float(_)))
}

pub fn healing_records(pf: &Prefab) -> Vec<Map> {
    let mut out: Vec<Map> = Vec::new();
    for (_, obj) in &pf.components {
        obj.walk(&mut |v| {
            if is_healing(v) {
                let rec = labelled(v.as_dict().expect("dict"), &HEALING);
                let hit = ["health", "max_health_percent", "energy", "max_energy_percent"]
                    .iter()
                    .any(|k| matches!(rec.get(*k), Some(J::Int(_) | J::Float(_))));
                if hit && !out.iter().any(|o| J::Dict(o.clone()) == J::Dict(rec.clone())) {
                    out.push(rec);
                }
            }
        });
    }
    out
}

/// Stat modifier records, in the units the site's other stat rows use.
pub fn modifiers(values: &[W]) -> Vec<J> {
    let mut out: Vec<J> = Vec::new();
    for v in values {
        if !is_modifier(v) {
            continue;
        }
        let d = v.as_dict().expect("dict");
        let (ordinal, op) = (d.int0(MOD_STAT), d.int0(MOD_OP));
        if !(0..7).contains(&op) {
            continue;
        }
        let mut key = stat_key(ordinal);
        let amount = d.get(MOD_VALUE).and_then(W::as_num).unwrap_or(0.0);
        let (mut shown, mut percent) = normalize(&key, op * 2, amount);
        if op == 1 && (key == "$Stat_IncomingDamageMod" || key == "$Stat_OutgoingDamageMod") && amount.abs() <= 1.0 {
            (shown, percent) = (amount * 100.0, true);
        }
        if op == 0 {
            key = bonus_key(&key);
        }
        let mut row = obj! {
            "stat" => key.as_str(), "name" => resolve_stat_name(&key), "op" => MOD_OPS[op as usize],
            "value" => r4(shown), "percent" => percent, "amount" => r4(amount),
        };
        let flags = d.int(MOD_FLAGS).unwrap_or(0);
        let modes: Vec<&str> = MOD_FLAG_NAMES.iter().filter(|(bit, _)| flags & bit != 0).map(|(_, n)| *n).collect();
        if !modes.is_empty() && !(modes.contains(&"PVE") && modes.contains(&"PVP")) {
            row.as_dict_mut().expect("dict").insert("modes".into(), J::List(modes.into_iter().map(J::from).collect()));
        }
        if !out.contains(&row) {
            out.push(row);
        }
    }
    out
}

/// Each effect component: how long it lasts and the stats it changes.
pub fn effects(pf: &Prefab) -> Vec<Map> {
    let mut out = Vec::new();
    for (cid, obj) in &pf.components {
        if !is_effect(*cid) || obj.is_empty() {
            continue;
        }
        let base = obj.section(0);
        let mut row = Map::new();
        row.insert("component".into(), J::Int(*cid));
        if let Some(k) = dollar(base, EFFECT_NAME) {
            row.insert("name_key".into(), J::Str(k));
        }
        if let Some(k) = dollar(base, EFFECT_DESCRIPTION) {
            row.insert("description_key".into(), J::Str(k));
        }
        if let Some(dur) = base.get(EFFECT_DURATION).and_then(W::as_num)
            && dur > 0.0
            && dur < 1e6
        {
            row.insert("duration".into(), J::Float(r4(dur)));
        }
        let stats = modifiers(&walk_values(obj));
        if !stats.is_empty() {
            row.insert("stats".into(), J::List(stats));
        }
        if row.keys().any(|k| !matches!(k.as_str(), "component" | "name_key" | "description_key")) {
            out.push(row);
        }
    }
    out
}

/// A combat-event proc's own cooldown, in seconds.
pub fn proc_cooldown(pf: Option<&Prefab>) -> Option<f64> {
    let obj = pf?.component(PROC_SPAWNER)?;
    if obj.sections().len() <= PROC_SECTION {
        return None;
    }
    let v = obj.section(PROC_SECTION).get(PROC_COOLDOWN)?.as_num()?;
    (v > 0.0).then(|| r4(v))
}

fn visuals(pf: &Prefab) -> (Vec<String>, Vec<String>) {
    let (mut vfx, mut sfx): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    component_values(pf, &mut |v| {
        if let W::Str(s) = v {
            if s.starts_with("$VFX_") {
                if !vfx.iter().any(|x| **x == **s) {
                    vfx.push(s.to_string());
                }
            } else if s.starts_with("$SFX_") && !sfx.iter().any(|x| **x == **s) {
                sfx.push(s.to_string());
            }
        }
    });
    (vfx, sfx)
}

/// `root` and every ability prefab it reaches, breadth first, with depth.
pub fn walk(
    prefabs: &Prefabs,
    root: &str,
    stop: &[String],
    limit: usize,
    prefix: &[&str],
) -> Vec<(String, Arc<Prefab>, usize)> {
    let mut out = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::from([root.to_owned()]);
    let mut queue: std::collections::VecDeque<(String, usize)> =
        std::collections::VecDeque::from([(root.to_owned(), 0)]);
    while out.len() < limit {
        let Some((rel, depth)) = queue.pop_front() else { break };
        let Some(pf) = prefabs.get(&rel) else { continue };
        for child in refs(&pf, prefix) {
            if !seen.contains(&child) && (child == root || !stop.contains(&child)) {
                seen.insert(child.clone());
                queue.push_back((child, depth + 1));
            }
        }
        out.push((rel, pf, depth));
    }
    out
}

/// A readable label from the prefab stem.
pub fn stage_name(rel: &str) -> String {
    title(&rel.rsplit('/').next().unwrap_or(rel).replace('_', " "))
}

fn with_head(head: &[(&str, J)], rest: &Map) -> Map {
    let mut m = Map::new();
    for (k, v) in head {
        m.insert((*k).into(), v.clone());
    }
    for (k, v) in rest {
        m.insert(k.clone(), v.clone());
    }
    m
}

/// Everything the chain under `root` does.
pub fn describe(prefabs: &Prefabs, root: &str, stop: &[String], prefix: &[&str]) -> Map {
    let nodes = walk(prefabs, root, stop, WALK_LIMIT, prefix);
    let mut out = Map::new();
    out.insert("prefab".into(), J::from(root));
    if let Some(act) = nodes.first().and_then(|n| action(Some(&n.1))) {
        for (k, v) in act {
            if k != "component" {
                out.insert(k, v);
            }
        }
    }
    if let Some(p) = nodes.first().and_then(|n| proc_cooldown(Some(&n.1))) {
        out.insert("proc_cooldown".into(), J::Float(p));
    }
    let (mut stages, mut heals, mut effs, mut actions) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let (mut vfx, mut sfx, mut touched): (Vec<String>, Vec<String>, Vec<J>) = (Vec::new(), Vec::new(), Vec::new());
    for (rel, pf, depth) in &nodes {
        let mut found = false;
        let name = J::Str(stage_name(rel));
        if let Some(act) = action(Some(pf))
            && *depth > 0
        {
            let rest: Map = act.into_iter().filter(|(k, _)| k != "component").collect();
            actions.push(J::Dict(with_head(
                &[("name", name.clone()), ("prefab", J::from(rel.as_str())), ("_depth", J::from(*depth))],
                &rest,
            )));
        }
        for rec in damage_records(pf) {
            let rest: Map = rec
                .iter()
                .filter(|(k, _)| *k != "damage" && *k != "multiplier")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            stages.push(J::Dict(with_head(
                &[
                    ("name", name.clone()),
                    ("prefab", J::from(rel.as_str())),
                    ("base", rec.get("damage").cloned().unwrap_or(J::Int(0))),
                    ("multiplier", rec.get("multiplier").cloned().unwrap_or(J::Int(0))),
                    ("_depth", J::from(*depth)),
                ],
                &rest,
            )));
            found = true;
        }
        for rec in healing_records(pf) {
            heals.push(J::Dict(with_head(
                &[("name", name.clone()), ("prefab", J::from(rel.as_str())), ("_depth", J::from(*depth))],
                &rec,
            )));
            found = true;
        }
        for eff in effects(pf) {
            effs.push(J::Dict(with_head(
                &[("name", name.clone()), ("prefab", J::from(rel.as_str())), ("_depth", J::from(*depth))],
                &eff,
            )));
            found = true;
        }
        let (v, s) = visuals(pf);
        let new_v: Vec<String> = v.into_iter().filter(|x| !vfx.contains(x)).collect();
        vfx.extend(new_v);
        let new_s: Vec<String> = s.into_iter().filter(|x| !sfx.contains(x)).collect();
        sfx.extend(new_s);
        if found {
            touched.push(J::from(rel.as_str()));
        }
    }
    let table = prefabs.vfx_table();
    out.insert("stages".into(), J::List(stages));
    out.insert("healing".into(), J::List(heals));
    out.insert("effects".into(), J::List(effs));
    out.insert("actions".into(), J::List(actions));
    out.insert(
        "vfx".into(),
        J::List(
            vfx.iter().filter_map(|k| table.get(k).map(|p| obj! {"key" => k.as_str(), "pkfx" => p.as_str()})).collect(),
        ),
    );
    out.insert("sfx".into(), J::from(sfx));
    out.insert("prefabs".into(), J::List(touched));
    out
}

/// `describe` with the default `abilities/` prefix and no boundaries.
pub fn describe_plain(prefabs: &Prefabs, root: &str) -> Map {
    describe(prefabs, root, &[], ABILITIES)
}
