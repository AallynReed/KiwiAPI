//! `companions.py`: the Geode companions, what each level costs and grants.

use indexmap::IndexMap;

use super::pvp_powerups::all_names;
use crate::decode::{
    ability::{Prefabs, healing_records, identity, modifiers},
    common::{Locale, MOD_OPS, is_effect, lookup, r4, rs, stat_rows, stem, walk_values},
    json::{J, Map},
    registry::{Built, Ctx},
    tree::Tree,
    value::{K, Prefab, W, parse},
};
use crate::obj;

const COLLECTION: &str = "collections/collection_geodecompanion";
const BLUEPRINT: i128 = 37;
const CREATURE: i128 = 444;
const UPGRADE_TREE: i128 = 440;
const MODEL: i128 = 40;
const NODE_KEY: i128 = 0;
const NODE_COST: i128 = 1;
const NODE_CHILDREN: i128 = 3;
const GRANT_KIND: i128 = 0;
const GRANT_BODY: i128 = 1;
const GRANT_TEXT: [(i128, &str); 3] = [(0, "name"), (1, "description"), (4, "detail")];
const VALUE_EFFECT: i128 = 453;
const CONDITIONS: i128 = 239;

/// `key -> node` over every file under `prefix` (each is one stringmap).
pub fn keyed(tree: &Tree, prefix: &str) -> IndexMap<String, W> {
    let mut out: IndexMap<String, W> = IndexMap::new();
    for path in tree.files(prefix, ".binfab") {
        let Ok(pf) = parse(&tree.read(&path).unwrap_or_default()) else { continue };
        let table = pf.root_leaf().get(0).and_then(W::as_dict).map(|d| d.items().to_vec()).unwrap_or_default();
        for (key, node) in table {
            if !node.is_obj() {
                continue;
            }
            let key = match key {
                K::Str(s) => s.to_string(),
                K::Int(i) => i.to_string(),
            };
            out.entry(key).or_insert(node);
        }
    }
    out
}

/// A companion tree's nodes, root first; stops where the tree branches.
pub fn chain(root: &W) -> Vec<W> {
    let mut out = vec![root.clone()];
    let mut node = root.clone();
    loop {
        let kids: Vec<W> = node.leaf().iter_of(NODE_CHILDREN).into_iter().filter(W::is_obj).collect();
        if kids.len() != 1 {
            return out;
        }
        node = kids[0].clone();
        out.push(node.clone());
    }
}

fn value_changes(obj: &W, names: &Locale, prefabs: &Prefabs) -> Map {
    let leaf = obj.leaf();
    let mut rows = Vec::new();
    for row in leaf.iter_of(0) {
        let r = row.leaf();
        if let (Some(v), Some(op)) = (r.str(0), r.int(1))
            && (0..MOD_OPS.len() as i128).contains(&op)
        {
            let amount = r.get(2).and_then(W::as_num).unwrap_or(0.0);
            rows.push(obj! {"value" => v, "op" => MOD_OPS[op as usize], "amount" => r4(amount)});
        }
    }
    let mut out = Map::new();
    if let Some(target) = leaf.str(1).filter(|t| !t.is_empty()) {
        out.insert("ability".into(), J::from(rs(target, ".binfab")));
        let title = names.get(identity(prefabs.get(target).as_deref()).name()).cloned().unwrap_or_default();
        if !title.is_empty() {
            out.insert("ability_name".into(), J::from(title));
        }
    }
    if !rows.is_empty() {
        out.insert("values".into(), J::List(rows));
    }
    out
}

fn conditions(obj: &W) -> Vec<J> {
    let mut out = Vec::new();
    for row in obj.leaf().iter_of(0) {
        let r = row.leaf();
        let values = r.get(1).map(|v| v.leaf()).and_then(|l| l.get(0)).filter(|v| v.is_list());
        if let (Some(t), Some(values)) = (r.str(0).filter(|t| !t.is_empty()), values) {
            let vals: Vec<J> = values.elems().iter().filter(|v| !v.is_none()).map(J::from).collect();
            out.push(obj! {"type" => t, "values" => vals});
        }
    }
    out
}

