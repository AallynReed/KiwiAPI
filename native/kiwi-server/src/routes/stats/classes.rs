//! The class objects: `_classes_index()` (the cleaning, which raises on a
//! malformed entry) projected through the `TroveClass` model, which keeps
//! only its own fields and rejects wrong types.

use std::{collections::HashMap, sync::Arc};

use bytes::Bytes;
use serde_json::{Map, Value, json};

use super::{get_or, py_items};
use crate::{
    error::ApiError,
    gamedata::Gamedata,
    pyjson,
    validation::{Field, Input, Validator},
};

/// A Python exception while cleaning: the whole index is a 500.
struct Broken;

type Clean<T> = Result<T, Broken>;

fn dict(v: &Value) -> Clean<&Map<String, Value>> {
    v.as_object().ok_or(Broken)
}

fn key<'a>(m: &'a Map<String, Value>, k: &str) -> Clean<&'a Value> {
    m.get(k).ok_or(Broken)
}

fn items(v: &Value) -> Clean<&[Value]> {
    py_items(v).ok_or(Broken)
}

/// A dict key: unhashable values raise, a non-str one can never be looked up.
fn hash_key(v: &Value) -> Clean<Option<String>> {
    match v {
        Value::String(s) => Ok(Some(s.clone())),
        Value::Array(_) | Value::Object(_) => Err(Broken),
        _ => Ok(None),
    }
}

fn clean_stat(s: &Value) -> Clean<Value> {
    let m = dict(s)?;
    Ok(json!({
        "name": key(m, "name")?,
        "value": get_or(m, "value", Value::Null),
        "percentage": get_or(m, "percentage", false.into()),
        "absolute": get_or(m, "absolute", false.into()),
    }))
}

fn clean_stats(v: &Value) -> Clean<Value> {
    Ok(Value::Array(items(v)?.iter().map(clean_stat).collect::<Clean<_>>()?))
}

/// `{lvl: [_clean_stat(x) for x in stats] for lvl, stats in d.items()}`.
fn clean_levels(v: &Value) -> Clean<Value> {
    let mut out = Map::new();
    for (lvl, stats) in dict(v)? {
        out.insert(lvl.clone(), clean_stats(stats)?);
    }
    Ok(Value::Object(out))
}

fn clean_stage(st: &Value) -> Clean<Value> {
    let m = dict(st)?;
    Ok(json!({
        "name": get_or(m, "name", "".into()),
        "base": get_or(m, "base", 0.into()),
        "multiplier": get_or(m, "multiplier", 0.into()),
        "prefab": get_or(m, "prefab", "".into()),
    }))
}

fn clean_ability(a: &Value) -> Clean<Value> {
    let m = dict(a)?;
    let stages = items(&get_or(m, "stages", json!([])))?.iter().map(clean_stage).collect::<Clean<Vec<_>>>()?;
    Ok(json!({
        "name": get_or(m, "name", "".into()),
        "description": get_or(m, "description", "".into()),
        "icon": get_or(m, "icon", "".into()),
        "type": get_or(m, "type", "".into()),
        "prefab": get_or(m, "prefab", "".into()),
        "active": get_or(m, "active", true.into()),
        "stages": stages,
    }))
}

fn clean_abilities(v: &Value) -> Clean<Vec<Value>> {
    items(v)?.iter().map(clean_ability).collect()
}

fn clean_subclass(sc: &Value) -> Clean<Value> {
    let m = dict(sc)?;
    Ok(json!({
        "name": get_or(m, "name", "".into()),
        "description": get_or(m, "description", "".into()),
        "level": clean_levels(&get_or(m, "level", json!({})))?,
        "power": get_or(m, "power", json!({})),
    }))
}

/// `_clean_class` plus the `class_abilities.json` fallback of `_classes_index`.
fn clean_class(c: &Value, fallback: &HashMap<String, Value>) -> Clean<(Option<String>, Value)> {
    let m = dict(c)?;
    let tech = key(m, "qualified_name")?;
    let name = key(m, "name")?;
    let stats = clean_stats(&get_or(m, "stats", json!([])))?;
    let bonuses = clean_stats(&get_or(m, "bonuses", json!([])))?;
    clean_levels(&get_or(m, "levels", json!({})))?;
    let subclass = clean_subclass(&get_or(m, "subclass", json!({})))?;
    let mut abilities = clean_abilities(&get_or(m, "abilities", json!([])))?;
    if abilities.is_empty() {
        let own = hash_key(name)?.and_then(|n| fallback.get(&n));
        abilities = clean_abilities(own.unwrap_or(&json!([])))?;
    }
    let cleaned = json!({
        "tech_name": tech,
        "name": name,
        "shorts": get_or(m, "shorts", json!([])),
        "damage_type": get_or(m, "damage_type", "".into()),
        "weapons": get_or(m, "weapons", json!([])),
        "attributes": get_or(m, "attributes", json!([])),
        "stats": stats,
        "bonuses": bonuses,
        "subclass": subclass,
        "abilities": abilities,
    });
    Ok((hash_key(tech)?, cleaned))
}

fn class_stat(f: Field<'_>) -> Value {
    f.model(|m| {
        json!({
            "name": m.field("name").str().required(),
            "value": m.field("value").float().nullable(),
            "percentage": m.field("percentage").bool().required(),
            "absolute": m.field("absolute").bool().required(),
        })
    })
    .required()
}

fn stage(f: Field<'_>) -> Value {
    f.model(|m| {
        json!({
            "name": m.field("name").str().required(),
            "base": m.field("base").float().required(),
            "multiplier": m.field("multiplier").float().required(),
            "prefab": m.field("prefab").str().required(),
        })
    })
    .required()
}

