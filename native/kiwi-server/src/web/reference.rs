//! The reference pages: /commands from `site/static/commands.json` and
//! /classes, /abilities and /allies from the decoded game data, with the view
//! models of `app/site/{commands,classes,abilities,allies}_page.py`. Each model
//! is rebuilt only when its files change.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::SystemTime;

use axum::{
    extract::State,
    http::{HeaderMap, Uri},
    response::Response,
};
use minijinja::Value as Tpl;
use regex::Regex;
use serde_json::{Map, Value, json};

use super::catalog::{Broken, EMPTY, NONE, Py, as_str, float_num, group, or, page, py_iter, py_str, truthy};
use super::{Gate, Web};
use crate::gamedata::Gamedata;
use crate::middleware::pyurl::query_param;
use kiwi_core::pystr;

/// `for x in v` without the `or []`: `None` and numbers raise.
fn iter_strict(v: &Value) -> Py<Cow<'_, [Value]>> {
    match v {
        Value::Array(_) | Value::Object(_) | Value::String(_) => py_iter(v),
        _ => Err(Broken),
    }
}

fn obj(v: &Value) -> Py<&Map<String, Value>> {
    v.as_object().ok_or(Broken)
}

fn key<'a>(m: &'a Map<String, Value>, k: &str) -> Py<&'a Value> {
    m.get(k).ok_or(Broken)
}

/// `" ".join(parts)` where every part must be a str.
fn join(parts: &[&Value], sep: &str) -> Py<String> {
    Ok(parts.iter().map(|p| as_str(p)).collect::<Py<Vec<_>>>()?.join(sep))
}

/// A Python int or float (a bool counts as an int).
#[derive(Clone, Copy, Debug)]
enum Num {
    I(i128),
    F(f64),
}

impl Num {
    fn of(v: &Value) -> Option<Num> {
        match v {
            Value::Bool(b) => Some(Num::I(i128::from(*b))),
            Value::Number(n) => Some(match (n.as_i64(), n.as_u64()) {
                (Some(i), _) => Num::I(i128::from(i)),
                (_, Some(u)) => Num::I(i128::from(u)),
                _ => Num::F(n.as_f64().unwrap_or(f64::NAN)),
            }),
            _ => None,
        }
    }

    fn f(self) -> f64 {
        match self {
            Num::I(i) => i as f64,
            Num::F(f) => f,
        }
    }

    fn sub(self, o: Num) -> Num {
        match (self, o) {
            (Num::I(a), Num::I(b)) => Num::I(a - b),
            _ => Num::F(self.f() - o.f()),
        }
    }

    fn mul(self, k: i128) -> Num {
        match self {
            Num::I(a) => Num::I(a * k),
            Num::F(f) => Num::F(f * k as f64),
        }
    }

    fn abs(self) -> Num {
        match self {
            Num::I(a) => Num::I(a.abs()),
            Num::F(f) => Num::F(f.abs()),
        }
    }

    fn is_zero(self) -> bool {
        self.f() == 0.0
    }

    fn eq(self, o: Num) -> bool {
        match (self, o) {
            (Num::I(a), Num::I(b)) => a == b,
            _ => self.f() == o.f(),
        }
    }

    /// `f"{x:g}"`.
    fn g(self) -> String {
        crate::dressing::service::format_g(self.f())
    }
}

/// `x or 0` that must be a number.
fn num_or_zero(v: Option<&Value>) -> Py<Num> {
    match v {
        Some(v) if truthy(v) => Num::of(v).ok_or(Broken),
        _ => Ok(Num::I(0)),
    }
}

/// CPython 3.13 `sum()`: exact while the items are ints, then Neumaier
/// compensation over the float items.
fn py_sum(items: impl IntoIterator<Item = Num>) -> Num {
    let mut it = items.into_iter();
    let mut acc: i128 = 0;
    let mut f = loop {
        match it.next() {
            None => return Num::I(acc),
            Some(Num::I(i)) => acc += i,
            Some(Num::F(x)) => break acc as f64 + x,
        }
    };
    let mut c = 0.0f64;
    for item in it {
        match item {
            Num::F(x) => {
                let t = f + x;
                c += if f.abs() >= x.abs() { (f - t) + x } else { (x - t) + f };
                f = t;
            }
            Num::I(i) => f += i as f64,
        }
    }
    if c != 0.0 && c.is_finite() {
        f += c;
    }
    Num::F(f)
}

fn int_key(k: &str) -> Py<i64> {
    kiwi_core::pynum::int(k).ok_or(Broken)
}

// --- /commands ------------------------------------------------------------------

static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<[^<>]+>").expect("regex"));

/// `_t(field)`: the English text of a `{lang: text}` field.
fn tr(field: Option<&Value>) -> Py<Value> {
    match field {
        Some(f) if truthy(f) => Ok(or(obj(f)?.get("en"), &EMPTY).clone()),
        _ => Ok(EMPTY.clone()),
    }
}

fn hash(v: &Value) -> String {
    crate::pyjson::dumps(v)
}