fn ability(prefabs: &Prefabs, r: &str, names: &Locale) -> Map {
    let mut effects: Vec<J> = Vec::new();
    let mut conds: Vec<J> = Vec::new();
    let mut seen: Vec<String> = vec![r.to_owned()];
    let mut queue: std::collections::VecDeque<String> = std::collections::VecDeque::from([r.to_owned()]);
    while let Some(next) = queue.pop_front() {
        let Some(pf) = prefabs.get(&next) else { continue };
        let mut targets: Vec<Option<String>> = Vec::new();
        for (cid, obj) in &pf.components {
            if *cid == VALUE_EFFECT {
                let change = value_changes(obj, names, prefabs);
                targets.push(change.get("ability").and_then(J::as_str).map(str::to_owned));
                if change.get("values").is_some_and(J::truthy) {
                    effects.push(J::Dict(change));
                }
            } else if *cid == CONDITIONS {
                for c in conditions(obj) {
                    if !conds.contains(&c) {
                        conds.push(c);
                    }
                }
            } else if is_effect(*cid) {
                let stats = modifiers(&walk_values(obj));
                if !stats.is_empty() {
                    effects.push(obj! {"stats" => stats});
                }
            }
        }
        for rec in healing_records(&pf) {
            let restores: Map = rec.into_iter().filter(|(k, _)| k == "health" || k == "energy").collect();
            effects.push(obj! {"restores" => restores});
        }
        for (_, o) in &pf.components {
            o.walk(&mut |v| {
                if let W::Str(s) = v
                    && s.starts_with("abilities/")
                {
                    let child = rs(s, ".binfab").to_owned();
                    if !seen.contains(&child) && !targets.iter().any(|t| t.as_deref() == Some(child.as_str())) {
                        seen.push(child.clone());
                        queue.push_back(child);
                    }
                }
            });
        }
    }
    let mut out = Map::new();
    if !effects.is_empty() {
        out.insert("effects".into(), J::List(effects));
    }
    if !conds.is_empty() {
        out.insert("conditions".into(), J::List(conds));
    }
    out
}

/// `_grant`: what an upgrade node grants.
pub fn grant(node: Option<&W>, prefabs: &Prefabs, names: &Locale) -> Map {
    let mut out = Map::new();
    let Some(node) = node else { return out };
    let kind = node.leaf().get(GRANT_KIND);
    let Some(body) = node.leaf().get(GRANT_BODY).filter(|b| b.is_obj() && !b.is_empty()) else { return out };
    for (idx, key) in GRANT_TEXT {
        let text = lookup(names, body.section(0).get(idx).filter(|v| v.truthy()));
        if !text.is_empty() {
            out.insert(key.into(), J::from(text));
        }
    }
    let payload = if body.sections().len() > 1 { body.leaf().get(0).cloned() } else { None };
    match kind.and_then(W::as_str) {
        Some("stats") => {
            let stats = payload.as_ref().map(|p| stat_rows(&walk_values(p))).unwrap_or_else(|| stat_rows(&[W::None]));
            if !stats.is_empty() {
                out.insert("stats".into(), J::List(stats));
            }
        }
        Some("effects") => {
            let mut powers = Vec::new();
            for r in payload.as_ref().map(W::iter).unwrap_or_default() {
                if let W::Str(s) = &r
                    && s.starts_with("abilities/")
                {
                    let r = rs(s, ".binfab");
                    let mut row = Map::new();
                    row.insert("ref".into(), J::from(r));
                    row.extend(ability(prefabs, r, names));
                    powers.push(J::Dict(row));
                }
            }
            if !powers.is_empty() {
                out.insert("abilities".into(), J::List(powers));
            }
        }
        _ => {}
    }
    out
}

/// `_costs`: a node's cost rows.
pub fn costs(node: &W, prefabs: &Prefabs, names: &Locale) -> Vec<J> {
    let mut out = Vec::new();
    for row in node.leaf().iter_of(NODE_COST) {
        let r = row.leaf();
        if let (Some(item), Some(qty)) = (r.str(0), r.int(1))
            && qty > 0
        {
            let name = names.get(identity(prefabs.get(item).as_deref()).name()).cloned().unwrap_or_default();
            let mut m = Map::new();
            m.insert("item".into(), J::from(item));
            if !name.is_empty() {
                m.insert("name".into(), J::from(name));
            }
            m.insert("quantity".into(), J::Int(qty));
            out.push(J::Dict(m));
        }
    }
    out
}

