//! `/v1/btt/workshop` and the `steam_workshop` loop that mirrors Trove's
//! Steam Workshop into `steam_workshop_items` (`app/steam_workshop`).

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    time::Duration,
};

use anyhow::{Context, anyhow};
use axum::{extract::State, response::IntoResponse};
use kiwi_core::btt::py_strip;
use mongodb::{
    Collection,
    bson::{Bson, Document, doc},
};
use serde_json::Value;

use super::{Mongo, Store, blocklist, dt_field, http_client, int_field, obj, py_str, str_field, str_list, truthy};
use crate::{
    auth::{
        Public,
        pyutil::{bson_int, int_of},
        scopes::BTT_READ,
    },
    dt,
    error::ApiResult,
    pyjson::PydanticJson,
    state::AppState,
};

pub const ITEMS: &str = "steam_workshop_items";
const APP_ID: &str = "304050";
const QUERY_URL: &str = "https://api.steampowered.com/IPublishedFileService/QueryFiles/v1/";
const PLAYERS_URL: &str = "https://api.steampowered.com/ISteamUser/GetPlayerSummaries/v2/";
const PAGE: &str = "100";
const MAX_PAGES: usize = 200;

/// `_int`: `int(x or 0)`, 0 where Python raises TypeError/ValueError.
fn steam_int(v: Option<&Value>) -> i64 {
    if !truthy(v) {
        return 0;
    }
    v.and_then(int_of).map_or(0, |n| n.clamp(i64::MIN.into(), i64::MAX.into()) as i64)
}

/// `str(x or "")`.
fn str_or_empty(v: Option<&Value>) -> String {
    match v {
        Some(v) if truthy(Some(v)) => py_str(v),
        _ => String::new(),
    }
}

/// `d.get("result", 1) != 1` is false.
fn result_is_one(v: Option<&Value>) -> bool {
    match v {
        None => true,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64() == Some(1.0),
        _ => false,
    }
}

/// `parse_items`: Steam's QueryFiles details as stored items (live, public
/// ones only), fields in the order Python's `$set` writes them.
pub fn parse_items(details: &[Value]) -> Vec<Document> {
    let mut items = Vec::new();
    for d in details {
        let Some(d) = d.as_object() else { continue };
        if !result_is_one(d.get("result")) || truthy(d.get("banned")) {
            continue;
        }
        if steam_int(d.get("visibility")) != 0 || !truthy(d.get("publishedfileid")) {
            continue;
        }
        let description = match d.get("short_description") {
            v if truthy(v) => v,
            _ => d.get("file_description"),
        };
        let tags: Vec<Bson> = match d.get("tags") {
            Some(Value::Array(tags)) => tags
                .iter()
                .filter_map(|t| t.as_object())
                .filter(|t| truthy(t.get("tag")))
                .map(|t| Bson::String(py_str(&t["tag"])))
                .collect(),
            _ => Vec::new(),
        };
        items.push(doc! {
            "published_file_id": py_str(&d["publishedfileid"]),
            "title": py_strip(&str_or_empty(d.get("title"))),
            "description": py_strip(&str_or_empty(description)),
            "creator": str_or_empty(d.get("creator")),
            "preview_url": str_or_empty(d.get("preview_url")),
            "file_size": bson_int(steam_int(d.get("file_size"))),
            "created": bson_int(steam_int(d.get("time_created"))),
            "updated": bson_int(steam_int(d.get("time_updated"))),
            "subscriptions": bson_int(steam_int(d.get("subscriptions"))),
            "favorites": bson_int(steam_int(d.get("favorited"))),
            "tags": tags,
        });
    }
    items
}

/// `BttWorkshopItem` of a stored `SteamWorkshopItem` (model defaults for
/// missing fields).
fn item(d: &Document) -> ApiResult<Value> {
    Ok(obj(vec![
        ("published_file_id", str_field(d, "published_file_id", None)?.into()),
        ("title", str_field(d, "title", Some(""))?.into()),
        ("description", str_field(d, "description", Some(""))?.into()),
        ("creator", str_field(d, "creator", Some(""))?.into()),
        ("creator_name", str_field(d, "creator_name", Some(""))?.into()),
        ("preview_url", str_field(d, "preview_url", Some(""))?.into()),
        ("file_size", int_field(d, "file_size", Some(0))?.into()),
        ("created", int_field(d, "created", Some(0))?.into()),
        ("updated", int_field(d, "updated", Some(0))?.into()),
        ("subscriptions", int_field(d, "subscriptions", Some(0))?.into()),
        ("favorites", int_field(d, "favorites", Some(0))?.into()),
        ("tags", Value::Array(str_list(d, "tags")?)),
    ]))
}

/// `catalog()`: rows sorted by subscriptions (desc), blocklisted items and
/// authors left out; `fetched_at` is the newest `seen_at`, isoformat.
pub fn catalog_body(rows: &[Document], served: &blocklist::Served) -> ApiResult<Value> {
    let blocked_items: HashSet<&str> = served.workshop.iter().map(String::as_str).collect();
    let blocked_authors: HashSet<&str> = served.workshop_author.iter().map(String::as_str).collect();
    let mut items = Vec::new();
    let mut newest = None;
    for r in rows {
        let v = item(r)?;
        let seen = dt_field(r, "seen_at", true)?;
        newest = Some(newest.map_or(seen, |n: chrono::DateTime<chrono::Utc>| n.max(seen)));
        let blocked = v["published_file_id"].as_str().is_some_and(|id| blocked_items.contains(id))
            || v["creator"].as_str().is_some_and(|c| blocked_authors.contains(c));
        if !blocked {
            items.push(v);
        }
    }
    Ok(obj(vec![
        ("items", Value::Array(items)),
        ("fetched_at", newest.map_or(Value::Null, |n| dt::isoformat(&n).into())),
    ]))
}