fn ability(f: Field<'_>) -> Value {
    f.model(|m| {
        json!({
            "name": m.field("name").str().required(),
            "description": m.field("description").str().required(),
            "icon": m.field("icon").str().required(),
            "type": m.field("type").str().required(),
            "prefab": m.field("prefab").str().required(),
            "active": m.field("active").bool().required(),
            "stages": m.field("stages").list(stage).required(),
        })
    })
    .required()
}

fn pairs(p: Vec<(String, Value)>) -> Value {
    Value::Object(p.into_iter().collect())
}

/// `TroveClass(**cleaned)` dumped, or `None` when pydantic rejects it.
fn validate(cleaned: &Value) -> Option<Value> {
    let mut v = Validator::default();
    let out = Field::new(&mut v, vec![], Input::Json(cleaned))
        .model(|m| {
            json!({
                "tech_name": m.field("tech_name").str().required(),
                "name": m.field("name").str().required(),
                "shorts": m.field("shorts").list(|f| f.str().required()).required(),
                "damage_type": m.field("damage_type").str().required(),
                "weapons": m.field("weapons").list(|f| f.str().required()).required(),
                "attributes": m.field("attributes").list(|f| f.str().required()).required(),
                "stats": m.field("stats").list(class_stat).required(),
                "bonuses": m.field("bonuses").list(class_stat).required(),
                "subclass": m.field("subclass").model(|s| json!({
                    "name": s.field("name").str().required(),
                    "description": s.field("description").str().required(),
                    "level": pairs(s.field("level").dict(|f| Value::Array(f.list(class_stat).required())).required()),
                    "power": pairs(s.field("power").dict(|f| Value::String(f.str().required())).required()),
                })).required(),
                "abilities": m.field("abilities").list(ability).required(),
            })
        })
        .required();
    v.is_ok().then_some(out)
}

/// The index in `classes.json` order: each class's lookup key and rendered
/// `TroveClass` (`None` where pydantic rejects it), plus the `ClassList`.
struct Index {
    classes: Vec<(Option<String>, Option<Bytes>)>,
    list: Option<Bytes>,
    /// tech_name -> the cleaned `name` (`stats.class_name`'s value).
    names: HashMap<String, Value>,
}

fn build(gd: &Gamedata) -> Clean<Index> {
    let raw_abilities = gd.load("class_abilities.json");
    let mut fallback = HashMap::new();
    for c in items(raw_abilities.as_deref().unwrap_or(&json!([])))? {
        let m = dict(c)?;
        if let Some(name) = hash_key(key(m, "name")?)? {
            fallback.insert(name, get_or(m, "abilities", json!([])));
        }
    }
    let raw = gd.load("classes.json");
    let mut classes: Vec<(Option<String>, Value)> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for c in items(raw.as_deref().unwrap_or(&json!([])))? {
        let (tech, cleaned) = clean_class(c, &fallback)?;
        // A repeated key keeps its first position and takes the later value.
        match tech.as_ref().and_then(|t| at.get(t)) {
            Some(&i) => classes[i].1 = cleaned,
            None => {
                if let Some(t) = &tech {
                    at.insert(t.clone(), classes.len());
                }
                classes.push((tech, cleaned));
            }
        }
    }
    let names = classes.iter().filter_map(|(t, c)| Some((t.clone()?, c.get("name")?.clone()))).collect();
    let valid: Vec<(Option<String>, Option<Value>)> = classes.into_iter().map(|(t, c)| (t, validate(&c))).collect();
    let list = valid.iter().map(|(_, c)| c.clone()).collect::<Option<Vec<_>>>().map(|items| {
        let count = items.len();
        Bytes::from(pyjson::pydantic(&json!({"items": items, "count": count})))
    });
    let classes = valid.into_iter().map(|(t, c)| (t, c.map(|v| Bytes::from(pyjson::pydantic(&v))))).collect();
    Ok(Index { classes, list, names })
}

fn index(gd: &Gamedata) -> Arc<Clean<Index>> {
    gd.cached("stats.classes", &["classes.json", "class_abilities.json"], || build(gd))
}

/// `ClassList(**all_classes())` rendered.
pub fn list_body(gd: &Gamedata) -> Result<Bytes, ApiError> {
    match index(gd).as_ref() {
        Ok(idx) => idx.list.clone().ok_or_else(ApiError::internal),
        Err(Broken) => Err(ApiError::internal()),
    }
}

/// `TroveClass(**class_by_tech_name(tech_name))` rendered, or the route's 404.
pub fn class_body(gd: &Gamedata, tech_name: &str) -> Result<Bytes, ApiError> {
    let idx = index(gd);
    let Ok(idx) = idx.as_ref() else { return Err(ApiError::internal()) };
    match idx.classes.iter().find(|(t, _)| t.as_deref() == Some(tech_name)) {
        Some((_, class)) => class.clone().ok_or_else(ApiError::internal),
        None => Err(ApiError::not_found(format!("No class with tech_name '{tech_name}'"))),
    }
}

/// The `name` of the class `tech_name` in `_classes_index()` (what
/// `stats.class_name` reads), `Err` where building the index raises.
pub fn class_display_name(gd: &Gamedata, tech_name: &str) -> Result<Option<Value>, ApiError> {
    match index(gd).as_ref() {
        Ok(idx) => Ok(idx.names.get(tech_name).cloned()),
        Err(Broken) => Err(ApiError::internal()),
    }
}