fn rig(pf: Option<&Prefab>) -> String {
    pf.and_then(|p| p.component(MODEL))
        .and_then(|m| m.leaf().str(0))
        .and_then(|s| s.strip_suffix(".skeleton.gr2"))
        .map(str::to_owned)
        .unwrap_or_default()
}

fn members(prefabs: &Prefabs, names: &Locale) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Some(pf) = prefabs.get(COLLECTION) else { return out };
    for group in pf.root.as_ref().map(|r| r.leaf().iter_of(0)).unwrap_or_default() {
        let g = group.leaf();
        let named = lookup(names, g.get(1).filter(|v| v.truthy()));
        let rarity = if !named.is_empty() {
            named
        } else {
            match g.get(0).filter(|v| v.truthy()) {
                Some(W::Str(s)) => s.to_string(),
                _ => String::new(),
            }
        };
        for row in g.iter_of(3) {
            if let Some(r) = row.leaf().str(0)
                && r.starts_with("item/companion/")
            {
                out.push((rs(r, ".binfab").to_owned(), rarity.clone()));
            }
        }
    }
    out
}

pub fn build(ctx: &Ctx) -> Built {
    let tree = &ctx.tree;
    let prefabs = Prefabs::new(tree);
    let names = all_names(tree);
    let trees = keyed(tree, "prefabs/upgrade/trees/");
    let grants = keyed(tree, "prefabs/upgrade/upgrades/");
    let mut companions = Vec::new();
    for (r, rarity) in members(&prefabs, &names) {
        let Some(pf) = prefabs.get(&r) else { continue };
        let ident = identity(Some(&pf));
        let slug = stem(&r);
        let mut entry = Map::new();
        let name = names.get(ident.name()).filter(|n| !n.is_empty()).cloned().unwrap_or_else(|| slug.clone());
        entry.insert("slug".into(), J::from(slug.as_str()));
        entry.insert("name".into(), J::from(name));
        entry.insert("description".into(), J::from(names.get(ident.desc()).cloned().unwrap_or_default()));
        entry.insert("prefab".into(), J::from(r.as_str()));
        entry.insert("rarity".into(), J::from(rarity));
        if let Some(bp) = pf.component(BLUEPRINT).and_then(|c| c.leaf().str(0)).filter(|s| !s.is_empty()) {
            entry.insert("blueprint".into(), J::from(bp));
        }
        let npc = pf
            .component(CREATURE)
            .filter(|c| !c.is_empty())
            .and_then(|c| c.section(0).str(0))
            .filter(|s| !s.is_empty());
        if let Some(npc) = npc {
            entry.insert("creature".into(), J::from(npc));
            let rg = rig(prefabs.get(npc).as_deref());
            if !rg.is_empty() {
                entry.insert("rig".into(), J::from(rg));
            }
        }
        let mut levels = Vec::new();
        let tree_id = pf.component(UPGRADE_TREE).and_then(|c| c.leaf().str(0)).map(str::to_owned);
        if let Some(tree_id) = tree_id
            && let Some(root) = trees.get(&tree_id)
        {
            entry.insert("upgrade_tree".into(), J::from(tree_id.as_str()));
            for (depth, node) in chain(root).iter().enumerate() {
                let key = node.leaf().get(NODE_KEY);
                let mut level = Map::new();
                level.insert("level".into(), J::from(depth + 1));
                level.insert("key".into(), key.map_or(J::Null, J::from));
                let cost = costs(node, &prefabs, &names);
                if !cost.is_empty() {
                    level.insert("cost".into(), J::List(cost));
                }
                let g = key.and_then(W::as_str).and_then(|k| grants.get(k));
                level.extend(grant(g, &prefabs, &names));
                levels.push(J::Dict(level));
            }
        }
        entry.insert("levels".into(), J::List(levels));
        companions.push(J::Dict(entry));
    }
    Ok(J::List(companions))
}
