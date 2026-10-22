//! BetterTroveTools: the releases relay and changelog (`app/trove/btt_releases.py`
//! and the `/v1/btt` routes in `app/trove/router.py`), the Steam Workshop
//! mirror (`app/steam_workshop`) and the mod blocklist (`app/btt_blocklist`).

mod blocklist;
mod github;
mod releases;
mod workshop;

#[cfg(test)]
mod tests;

use std::time::Duration;

use axum::{
    Router,
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
    routing::{delete, get},
};
use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use mongodb::{
    bson::{Bson, Document, doc, oid::ObjectId},
    error::{ErrorKind, WriteFailure},
};
use serde_json::{Map, Value};

use crate::{
    auth::pyutil::bson_int,
    dt,
    error::{ApiError, ApiResult},
    jobs,
    middleware::errors::MethodNotAllowed,
    pyfloat,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/btt/releases", get(releases::list))
        .route("/v1/btt/latest", get(releases::latest))
        .route("/v1/btt/check", get(releases::check))
        .route("/v1/btt/latest/{platform}", get(releases::latest_for_platform))
        .route("/v1/btt/changelog", get(releases::changelog))
        .route("/v1/btt/workshop", get(workshop::catalog))
        .route("/v1/btt/mod-blocklist", get(blocklist::public))
        .route(
            "/admin/btt/mod-blocklist",
            get(blocklist::admin_list).post(blocklist::admin_add).fallback(|| async { first_route_405("GET") }),
        )
        .route("/admin/btt/mod-blocklist/{entry_id}", delete(blocklist::admin_remove))
}

/// The loops: GitHub releases + changelog and the Steam Workshop mirror.
pub fn start_jobs(state: &AppState) {
    jobs::spawn_periodic(state, "btt_releases", github::refresh_cycle, |s: AppState| async move {
        Duration::from_secs(s.settings().btt_releases_refresh_seconds.max(0) as u64)
    });
    jobs::spawn_periodic(state, "steam_workshop", workshop::refresh_cycle, |s: AppState| async move {
        Duration::from_secs(s.settings().steam_workshop_refresh_seconds.max(0) as u64)
    });
}

/// Starlette's 405 when two routes share a path: `Allow` names only the
/// first route's methods.
fn first_route_405(allow: &'static str) -> Response {
    let mut res = ApiError::http(axum::http::StatusCode::METHOD_NOT_ALLOWED, None).into_response();
    res.headers_mut().insert(header::ALLOW, HeaderValue::from_static(allow));
    res.extensions_mut().insert(MethodNotAllowed);
    res
}

fn internal(err: impl std::fmt::Display) -> ApiError {
    tracing::error!("btt: {err}");
    ApiError::internal()
}

/// What the routes read and write: Mongo in production, fixtures in tests.
pub(crate) trait Store: Sync {
    /// `btt_releases` newest first (optionally one channel), a page and the total.
    async fn release_page(&self, prerelease: Option<bool>, offset: u64, limit: i64) -> ApiResult<(Vec<Document>, u64)>;
    /// One channel's releases, newest first.
    async fn channel_releases(&self, prerelease: bool) -> ApiResult<Vec<Document>>;
    async fn changelog(&self, repo: &str) -> ApiResult<Option<Document>>;
    /// `steam_workshop_items`, most subscribed first.
    async fn workshop_items(&self) -> ApiResult<Vec<Document>>;
    /// `btt_mod_blocklist` by `created_at`.
    async fn blocklist(&self, newest_first: bool) -> ApiResult<Vec<Document>>;
    /// The distinct non-empty takedown md5s, unsorted.
    async fn takedown_md5s(&self) -> ApiResult<Vec<Bson>>;
    /// `false` on a duplicate key.
    async fn insert_block(&self, entry: Document) -> ApiResult<bool>;
    /// `false` when nothing had that id.
    async fn delete_block(&self, id: ObjectId) -> ApiResult<bool>;
}

pub(crate) struct Mongo<'a>(pub &'a AppState);

impl Mongo<'_> {
    fn coll(&self, name: &str) -> mongodb::Collection<Document> {
        self.0.mongo().collection::<Document>(name)
    }

    async fn all(&self, name: &str, filter: Document, sort: Document) -> ApiResult<Vec<Document>> {
        self.coll(name).find(filter).sort(sort).await.map_err(internal)?.try_collect().await.map_err(internal)
    }
}

impl Store for Mongo<'_> {
    async fn release_page(&self, prerelease: Option<bool>, offset: u64, limit: i64) -> ApiResult<(Vec<Document>, u64)> {
        let filter = prerelease.map_or_else(Document::new, |p| doc! { "prerelease": p });
        let coll = self.coll(releases::RELEASES);
        let total = coll.count_documents(filter.clone()).await.map_err(internal)?;
        let docs = coll
            .find(filter)
            .sort(doc! { "published_at": -1 })
            .skip(offset)
            .limit(limit)
            .await
            .map_err(internal)?
            .try_collect()
            .await
            .map_err(internal)?;
        Ok((docs, total))
    }

    async fn channel_releases(&self, prerelease: bool) -> ApiResult<Vec<Document>> {
        self.all(releases::RELEASES, doc! { "prerelease": prerelease }, doc! { "published_at": -1 }).await
    }

    async fn changelog(&self, repo: &str) -> ApiResult<Option<Document>> {
        self.coll(releases::CHANGELOGS).find_one(doc! { "repo": repo }).await.map_err(internal)
    }

    async fn workshop_items(&self) -> ApiResult<Vec<Document>> {
        self.all(workshop::ITEMS, doc! {}, doc! { "subscriptions": -1 }).await
    }

    async fn blocklist(&self, newest_first: bool) -> ApiResult<Vec<Document>> {
        self.all(blocklist::BLOCKLIST, doc! {}, doc! { "created_at": if newest_first { -1 } else { 1 } }).await
    }

    async fn takedown_md5s(&self) -> ApiResult<Vec<Bson>> {
        self.coll(blocklist::TAKEDOWNS)
            .distinct("md5", doc! { "md5": { "$nin": [Bson::Null, ""] } })
            .await
            .map_err(internal)
    }

    async fn insert_block(&self, entry: Document) -> ApiResult<bool> {
        match self.coll(blocklist::BLOCKLIST).insert_one(entry).await {
            Ok(_) => Ok(true),
            Err(e) if matches!(&*e.kind, ErrorKind::Write(WriteFailure::WriteError(w)) if w.code == 11000) => Ok(false),
            Err(e) => Err(internal(e)),
        }
    }

    async fn delete_block(&self, id: ObjectId) -> ApiResult<bool> {
        let res = self.coll(blocklist::BLOCKLIST).delete_one(doc! { "_id": id }).await.map_err(internal)?;
        Ok(res.deleted_count > 0)
    }
}

