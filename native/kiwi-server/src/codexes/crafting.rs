//! `app/trove/codexes/crafting.py`: the Recipe Cost Calculator's tree.
//! `craft_cost` is `None` when any descendant is unpriced; `craft_cost_partial`
//! keeps the known sum.

use std::{
    collections::{BTreeSet, HashMap},
    sync::{Arc, LazyLock, Mutex},
};

use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};

use super::{internal, store};
use crate::{
    error::ApiResult,
    market::store::{Live, active_medians},
    pyfloat,
    state::AppState,
};

pub const MAX_DEPTH: usize = 20;
pub const MAX_NODES: usize = 600;

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub path: String,
    pub name: String,
    pub amount: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Recipe {
    pub output: Item,
    pub ingredients: Vec<Item>,
}

pub type RecipeMap = HashMap<String, Recipe>;
type Signature = (i64, Option<DateTime<Utc>>);

type Cache = HashMap<String, (Signature, Arc<RecipeMap>)>;

static MAP_CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Mutex::default);

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// `int(x or 1)` of a stored JSON amount.
fn amount(v: Option<&Value>) -> i64 {
    let Some(Value::Number(n)) = v else { return 1 };
    match n.as_i64() {
        Some(0) => 1,
        Some(i) => i,
        None => match n.as_f64() {
            Some(f) if f != 0.0 => f.trunc() as i64,
            _ => 1,
        },
    }
}

/// `_recipe_map` from `(path, name, category, data)` rows; later rows win.
pub fn recipe_map(rows: &[(String, String, Value)]) -> RecipeMap {
    let mut out = RecipeMap::new();
    for (_, name, data) in rows {
        let rec = data.get("recipe").filter(|r| crate::features::py_truthy(r)).cloned().unwrap_or(Value::Null);
        let Some(o) = rec.get("output").filter(|o| crate::features::py_truthy(o)) else { continue };
        let path = s(o, "path");
        if path.is_empty() {
            continue;
        }
        let out_name = if s(o, "name").is_empty() { name.clone() } else { s(o, "name").to_owned() };
        let ingredients = rec
            .get("ingredients")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter(|i| !s(i, "path").is_empty())
                    .map(|i| Item {
                        path: s(i, "path").to_owned(),
                        name: if s(i, "name").is_empty() { s(i, "path").to_owned() } else { s(i, "name").to_owned() },
                        amount: amount(i.get("amount")),
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.insert(
            path.to_owned(),
            Recipe {
                output: Item { path: path.to_owned(), name: out_name, amount: amount(o.get("amount")) },
                ingredients,
            },
        );
    }
    out
}

/// One tree node before pricing.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub path: String,
    pub name: String,
    pub need: i64,
    pub crafts: Option<i64>,
    pub output_amount: Option<i64>,
    pub children: Vec<Node>,
}

/// `_raw`'s walk state: the ancestor stack, the node counter, every name seen.
pub struct Walk<'a> {
    pub map: &'a RecipeMap,
    pub stack: Vec<String>,
    pub counter: usize,
    pub names: BTreeSet<String>,
}

impl<'a> Walk<'a> {
    pub fn new(map: &'a RecipeMap) -> Self {
        Walk { map, stack: Vec::new(), counter: 0, names: BTreeSet::new() }
    }

    /// `_raw`.
    pub fn node(&mut self, path: &str, name: &str, need: i64, depth: usize) -> Node {
        self.counter += 1;
        self.names.insert(name.to_owned());
        let mut node = Node {
            path: path.into(),
            name: name.into(),
            need,
            crafts: None,
            output_amount: None,
            children: Vec::new(),
        };
        if let Some(rec) = self.map.get(path)
            && depth < MAX_DEPTH
            && !self.stack.iter().any(|p| p == path)
            && self.counter < MAX_NODES
        {
            let out_amt = if rec.output.amount == 0 { 1 } else { rec.output.amount };
            let crafts = 1.max(ceil_div(need, out_amt));
            node.crafts = Some(crafts);
            node.output_amount = Some(out_amt);
            self.stack.push(path.to_owned());
            for ing in &rec.ingredients {
                let child = self.node(&ing.path, &ing.name, ing.amount * crafts, depth + 1);
                node.children.push(child);
            }
            self.stack.pop();
        }
        node
    }
}

/// `math.ceil(need / out_amt)` (true division, then ceil).
fn ceil_div(need: i64, out_amt: i64) -> i64 {
    (need as f64 / out_amt as f64).ceil() as i64
}

