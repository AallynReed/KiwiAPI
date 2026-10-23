//! `upgrade_trees.py`: the Star Chart and the other upgrade trees players spend on.

use indexmap::{IndexMap, IndexSet};

use super::{
    companions::{chain, costs, grant, keyed},
    pvp_powerups::all_names,
};
use crate::decode::{
    ability::{Prefabs, identity},
    common::{Locale, lookup, rp, rs, title},
    json::{J, Map},
    registry::{Built, Ctx},
    tree::Tree,
    value::{W, parse},
};
use crate::obj;

const SYSTEMS: [&str; 4] = ["standard", "fishing", "runecrafting", "gearcrafting"];
const MODULES: [&str; 9] =
    ["omni_tool", "hookshot", "jump_thruster", "barrier", "pathpainter", "vision", "battery", "ncharge", "vacuum"];
const NODE_KEY: i128 = 0;
const NODE_CHILDREN: i128 = 3;

fn tree_nodes(root: &W, parents: Option<&mut IndexMap<String, String>>) -> IndexMap<String, W> {
    let mut out: IndexMap<String, W> = IndexMap::new();
    let mut parents = parents;
    let mut queue = vec![root.clone()];
    while let Some(node) = queue.pop() {
        let Some(key) = node.leaf().str(NODE_KEY).map(str::to_owned) else { continue };
        if out.contains_key(&key) {
            continue;
        }
        out.insert(key.clone(), node.clone());
        for c in node.leaf().iter_of(NODE_CHILDREN) {
            let ck = if c.is_obj() { c.leaf().str(NODE_KEY).map(str::to_owned) } else { None };
            if let Some(ck) = ck {
                queue.push(c.clone());
                if let Some(p) = parents.as_deref_mut() {
                    p.entry(ck).or_insert_with(|| key.clone());
                }
            }
        }
    }
    out
}

fn node(key: &str, grants: &IndexMap<String, W>, prefabs: &Prefabs, names: &Locale) -> Map {
    let g = grant(grants.get(key), prefabs, names);
    let mut out = Map::new();
    out.insert("id".into(), J::from(key));
    for field in ["name", "description", "detail", "stats"] {
        if let Some(v) = g.get(field).filter(|v| v.truthy()) {
            out.insert(field.into(), v.clone());
        }
    }
    out
}