/// `commands_page.commands_view()`.
pub fn commands_view(data: &Value) -> Py<Value> {
    let d = obj(data)?;
    let empty = json!({});
    let intro_raw = obj(d.get("intro").unwrap_or(&empty))?;
    let rules_raw = obj(or(intro_raw.get("rules"), &empty))?;
    let none_list = json!([]);
    let intro = json!({
        "title": tr(intro_raw.get("title"))?,
        "subtitle": tr(intro_raw.get("subtitle"))?,
        "search_placeholder": tr(intro_raw.get("search_placeholder"))?,
        "source_note": tr(intro_raw.get("source_note"))?,
        "placeholder_warning": tr(intro_raw.get("placeholder_warning"))?,
        "rules": or(rules_raw.get("en"), &none_list),
    });
    let commands = iter_strict(d.get("commands").unwrap_or(&none_list))?;
    let categories = iter_strict(d.get("categories").unwrap_or(&none_list))?;
    let mut counts: HashMap<String, i64> = HashMap::new();
    for cmd in commands.iter() {
        *counts.entry(hash(key(obj(cmd)?, "category")?)).or_default() += 1;
    }
    let mut chips = Vec::new();
    for cat in categories.iter() {
        let cat = obj(cat)?;
        let k = key(cat, "key")?;
        chips
            .push(json!({"key": k, "name": tr(cat.get("name"))?, "count": counts.get(&hash(k)).copied().unwrap_or(0)}));
    }
    let mut sections = Vec::new();
    let mut total = 0;
    for cat in categories.iter() {
        let cat = obj(cat)?;
        let k = key(cat, "key")?;
        let mut picked = Vec::new();
        for c in commands.iter() {
            let c = obj(c)?;
            if key(c, "category")? == k {
                picked.push(c);
            }
        }
        if picked.is_empty() {
            continue;
        }
        total += picked.len();
        let mut rows = Vec::new();
        for cmd in &picked {
            let aliases = or(cmd.get("aliases"), &none_list);
            let desc = tr(cmd.get("description"))?;
            let note = if cmd.get("note").is_some_and(truthy) { tr(cmd.get("note"))? } else { EMPTY.clone() };
            let syntax = key(cmd, "syntax")?;
            let alias_text = join(&py_iter(aliases)?.iter().collect::<Vec<_>>(), " ")?;
            let search =
                format!("{} {} {} {}", py_str(syntax), alias_text, py_str(&desc), py_str(&note)).to_lowercase();
            rows.push(json!({
                "syntax": syntax,
                "aliases": aliases,
                "desc": desc,
                "note": note,
                "has_placeholder": PLACEHOLDER.is_match(as_str(syntax)?),
                "search": search,
            }));
        }
        sections.push(json!({"key": k, "name": tr(cat.get("name"))?, "count": picked.len(), "rows": rows}));
    }
    Ok(json!({"intro": intro, "chips": chips, "sections": sections, "total": total, "ssr_lang": "en"}))
}

type StaticMemo = Mutex<HashMap<PathBuf, (SystemTime, Arc<Py<Tpl>>)>>;

/// `load_static_json(rel)` turned into a view, rebuilt when the file's mtime changes.
fn static_view(web: &Web, rel: &str, build: fn(&Value) -> Py<Value>) -> Arc<Py<Tpl>> {
    static MEMO: LazyLock<StaticMemo> = LazyLock::new(Mutex::default);
    let path = web.static_root.join(rel);
    let Ok(mtime) = std::fs::metadata(&path).and_then(|m| m.modified()) else { return Arc::new(Err(Broken)) };
    if let Some((at, v)) = MEMO.lock().unwrap_or_else(|e| e.into_inner()).get(&path)
        && *at == mtime
    {
        return v.clone();
    }
    let built = std::fs::read(&path)
        .ok()
        .and_then(|b| crate::pyjson::loads(&b).ok())
        .ok_or(Broken)
        .and_then(|data| build(&data))
        .map(|v| Tpl::from_serialize(&v));
    let built = Arc::new(built);
    MEMO.lock().unwrap_or_else(|e| e.into_inner()).insert(path, (mtime, built.clone()));
    built
}

pub async fn commands(Gate(flags): Gate, State(web): State<Arc<Web>>, uri: Uri, headers: HeaderMap) -> Response {
    let view = static_view(&web, "commands.json", commands_view);
    let ctx = view.as_ref().clone().map(|v| vec![("cmd", v)]);
    page(&web, &flags, &uri, &headers, "commands.html", ctx)
}

// --- /classes -------------------------------------------------------------------

fn clean_stat(s: &Value) -> Py<Value> {
    let m = obj(s)?;
    Ok(json!({
        "name": key(m, "name")?,
        "value": m.get("value").cloned().unwrap_or(Value::Null),
        "percentage": m.get("percentage").cloned().unwrap_or(Value::Bool(false)),
        "absolute": m.get("absolute").cloned().unwrap_or(Value::Bool(false)),
    }))
}

fn clean_stats(v: Option<&Value>) -> Py<Value> {
    let empty = json!([]);
    Ok(Value::Array(iter_strict(v.unwrap_or(&empty))?.iter().map(clean_stat).collect::<Py<_>>()?))
}

fn clean_levels(v: Option<&Value>) -> Py<Value> {
    let mut out = Map::new();
    if let Some(v) = v {
        for (lvl, stats) in obj(v)? {
            out.insert(lvl.clone(), clean_stats(Some(stats))?);
        }
    }
    Ok(Value::Object(out))
}

fn clean_ability(a: &Value) -> Py<Value> {
    let m = obj(a)?;
    let empty = json!([]);
    let mut stages = Vec::new();
    for st in iter_strict(m.get("stages").unwrap_or(&empty))?.iter() {
        let s = obj(st)?;
        stages.push(json!({
            "name": s.get("name").cloned().unwrap_or_else(|| EMPTY.clone()),
            "base": s.get("base").cloned().unwrap_or_else(|| Value::from(0)),
            "multiplier": s.get("multiplier").cloned().unwrap_or_else(|| Value::from(0)),
        }));
    }
    let get = |k: &str| m.get(k).cloned().unwrap_or_else(|| EMPTY.clone());
    Ok(json!({
        "name": get("name"),
        "description": get("description"),
        "type": get("type"),
        "active": m.get("active").cloned().unwrap_or(Value::Bool(true)),
        "stages": stages,
    }))
}

fn clean_abilities(v: Option<&Value>) -> Py<Vec<Value>> {
    let empty = json!([]);
    iter_strict(v.unwrap_or(&empty))?.iter().map(clean_ability).collect()
}

