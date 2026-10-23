//! `subclass_abilities.py`: what each class grants equipped as a subclass, tier
//! by tier.

use indexmap::{IndexMap, IndexSet};

use super::ally_abilities::abilities_text;
use crate::decode::{
    ability::{Prefabs, describe, identity, refs, walk},
    common::{
        Locale, MOD_LABEL, MOD_OP, PROC_COOLDOWN, PROC_SECTION, PROC_SPAWNER, is_modifier, locale_at, r4, rs, stem,
        title, walk_values,
    },
    json::{J, Map},
    registry::{Built, Ctx},
    value::{Prefab, W},
};

const ROOT: &str = "abilities/subclass/";
const REQUIREMENTS: i128 = 239;
const PROC_CHANCE: i128 = 5;
const PROC_HEALTH: i128 = 7;
const PROC_TAG: i128 = 16;
const PROC_TYPED: i128 = 26;
const PROC_MAGIC: i128 = 27;
const PROC_IGNORES: i128 = 30;
const EVENTS: [(i128, &str); 10] = [
    (0, "damage_taken"),
    (1, "damage_dealt"),
    (3, "health_below"),
    (8, "damage_received"),
    (9, "nearby_enemy_death"),
    (13, "critical_hit"),
    (17, "enemy_defeated"),
    (26, "damage_dealt"),
    (28, "target_health_below"),
    (35, "health_drops_below"),
];
const HEALTH_EVENTS: [i128; 3] = [3, 28, 35];
const LEECH: i128 = 281;
const DOT: i128 = 156;
const REFLECT: i128 = 336;
const SHIELD: i128 = 131;
const WEIGHTED: i128 = 275;
const NULLIFY: i128 = 3;

fn num(v: Option<&W>) -> Option<J> {
    match v? {
        W::Int(i) => Some(J::Int(*i)),
        W::Float(f) => Some(J::Float(r4(*f))),
        _ => None,
    }
}

fn truthy_num(v: Option<&W>) -> Option<J> {
    num(v).filter(J::truthy)
}

type Window = (String, i128, Option<i128>);

fn window(pf: Option<&Prefab>) -> Option<Window> {
    let obj = pf?.component(REQUIREMENTS)?;
    for row in obj.leaf().iter_of(0) {
        let kind = match row.leaf().get(0) {
            Some(W::Str(s)) if &**s == "subclasspowerrank" => "subclass_power_rank",
            Some(W::Str(s)) if &**s == "subclasslevel" => "subclass_level",
            _ => continue,
        };
        let Some(params) = row.leaf().get(1).filter(|p| p.is_obj()) else { continue };
        let own = params.leaf();
        let mode = own.int(2).unwrap_or(0);
        let low = own.int(0).unwrap_or(0);
        let high = own.int(1).unwrap_or(0);
        return Some((
            kind.to_owned(),
            if mode == 1 { 0 } else { low },
            if mode == 1 || mode == 2 { Some(high) } else { None },
        ));
    }
    None
}

fn trigger(rel: &str, pf: &Prefab) -> Option<J> {
    let obj = pf.component(PROC_SPAWNER)?;
    if obj.sections().len() <= PROC_SECTION {
        return None;
    }
    let s = obj.section(PROC_SECTION);
    let set = |i: i128| s.get(i).is_some_and(W::truthy);
    let mut on: Vec<&str> = Vec::new();
    for (i, n) in EVENTS {
        if set(i) && !on.contains(&n) {
            on.push(n);
        }
    }
    let mut out = Map::new();
    out.insert("prefab".into(), J::from(rel));
    out.insert("on".into(), J::List(on.into_iter().map(J::from).collect()));
    for (key, i) in [("chance", PROC_CHANCE), ("cooldown", PROC_COOLDOWN)] {
        if let Some(v) = num(s.get(i)) {
            out.insert(key.into(), v);
        }
    }
    if HEALTH_EVENTS.iter().any(|i| set(*i))
        && let Some(v) = truthy_num(s.get(PROC_HEALTH))
    {
        out.insert("health".into(), v);
    }
    if set(PROC_TYPED) {
        out.insert("damage_type".into(), J::from(if set(PROC_MAGIC) { "magic" } else { "physical" }));
    }
    if let Some(t) = s.str(PROC_TAG).filter(|t| !t.is_empty()) {
        out.insert("requires_tag".into(), J::from(t));
    }
    let ignores: Vec<J> = s.iter_of(PROC_IGNORES).iter().filter_map(W::as_str).map(J::from).collect();
    if !ignores.is_empty() {
        out.insert("ignores_tags".into(), J::List(ignores));
    }
    Some(J::Dict(out))
}

