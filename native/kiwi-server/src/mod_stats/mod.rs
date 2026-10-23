//! `/site/mod-stats` and the hourly snapshot loop (`app/mod_stats`, loop
//! `mod_stats`): one creator's Zakros mods across the hub and the outside
//! mod platforms, one snapshot per UTC day.

#[cfg(test)]
mod tests;

use std::{collections::HashMap, time::Duration};

use axum::{
    Router,
    extract::State,
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, TimeDelta, Utc};
use futures::TryStreamExt;
use kiwi_core::{pynum, pystr};
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use serde_json::{Map, Value, json};

use crate::{
    auth::pyutil::{bson_int, eq_int},
    custom_art::{builder::py_str, model::int_of},
    dt,
    error::ApiError,
    jobs, outbound,
    pyjson::{self, PydanticJson},
    state::AppState,
};

pub const COLLECTION: &str = "mod_stats_snapshots";
pub const TROVESAURUS_CATALOG_URL: &str = "https://trovesaurus.com/api/mods-all";
pub const STEAM_DETAILS_URL: &str = "https://api.steampowered.com/ISteamRemoteStorage/GetPublishedFileDetails/v1/";
const HISTORY_DAYS: i64 = 365;
const TITLE_REGEX: &str = "^Zakros\\ ";
const FRESH: TimeDelta = TimeDelta::minutes(50);

pub fn router() -> Router<AppState> {
    Router::new().route("/site/mod-stats", get(mod_stats))
}

pub fn start(state: &AppState) {
    jobs::spawn_periodic(
        state,
        "mod_stats",
        |st: AppState| async move {
            let n = refresh(&st).await?;
            let what = if n > 0 { format!("{n} mod(s)") } else { "taken by another worker".into() };
            tracing::info!("Mod statistics snapshot: {what}");
            Ok(())
        },
        |_| async { Duration::from_secs(3600) },
    );
}

/// A Python number as `sum()` builds it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Num {
    Int(i128),
    Float(f64, f64),
}

fn add(acc: Num, v: &Value) -> Option<Num> {
    let x = match v {
        Value::Bool(b) => Num::Int(i128::from(*b)),
        Value::Number(n) => match (n.as_i64(), n.as_u64()) {
            (Some(i), _) => Num::Int(i128::from(i)),
            (_, Some(u)) => Num::Int(i128::from(u)),
            _ => Num::Float(n.as_f64()?, 0.0),
        },
        _ => return None,
    };
    Some(match (acc, x) {
        (Num::Int(a), Num::Int(b)) => Num::Int(a.saturating_add(b)),
        (Num::Int(a), Num::Float(b, _)) => Num::Float(a as f64 + b, 0.0),
        (Num::Float(a, c), Num::Int(b)) => Num::Float(a + b as f64, c),
        (Num::Float(a, c), Num::Float(b, _)) => neumaier(a, c, b),
    })
}

/// CPython 3.12+ `sum()`: only float items are Neumaier-compensated.
fn neumaier(sum: f64, c: f64, x: f64) -> Num {
    let t = sum + x;
    let c = if sum.abs() >= x.abs() { c + ((sum - t) + x) } else { c + ((x - t) + sum) };
    Num::Float(t, c)
}