/// `stats._classes_index().values()` with the class levels kept.
fn classes_index(gd: &Gamedata) -> Py<Vec<Value>> {
    let empty = json!([]);
    let raw_abilities = gd.load("class_abilities.json");
    let mut fallback: HashMap<String, Value> = HashMap::new();
    for c in iter_strict(raw_abilities.as_deref().unwrap_or(&empty))?.iter() {
        let m = obj(c)?;
        fallback.insert(hash(key(m, "name")?), m.get("abilities").cloned().unwrap_or_else(|| empty.clone()));
    }
    let raw = gd.load("classes.json");
    let mut index: indexmap::IndexMap<String, Value> = indexmap::IndexMap::new();
    for c in iter_strict(raw.as_deref().unwrap_or(&empty))?.iter() {
        let m = obj(c)?;
        let tech = key(m, "qualified_name")?;
        let name = key(m, "name")?;
        if matches!(tech, Value::Array(_) | Value::Object(_)) {
            return Err(Broken);
        }
        let mut abilities = clean_abilities(m.get("abilities"))?;
        let sc = obj(m.get("subclass").unwrap_or(&json!({})))?.clone();
        let subclass = json!({
            "name": sc.get("name").cloned().unwrap_or_else(|| EMPTY.clone()),
            "description": sc.get("description").cloned().unwrap_or_else(|| EMPTY.clone()),
            "level": clean_levels(sc.get("level"))?,
        });
        let stats = clean_stats(m.get("stats"))?;
        let bonuses = clean_stats(m.get("bonuses"))?;
        let levels = clean_levels(m.get("levels"))?;
        if abilities.is_empty() {
            abilities = clean_abilities(Some(fallback.get(&hash(name)).unwrap_or(&empty)))?;
        }
        let get = |k: &str, d: Value| m.get(k).cloned().unwrap_or(d);
        let cleaned = json!({
            "tech_name": tech,
            "name": name,
            "shorts": get("shorts", json!([])),
            "damage_type": get("damage_type", EMPTY.clone()),
            "weapons": get("weapons", json!([])),
            "stats": stats,
            "bonuses": bonuses,
            "levels": levels,
            "subclass": subclass,
            "abilities": abilities,
        });
        index.insert(hash(tech), cleaned);
    }
    Ok(index.into_values().collect())
}

fn dmg_class(v: Option<&Value>) -> Py<&'static str> {
    let t = or(v, &EMPTY);
    Ok(if as_str(t)?.to_lowercase() == "physical" { "physical" } else { "magic" })
}

/// A sheet value as `_fmt_stat` prints it.
#[derive(Clone, Debug)]
enum Cell {
    N(Num),
    Other(Value),
}

impl Cell {
    fn of(v: Option<&Value>) -> Cell {
        match v {
            Some(v @ Value::Number(_)) => Cell::N(Num::of(v).unwrap_or(Num::I(0))),
            Some(v) => Cell::Other(v.clone()),
            None => Cell::Other(Value::Null),
        }
    }

    fn same(&self, o: &Cell) -> bool {
        match (self, o) {
            (Cell::N(a), Cell::N(b)) => a.eq(*b),
            (Cell::Other(a), Cell::Other(b)) => a == b,
            _ => false,
        }
    }
}

/// `classes_page._fmt_stat`.
fn fmt_cell(c: &Cell, pct: bool) -> Py<String> {
    let out = match c {
        Cell::Other(Value::Null) => return Ok("—".into()),
        Cell::N(Num::I(i)) => group(&i.to_string()),
        Cell::N(Num::F(f)) => float_num(*f)?,
        Cell::Other(_) => "0".into(),
    };
    Ok(if pct { format!("{out}%") } else { out })
}

fn fmt_stat(s: &Map<String, Value>) -> Py<String> {
    fmt_cell(&Cell::of(s.get("value")), s.get("percentage").is_some_and(truthy))
}

/// `classes_page._fmt_bonus`.
fn fmt_bonus(s: &Map<String, Value>) -> Py<String> {
    let out = fmt_stat(s)?;
    Ok(if s.get("absolute").is_some_and(truthy) || out.starts_with('-') { out } else { format!("+{out}") })
}

fn fmt_stages(stages: &Value) -> Py<Vec<Value>> {
    let mut out = Vec::new();
    for s in py_iter(stages)?.iter() {
        let s = obj(s)?;
        let mult = num_or_zero(s.get("multiplier"))?;
        let base = num_or_zero(s.get("base"))?;
        if mult.is_zero() && base.is_zero() {
            continue;
        }
        out.push(json!({
            "name": or(s.get("name"), &EMPTY),
            "damage": if mult.is_zero() { String::new() } else { format!("{}%", mult.mul(100).g()) },
            "base": if base.is_zero() { String::new() } else { format!("+{}", base.g()) },
        }));
    }
    Ok(out)
}

/// The levels of a `{level: [stat]}` dict, ordered by `int(level)`.
fn level_order(levels: &Map<String, Value>) -> Py<Vec<(&String, &Value)>> {
    let mut order = levels.iter().map(|(k, v)| Ok((int_key(k)?, k, v))).collect::<Py<Vec<_>>>()?;
    order.sort_by_key(|(n, ..)| *n);
    Ok(order.into_iter().map(|(_, k, v)| (k, v)).collect())
}

/// A cleaned `[stat]` list.
fn stat_rows(v: &Value) -> Py<Vec<&Map<String, Value>>> {
    match v {
        Value::Array(a) => a.iter().map(obj).collect(),
        _ => Err(Broken),
    }
}

/// `classes_page._growth`.
fn growth(levels: &Map<String, Value>) -> Py<Value> {
    let order = level_order(levels)?;
    let mut cols: Vec<&Value> = Vec::new();
    for (_, stats) in &order {
        for s in stat_rows(stats)? {
            if let Some(n) = s.get("name").filter(|n| truthy(n))
                && !cols.contains(&n)
            {
                cols.push(n);
            }
        }
    }
    if cols.is_empty() {
        return Ok(Value::Null);
    }
    let mut rows = Vec::new();
    for (lvl, stats) in &order {
        let stats = stat_rows(stats)?;
        let mut cells = Vec::new();
        for n in &cols {
            let hit = stats.iter().rev().find(|s| s.get("name").is_some_and(|x| truthy(x) && x == *n));
            cells.push(match hit {
                Some(s) => fmt_bonus(s)?,
                None => String::new(),
            });
        }
        rows.push(json!({"level": lvl, "cells": cells}));
    }
    Ok(json!({"cols": cols, "rows": rows}))
}

