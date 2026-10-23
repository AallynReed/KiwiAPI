//! `app/trove/codexes/links.py`: typed edges between codex entries.

use std::collections::HashSet;

use serde_json::{Map, Value, json};

use super::{py, types::PREFABS_ROOT, upgrades::Tree};

pub const CRAFTS: &str = "crafts";
pub const INGREDIENT: &str = "ingredient";
pub const CRAFTABLE_AT: &str = "craftable_at";
pub const UNLOCKS: &str = "unlocks";
pub const UPGRADE_COST: &str = "upgrade_cost";
pub const MEMBER_OF: &str = "member_of";

pub const ALL_RELATIONS: [&str; 6] = [CRAFTS, INGREDIENT, CRAFTABLE_AT, UNLOCKS, UPGRADE_COST, MEMBER_OF];

/// A `codex_link` row (branch added by the writer).
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub src_path: String,
    pub rel: &'static str,
    pub dst_path: String,
    pub ord: i64,
    pub qty: Option<f64>,
    pub data: Map<String, Value>,
}

/// `entry_path`: any written prefab reference -> `prefabs/<rel>.binfab`.
pub fn entry_path(rel: &str) -> String {
    let p = py::lower(py::strip(&rel.replace('\\', "/")));
    if p.is_empty() {
        return String::new();
    }
    let p = p.strip_suffix(".binfab").unwrap_or(&p);
    let p =
        if p.starts_with(PREFABS_ROOT) { p.to_owned() } else { format!("{PREFABS_ROOT}{}", py::lstrip_chars(p, "/")) };
    format!("{p}.binfab")
}

/// `_amount`: `float(v)` when finite, else `None`.
pub fn amount(v: &Value) -> Option<f64> {
    let f = match v {
        Value::Number(n) => n.as_f64()?,
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::String(s) => py::strip(s).parse::<f64>().ok()?,
        _ => return None,
    };
    f.is_finite().then_some(f)
}

fn s<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// `recipe_links`: crafts / ingredient / craftable_at, de-duped on (rel, target).
pub fn recipe_links(path: &str, recipe: &Value) -> Vec<Link> {
    let mut out = Vec::new();
    let mut seen: HashSet<(&str, String)> = HashSet::new();
    let mut add = |rel: &'static str, target: &str, ord: i64, qty: Option<f64>, data: Map<String, Value>| {
        let dst = entry_path(target);
        if dst.is_empty() || !seen.insert((rel, dst.clone())) {
            return;
        }
        out.push(Link { src_path: path.to_owned(), rel, dst_path: dst, ord, qty, data });
    };
    let empty = Value::Null;
    let output = recipe.get("output").unwrap_or(&empty);
    if !s(output, "path").is_empty() {
        add(CRAFTS, s(output, "path"), 0, output.get("amount").and_then(amount), Map::new());
    }
    if let Some(list) = recipe.get("ingredients").and_then(Value::as_array) {
        for (ord, ing) in list.iter().enumerate() {
            if !s(ing, "path").is_empty() {
                add(INGREDIENT, s(ing, "path"), ord as i64, ing.get("amount").and_then(amount), Map::new());
            }
        }
    }
    if let Some(list) = recipe.get("providers").and_then(Value::as_array) {
        for (ord, p) in list.iter().enumerate() {
            let target = if s(p, "provider_path").is_empty() { s(p, "provider") } else { s(p, "provider_path") };
            if !target.is_empty() {
                let mut data = Map::new();
                data.insert("lane".into(), json!(s(p, "lane")));
                data.insert("category".into(), json!(s(p, "category")));
                data.insert("category_order".into(), p.get("category_order").cloned().unwrap_or(Value::Null));
                add(CRAFTABLE_AT, target, ord as i64, None, data);
            }
        }
    }
    out
}

/// `upgrade_links`: one `upgrade_cost` edge per (node, item).
pub fn upgrade_links(tree: &Tree, source_path: &str) -> Vec<Link> {
    let mut out = Vec::new();
    for node in &tree.nodes {
        let src = format!("{}:{}", tree.system_key, node.node_key);
        let mut seen = HashSet::new();
        for (ord, cost) in node.costs.iter().enumerate() {
            let dst = entry_path(s(cost, "item"));
            if dst.is_empty() || !seen.insert(dst.clone()) {
                continue;
            }
            let mut data = Map::new();
            data.insert("system_kind".into(), json!(tree.system_kind));
            data.insert("system_key".into(), json!(tree.system_key));
            data.insert("node_key".into(), json!(node.node_key));
            data.insert("source_path".into(), json!(source_path));
            out.push(Link {
                src_path: src.clone(),
                rel: UPGRADE_COST,
                dst_path: dst,
                ord: ord as i64,
                qty: cost.get("quantity").and_then(amount),
                data,
            });
        }
    }
    out
}

/// `unlock_links`.
pub fn unlock_links(pairs: &[(String, String)]) -> Vec<Link> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for (ord, (source, target)) in pairs.iter().enumerate() {
        let (src, dst) = (entry_path(source), entry_path(target));
        if src.is_empty() || dst.is_empty() || !seen.insert((src.clone(), dst.clone())) {
            continue;
        }
        out.push(Link { src_path: src, rel: UNLOCKS, dst_path: dst, ord: ord as i64, qty: None, data: Map::new() });
    }
    out
}

/// `membership_links`: member -> catalogue, members sorted by path.
pub fn membership_links(members: &[(String, String)], catalogue_path: &str) -> Vec<Link> {
    let dst = entry_path(catalogue_path);
    let mut sorted: Vec<&(String, String)> = members.iter().collect();
    sorted.sort();
    let mut out = Vec::new();
    for (ord, (member, group)) in sorted.into_iter().enumerate() {
        let src = entry_path(member);
        if !src.is_empty() && !dst.is_empty() {
            let mut data = Map::new();
            data.insert("group".into(), json!(group));
            out.push(Link { src_path: src, rel: MEMBER_OF, dst_path: dst.clone(), ord: ord as i64, qty: None, data });
        }
    }
    out
}