fn num_json(n: Num) -> Value {
    match n {
        Num::Int(i) => i64::try_from(i).map_or_else(|_| json!(i as f64), Value::from),
        Num::Float(s, c) => {
            let s = if c != 0.0 && c.is_finite() { s + c } else { s };
            serde_json::Number::from_f64(s).map_or(Value::Null, Value::Number)
        }
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `totals(rows)`; None where Python raises.
pub fn totals(rows: &[Map<String, Value>]) -> Option<Value> {
    let total = |platform: &str, field: &str| -> Option<Value> {
        let mut acc = Num::Int(0);
        for r in rows {
            let p = r.get(platform)?;
            let v = if truthy(p) {
                match p {
                    Value::Object(m) => m.get(field).cloned().unwrap_or(json!(0)),
                    _ => return None,
                }
            } else {
                json!(0)
            };
            acc = add(acc, &v)?;
        }
        Some(num_json(acc))
    };
    Some(json!({
        "hub_top_release": total("hub", "top_release")?,
        "trovesaurus_downloads": total("trovesaurus", "downloads")?,
        "steam_subscriptions": total("steam", "subscriptions")?,
        "steam_lifetime_subscriptions": total("steam", "lifetime_subscriptions")?,
    }))
}

/// A snapshot as Beanie validates it.
pub struct Snapshot {
    pub day: String,
    pub taken_at: DateTime<Utc>,
    pub mods: Vec<Map<String, Value>>,
}

impl Snapshot {
    pub fn from_doc(d: &Document) -> Result<Self, String> {
        let day = d.get_str("day").map_err(|e| format!("day: {e}"))?.to_owned();
        let taken_at = match d.get("taken_at") {
            None => dt::now(),
            Some(Bson::DateTime(t)) => dt::from_bson(*t),
            other => return Err(format!("taken_at: {other:?}")),
        };
        let mods = match d.get("mods") {
            None => Vec::new(),
            Some(Bson::Array(a)) => a
                .iter()
                .map(|m| match m {
                    Bson::Document(m) => Ok(outbound::doc_json(m)),
                    other => Err(format!("mods: {other:?}")),
                })
                .collect::<Result<_, _>>()?,
            other => return Err(format!("mods: {other:?}")),
        };
        Ok(Snapshot { day, taken_at, mods })
    }
}

/// `public_stats()` over the newest-first snapshots.
pub fn public_stats(snaps: &[Snapshot]) -> Option<Value> {
    let Some(latest) = snaps.first() else {
        return Some(json!({"updated_at": null, "mods": [], "totals": totals(&[])?, "history": []}));
    };
    let mut history = Vec::with_capacity(snaps.len());
    for s in snaps.iter().rev() {
        let mut row = Map::new();
        row.insert("day".into(), json!(s.day));
        if let Value::Object(t) = totals(&s.mods)? {
            row.extend(t);
        }
        history.push(Value::Object(row));
    }
    let mods: Vec<Value> = latest.mods.iter().cloned().map(Value::Object).collect();
    Some(json!({
        "updated_at": outbound::iso(&Some(latest.taken_at)),
        "mods": mods,
        "totals": totals(&latest.mods)?,
        "history": history,
    }))
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    tracing::error!("mod stats: {e}");
    ApiError::internal()
}

pub async fn mod_stats(State(state): State<AppState>) -> Result<Response, ApiError> {
    let docs: Vec<Document> = state
        .mongo()
        .collection::<Document>(COLLECTION)
        .find(doc! {})
        .sort(doc! {"day": -1})
        .limit(HISTORY_DAYS)
        .await
        .map_err(internal)?
        .try_collect()
        .await
        .map_err(internal)?;
    let snaps: Vec<Snapshot> = docs.iter().map(Snapshot::from_doc).collect::<Result<_, _>>().map_err(internal)?;
    let body = public_stats(&snaps).ok_or_else(|| internal("a snapshot row Python could not total"))?;
    Ok(([(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=600"))], PydanticJson(body)).into_response())
}

// -- Collecting -------------------------------------------------------------------

/// One hub project's part of a row.
#[derive(Debug, Clone, PartialEq)]
pub struct HubProject {
    pub title: String,
    pub owner_handle: String,
    pub slug: String,
    pub top_release: i64,
    pub downloads_7d: i64,
    pub star_count: i64,
}

/// `_int`.
pub fn py_int(v: Option<&Value>) -> i64 {
    let Some(v) = v.filter(|v| truthy(v)) else { return 0 };
    match v {
        Value::Bool(b) => i64::from(*b),
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_u64().map(|u| i64::try_from(u).unwrap_or(i64::MAX)))
            .or_else(|| n.as_f64().map(|f| f.trunc() as i64))
            .unwrap_or(0),
        Value::String(s) => pynum::int(s).unwrap_or(0),
        _ => 0,
    }
}

fn get_or_none(m: &Value, key: &str) -> Value {
    m.as_object().and_then(|o| o.get(key)).cloned().unwrap_or(Value::Null)
}

/// `build_rows`; None where Python raises.
pub fn build_rows(
    projects: &[HubProject],
    trovesaurus_ids: &Value,
    steam_ids: &Value,
    catalog: &[Value],
    steam_details: &[Value],
) -> Option<Vec<Value>> {
    let mut trovesaurus: HashMap<String, &Value> = HashMap::new();
    for m in catalog.iter().filter(|m| m.is_object()) {
        trovesaurus.insert(py_str(&get_or_none(m, "id")), m);
    }
    let mut steam: HashMap<String, &Value> = HashMap::new();
    for d in steam_details {
        let o = d.as_object()?;
        if eq_int(o.get("result"), 1) {
            steam.insert(py_str(&get_or_none(d, "publishedfileid")), d);
        }
    }
    let (ts_ids, st_ids) = (trovesaurus_ids.as_object()?, steam_ids.as_object()?);
    let mut rows: Vec<(String, Value)> = Vec::with_capacity(projects.len());
    for p in projects {
        let key = |ids: &Map<String, Value>| py_str(ids.get(&p.title).unwrap_or(&Value::Null));
        let ts = trovesaurus.get(&key(ts_ids)).copied();
        let st = steam.get(&key(st_ids)).copied();
        let trovesaurus = match ts {
            None => Value::Null,
            Some(t) if !truthy(t) => t.clone(),
            Some(t) => json!({
                "id": py_int(t.get("id")),
                "downloads": py_int(t.get("totaldownloads")),
                "likes": py_int(t.get("likes")),
            }),
        };
        let steam = match st {
            None => Value::Null,
            Some(s) => json!({
                "id": py_int(s.get("publishedfileid")),
                "subscriptions": py_int(s.get("subscriptions")),
                "lifetime_subscriptions": py_int(s.get("lifetime_subscriptions")),
                "favorites": py_int(s.get("favorited")),
                "views": py_int(s.get("views")),
            }),
        };
        let row = json!({
            "title": p.title,
            "handle": p.owner_handle,
            "slug": p.slug,
            "hub": {"top_release": p.top_release, "downloads_7d": p.downloads_7d, "stars": p.star_count},
            "trovesaurus": trovesaurus,
            "steam": steam,
        });
        rows.push((p.title.to_lowercase(), row));
    }
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    Some(rows.into_iter().map(|(_, r)| r).collect())
}

/// A row as pymongo encodes the dict (ints as int32 when they fit).
pub fn to_bson(v: &Value) -> Bson {
    match v {
        Value::Null => Bson::Null,
        Value::Bool(b) => Bson::Boolean(*b),
        Value::Number(n) => match n.as_i64() {
            Some(i) => bson_int(i),
            None => Bson::Double(n.as_f64().unwrap_or_default()),
        },
        Value::String(s) => Bson::String(s.clone()),
        Value::Array(a) => Bson::Array(a.iter().map(to_bson).collect()),
        Value::Object(o) => Bson::Document(o.iter().map(|(k, v)| (k.clone(), to_bson(v))).collect()),
    }
}

async fn repo_ids(state: &AppState, name: &str) -> anyhow::Result<Value> {
    let out = tokio::process::Command::new("git")
        .args(["--git-dir", &state.settings().custom_art_remote, "show", &format!("master:{name}")])
        .stdin(std::process::Stdio::null())
        .output()
        .await?;
    if !out.status.success() {
        anyhow::bail!("git show {name}: {}", pystr::strip(&String::from_utf8_lossy(&out.stderr)));
    }
    pyjson::loads(&out.stdout).map_err(|e| anyhow::anyhow!("git show {name}: not JSON: {e:?}"))
}

fn client(timeout: u64, redirects: bool) -> anyhow::Result<reqwest::Client> {
    let policy = if redirects { reqwest::redirect::Policy::limited(20) } else { reqwest::redirect::Policy::none() };
    Ok(crate::http::client_builder()
        .timeout(Duration::from_secs(timeout))
        .redirect(policy)
        .user_agent(crate::http::KIWI_UA)
        .build()?)
}

/// httpx's form value text.
fn form_value(v: &Value) -> String {
    match v {
        Value::Bool(true) => "true".into(),
        Value::Bool(false) => "false".into(),
        Value::Null => String::new(),
        other => py_str(other),
    }
}

async fn steam_details(ids: &[Value]) -> anyhow::Result<Vec<Value>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut form: Vec<(String, String)> = vec![("itemcount".into(), ids.len().to_string())];
    form.extend(ids.iter().enumerate().map(|(i, v)| (format!("publishedfileids[{i}]"), form_value(v))));
    let body: String = form_urlencoded::Serializer::new(String::new()).extend_pairs(&form).finish();
    let resp = client(30, false)?
        .post(STEAM_DETAILS_URL)
        .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await?
        .error_for_status()?;
    let doc = pyjson::loads(&resp.bytes().await?).map_err(|e| anyhow::anyhow!("steam reply: {e:?}"))?;
    let details = doc.get("response").ok_or_else(|| anyhow::anyhow!("KeyError: 'response'"))?;
    let details = details.as_object().ok_or_else(|| anyhow::anyhow!("steam response is not an object"))?;
    match details.get("publishedfiledetails") {
        None => Ok(Vec::new()),
        Some(Value::Array(a)) => Ok(a.clone()),
        Some(other) => anyhow::bail!("publishedfiledetails: {other}"),
    }
}

async fn hub_projects(state: &AppState) -> anyhow::Result<Vec<HubProject>> {
    let db = state.mongo();
    let filter = doc! {
        "owner_handle": &state.settings().mod_stats_handle,
        "visibility": "public",
        "taken_down": false,
        "title": {"$regex": TITLE_REGEX},
    };
    let docs: Vec<Document> = db.collection::<Document>("mod_projects").find(filter).await?.try_collect().await?;
    let ids: Vec<ObjectId> = docs.iter().filter_map(|d| d.get_object_id("_id").ok()).collect();
    let pipeline = vec![
        doc! {"$match": {"project_id": {"$in": ids}, "status": "published"}},
        doc! {"$group": {"_id": "$project_id", "top": {"$max": "$download_count"}}},
    ];
    let tops: Vec<Document> =
        db.collection::<Document>("mod_releases").aggregate(pipeline).await?.try_collect().await?;
    let mut top: HashMap<ObjectId, i64> = HashMap::new();
    for r in &tops {
        let id = r.get_object_id("_id")?;
        let n = match r.get("top") {
            Some(Bson::Double(f)) if f.is_finite() => f.trunc() as i64,
            Some(b) => int_of(b).ok_or_else(|| anyhow::anyhow!("int(top): {b:?}"))?,
            None => anyhow::bail!("KeyError: 'top'"),
        };
        top.insert(id, n);
    }
    docs.iter()
        .map(|d| {
            let s = |k: &str| d.get_str(k).map(str::to_owned).map_err(|e| anyhow::anyhow!("{k}: {e}"));
            let int = |k: &str| match d.get(k) {
                None => Ok(0),
                Some(b) => int_of(b).ok_or_else(|| anyhow::anyhow!("{k}: {b:?}")),
            };
            let id = d.get_object_id("_id")?;
            Ok(HubProject {
                title: s("title")?,
                owner_handle: match d.get("owner_handle") {
                    None => String::new(),
                    Some(_) => s("owner_handle")?,
                },
                slug: s("slug")?,
                top_release: top.get(&id).copied().unwrap_or(0),
                downloads_7d: int("downloads_7d")?,
                star_count: int("star_count")?,
            })
        })
        .collect()
}

/// `collect()`.
pub async fn collect(state: &AppState) -> anyhow::Result<Vec<Value>> {
    let projects = hub_projects(state).await?;
    let trovesaurus_ids = repo_ids(state, "trovesaurus_ids.json").await?;
    let steam_ids = repo_ids(state, "steam_ids.json").await?;
    let resp = client(60, true)?.get(TROVESAURUS_CATALOG_URL).send().await?.error_for_status()?;
    let catalog = pyjson::loads(&resp.bytes().await?).map_err(|e| anyhow::anyhow!("catalog: {e:?}"))?;
    let titles: Vec<&str> = projects.iter().map(|p| p.title.as_str()).collect();
    let st = steam_ids.as_object().ok_or_else(|| anyhow::anyhow!("steam_ids.json is not an object"))?;
    let wanted: Vec<Value> = st.iter().filter(|(k, _)| titles.contains(&k.as_str())).map(|(_, v)| v.clone()).collect();
    let details = steam_details(&wanted).await?;
    let catalog = match catalog {
        Value::Array(a) => a,
        _ => Vec::new(),
    };
    build_rows(&projects, &trovesaurus_ids, &steam_ids, &catalog, &details)
        .ok_or_else(|| anyhow::anyhow!("build_rows raised"))
}

/// `refresh()`: rewrite today's snapshot unless it is under 50 minutes old.
pub async fn refresh(state: &AppState) -> anyhow::Result<usize> {
    let c = state.mongo().collection::<Document>(COLLECTION);
    let now = dt::now();
    let day = dt::date_iso(&now.date_naive());
    if c.find_one(doc! {"day": &day, "taken_at": {"$gt": dt::to_bson(now - FRESH)}}).await?.is_some() {
        return Ok(0);
    }
    let rows = collect(state).await?;
    let mods: Vec<Bson> = rows.iter().map(to_bson).collect();
    c.update_one(doc! {"day": &day}, doc! {"$set": {"taken_at": dt::to_bson(dt::now()), "mods": mods}})
        .upsert(true)
        .await?;
    Ok(rows.len())
}