/// `classes_page._sheet_at`: a level-30 sheet value wound back to `level`.
fn sheet_at(stat: &Map<String, Value>, levels: &Map<String, Value>, level: i64) -> Py<Cell> {
    let v = match stat.get("value") {
        Some(v @ Value::Number(_)) => Num::of(v).ok_or(Broken)?,
        other => return Ok(Cell::Other(other.cloned().unwrap_or(Value::Null))),
    };
    let name = key(stat, "name")?;
    let mut v = v;
    for (lvl, rows) in levels {
        if int_key(lvl)? > level {
            let mut parts = Vec::new();
            for r in stat_rows(rows)? {
                if r.get("name") == Some(name) {
                    parts.push(num_or_zero(r.get("value"))?);
                }
            }
            v = v.sub(py_sum(parts));
        }
    }
    Ok(Cell::N(match v {
        Num::I(i) => Num::I(i),
        Num::F(f) => Num::F(crate::pyfloat::round(f, 2)),
    }))
}

/// `classes_page._totals`.
fn totals(stats: &[&Map<String, Value>], levels: &Map<String, Value>) -> Py<Value> {
    if levels.is_empty() {
        return Ok(Value::Null);
    }
    let mut grid = Vec::with_capacity(30);
    for lv in 1..=30 {
        grid.push(stats.iter().map(|s| sheet_at(s, levels, lv)).collect::<Py<Vec<_>>>()?);
    }
    let keep: Vec<usize> = (0..stats.len()).filter(|&i| !grid[0][i].same(&grid[29][i])).collect();
    if keep.is_empty() {
        return Ok(Value::Null);
    }
    let cols: Vec<&Value> = keep.iter().map(|&i| &stats[i]["name"]).collect();
    let mut rows = Vec::new();
    for lv in 1..=30usize {
        let cells = keep
            .iter()
            .map(|&i| fmt_cell(&grid[lv - 1][i], stats[i].get("percentage").is_some_and(truthy)))
            .collect::<Py<Vec<_>>>()?;
        rows.push(json!({"level": lv.to_string(), "cells": cells}));
    }
    Ok(json!({"cols": cols, "rows": rows}))
}

/// `[s for s in (v or []) if s and s.get("name")]`.
fn named_rows(v: Option<&Value>) -> Py<Vec<&Map<String, Value>>> {
    let mut out = Vec::new();
    match or(v, &NONE) {
        Value::Array(a) => {
            for s in a.iter().filter(|s| truthy(s)) {
                let m = obj(s)?;
                if m.get("name").is_some_and(truthy) {
                    out.push(m);
                }
            }
        }
        other => {
            if py_iter(other)?.iter().any(truthy) {
                return Err(Broken);
            }
        }
    }
    Ok(out)
}

/// `classes_page._detail(c)`.
fn detail(c: &Map<String, Value>) -> Py<Value> {
    let stat_list = named_rows(c.get("stats"))?;
    let stats =
        stat_list.iter().map(|s| Ok(json!({"name": s["name"], "val": fmt_stat(s)?}))).collect::<Py<Vec<_>>>()?;
    let mut bonuses = Vec::new();
    for b in named_rows(c.get("bonuses"))? {
        if b.get("value").is_some_and(truthy) {
            bonuses.push(json!({"name": b["name"], "val": fmt_bonus(b)?}));
        }
    }
    let empty = json!({});
    let sc = obj(or(c.get("subclass"), &empty))?;
    let levels = obj(or(sc.get("level"), &empty))?;
    let mut tiers = Vec::new();
    for (tier, rows) in level_order(levels)? {
        let mut meaningful = Vec::new();
        let list = py_iter(rows)?;
        for b in list.iter() {
            if !truthy(b) {
                continue;
            }
            let b = obj(b)?;
            let name = or(b.get("name"), &EMPTY);
            if !pystr::strip(as_str(name)?).is_empty() || b.get("value").is_some_and(truthy) {
                meaningful.push(b);
            }
        }
        if meaningful.is_empty() {
            continue;
        }
        let bonuses = meaningful
            .iter()
            .map(|b| Ok(json!({"name": pystr::strip(as_str(or(b.get("name"), &EMPTY))?), "val": fmt_bonus(b)?})))
            .collect::<Py<Vec<_>>>()?;
        tiers.push(json!({"tier": tier, "bonuses": bonuses}));
    }
    let subclass = if sc.get("name").is_some_and(truthy)
        || sc.get("description").is_some_and(truthy)
        || !tiers.is_empty()
    {
        json!({"name": or(sc.get("name"), &EMPTY), "description": or(sc.get("description"), &EMPTY), "tiers": tiers})
    } else {
        Value::Null
    };
    let (mut abilities, mut legacy) = (Vec::new(), Vec::new());
    for a in py_iter(or(c.get("abilities"), &NONE))?.iter() {
        if !truthy(a) {
            continue;
        }
        let a = obj(a)?;
        if !(a.get("name").is_some_and(truthy) || a.get("description").is_some_and(truthy)) {
            continue;
        }
        let inactive = a.get("active") == Some(&Value::Bool(false));
        let row = json!({
            "name": or(a.get("name"), &EMPTY),
            "description": or(a.get("description"), &EMPTY),
            "type": or(a.get("type"), &EMPTY),
            "inactive": inactive,
            "stages": fmt_stages(or(a.get("stages"), &NONE))?,
        });
        if inactive { legacy.push(row) } else { abilities.push(row) }
    }
    let shorts = py_iter(or(c.get("shorts"), &NONE))?;
    let level_map = obj(or(c.get("levels"), &empty))?;
    Ok(json!({
        "tech": key(c, "tech_name")?,
        "name": or(c.get("name"), &EMPTY),
        "damage_type": or(c.get("damage_type"), &EMPTY),
        "dmg_class": dmg_class(c.get("damage_type"))?,
        "weapons": or(c.get("weapons"), &json!([])),
        "shorts": join(&shorts.iter().collect::<Vec<_>>(), " / ")?,
        "stats": stats,
        "growth": growth(level_map)?,
        "totals": totals(&stat_list, level_map)?,
        "bonuses": bonuses,
        "subclass": subclass,
        "abilities": abilities,
        "legacy": legacy,
    }))
}