fn extras(prefabs: &Prefabs, row: &mut Map) {
    let prefab = row.get("prefab").and_then(J::as_str).unwrap_or("").to_owned();
    let Some(cid) = row.get("component").and_then(J::as_int) else { return };
    let Some(pf) = prefabs.get(&prefab) else { return };
    let Some(obj) = pf.component(cid) else { return };
    let leaf = obj.leaf();
    if cid == LEECH {
        if let Some(v) = truthy_num(leaf.get(3)) {
            row.insert("life_leech".into(), v);
        }
    } else if cid == DOT {
        if obj.sections().len() > 1
            && let Some(v) = truthy_num(obj.section(1).get(5))
        {
            row.insert("tick".into(), v);
        }
    } else if cid == REFLECT {
        if let Some(v) = truthy_num(leaf.get(2)) {
            row.insert("reflect".into(), v);
            let tags: Vec<J> = leaf.iter_of(5).iter().filter_map(W::as_str).map(J::from).collect();
            if !tags.is_empty() {
                row.insert("reflect_ignores_tags".into(), J::List(tags));
            }
        }
    } else if cid == SHIELD
        && obj.sections().len() > 2
        && let Some(v) = num(obj.section(2).get(3))
        && match &v {
            J::Int(i) => *i > 0,
            J::Float(f) => *f > 0.0,
            _ => false,
        }
    {
        row.insert("absorbs_over_max_health".into(), v);
    }
    let vetoed: Vec<J> = walk_values(obj)
        .iter()
        .filter(|v| is_modifier(v) && v.as_dict().and_then(|d| d.get(MOD_OP)).and_then(W::as_int) == Some(NULLIFY))
        .filter_map(|v| v.as_dict().and_then(|d| d.str(MOD_LABEL)).map(J::from))
        .collect();
    if !vetoed.is_empty() {
        row.insert("nullifies".into(), J::List(vetoed));
    }
}