/// The priced node, plus its `best_cost` and `unpriced_count` for the parent.
pub(crate) fn annotate(node: &Node, prices: &HashMap<String, (f64, i64)>) -> (Value, Option<f64>, i64) {
    let price = prices.get(&node.name);
    let unit = price.map(|p| p.0);
    let buy = unit.map(|u| pyfloat::round(u * node.need as f64, 2));
    let craftable = node.crafts.is_some();
    let mut children = Vec::new();
    let (partial, craft, unpriced) = if craftable && !node.children.is_empty() {
        let mut known = 0.0;
        let mut all_known = true;
        let mut unpriced = 0;
        for child in &node.children {
            let (v, best, n) = annotate(child, prices);
            match best {
                None => all_known = false,
                Some(b) => known += b,
            }
            unpriced += n;
            children.push(v);
        }
        let k = pyfloat::round(known, 2);
        (Some(k), all_known.then_some(k), unpriced)
    } else {
        (None, None, if unit.is_some() { 0 } else { 1 })
    };
    let options: Vec<f64> = [buy, craft].into_iter().flatten().collect();
    let (best, rec) = if options.is_empty() {
        (None, "unknown")
    } else {
        let best = options.iter().copied().fold(options[0], |a, b| if b < a { b } else { a });
        let rec = if craft.is_some_and(|c| buy.is_none_or(|b| c <= b)) { "craft" } else { "buy" };
        (Some(best), rec)
    };
    let f = |v: Option<f64>| v.map_or(Value::Null, |x| json!(x));
    let mut m = Map::new();
    m.insert("path".into(), json!(node.path));
    m.insert("name".into(), json!(node.name));
    m.insert("need".into(), json!(node.need));
    m.insert("craftable".into(), json!(craftable));
    m.insert("crafts".into(), node.crafts.map_or(Value::Null, Value::from));
    m.insert("output_amount".into(), node.output_amount.map_or(Value::Null, Value::from));
    m.insert("children".into(), Value::Array(children));
    m.insert("market_price_each".into(), f(unit));
    m.insert("market_count".into(), json!(price.map_or(0, |p| p.1)));
    m.insert("buy_cost".into(), f(buy));
    m.insert("craft_cost_partial".into(), f(partial));
    m.insert("craft_cost".into(), f(craft));
    m.insert("unpriced_count".into(), json!(unpriced));
    m.insert("best_cost".into(), f(best));
    m.insert("recommendation".into(), json!(rec));
    (Value::Object(m), best, unpriced)
}

async fn cached_map(state: &AppState, branch: &str) -> ApiResult<Arc<RecipeMap>> {
    let Some(pool) = state.inner.db.pg.as_ref() else {
        return Ok(Arc::new(RecipeMap::new()));
    };
    let sig = store::meta_signature(pool, branch).await.map_err(internal)?;
    if let Some((s, m)) = MAP_CACHE.lock().expect("map cache").get(branch)
        && *s == sig
    {
        return Ok(m.clone());
    }
    let rows = store::all_recipes(pool, branch).await.map_err(internal)?;
    let rows: Vec<(String, String, Value)> =
        rows.iter().map(|r| (r.text("path"), r.text("name"), r.get("data").json())).collect();
    let map = Arc::new(recipe_map(&rows));
    MAP_CACHE.lock().expect("map cache").insert(branch.to_owned(), (sig, map.clone()));
    Ok(map)
}

/// `build_tree`: the priced tree for one recipe, or `None`.
pub async fn build_tree(state: &AppState, branch: &str, recipe_path: &str) -> ApiResult<Option<Value>> {
    let Some(pool) = state.inner.db.pg.as_ref() else { return Ok(None) };
    let Some(row) = store::get_entry(pool, branch, "recipe", recipe_path).await.map_err(internal)? else {
        return Ok(None);
    };
    let data = row.get("data").json();
    let rec = data.get("recipe").filter(|r| crate::features::py_truthy(r)).cloned().unwrap_or(Value::Null);
    let Some(out) = rec.get("output").filter(|o| crate::features::py_truthy(o)) else { return Ok(None) };
    let out_path = s(out, "path");
    if out_path.is_empty() {
        return Ok(None);
    }
    let name = if s(out, "name").is_empty() { row.text("name") } else { s(out, "name").to_owned() };
    let out_amount = amount(out.get("amount"));
    let map = cached_map(state, branch).await?;
    let mut walk = Walk::new(&map);
    let tree = walk.node(out_path, &name, out_amount, 0);
    let counter = walk.counter;
    let names: Vec<String> = walk.names.into_iter().collect();
    let prices = active_medians(&Live(state), &names).await?;
    let (root, _, _) = annotate(&tree, &prices);
    Ok(Some(json!({
        "branch": branch,
        "recipe_path": recipe_path,
        "output": {"path": out_path, "name": name, "amount": out_amount},
        "category": row.text("category"),
        "root": root,
        "node_count": counter,
        "priced_items": prices.len(),
        "truncated": counter >= MAX_NODES,
    })))
}