/// `classes_page.classes_view()`.
pub fn classes_view(gd: &Gamedata) -> Py<Value> {
    let items = classes_index(gd)?;
    let mut nav = Vec::new();
    for c in &items {
        let c = obj(c)?;
        nav.push(json!({
            "tech": c["tech_name"],
            "name": or(c.get("name"), &EMPTY),
            "damage_type": or(c.get("damage_type"), &EMPTY),
            "dmg_class": dmg_class(c.get("damage_type"))?,
        }));
    }
    let first = items.first().map(|c| obj(c)).transpose()?;
    Ok(json!({
        "nav": nav,
        "initial": first.map(detail).transpose()?,
        "initial_tech": first.map_or(EMPTY.clone(), |c| c["tech_name"].clone()),
        "count": items.len(),
    }))
}

fn gamedata(web: &Web) -> &'static Gamedata {
    Gamedata::global(&web.settings)
}

pub async fn classes(Gate(flags): Gate, State(web): State<Arc<Web>>, uri: Uri, headers: HeaderMap) -> Response {
    let gd = gamedata(&web);
    let view = gd.cached("web.classes_view", &["classes.json", "class_abilities.json"], || {
        classes_view(gd).map(|v| Tpl::from_serialize(&v))
    });
    let ctx = view.as_ref().clone().map(|v| vec![("cls", v)]);
    page(&web, &flags, &uri, &headers, "classes.html", ctx)
}

// --- /abilities -----------------------------------------------------------------

const ELEMENT_ORDER: [&str; 5] = ["Water", "Fire", "Air", "Cosmic", "Prismatic"];

static LEAD: LazyLock<Regex> = LazyLock::new(|| {
    let s = r"[\s\x1c-\x1f]";
    Regex::new(&format!(r"(?si)^Empowered{s}+\w+{s}+Gem\b(?:{s}+for{s}+(?:the{s}+)?([^.;]+?))?{s}*[.;]{s}*(.*)$"))
        .expect("regex")
});

static NOT_NUMERIC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^0-9.\-]").expect("regex"));

/// `abilities_page._split_description`: who the gem is for, and the rest.
fn split_description(text: &str) -> (String, String) {
    let text = pystr::strip(text);
    let Some(caps) = LEAD.captures(text) else { return (String::new(), text.to_owned()) };
    let who = pystr::strip(caps.get(1).map_or("", |m| m.as_str())).to_owned();
    let rest = pystr::strip(caps.get(2).map_or("", |m| m.as_str())).to_owned();
    if who.is_empty() {
        return (String::new(), rest);
    }
    let lower = who.to_lowercase();
    (if lower == "any class" || lower == "all classes" { "Any class".into() } else { who }, rest)
}

#[derive(PartialEq, Clone)]
struct Effect {
    label: Value,
    value: String,
    base: String,
    kind: &'static str,
    prefab: Value,
}

impl Effect {
    fn json(&self) -> Value {
        json!({"label": self.label, "value": self.value, "base": self.base, "kind": self.kind, "prefab": self.prefab})
    }
}

/// A stat row's value in its operation's sense: `+350`, `+25%`, `x1.5`
/// (`abilities_page._effects` and `allies_page._fmt`).
fn shown_stat(s: &Map<String, Value>) -> Py<String> {
    let value = || num_or_zero(s.get("value")).map(Num::g);
    Ok(match s.get("op").and_then(Value::as_str) {
        Some("Add") => format!("+{}", value()?),
        Some("Multiply") => match s.get("amount").filter(|a| truthy(a)) {
            Some(a) => format!("x{}", Num::of(a).ok_or(Broken)?.g()),
            None => format!("{}%", value()?),
        },
        _ => format!("+{}%", value()?),
    })
}

/// `abilities_page._effects(entry)` over its `stats` and `stages`.
fn effects(stats: &[Value], stages: &[Value]) -> Py<Vec<Effect>> {
    let mut rows: Vec<Effect> = Vec::new();
    for stat in stats {
        let s = obj(stat)?;
        let row = Effect {
            label: s.get("name").cloned().unwrap_or_else(|| EMPTY.clone()),
            value: shown_stat(s)?,
            base: String::new(),
            kind: "stat",
            prefab: s.get("prefab").cloned().unwrap_or_else(|| EMPTY.clone()),
        };
        if !rows.contains(&row) {
            rows.push(row);
        }
    }
    for stage in stages {
        let s = obj(stage)?;
        let mult = num_or_zero(s.get("multiplier"))?;
        let base = num_or_zero(s.get("base"))?;
        if mult.is_zero() && base.is_zero() {
            continue;
        }
        let row = Effect {
            label: s.get("name").cloned().unwrap_or_else(|| EMPTY.clone()),
            value: if mult.is_zero() { String::new() } else { format!("{}%", mult.mul(100).g()) },
            base: if base.is_zero() { String::new() } else { format!("+{}", base.g()) },
            kind: "damage",
            prefab: s.get("prefab").cloned().unwrap_or_else(|| EMPTY.clone()),
        };
        if !rows.contains(&row) {
            rows.push(row);
        }
    }
    Ok(rows)
}

