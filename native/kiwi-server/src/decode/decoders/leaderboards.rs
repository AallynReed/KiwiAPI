//! `leaderboards.py`: every leaderboard the client defines.

use super::badges::{EXE_PATH, Text, parse_metric_names};
use crate::decode::{
    json::{J, Map},
    registry::{Built, Ctx},
    value::{Dict, W, parse},
};
use crate::obj;

const TABLE: &str = "prefabs/leaderboard/leaderboards.binfab";
const COMPOSITIONS: [&str; 3] = ["sum", "max", "min"];

fn leaf_of(v: &W) -> &Dict {
    match v {
        W::Dict(d) => d,
        _ => v.leaf(),
    }
}

fn rows(v: Option<&W>) -> Vec<&Dict> {
    match v {
        Some(x) if x.is_list() => x.elems().iter().map(leaf_of).collect(),
        _ => Vec::new(),
    }
}

fn class_field(kind: &str) -> Option<i128> {
    match kind {
        "classmetric" => Some(1),
        "metriccollection" | "powerrankclass" => Some(0),
        "delveclassdepth" => Some(1),
        _ => None,
    }
}

struct Metrics<'a> {
    names: Vec<String>,
    text: &'a Text,
}

impl Metrics<'_> {
    fn call(&self, mid: Option<&W>) -> Map {
        let id = mid.map_or(J::Null, J::from);
        let name = match mid {
            Some(W::Int(i)) if *i > 0 && (*i as usize) < self.names.len() => self.names[*i as usize].clone(),
            _ => String::new(),
        };
        let mut out = Map::new();
        out.insert("id".into(), id);
        if !name.is_empty() {
            let label = self.text.of(&format!("$Metrics_{name}"));
            out.insert("name".into(), J::from(name.as_str()));
            out.insert("label".into(), J::from(if label.is_empty() { name } else { label }));
        }
        out
    }
}

fn board(row: &Dict, text: &Text, metric: &Metrics) -> Option<J> {
    let bid = row.int(0)?;
    let source = row.get(3).filter(|s| s.is_obj())?;
    let src = source.leaf();
    let kind = src.str(0).unwrap_or("").to_owned();
    let parts: Vec<W> = src.get(1).filter(|p| p.is_list()).map(|p| p.elems().to_vec()).unwrap_or_default();
    let common = parts.first().map(leaf_of).unwrap_or(&crate::decode::value::EMPTY);
    let own = parts.get(1).map(leaf_of).unwrap_or(&crate::decode::value::EMPTY);
    let resets = match common.get(5) {
        Some(W::Int(0)) => "never",
        Some(W::Int(1)) => "daily",
        Some(W::Int(2)) => "weekly",
        _ => "",
    };
    let mut b = Map::new();
    b.insert("id".into(), J::Int(bid));
    b.insert("name".into(), J::from(text.get(row.get(1))));
    b.insert("icon".into(), J::from(row.str(2).unwrap_or("")));
    b.insert("kind".into(), J::from(kind.as_str()));
    b.insert("resets".into(), J::from(resets));
    let low = kind.to_lowercase();
    if let Some(ci) = class_field(&low).and_then(|f| own.int(f))
        && let Some(c) = usize::try_from(ci).ok().and_then(|i| kiwi_core::stats::BOARD_CLASS_ORDER.get(i))
    {
        b.insert("class".into(), J::from(*c));
    }
    if low == "playermetric" || low == "classmetric" {
        b.insert("metric".into(), J::Dict(metric.call(own.get(0))));
    } else if low == "metriccollection" {
        let points: Vec<J> = rows(own.get(1))
            .into_iter()
            .filter(|p| matches!(p.get(1), Some(W::Int(_) | W::Float(_))))
            .map(|p| {
                let mut m = metric.call(p.get(0));
                m.insert("points".into(), p.get(1).map_or(J::Null, J::from));
                J::Dict(m)
            })
            .collect();
        b.insert("points".into(), J::List(points));
    } else if low == "composite" {
        let ids = own.get(1);
        let combines: Vec<J> = match ids {
            Some(x) if x.is_list() => x.elems().iter().filter_map(W::as_int).map(J::Int).collect(),
            _ => Vec::new(),
        };
        b.insert("combines".into(), J::List(combines));
        if let Some(c) = own.int(3).and_then(|h| usize::try_from(h).ok()).and_then(|h| COMPOSITIONS.get(h)) {
            b.insert("composition".into(), J::from(*c));
        }
    }
    Some(J::Dict(b))
}

pub fn build(ctx: &Ctx) -> Built {
    let tree = &ctx.tree;
    let pf = parse(&tree.read(TABLE).unwrap_or_default()).map_err(|_| crate::decode::registry::DecodeError {
        kind: "WireError",
        msg: "no reading fills the object exactly".into(),
    })?;
    let root = pf.root.clone().unwrap_or(W::Obj(Vec::new().into()));
    let text = Text::new(tree);
    let metric =
        Metrics { names: tree.read(EXE_PATH).map(|e| parse_metric_names(&e)).unwrap_or_default(), text: &text };
    let boards: Vec<J> = rows(root.get(2)).into_iter().filter_map(|r| board(r, &text, &metric)).collect();
    let known: Vec<i128> = boards.iter().filter_map(|b| b.get("id").and_then(J::as_int)).collect();
    let mut categories = Vec::new();
    for c in rows(root.get(1)) {
        let ids: Vec<J> = c.iter_of(4).iter().filter_map(W::as_int).filter(|i| known.contains(i)).map(J::Int).collect();
        if !ids.is_empty() {
            categories
                .push(obj! {"id" => c.get(0).map_or(J::Null, J::from), "name" => text.get(c.get(3)), "boards" => ids});
        }
    }
    Ok(obj! {"categories" => categories, "boards" => boards})
}
