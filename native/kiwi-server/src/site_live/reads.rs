//! The plain `/site` reads: server time, news and videos, the calendar
//! events, the classes, the sigil, giveaways and the hero screenshots.

use std::{collections::HashMap, path::Path, sync::Arc};

use axum::{
    body::Body,
    extract::State,
    http::{HeaderValue, StatusCode, header},
    response::Response,
};
use mongodb::bson::{Bson, Document, doc};
use serde_json::{Map, Value, json};

use super::{cached, flags::SiteGate, jsonable};
use crate::{
    dt,
    error::ApiError,
    feeds::{routes::dedupe_news, store as feeds},
    gamedata::Gamedata,
    routes::{rotations::bodies, stats as v1_stats},
    state::AppState,
    validation::{FromParams, Params, Query},
};

pub async fn server_time(_: SiteGate) -> Response {
    cached(bodies::server_time(dt::now()), "public, max-age=15")
}

fn iso(d: Option<chrono::DateTime<chrono::Utc>>) -> Value {
    d.map_or(Value::Null, |d| Value::String(dt::isoformat(&d)))
}

pub struct NewsQuery {
    limit: i64,
}

impl FromParams for NewsQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        NewsQuery { limit: p.query("limit").int().ge(1).le(50).default(16) }
    }
}

pub fn news_item(d: &feeds::News) -> Value {
    json!({
        "title": d.title,
        "url": d.url,
        "author": d.author,
        "summary": d.summary,
        "category": d.category,
        "categories": d.categories,
        "image": d.image,
        "published_at": iso(d.published_at),
    })
}

pub async fn news(_: SiteGate, State(state): State<AppState>, Query(q): Query<NewsQuery>) -> Response {
    let docs = feeds::news_page(state.mongo(), None, (q.limit * 3).max(q.limit)).await;
    let items: Vec<Value> = dedupe_news(docs, q.limit as usize).iter().map(news_item).collect();
    cached(json!({"items": items}), "public, max-age=300")
}

pub struct VideosQuery {
    platform: String,
}

impl FromParams for VideosQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        VideosQuery { platform: p.query("platform").str().default("youtube".into()) }
    }
}

/// The `/site/feeds/videos` body from a `feed_cache` document's items.
pub fn videos_body(platform: &str, feed: Option<(Vec<Document>, chrono::DateTime<chrono::Utc>)>) -> Value {
    let (items, fetched_at) = match feed {
        Some((items, at)) => (items.iter().map(|d| jsonable(&Bson::Document(d.clone()))).collect(), iso(Some(at))),
        None => (Vec::new(), Value::Null),
    };
    json!({"platform": platform, "items": items, "fetched_at": fetched_at})
}

pub async fn videos(_: SiteGate, State(state): State<AppState>, Query(q): Query<VideosQuery>) -> Response {
    let platform = if matches!(q.platform.as_str(), "youtube" | "twitch" | "bilibili") { q.platform.as_str() } else { "youtube" };
    let feed = feeds::feed(state.mongo(), platform).await;
    cached(videos_body(platform, feed), "public, max-age=300")
}

/// `_calendar_event(ev, now)`.
pub fn calendar_event(e: &feeds::Event, now: i64) -> Value {
    let (status, until) = if now < e.starts_at {
        ("upcoming", e.starts_at - now)
    } else if now < e.ends_at {
        ("ongoing", e.ends_at - now)
    } else {
        ("ended", 0)
    };
    json!({
        "event_id": e.event_id,
        "name": e.name,
        "url": e.url,
        "category": e.category,
        "image": e.image,
        "icon": e.icon,
        "starts_at": e.starts_at,
        "ends_at": e.ends_at,
        "status": status,
        "seconds_until": until,
    })
}

fn int_bson(n: i64) -> Bson {
    i32::try_from(n).map_or(Bson::Int64(n), Bson::Int32)
}

pub async fn calendar_events(_: SiteGate, State(state): State<AppState>) -> Response {
    let now = chrono::Utc::now().timestamp();
    let db = state.mongo();
    let ongoing = feeds::events(
        db,
        doc! {"starts_at": {"$lte": int_bson(now)}, "ends_at": {"$gt": int_bson(now)}},
        doc! {"ends_at": 1},
        100,
    )
    .await;
    let upcoming = feeds::events(db, doc! {"starts_at": {"$gt": int_bson(now)}}, doc! {"starts_at": 1}, 100).await;
    let body = json!({
        "ongoing": ongoing.iter().map(|e| calendar_event(e, now)).collect::<Vec<_>>(),
        "upcoming": upcoming.iter().map(|e| calendar_event(e, now)).collect::<Vec<_>>(),
        "now": now,
    });
    cached(body, "public, max-age=120")
}