fn without(row: &J, keys: &[&str]) -> Map {
    row.as_dict()
        .map(|m| m.iter().filter(|(k, _)| !keys.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}

fn detail(prefabs: &Prefabs, rel: &str, stop: &[String], prefix: &str) -> Map {
    let info = describe(prefabs, rel, stop, &[prefix]);
    let mut out = Map::new();
    for key in ["stages", "healing", "effects"] {
        let mut rows: Vec<Map> = info
            .get(key)
            .and_then(J::as_list)
            .into_iter()
            .flatten()
            .map(|r| without(r, &["_depth", "name_key", "description_key"]))
            .collect();
        if key == "effects" {
            for row in rows.iter_mut() {
                extras(prefabs, row);
            }
            rows.retain(|r| r.keys().any(|k| !matches!(k.as_str(), "name" | "prefab" | "component")));
        }
        if !rows.is_empty() {
            out.insert(key.into(), J::List(rows.into_iter().map(J::Dict).collect()));
        }
    }
    let (mut triggers, mut choices, mut summons): (Vec<J>, Vec<J>, Vec<String>) = (Vec::new(), Vec::new(), Vec::new());
    for (node, pf, _) in walk(prefabs, rel, stop, 400, &[prefix]) {
        if let Some(t) = trigger(&node, &pf) {
            triggers.push(t);
        }
        for row in pf.component(WEIGHTED).map(|c| c.leaf().iter_of(0)).unwrap_or_default() {
            let (r, weight) = (row.leaf().str(0), num(row.leaf().get(1)));
            if let (Some(r), Some(w)) = (r, weight) {
                choices.push(crate::obj! {"prefab" => rs(r, ".binfab"), "weight" => w});
            }
        }
        for (_, obj) in &pf.components {
            let found: Vec<String> = obj
                .strings()
                .iter()
                .filter(|s| s.starts_with("npc/") && !summons.iter().any(|x| x == rs(s, ".binfab")))
                .map(|s| rs(s, ".binfab").to_owned())
                .collect();
            summons.extend(found);
        }
    }
    if !triggers.is_empty() {
        out.insert("triggers".into(), J::List(triggers));
    }
    if !choices.is_empty() {
        out.insert("choices".into(), J::List(choices));
    }
    if !summons.is_empty() {
        out.insert("summons".into(), J::from(summons));
    }
    out
}

fn subclass(prefabs: &Prefabs, text: &Locale, root: &str) -> Map {
    let prefix = format!("{}/", root.rsplit_once('/').map_or(root, |(a, _)| a));
    let nodes = walk(prefabs, root, &[], 400, &[&prefix]);
    let mut parent: IndexMap<String, String> = IndexMap::new();
    let mut missing: Vec<String> = Vec::new();
    for (rel, pf, _) in &nodes {
        for child in refs(pf, &[&prefix]) {
            parent.entry(child.clone()).or_insert_with(|| rel.clone());
            if !prefabs.exists(&child) && !missing.contains(&child) {
                missing.push(child);
            }
        }
    }
    let mut windows: IndexMap<String, Window> = IndexMap::new();
    for (rel, pf, _) in &nodes {
        if let Some(w) = window(Some(pf)) {
            windows.insert(rel.clone(), w);
        }
    }
    let tiered: Vec<String> = windows.keys().cloned().collect();
    let ident = identity(prefabs.get(root).as_deref());
    let mut entry = Map::new();
    entry.insert("name".into(), J::from(text.get(ident.name()).cloned().unwrap_or_default()));
    entry.insert("description".into(), J::from(text.get(ident.desc()).cloned().unwrap_or_default()));
    entry.insert("icon".into(), J::from(ident.icon.clone().unwrap_or_default()));
    entry.insert("prefab".into(), J::from(root));
    let base = detail(prefabs, root, &tiered, &prefix);
    if !base.is_empty() {
        entry.insert("base".into(), J::Dict(base));
    }
    let mut parts: IndexMap<String, Map> = IndexMap::new();
    for rel in windows.keys() {
        let mut p = Map::new();
        p.insert("name".into(), J::from(title(&stem(rel).replace('_', " "))));
        p.insert("prefab".into(), J::from(rel.as_str()));
        let stop: Vec<String> = tiered.iter().filter(|t| *t != rel).cloned().collect();
        p.extend(detail(prefabs, rel, &stop, &prefix));
        parts.insert(rel.clone(), p);
    }
    let mut points: IndexSet<(String, i128)> = IndexSet::new();
    for (kind, t, _) in windows.values() {
        points.insert((kind.clone(), *t));
    }
    for (kind, _, u) in windows.values() {
        if let Some(u) = u {
            points.insert((kind.clone(), u + 1));
        }
    }
    let mut points: Vec<(String, i128)> = points.into_iter().collect();
    points.sort_by_key(|p| (p.0 != "subclass_power_rank", p.1));
    let mut tiers = Vec::new();
    for (kind, t) in points {
        let active: Vec<&String> = windows
            .iter()
            .filter(|(_, (k, low, high))| *k == kind && *low <= t && high.is_none_or(|h| t <= h))
            .map(|(r, _)| r)
            .collect();
        let mut tier = Map::new();
        tier.insert("threshold".into(), J::Int(t));
        tier.insert("threshold_kind".into(), J::from(kind.as_str()));
        let mut list = Vec::new();
        for rel in &active {
            let mut p = parts[*rel].clone();
            if let Some(u) = windows[*rel].2 {
                p.insert("until".into(), J::Int(u));
            }
            list.push(J::Dict(p));
        }
        tier.insert("parts".into(), J::List(list));
        let mut overlapping = Vec::new();
        for (i, a) in active.iter().enumerate() {
            for b in &active[i + 1..] {
                if parent.get(*a) == parent.get(*b) && windows[*a] != windows[*b] {
                    overlapping.push(J::List(vec![J::from(a.as_str()), J::from(b.as_str())]));
                }
            }
        }
        if !overlapping.is_empty() {
            tier.insert("overlapping".into(), J::List(overlapping));
        }
        tiers.push(J::Dict(tier));
    }
    entry.insert("tiers".into(), J::List(tiers));
    if !missing.is_empty() {
        entry.insert("missing".into(), J::from(missing));
    }
    entry
}

pub fn build(ctx: &Ctx) -> Built {
    let tree = &ctx.tree;
    let prefabs = Prefabs::new(tree);
    let mut display = Locale::new();
    for name in ["prefabs_class.binfab", "ui.binfab", "new.binfab"] {
        display.extend(locale_at(tree, &format!("languages/en/{name}")));
    }
    let text = abilities_text(tree);
    let mut out: Vec<Map> = Vec::new();
    for class_path in tree.files("prefabs/class/", ".binfab") {
        let folder = stem(&class_path);
        if class_path.matches('/').count() != 2 || folder.ends_with("_ultimate") {
            continue;
        }
        let cls = prefabs.get(&format!("class/{folder}"));
        let texts = cls.as_ref().and_then(|c| c.root.as_ref()).map(W::strings).unwrap_or_default();
        let key = texts.iter().find(|s| s.starts_with("$DisplayName")).map(|s| s.to_string());
        let root = texts.iter().find(|s| s.starts_with(ROOT)).map(|s| rs(s, ".binfab").to_owned());
        let (Some(key), Some(root)) = (key, root) else { continue };
        if prefabs.get(&root).is_none() {
            continue;
        }
        let mut e = Map::new();
        e.insert(
            "class".into(),
            J::from(display.get(&key).cloned().unwrap_or_else(|| key.replace("$DisplayName_", ""))),
        );
        e.insert("game_folder".into(), J::from(folder));
        e.extend(subclass(&prefabs, &text, &root));
        out.push(e);
    }
    out.sort_by_cached_key(|e| e["class"].as_str().unwrap_or("").to_owned());
    Ok(J::List(out.into_iter().map(J::Dict).collect()))
}