fn cost_rows(rows: Option<&W>, prefabs: &Prefabs, names: &Locale) -> Vec<J> {
    let mut out = Vec::new();
    for row in rows.filter(|r| r.truthy()).map(W::iter).unwrap_or_default() {
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

fn system(
    sid: &str,
    prefabs: &Prefabs,
    trees: &IndexMap<String, W>,
    grants: &IndexMap<String, W>,
    names: &Locale,
    tree: &Tree,
) -> Option<J> {
    let pf = parse(&tree.read(&format!("prefabs/progression/{sid}.binfab")).unwrap_or_default()).ok()?;
    let root = pf.root?;
    let leaf = root.leaf();
    let mut cost_nodes: IndexMap<String, W> = IndexMap::new();
    let mut parents: IndexMap<String, String> = IndexMap::new();
    for n in leaf.iter_of(2) {
        if let Some(key) = n.leaf().str(0)
            && let Some(t) = trees.get(key)
        {
            cost_nodes.extend(tree_nodes(t, Some(&mut parents)));
        }
    }
    let mut nodes = Vec::new();
    let mut seen: IndexSet<String> = IndexSet::new();
    for n in leaf.iter_of(2) {
        let nl = n.leaf();
        let Some(key) = nl.str(0) else { continue };
        if !seen.insert(key.to_owned()) {
            continue;
        }
        let mut entry = node(key, grants, prefabs, names);
        let link = nl.get(3).map(W::leaf).and_then(|l| l.get(1));
        let parent = parents.get(key).filter(|p| !p.is_empty()).cloned().or_else(|| match link {
            Some(l) if l.is_obj() && l.sections().len() > 1 => l.section(1).str(1).map(str::to_owned),
            _ => None,
        });
        if let Some(p) = parent.filter(|p| !p.is_empty()) {
            entry.insert("parent".into(), J::from(p));
        }
        if let Some(pos) =
            nl.get(9).filter(|p| matches!(p, W::List(_) | W::Rep(_) | W::Obj(_) | W::Tuple(_)) && p.len() == 2)
        {
            entry.insert("position".into(), J::List(pos.elems().iter().map(J::from).collect()));
        }
        let opens = cost_rows(nl.get(2), prefabs, names);
        if !opens.is_empty() {
            entry.insert("opens_with".into(), J::List(opens));
        }
        if let Some(c) = cost_nodes.get(key) {
            let cost = costs(c, prefabs, names);
            if !cost.is_empty() {
                entry.insert("cost".into(), J::List(cost));
            }
        }
        nodes.push(J::Dict(entry));
    }
    let text = |k: Option<&W>| lookup(names, k);
    let name = text(leaf.get(3));
    Some(obj! {
        "slug" => sid, "name" => if name.is_empty() { title(sid) } else { name },
        "description" => text(leaf.get(14)), "detail" => text(leaf.get(15)), "nodes" => nodes,
    })
}

fn module(
    mid: &str,
    prefabs: &Prefabs,
    trees: &IndexMap<String, W>,
    grants: &IndexMap<String, W>,
    names: &Locale,
) -> Option<J> {
    let root = trees.get(&format!("{mid}_upgrade_tree"))?;
    let name = names
        .get(&format!("$module_{mid}_name"))
        .filter(|n| !n.is_empty())
        .or_else(|| names.get(&format!("$module_{}_name", mid.replace('_', ""))))
        .filter(|n| !n.is_empty())?
        .clone();
    let mut levels = Vec::new();
    for (depth, n) in chain(root).iter().enumerate().skip(1) {
        let Some(key) = n.leaf().str(NODE_KEY) else { continue };
        let mut level = Map::new();
        level.insert("level".into(), J::from(depth + 1));
        level.extend(node(key, grants, prefabs, names));
        let cost = costs(n, prefabs, names);
        if !cost.is_empty() {
            level.insert("cost".into(), J::List(cost));
        }
        levels.push(J::Dict(level));
    }
    Some(obj! {"slug" => mid, "name" => name, "levels" => levels})
}

fn paragon(
    cls: &str,
    prefabs: &Prefabs,
    trees: &IndexMap<String, W>,
    grants: &IndexMap<String, W>,
    names: &Locale,
) -> Option<J> {
    let mut sorted: Vec<(&String, &W)> = trees.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let needle = format!("_{cls}_root_");
    let roots: Vec<&W> =
        sorted.into_iter().filter(|(k, _)| format!("_{k}").contains(&needle)).map(|(_, n)| n).collect();
    let mut branches: IndexMap<String, W> = IndexMap::new();
    for root in roots {
        for child in root.leaf().iter_of(NODE_CHILDREN) {
            if child.is_obj()
                && let Some(key) = child.leaf().str(NODE_KEY)
            {
                branches.entry(key.to_owned()).or_insert_with(|| child.clone());
            }
        }
    }
    let mut keys: Vec<&String> = branches.keys().collect();
    keys.sort();
    let mut powers = Vec::new();
    for key in keys {
        let g = grant(grants.get(key), prefabs, names);
        let Some(name) = g.get("name").filter(|v| v.truthy()) else { continue };
        let mut power = Map::new();
        power.insert("id".into(), J::from(key.as_str()));
        power.insert("name".into(), name.clone());
        if let Some(d) = g.get("description").filter(|v| v.truthy()) {
            power.insert("description".into(), d.clone());
        }
        let refs: Vec<J> = g
            .get("abilities")
            .and_then(J::as_list)
            .into_iter()
            .flatten()
            .map(|a| a.get("ref").cloned().unwrap_or(J::Null))
            .collect();
        if !refs.is_empty() {
            power.insert("abilities".into(), J::List(refs));
        }
        let cost = costs(&branches[key], prefabs, names);
        if !cost.is_empty() {
            power.insert("cost".into(), J::List(cost));
        }
        powers.push(J::Dict(power));
    }
    (!powers.is_empty()).then(|| obj! {"class" => cls, "powers" => powers})
}

pub fn build(ctx: &Ctx) -> Built {
    let tree = &ctx.tree;
    let prefabs = Prefabs::new(tree);
    let names = all_names(tree);
    let trees = keyed(tree, "prefabs/upgrade/trees/");
    let grants = keyed(tree, "prefabs/upgrade/upgrades/");
    let systems: Vec<J> = SYSTEMS.iter().filter_map(|s| system(s, &prefabs, &trees, &grants, &names, tree)).collect();
    let modules: Vec<J> = MODULES.iter().filter_map(|m| module(m, &prefabs, &trees, &grants, &names)).collect();
    let mut classes: Vec<String> = tree
        .files("prefabs/upgrade/trees/", ".binfab")
        .iter()
        .filter(|p| p.contains("/prestige_"))
        .map(|p| rp(rs(p.rsplit('/').next().unwrap_or(p), ".binfab"), "prestige_").to_owned())
        .collect();
    classes.sort();
    classes.dedup();
    let paragon: Vec<J> = classes.iter().filter_map(|c| paragon(c, &prefabs, &trees, &grants, &names)).collect();
    Ok(obj! {"systems" => systems, "geode_tools" => modules, "paragon" => paragon})
}