// --- classes: `_classes_index()` as raw dicts ------------------------------------

/// A Python exception while cleaning: the route answers 500.
struct Broken;

type Clean<T> = Result<T, Broken>;

fn dict(v: &Value) -> Clean<&Map<String, Value>> {
    v.as_object().ok_or(Broken)
}

/// `for x in v`, where each `x` is then indexed like a dict.
fn items(v: &Value) -> Clean<&[Value]> {
    match v {
        Value::Array(a) => Ok(a),
        Value::Object(m) if m.is_empty() => Ok(&[]),
        Value::String(s) if s.is_empty() => Ok(&[]),
        _ => Err(Broken),
    }
}

fn get_or(m: &Map<String, Value>, k: &str, default: Value) -> Value {
    m.get(k).cloned().unwrap_or(default)
}

fn key(m: &Map<String, Value>, k: &str) -> Clean<Value> {
    m.get(k).cloned().ok_or(Broken)
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

fn clean_levels(v: &Value) -> Clean<Value> {
    let mut out = Map::new();
    for (lvl, stats) in dict(v)? {
        out.insert(lvl.clone(), clean_stats(stats)?);
    }
    Ok(Value::Object(out))
}

fn clean_stage(st: &Value) -> Clean<Value> {
    let m = dict(st)?;
    let mut out = Map::new();
    out.insert("name".into(), get_or(m, "name", "".into()));
    out.insert("base".into(), get_or(m, "base", 0.into()));
    out.insert("multiplier".into(), get_or(m, "multiplier", 0.into()));
    out.insert("prefab".into(), get_or(m, "prefab", "".into()));
    for (k, v) in m {
        if !matches!(k.as_str(), "name" | "base" | "multiplier" | "prefab") {
            out.insert(k.clone(), v.clone());
        }
    }
    Ok(Value::Object(out))
}

fn clean_ability(a: &Value) -> Clean<Value> {
    let m = dict(a)?;
    let mut out = Map::new();
    out.insert("name".into(), get_or(m, "name", "".into()));
    out.insert("description".into(), get_or(m, "description", "".into()));
    out.insert("icon".into(), get_or(m, "icon", "".into()));
    out.insert("type".into(), get_or(m, "type", "".into()));
    out.insert("prefab".into(), get_or(m, "prefab", "".into()));
    out.insert("active".into(), get_or(m, "active", true.into()));
    let stages = items(&get_or(m, "stages", json!([])))?.iter().map(clean_stage).collect::<Clean<Vec<_>>>()?;
    out.insert("stages".into(), Value::Array(stages));
    for k in ["energy", "cooldown"] {
        if let Some(v) = m.get(k) {
            out.insert(k.into(), v.clone());
        }
    }
    for k in ["healing", "effects", "vfx"] {
        out.insert(k.into(), get_or(m, k, json!([])));
    }
    Ok(Value::Object(out))
}

fn clean_abilities(v: &Value) -> Clean<Vec<Value>> {
    items(v)?.iter().map(clean_ability).collect()
}

fn clean_class(c: &Value, fallback: &HashMap<String, Value>) -> Clean<(Value, Value)> {
    let m = dict(c)?;
    let tech = key(m, "qualified_name")?;
    let name = key(m, "name")?;
    let sc = get_or(m, "subclass", json!({}));
    let scm = dict(&sc)?;
    let subclass = json!({
        "name": get_or(scm, "name", "".into()),
        "description": get_or(scm, "description", "".into()),
        "level": clean_levels(&get_or(scm, "level", json!({})))?,
        "power": get_or(scm, "power", json!({})),
    });
    let mut cleaned = json!({
        "tech_name": tech,
        "name": name,
        "shorts": get_or(m, "shorts", json!([])),
        "damage_type": get_or(m, "damage_type", "".into()),
        "weapons": get_or(m, "weapons", json!([])),
        "attributes": get_or(m, "attributes", json!([])),
        "stats": clean_stats(&get_or(m, "stats", json!([])))?,
        "bonuses": clean_stats(&get_or(m, "bonuses", json!([])))?,
        "levels": clean_levels(&get_or(m, "levels", json!({})))?,
        "subclass": subclass,
        "abilities": clean_abilities(&get_or(m, "abilities", json!([])))?,
    });
    if cleaned["abilities"].as_array().is_some_and(Vec::is_empty) {
        if matches!(name, Value::Array(_) | Value::Object(_)) {
            return Err(Broken);
        }
        let own = name.as_str().and_then(|n| fallback.get(n)).cloned().unwrap_or_else(|| json!([]));
        cleaned["abilities"] = Value::Array(clean_abilities(&own)?);
    }
    Ok((tech, cleaned))
}

/// `jsonable_encoder(all_classes())`, rendered; `None` where Python raises.
pub fn classes_body(gd: &Gamedata) -> Option<Vec<u8>> {
    let raw_abilities = gd.load("class_abilities.json");
    let mut fallback = HashMap::new();
    for c in items(raw_abilities.as_deref().unwrap_or(&json!([]))).ok()? {
        let m = dict(c).ok()?;
        match key(m, "name").ok()? {
            Value::String(n) => {
                fallback.insert(n, get_or(m, "abilities", json!([])));
            }
            Value::Array(_) | Value::Object(_) => return None,
            _ => {}
        }
    }
    let raw = gd.load("classes.json");
    let mut out: Vec<(Value, Value)> = Vec::new();
    for c in items(raw.as_deref().unwrap_or(&json!([]))).ok()? {
        let (tech, cleaned) = clean_class(c, &fallback).ok()?;
        if matches!(tech, Value::Array(_) | Value::Object(_)) {
            return None;
        }
        match out.iter_mut().find(|(t, _)| *t == tech) {
            Some(slot) => slot.1 = cleaned,
            None => out.push((tech, cleaned)),
        }
    }
    let items: Vec<Value> = out.into_iter().map(|(_, c)| c).collect();
    let count = items.len();
    Some(crate::pyjson::dumps(&json!({"items": items, "count": count})).into_bytes())
}

pub async fn classes(_: SiteGate, State(state): State<AppState>) -> Result<Response, ApiError> {
    let gd = Gamedata::global(state.settings());
    let body: Arc<Option<Vec<u8>>> =
        gd.cached("site.classes", &["classes.json", "class_abilities.json"], || classes_body(gd));
    let body = body.as_ref().clone().ok_or_else(ApiError::internal)?;
    let mut res = crate::pyjson::json_response(body);
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=3600"));
    Ok(res)
}

pub async fn sigil(
    _: SiteGate,
    State(state): State<AppState>,
    Query(q): Query<v1_stats::SigilQuery>,
) -> Result<Response, ApiError> {
    let png = match v1_stats::sigil_png(Gamedata::global(state.settings()), q.power_rank, q.mastery) {
        Ok(png) => png,
        Err(e) if e.status == StatusCode::SERVICE_UNAVAILABLE => {
            return Err(ApiError::http(StatusCode::NOT_FOUND, Some("sigil art not available")));
        }
        Err(e) => return Err(e),
    };
    let mut res = Response::new(Body::from(png));
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=86400"));
    Ok(res)
}

pub async fn giveaways(_: SiteGate, State(state): State<AppState>) -> Result<Response, ApiError> {
    let items = crate::giveaways::service::list_public(&crate::giveaways::mongo(&state)).await?;
    Ok(cached(json!({"items": items}), "public, max-age=15"))
}

const SCREENSHOT_EXTS: [&str; 5] = [".webp", ".png", ".jpg", ".jpeg", ".gif"];

/// `Path.suffix`.
fn suffix(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => &name[i..],
        _ => "",
    }
}

/// The hero slideshow list from `<site_root>/static/trove-screens`.
pub fn screenshots_body(site_root: &str) -> Value {
    let folder = Path::new(site_root).join("static").join("trove-screens");
    let mut names: Vec<String> = match std::fs::read_dir(&folder) {
        Ok(rd) => rd
            .filter_map(Result::ok)
            .filter(|e| e.path().is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| SCREENSHOT_EXTS.contains(&suffix(n).to_lowercase().as_str()))
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    let files: Vec<String> = names.iter().map(|n| format!("/static/trove-screens/{n}")).collect();
    let count = files.len();
    json!({"screenshots": files, "count": count})
}

pub async fn screenshots(_: SiteGate, State(state): State<AppState>) -> Response {
    cached(screenshots_body(&state.settings().site_root), "public, max-age=60")
}
