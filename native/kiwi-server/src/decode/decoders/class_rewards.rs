//! `class_rewards.py`: what each class hands out as it levels, and per Paragon level.

use indexmap::IndexSet;

use super::{mastery::claim, pvp_powerups::all_names};
use crate::decode::{
    ability::{Prefabs, identity},
    common::{lookup, rs},
    json::{J, Map},
    registry::{Built, Ctx},
    value::{Dict, K, W, parse},
};
use crate::obj;

const LEVELS: i128 = 7;
const PARAGON: i128 = 24;
const PARAGON_DEFAULT: i128 = 25;
const PARAGON_PRIME: i128 = 28;

fn leaf(v: Option<&W>) -> &Dict {
    match v {
        Some(W::Dict(d)) => d,
        Some(o) => o.leaf(),
        None => &crate::decode::value::EMPTY,
    }
}

fn bp(name: &str) -> String {
    format!("bp:{}", rs(&name.rsplit('/').next().unwrap_or(name).to_lowercase(), ".blueprint"))
}

pub fn build(ctx: &Ctx) -> Built {
    let tree = &ctx.tree;
    let prefabs = Prefabs::new(tree);
    let names = all_names(tree);
    let claim_pf = prefabs.get("claim/levelup");
    let table =
        claim_pf.as_ref().and_then(|p| p.root.as_ref()).and_then(|r| r.leaf().get(0).filter(|v| v.truthy()).cloned());
    let claim_of = |cid: Option<&W>| -> Map {
        let node = match (cid, table.as_ref()) {
            (Some(W::Str(s)), Some(W::Dict(t))) => t.get_s(s),
            _ => None,
        };
        match node.filter(|n| n.is_obj()) {
            Some(n) => claim(n.section(0).get(1), &prefabs, &names),
            None => Map::new(),
        }
    };
    let mut classes: Vec<(String, J)> = Vec::new();
    for path in tree.files("prefabs/class/", ".binfab") {
        let tech = rs(path.rsplit('/').next().unwrap_or(&path), ".binfab").to_owned();
        if tech.contains('_') {
            continue;
        }
        let Ok(pf) = parse(&tree.read(&path).unwrap_or_default()) else { continue };
        let root = leaf(pf.root.as_ref());
        let mut levels = Vec::new();
        let lv_rows: Vec<(K, W)> = match root.get(LEVELS).filter(|v| v.truthy()) {
            Some(W::Dict(d)) => d.items().to_vec(),
            _ => Vec::new(),
        };
        let mut lv_rows: Vec<(i128, W)> = lv_rows
            .into_iter()
            .filter_map(|(k, v)| match k {
                K::Int(i) => Some((i, v)),
                K::Str(_) => None,
            })
            .collect();
        lv_rows.sort_by_key(|(k, _)| *k);
        for (lv, row) in lv_rows {
            let r = leaf(Some(&row));
            if r.is_empty() {
                continue;
            }
            let mut entry = Map::new();
            entry.insert("level".into(), J::Int(lv));
            let text = lookup(&names, r.get(3).filter(|v| v.truthy()));
            if !text.is_empty() {
                entry.insert("text".into(), J::from(text));
            }
            let mut abilities = Vec::new();
            for a in r.iter_of(1) {
                if let W::Str(s) = a {
                    let rf = rs(&s, ".binfab").to_owned();
                    let name = names.get(identity(prefabs.get(&rf).as_deref()).name()).cloned().unwrap_or_default();
                    abilities.push(obj! {"ref" => rf, "name" => name});
                }
            }
            if !abilities.is_empty() {
                entry.insert("abilities".into(), J::List(abilities));
            }
            if let Some(c) = r.str(4).filter(|c| !c.is_empty()) {
                entry.insert("costume".into(), J::from(format!("skins/{c}")));
            }
            let granted: IndexSet<String> = r.iter_of(6).iter().filter_map(W::as_str).map(bp).collect();
            if !granted.is_empty() {
                entry.insert("styles".into(), J::from(granted.into_iter().collect::<Vec<_>>()));
            }
            if entry.len() > 1 {
                levels.push(J::Dict(entry));
            }
        }
        let paragon_levels =
            root.get(PARAGON).map(W::iter).unwrap_or_default().iter().filter(|k| matches!(k, W::Int(_))).count();
        let first = |i: i128| -> Option<W> {
            let list = leaf(root.get(i)).get(1).filter(|v| v.truthy()).map(W::iter).unwrap_or_else(|| vec![W::None]);
            list.into_iter().next()
        };
        let every = claim_of(first(PARAGON_DEFAULT).as_ref());
        let prime = claim_of(first(PARAGON_PRIME).as_ref());
        let mut paragon = Map::new();
        paragon.insert("levels".into(), J::from(paragon_levels));
        if let Some(g) = every.get("grants").filter(|g| g.truthy()) {
            paragon.insert("every".into(), g.clone());
        }
        if let Some(g) = prime.get("grants").filter(|g| g.truthy()) {
            paragon.insert("prime".into(), g.clone());
        }
        classes.push((tech, obj! {"levels" => levels, "paragon" => paragon}));
    }
    let mut out = Map::new();
    for (k, v) in classes {
        out.insert(k, v);
    }
    out.sort_keys();
    Ok(obj! {"classes" => out})
}