/// `abilities_page._phases(rows)`: effect rows grouped by their prefab, weakest first.
fn phases(rows: &[Effect]) -> Py<Vec<Value>> {
    let mut groups: Vec<(Value, Vec<&Effect>)> = Vec::new();
    for row in rows {
        let k = or(Some(&row.prefab), &EMPTY).clone();
        match groups.iter_mut().find(|(g, _)| *g == k) {
            Some((_, list)) => list.push(row),
            None => groups.push((k, vec![row])),
        }
    }
    if groups.len() < 2 {
        let all: Vec<Value> = rows.iter().map(Effect::json).collect();
        return Ok(vec![json!({"label": "", "prefab": "", "rows": all})]);
    }
    let mut weighted = Vec::with_capacity(groups.len());
    for (prefab, list) in groups {
        let mut parts = Vec::new();
        for r in &list {
            let digits = NOT_NUMERIC.replace_all(&r.value, "");
            let x = if digits.is_empty() { 0.0 } else { kiwi_core::pynum::float(&digits).ok_or(Broken)? };
            parts.push(Num::F(x.abs()));
        }
        weighted.push((py_sum(parts).f(), prefab, list));
    }
    weighted.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    weighted
        .into_iter()
        .map(|(_, prefab, list)| {
            let tail = as_str(&prefab)?.rsplit('/').next().unwrap_or("").replace('_', " ");
            let label = crate::og::board::py_title(&tail);
            Ok(json!({"label": label, "prefab": prefab, "rows": list.iter().map(|r| r.json()).collect::<Vec<_>>()}))
        })
        .collect()
}

struct Card {
    name: Value,
    filters: Vec<Value>,
    json: Value,
}

/// `abilities_page._card`.
fn card(
    name: Value,
    chips: Vec<Value>,
    description: Value,
    badge: &str,
    fx: &[Effect],
    filters: Vec<Value>,
) -> Py<Card> {
    let mut parts = vec![&name, &description];
    parts.extend(chips.iter().map(|c| &c["label"]));
    let search = join(&parts, " ")?.to_lowercase();
    let json = json!({
        "name": name, "chips": chips, "description": description, "badge": badge,
        "effects": fx.iter().map(Effect::json).collect::<Vec<_>>(), "phases": phases(fx)?,
        "filters": filters, "search": search,
    });
    Ok(Card { name, filters, json })
}

fn lower_name(c: &Card) -> Py<String> {
    Ok(as_str(&c.name)?.to_lowercase())
}

/// Sort by `(filters[0] or "", name.lower())`.
fn sort_by_owner(cards: &mut Vec<Card>) -> Py<Vec<Value>> {
    let mut keyed = cards
        .drain(..)
        .map(|c| {
            let owner = match c.filters.first() {
                Some(f) => as_str(f)?.to_owned(),
                None => String::new(),
            };
            Ok(((owner, lower_name(&c)?), c))
        })
        .collect::<Py<Vec<_>>>()?;
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    let mut owners: Vec<String> =
        keyed.iter().filter(|(_, c)| !c.filters.is_empty()).map(|((o, _), _)| o.clone()).collect();
    owners.sort();
    owners.dedup();
    cards.extend(keyed.into_iter().map(|(_, c)| c));
    Ok(owners.into_iter().map(Value::from).collect())
}

fn load(gd: &Gamedata, name: &str) -> Value {
    gd.load(name).map_or_else(|| json!([]), |v| v.as_ref().clone())
}

fn as_list(v: Option<&Value>) -> Py<Vec<Value>> {
    Ok(py_iter(or(v, &NONE))?.into_owned())
}

/// `list += iterable`.
fn extend(into: &mut Vec<Value>, v: Option<&Value>) -> Py<()> {
    into.extend(as_list(v)?);
    Ok(())
}

fn gem_cards(gd: &Gamedata) -> Py<(Vec<Card>, Vec<Value>)> {
    struct Entry {
        elements: Vec<Value>,
        tiers: Vec<Value>,
        who: String,
        description: String,
        stats: Vec<Value>,
        stages: Vec<Value>,
    }
    let gems = load(gd, "gem_abilities.json");
    let mut folded: indexmap::IndexMap<String, (Value, Entry)> = indexmap::IndexMap::new();
    for gem in iter_strict(&gems)?.iter() {
        let g = obj(gem)?;
        let name = or(g.get("name"), or(g.get("slug"), &EMPTY)).clone();
        let skip = g.get("element").and_then(Value::as_str) == Some("Prismatic")
            && g.get("slug").and_then(Value::as_str) == Some("mastery");
        if !truthy(&name) || skip {
            continue;
        }
        let (applies_to, body) = split_description(as_str(or(g.get("description"), &EMPTY))?);
        let entry = &mut folded
            .entry(hash(&name))
            .or_insert_with(|| {
                (
                    name.clone(),
                    Entry {
                        elements: Vec::new(),
                        tiers: Vec::new(),
                        who: applies_to.clone(),
                        description: body.clone(),
                        stats: Vec::new(),
                        stages: Vec::new(),
                    },
                )
            })
            .1;
        if let Some(el) = g.get("element").filter(|e| truthy(e))
            && !entry.elements.contains(el)
        {
            entry.elements.push(el.clone());
        }
        extend(&mut entry.tiers, g.get("tiers"))?;
        if entry.description.is_empty() {
            entry.description = body;
        }
        if entry.who.is_empty() {
            entry.who = applies_to;
        }
        let new: Vec<Value> = as_list(g.get("stats"))?.into_iter().filter(|s| !entry.stats.contains(s)).collect();
        entry.stats.extend(new);
        let new: Vec<Value> = as_list(g.get("stages"))?.into_iter().filter(|s| !entry.stages.contains(s)).collect();
        entry.stages.extend(new);
    }
    let mut cards = Vec::new();
    for (_, (name, mut entry)) in folded {
        let rank = |e: &Value| e.as_str().and_then(|s| ELEMENT_ORDER.iter().position(|o| *o == s)).unwrap_or(99);
        entry.elements.sort_by_key(rank);
        let mut tiers: Vec<Num> = Vec::new();
        for t in &entry.tiers {
            let n = Num::of(t).ok_or(Broken)?;
            if !tiers.iter().any(|x| x.eq(n)) {
                tiers.push(n);
            }
        }
        tiers.sort_by(|a, b| a.f().partial_cmp(&b.f()).unwrap_or(std::cmp::Ordering::Equal));
        let show = |n: &Num| match n {
            Num::I(i) => i.to_string(),
            Num::F(f) => crate::pyfloat::repr(*f),
        };
        let badge = match tiers.as_slice() {
            [] => String::new(),
            [one] => format!("T{}", show(one)),
            [first, .., last] => format!("T{}-T{}", show(first), show(last)),
        };
        let mut chips = Vec::new();
        for e in &entry.elements {
            chips.push(json!({"label": e, "tone": as_str(e)?.to_lowercase()}));
        }
        if !entry.who.is_empty() {
            chips.push(json!({"label": entry.who, "tone": "who"}));
        }
        let fx = effects(&entry.stats, &entry.stages)?;
        cards.push(card(name, chips, Value::from(entry.description), &badge, &fx, entry.elements.clone())?);
    }
    let cards = sorted_by_name(cards)?;
    let used = ELEMENT_ORDER
        .iter()
        .filter(|e| cards.iter().any(|c| c.filters.iter().any(|f| f.as_str() == Some(**e))))
        .map(|e| Value::from(*e))
        .collect();
    Ok((cards, used))
}