pub(crate) async fn catalog_core<S: Store>(s: &S) -> ApiResult<Value> {
    let served = blocklist::served(s).await?;
    catalog_body(&s.workshop_items().await?, &served)
}

pub async fn catalog(State(state): State<AppState>, Public(ctx): Public<{ BTT_READ }>) -> ApiResult<impl IntoResponse> {
    Ok((ctx.rate_limit, PydanticJson(catalog_core(&Mongo(&state)).await?)))
}

// --- the loop -------------------------------------------------------------------

async fn get_json(client: &reqwest::Client, url: &str, params: &[(&str, &str)]) -> anyhow::Result<Value> {
    let qs = form_urlencoded::Serializer::new(String::new()).extend_pairs(params).finish();
    let resp = client.get(format!("{url}?{qs}")).header("User-Agent", "KiwiAPI/1.0").send().await?;
    let status = resp.status();
    if !status.is_success() {
        return Err(anyhow!("Steam answered {status}"));
    }
    Ok(serde_json::from_slice(&resp.bytes().await?)?)
}

/// `.get("response") or {}` of a Steam answer.
fn response(body: &Value) -> anyhow::Result<serde_json::Map<String, Value>> {
    let resp = body.as_object().context("Steam answer is not an object")?.get("response");
    match resp {
        Some(Value::Object(m)) => Ok(m.clone()),
        r if !truthy(r) => Ok(serde_json::Map::new()),
        _ => Err(anyhow!("Steam response is not an object")),
    }
}

async fn query(client: &reqwest::Client, key: &str) -> anyhow::Result<Vec<Value>> {
    let mut details = Vec::new();
    let mut cursor = "*".to_owned();
    for _ in 0..MAX_PAGES {
        let body = get_json(
            client,
            QUERY_URL,
            &[
                ("key", key),
                ("appid", APP_ID),
                ("query_type", "1"),
                ("cursor", &cursor),
                ("numperpage", PAGE),
                ("return_metadata", "true"),
                ("return_tags", "true"),
                ("return_short_description", "true"),
            ],
        )
        .await?;
        let body = response(&body)?;
        let page = match body.get("publishedfiledetails") {
            Some(Value::Array(p)) => p.clone(),
            _ => Vec::new(),
        };
        let empty = page.is_empty();
        details.extend(page);
        let next = body.get("next_cursor").filter(|v| truthy(Some(v))).map(py_str);
        match next {
            Some(n) if !empty && n != cursor => cursor = n,
            _ => break,
        }
    }
    Ok(details)
}

async fn names(client: &reqwest::Client, key: &str, ids: &[String]) -> anyhow::Result<HashMap<String, String>> {
    let mut out = HashMap::new();
    for batch in ids.chunks(100) {
        let body = get_json(client, PLAYERS_URL, &[("key", key), ("steamids", &batch.join(","))]).await?;
        if let Some(Value::Array(players)) = response(&body)?.get("players") {
            for p in players {
                let p = p.as_object().context("player is not an object")?;
                let id = p.get("steamid").map_or_else(|| "None".to_owned(), py_str);
                out.insert(id, str_or_empty(p.get("personaname")));
            }
        }
    }
    Ok(out)
}

/// `refresh()`: mirror the whole catalog; prune items Steam stopped listing
/// only after a sweep that saw at least half of what is stored.
async fn refresh(state: &AppState) -> anyhow::Result<String> {
    let key = state.settings().steam_web_api_key.clone();
    if key.is_empty() {
        return Ok("skipped, no STEAM_WEB_API_KEY".into());
    }
    let client = http_client(Duration::from_secs(30))?;
    let items = parse_items(&query(&client, &key).await?);
    let creators: BTreeSet<String> =
        items.iter().filter_map(|i| i.get_str("creator").ok()).filter(|c| !c.is_empty()).map(str::to_owned).collect();
    let names = names(&client, &key, &creators.into_iter().collect::<Vec<_>>()).await?;
    let coll = state.inner.db.mongo.as_ref().context("MONGO_URI not configured")?.collection::<Document>(ITEMS);
    store_items(&coll, items, &names).await
}

pub async fn store_items(
    coll: &Collection<Document>,
    mut items: Vec<Document>,
    names: &HashMap<String, String>,
) -> anyhow::Result<String> {
    let now = dt::to_bson(dt::now());
    for item in &mut items {
        let creator = item.get_str("creator").unwrap_or_default();
        let name = names.get(creator).cloned().unwrap_or_default();
        item.insert("creator_name", name);
        item.insert("seen_at", now);
        let id = item.get("published_file_id").cloned().unwrap_or(Bson::Null);
        coll.update_one(doc! { "published_file_id": id }, doc! { "$set": item.clone() }).upsert(true).await?;
    }
    let stored = coll.count_documents(doc! {}).await?;
    if !items.is_empty() && items.len() as u64 * 2 >= stored {
        coll.delete_many(doc! { "seen_at": { "$lt": now } }).await?;
    }
    Ok(format!("{} item(s)", items.len()))
}

pub async fn refresh_cycle(state: AppState) -> anyhow::Result<()> {
    let result = refresh(&state).await?;
    tracing::info!("Steam Workshop catalog refresh: {result}");
    Ok(())
}