/// httpx's `timeout=` bounds each connect and read, not the whole request.
fn http_client(timeout: Duration) -> anyhow::Result<reqwest::Client> {
    Ok(crate::http::client_builder().connect_timeout(timeout).read_timeout(timeout).build()?)
}

// --- Python values ----------------------------------------------------------

/// Python truthiness of a decoded JSON value.
fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(crate::features::py_truthy)
}

/// `str(x)` of a decoded JSON value.
fn py_str(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => match n.as_i64().map(|i| i.to_string()).or_else(|| n.as_u64().map(|u| u.to_string())) {
            Some(s) => s,
            None => pyfloat::repr(n.as_f64().unwrap_or(f64::NAN)),
        },
        Value::String(s) => s.clone(),
        other => crate::pyjson::dumps(other),
    }
}

/// A decoded JSON value as pymongo stores the Python object.
fn json_to_bson(v: &Value) -> Bson {
    match v {
        Value::Null => Bson::Null,
        Value::Bool(b) => Bson::Boolean(*b),
        Value::Number(n) => match n.as_i64() {
            Some(i) => bson_int(i),
            None => Bson::Double(n.as_f64().unwrap_or(f64::NAN)),
        },
        Value::String(s) => Bson::String(s.clone()),
        Value::Array(items) => Bson::Array(items.iter().map(json_to_bson).collect()),
        Value::Object(map) => Bson::Document(map.iter().map(|(k, v)| (k.clone(), json_to_bson(v))).collect()),
    }
}

// --- Stored documents, read the way pydantic validates them -----------------

/// pydantic lax `int` of a stored value.
fn bson_to_int(b: &Bson) -> Option<i64> {
    match b {
        Bson::Int32(i) => Some((*i).into()),
        Bson::Int64(i) => Some(*i),
        Bson::Double(f) if f.fract() == 0.0 && f.is_finite() => Some(*f as i64),
        Bson::Boolean(b) => Some((*b).into()),
        _ => None,
    }
}

/// A `str` field: missing takes `default`, anything but a string fails.
fn str_field(doc: &Document, key: &str, default: Option<&str>) -> Result<String, ApiError> {
    match doc.get(key) {
        Some(Bson::String(s)) => Ok(s.clone()),
        None => default.map(str::to_owned).ok_or_else(|| internal(format!("missing {key}"))),
        Some(_) => Err(internal(format!("bad {key}"))),
    }
}

fn int_field(doc: &Document, key: &str, default: Option<i64>) -> Result<i64, ApiError> {
    match doc.get(key) {
        Some(b) => bson_to_int(b).ok_or_else(|| internal(format!("bad {key}"))),
        None => default.ok_or_else(|| internal(format!("missing {key}"))),
    }
}

fn bool_field(doc: &Document, key: &str, default: bool) -> Result<bool, ApiError> {
    match doc.get(key) {
        Some(Bson::Boolean(b)) => Ok(*b),
        Some(b) => match bson_to_int(b) {
            Some(0) => Ok(false),
            Some(1) => Ok(true),
            _ => Err(internal(format!("bad {key}"))),
        },
        None => Ok(default),
    }
}

/// A `datetime` field; `default_factory=utcnow` when missing and `now` is set.
fn dt_field(doc: &Document, key: &str, now: bool) -> Result<DateTime<Utc>, ApiError> {
    match doc.get(key) {
        Some(Bson::DateTime(d)) => Ok(dt::from_bson(*d)),
        None if now => Ok(dt::now()),
        _ => Err(internal(format!("bad {key}"))),
    }
}

/// `str | None` with a `None` default.
fn opt_str_field(doc: &Document, key: &str) -> Result<Value, ApiError> {
    match doc.get(key) {
        None | Some(Bson::Null) => Ok(Value::Null),
        Some(Bson::String(s)) => Ok(Value::String(s.clone())),
        Some(_) => Err(internal(format!("bad {key}"))),
    }
}

fn str_list(doc: &Document, key: &str) -> Result<Vec<Value>, ApiError> {
    match doc.get(key) {
        None => Ok(Vec::new()),
        Some(Bson::Array(items)) => items
            .iter()
            .map(|b| match b {
                Bson::String(s) => Ok(Value::String(s.clone())),
                _ => Err(internal(format!("bad {key} item"))),
            })
            .collect(),
        Some(_) => Err(internal(format!("bad {key}"))),
    }
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect::<Map<String, Value>>())
}