fn sorted_by_name(cards: Vec<Card>) -> Py<Vec<Card>> {
    let mut keyed = cards.into_iter().map(|c| Ok((lower_name(&c)?, c))).collect::<Py<Vec<_>>>()?;
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(keyed.into_iter().map(|(_, c)| c).collect())
}

fn stats_and_stages(m: &Map<String, Value>) -> Py<(Vec<Value>, Vec<Value>)> {
    Ok((as_list(m.get("stats"))?, as_list(m.get("stages"))?))
}

fn ring_cards(gd: &Gamedata) -> Py<(Vec<Card>, Vec<Value>)> {
    let rings = load(gd, "ring_abilities.json");
    let mut cards = Vec::new();
    for ring in iter_strict(&rings)?.iter() {
        let r = obj(ring)?;
        let owner = or(r.get("class"), &EMPTY).clone();
        let has = truthy(&owner);
        let chips = if has { vec![json!({"label": owner, "tone": "who"})] } else { Vec::new() };
        let (stats, stages) = stats_and_stages(r)?;
        let fx = effects(&stats, &stages)?;
        let filters = if has { vec![owner] } else { Vec::new() };
        cards.push(card(
            or(r.get("name"), &EMPTY).clone(),
            chips,
            or(r.get("description"), &EMPTY).clone(),
            "",
            &fx,
            filters,
        )?);
    }
    let filters = sort_by_owner(&mut cards)?;
    Ok((cards, filters))
}

fn class_cards(gd: &Gamedata) -> Py<(Vec<Card>, Vec<Value>)> {
    let classes = load(gd, "class_abilities.json");
    let mut cards = Vec::new();
    for entry in iter_strict(&classes)?.iter() {
        let e = obj(entry)?;
        let owner = or(e.get("name"), &EMPTY).clone();
        for ability in py_iter(or(e.get("abilities"), &NONE))?.iter() {
            let a = obj(ability)?;
            let mut chips = vec![json!({"label": owner, "tone": "who"})];
            if let Some(t) = a.get("type").filter(|t| truthy(t)) {
                chips.push(json!({"label": t, "tone": "kind"}));
            }
            let active = a.get("active").is_none_or(truthy);
            let (stats, stages) = stats_and_stages(a)?;
            let fx = effects(&stats, &stages)?;
            cards.push(card(
                or(a.get("name"), &EMPTY).clone(),
                chips,
                or(a.get("description"), &EMPTY).clone(),
                if active { "" } else { "Legacy" },
                &fx,
                vec![owner.clone()],
            )?);
        }
    }
    let filters = sort_by_owner(&mut cards)?;
    Ok((cards, filters))
}

fn star_chart_cards(gd: &Gamedata) -> Py<(Vec<Card>, Vec<Value>)> {
    let chart = load(gd, "star_chart.json");
    let Value::Object(roots) = &chart else { return Ok((Vec::new(), Vec::new())) };
    fn walk<'a>(node: &'a Value, out: &mut Vec<&'a Map<String, Value>>) -> Py<()> {
        let m = obj(node)?;
        out.push(m);
        match or(m.get("Stars"), &NONE) {
            Value::Array(a) => a.iter().try_for_each(|c| walk(c, out)),
            v if !truthy(v) => Ok(()),
            _ => Err(Broken),
        }
    }
    let mut nodes = Vec::new();
    for root in roots.values() {
        walk(root, &mut nodes)?;
    }
    let mut cards = Vec::new();
    for node in nodes {
        let abilities = as_list(node.get("Abilities"))?;
        let text = pystr::strip(&join(&abilities.iter().collect::<Vec<_>>(), " ")?).to_owned();
        if text.is_empty() {
            continue;
        }
        let constellation = or(node.get("Constellation"), &EMPTY).clone();
        let mut chips = vec![json!({"label": constellation, "tone": "who"})];
        if let Some(t) = node.get("Type").filter(|t| truthy(t)) {
            chips.push(json!({"label": t, "tone": "kind"}));
        }
        let fx = effects(&as_list(node.get("Ability_Values"))?, &[])?;
        let filters = if truthy(&constellation) { vec![constellation.clone()] } else { Vec::new() };
        let name = or(node.get("Name"), or(node.get("Path"), &EMPTY)).clone();
        cards.push(card(name, chips, Value::from(text), "", &fx, filters)?);
    }
    let filters = sort_by_owner(&mut cards)?;
    Ok((cards, filters))
}

fn ally_cards(gd: &Gamedata) -> Py<(Vec<Card>, Vec<Value>)> {
    let allies = load(gd, "ally_abilities.json");
    let mut cards = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for ally in iter_strict(&allies)?.iter() {
        let a = obj(ally)?;
        if !a.get("abilities").is_some_and(truthy) {
            continue;
        }
        let mut granted: Vec<String> = Vec::new();
        for s in py_iter(or(a.get("stats"), &NONE))?.iter() {
            let name = as_str(key(obj(s)?, "name")?)?;
            if !name.starts_with('$') {
                granted.push(name.to_owned());
            }
        }
        let mut unique: Vec<&String> = Vec::new();
        for g in &granted {
            if !unique.contains(&g) {
                unique.push(g);
            }
        }
        let chips = unique.iter().map(|n| json!({"label": n, "tone": "kind"})).collect();
        seen.extend(granted.iter().cloned());
        let mut texts = Vec::new();
        let list = iter_strict(&a["abilities"])?;
        for ab in list.iter() {
            let ab = obj(ab)?;
            if ab.get("text").is_some_and(truthy) {
                texts.push(key(ab, "text")?);
            }
        }
        let text = pystr::strip(&join(&texts, " ")?).to_owned();
        let (stats, stages) = stats_and_stages(a)?;
        let fx = effects(&stats, &stages)?;
        let mut filters = granted.clone();
        filters.sort();
        filters.dedup();
        let name = or(a.get("name"), or(a.get("slug"), &EMPTY)).clone();
        cards.push(card(name, chips, Value::from(text), "", &fx, filters.into_iter().map(Value::from).collect())?);
    }
    let cards = sorted_by_name(cards)?;
    seen.sort();
    seen.dedup();
    Ok((cards, seen.into_iter().map(Value::from).collect()))
}

type Builder = fn(&Gamedata) -> Py<(Vec<Card>, Vec<Value>)>;

const TABS: [(&str, &str, &str, Builder); 5] = [
    ("gems", "Gems", "fa-solid fa-gem", gem_cards),
    ("rings", "Rings", "fa-solid fa-ring", ring_cards),
    ("classes", "Classes", "fa-solid fa-user", class_cards),
    ("star-chart", "Star Chart", "fa-solid fa-star", star_chart_cards),
    ("allies", "Allies", "fa-solid fa-paw", ally_cards),
];

/// `abilities_page.abilities_view()["tabs"]`.
pub fn ability_tabs(gd: &Gamedata) -> Py<Value> {
    let mut tabs = Vec::new();
    for (key, label, icon, build) in TABS {
        let (cards, filters) = build(gd)?;
        let count = cards.len();
        let cards: Vec<Value> = cards.into_iter().map(|c| c.json).collect();
        tabs.push(
            json!({"key": key, "label": label, "icon": icon, "cards": cards, "filters": filters, "count": count}),
        );
    }
    Ok(Value::Array(tabs))
}

const ABILITY_FILES: [&str; 5] =
    ["gem_abilities.json", "ring_abilities.json", "class_abilities.json", "star_chart.json", "ally_abilities.json"];

pub async fn abilities(Gate(flags): Gate, State(web): State<Arc<Web>>, uri: Uri, headers: HeaderMap) -> Response {
    let gd = gamedata(&web);
    let tabs = gd.cached("web.ability_tabs", &ABILITY_FILES, || ability_tabs(gd).map(|v| Tpl::from_serialize(&v)));
    let tab = query_param(&uri, "tab").unwrap_or_else(|| "gems".into());
    let active = if TABS.iter().any(|(k, ..)| *k == tab) { tab } else { "gems".into() };
    let ctx = tabs
        .as_ref()
        .clone()
        .map(|tabs| vec![("abilities", Tpl::from_iter([("tabs", tabs), ("active", Tpl::from(active))]))]);
    page(&web, &flags, &uri, &headers, "abilities.html", ctx)
}

// --- /allies --------------------------------------------------------------------

static NON_SLUG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^a-z0-9]+").expect("regex"));

/// `allies_page.slug`.
fn slug(name: &str) -> String {
    NON_SLUG.replace_all(&name.to_lowercase(), "-").trim_matches('-').to_owned()
}

/// `allies_page.allies_view()`.
pub fn allies_view(gd: &Gamedata) -> Py<Value> {
    let allies = load(gd, "ally_abilities.json");
    let mut rows: Vec<(String, Value)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for ally in iter_strict(&allies)?.iter() {
        let a = obj(ally)?;
        let mut stats = Vec::new();
        let mut names = Vec::new();
        let mut sortable = Map::new();
        for stat in py_iter(or(a.get("stats"), &NONE))?.iter() {
            let s = obj(stat)?;
            let name = or(s.get("name"), &EMPTY);
            if !truthy(name) {
                continue;
            }
            let name = as_str(name)?;
            seen.push(name.to_owned());
            let sl = slug(name);
            stats.push(json!({"name": name, "slug": sl, "shown": shown_stat(s)?}));
            names.push(name.to_owned());
            let v = num_or_zero(s.get("value"))?.abs().f();
            let prev = sortable.get(&sl).and_then(Value::as_f64).unwrap_or(0.0);
            sortable.insert(sl, Value::from(if v > prev { v } else { prev }));
        }
        if stats.is_empty() {
            continue;
        }
        let mut powers = Vec::new();
        for ab in py_iter(or(a.get("abilities"), &NONE))?.iter() {
            let ab = obj(ab)?;
            if ab.get("text").is_some_and(truthy) {
                powers.push(as_str(key(ab, "text")?)?.to_owned());
            }
        }
        let name = or(a.get("name"), or(a.get("slug"), &EMPTY));
        let name_s = as_str(name)?;
        let mut search = vec![name_s.to_owned()];
        search.extend(names);
        search.extend(powers.iter().cloned());
        rows.push((
            name_s.to_lowercase(),
            json!({
                "name": name,
                "stats": stats,
                "sort_values": sortable,
                "ability": pystr::strip(&powers.join(" ")),
                "search": search.join(" ").to_lowercase(),
            }),
        ));
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    seen.sort();
    seen.dedup();
    let count = rows.len();
    let stats: Vec<Value> = seen.iter().map(|n| json!({"name": n, "slug": slug(n)})).collect();
    Ok(json!({"rows": rows.into_iter().map(|(_, r)| r).collect::<Vec<_>>(), "stats": stats, "count": count}))
}

pub async fn allies(Gate(flags): Gate, State(web): State<Arc<Web>>, uri: Uri, headers: HeaderMap) -> Response {
    let gd = gamedata(&web);
    let view =
        gd.cached("web.allies_view", &["ally_abilities.json"], || allies_view(gd).map(|v| Tpl::from_serialize(&v)));
    let ctx = view.as_ref().clone().map(|v| vec![("allies", v)]);
    page(&web, &flags, &uri, &headers, "allies.html", ctx)
}
