//! Rename and duplicate-name detection for the leaderboards
//! (`app/trove/leaderboards/{renames,duplicates}.py`): their `/v1/leaderboards`
//! and `/site/leaderboards` routes, the backfill jobs behind them (same Redis
//! lock and status keys as Python) and the live passes the warmer calls:
//! [`renames::detect_latest`] and [`duplicates::detect_latest`] (see [`live`]).

pub mod duplicates;
pub mod renames;
pub mod store;

#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod tests;

use std::sync::Mutex;

use axum::{
    Router,
    extract::{FromRequestParts, State},
    http::{HeaderValue, StatusCode, Uri, header, request::Parts},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Map, Value, json};

use crate::{
    auth::{MasterIngest, Public, scopes::LEADERBOARDS_READ},
    error::ApiError,
    features::{self, LeaderboardDuplicates, LeaderboardRenames, Leaderboards, RequireFeature},
    middleware::pyurl::scope_path,
    pyjson,
    state::AppState,
    validation::{FromParams, Params, Query},
};

use store::{Pg, Store};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/leaderboards/renames", get(v1_renames))
        .route("/v1/leaderboards/renames/{name}", get(v1_rename_history))
        .route("/v1/leaderboards/renames-backfill", post(renames_backfill))
        .route("/v1/leaderboards/renames-backfill-status", get(renames_backfill_status))
        .route("/v1/leaderboards/duplicates", get(v1_duplicates))
        .route("/v1/leaderboards/duplicates/{name}", get(v1_duplicate_lookup))
        .route("/v1/leaderboards/duplicates-backfill", post(duplicates_backfill))
        .route("/v1/leaderboards/duplicates-backfill-status", get(duplicates_backfill_status))
        .route("/site/leaderboards/renames", get(site_renames))
        .route("/site/leaderboards/renames/{name}", get(site_rename_history))
        .route("/site/leaderboards/duplicates", get(site_duplicates))
        .route("/site/leaderboards/duplicates/{name}", get(site_duplicate_lookup))
}

/// The warmer's entry points, on the production stores and the process-wide
/// scan guards.
pub mod live {
    use super::*;

    /// `renames.detect_latest(force=...)`.
    pub async fn renames(state: &AppState, force: bool) -> anyhow::Result<Value> {
        super::renames::detect_latest(&Pg(state), &super::renames::KEYS, &super::renames::LAST_PAIR, wall_clock, force)
            .await
    }

    /// `duplicates.detect_latest(force=...)`.
    pub async fn duplicates(state: &AppState, force: bool) -> anyhow::Result<Value> {
        super::duplicates::detect_latest(&Pg(state), &super::duplicates::LAST_ANCHOR, wall_clock, force).await
    }
}

// ---------------------------------------------------------------------------
// Shared helpers

/// `time.time()`; injected so tests can pin it.
pub type Clock = fn() -> f64;

pub fn wall_clock() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// `int(t)`.
pub fn int_secs(t: f64) -> i64 {
    t.trunc() as i64
}

/// `name.strip().lower()`.
pub fn lower_key(name: &str) -> String {
    kiwi_core::pystr::strip(name).to_lowercase()
}

/// The end of Python's `xs[:n]`.
pub fn py_slice_end(len: usize, n: i64) -> usize {
    if n >= 0 {
        usize::try_from(n).map_or(len, |n| n.min(len))
    } else {
        len.saturating_sub(usize::try_from(n.unsigned_abs()).unwrap_or(usize::MAX))
    }
}

/// A backfill status dict: keys keep their first-set position, as in Python.
#[derive(Clone, Debug, Default)]
pub struct StatusDoc(pub Map<String, Value>);

impl StatusDoc {
    pub fn set(&mut self, key: &str, value: impl Into<Value>) {
        self.0.insert(key.to_owned(), value.into());
    }

    pub fn int(&self, key: &str) -> i64 {
        self.0.get(key).and_then(Value::as_i64).unwrap_or(0)
    }

    pub fn bump(&mut self, key: &str, by: i64) {
        let n = self.int(key) + by;
        self.set(key, n);
    }

    /// `json.dumps(status)`, what goes to Redis.
    pub fn dumps(&self) -> String {
        pyjson::dumps_spaced(&Value::Object(self.0.clone()))
    }
}

/// `get_status`: Redis first, else this process's last status.
pub async fn load_status<S: Store>(s: &S, key: &str, local: &Mutex<Map<String, Value>>) -> Map<String, Value> {
    if let Some(raw) = s.load_status(key).await.filter(|r| !r.is_empty())
        && let Ok(Value::Object(m)) = pyjson::loads(raw.as_bytes())
    {
        return m;
    }
    local.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn internal(err: anyhow::Error) -> ApiError {
    tracing::error!("lb_names: {err:#}");
    ApiError::internal()
}

/// The `/site` router's feature gate for these paths: `HTTPException(404)`
/// while the leaderboards are off.
pub struct SiteLeaderboards;

impl FromRequestParts<AppState> for SiteLeaderboards {
    type Rejection = ApiError;

    async fn from_request_parts(_parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        match features::is_enabled(state, features::LEADERBOARDS_FLAG).await {
            Ok(true) => Ok(SiteLeaderboards),
            Ok(false) => Err(ApiError::http(StatusCode::NOT_FOUND, None)),
            Err(e) => Err(internal(e)),
        }
    }
}

/// `limit: int = Query(50, ge=1, le=200)`, `offset: int = Query(0, ge=0)`.
pub struct Page {
    pub limit: i64,
    pub offset: i64,
}

impl FromParams for Page {
    fn from_params(p: &mut Params<'_>) -> Self {
        Page {
            limit: p.query("limit").int().ge(1).le(200).default(50),
            offset: p.query("offset").int().ge(0).default(0),
        }
    }
}

/// [`Page`] plus `kind: str | None = Query(None, pattern="^(same_name|case)$")`.
pub struct DupPage {
    pub page: Page,
    pub kind: Option<String>,
}

impl FromParams for DupPage {
    fn from_params(p: &mut Params<'_>) -> Self {
        let page = Page::from_params(p);
        DupPage { page, kind: p.query("kind").str().one_of_pattern(&["same_name", "case"]).optional() }
    }
}

/// `clear_first: bool = Query(False)`.
pub struct ClearFirst(pub bool);

impl FromParams for ClearFirst {
    fn from_params(p: &mut Params<'_>) -> Self {
        ClearFirst(p.query("clear_first").bool().default(false))
    }
}

/// The `{name}` segment as Starlette sees it (cut from the decoded path).
pub fn tail(uri: &Uri, prefix: &str) -> String {
    scope_path(uri).strip_prefix(prefix).unwrap_or_default().to_owned()
}

fn model(v: Option<Value>) -> Result<Value, ApiError> {
    v.ok_or_else(|| internal(anyhow::anyhow!("response failed model validation")))
}

// ---------------------------------------------------------------------------
// Route cores (store-generic so the tests drive them with fixtures)

/// A 200 body and its `Cache-Control`.
pub struct Reply {
    pub body: String,
    pub cache: String,
}

async fn reply<S: Store>(s: &S, body: String, ttl_key: &str) -> Result<Reply, ApiError> {
    let ttl = store::int_setting(s, ttl_key).await.map_err(internal)?;
    Ok(Reply { body, cache: format!("public, max-age={ttl}") })
}

/// `/renames`: `RenamesResponse` on `/v1`, the raw dict on `/site`.
pub async fn renames_list<S: Store>(s: &S, q: &Page, v1: bool) -> Result<Reply, ApiError> {
    let payload = renames::serve_list(s, q.limit, q.offset).await.map_err(internal)?;
    let body = if v1 { pyjson::pydantic(&model(renames::list_model(&payload))?) } else { pyjson::dumps(&payload) };
    reply(s, body, "renames_cache_ttl_seconds").await
}

/// `/renames/{name}`.
pub async fn rename_history<S: Store>(s: &S, name: &str, v1: bool) -> Result<Reply, ApiError> {
    let payload = renames::history(s, name, renames::MAX_NODES).await.map_err(internal)?;
    let body = if v1 { pyjson::pydantic(&model(renames::history_model(&payload))?) } else { pyjson::dumps(&payload) };
    reply(s, body, "renames_cache_ttl_seconds").await
}

/// `/duplicates`.
pub async fn duplicates_list<S: Store>(s: &S, q: &DupPage, v1: bool) -> Result<Reply, ApiError> {
    let payload = duplicates::serve_list(s, q.page.limit, q.page.offset, q.kind.as_deref()).await.map_err(internal)?;
    let body = if v1 { pyjson::pydantic(&model(duplicates::list_model(&payload))?) } else { pyjson::dumps(&payload) };
    reply(s, body, "duplicates_cache_ttl_seconds").await
}

/// `/duplicates/{name}`.
pub async fn duplicate_lookup<S: Store>(s: &S, name: &str, v1: bool) -> Result<Reply, ApiError> {
    let payload = duplicates::for_name(s, name).await.map_err(internal)?;
    let body = if v1 { pyjson::pydantic(&model(duplicates::lookup_model(&payload))?) } else { pyjson::dumps(&payload) };
    reply(s, body, "duplicates_cache_ttl_seconds").await
}

/// `/renames-backfill-status`.
pub async fn renames_status<S: Store>(s: &S) -> Result<String, ApiError> {
    let mut status = renames::get_status(s).await;
    status.insert("recorded_renames".into(), s.count_renames().await.map_err(internal)?.into());
    Ok(pyjson::pydantic(&Value::Object(status)))
}

/// `/duplicates-backfill-status`.
pub async fn duplicates_status<S: Store>(s: &S) -> Result<String, ApiError> {
    let mut status = duplicates::get_status(s).await;
    status.insert("recorded_duplicates".into(), s.count_duplicates().await.map_err(internal)?.into());
    Ok(pyjson::pydantic(&Value::Object(status)))
}

/// The backfill routes' 202 body.
pub fn started(clear_first: bool) -> String {
    pyjson::pydantic(&json!({"started": true, "clear_first": clear_first}))
}

fn ok(r: Reply) -> Result<Response, ApiError> {
    let cache = HeaderValue::from_str(&r.cache).map_err(|e| internal(e.into()))?;
    Ok(([(header::CACHE_CONTROL, cache)], pyjson::json_response(r.body)).into_response())
}

// ---------------------------------------------------------------------------
// Handlers

async fn v1_renames(
    _lb: RequireFeature<Leaderboards>,
    _on: RequireFeature<LeaderboardRenames>,
    State(state): State<AppState>,
    Public(ctx): Public<{ LEADERBOARDS_READ }>,
    Query(q): Query<Page>,
) -> Result<Response, ApiError> {
    Ok((ctx.rate_limit, ok(renames_list(&Pg(&state), &q, true).await?)).into_response())
}

async fn v1_rename_history(
    _lb: RequireFeature<Leaderboards>,
    _on: RequireFeature<LeaderboardRenames>,
    State(state): State<AppState>,
    Public(ctx): Public<{ LEADERBOARDS_READ }>,
    uri: Uri,
) -> Result<Response, ApiError> {
    let name = tail(&uri, "/v1/leaderboards/renames/");
    Ok((ctx.rate_limit, ok(rename_history(&Pg(&state), &name, true).await?)).into_response())
}

async fn v1_duplicates(
    _lb: RequireFeature<Leaderboards>,
    _on: RequireFeature<LeaderboardDuplicates>,
    State(state): State<AppState>,
    Public(ctx): Public<{ LEADERBOARDS_READ }>,
    Query(q): Query<DupPage>,
) -> Result<Response, ApiError> {
    Ok((ctx.rate_limit, ok(duplicates_list(&Pg(&state), &q, true).await?)).into_response())
}

async fn v1_duplicate_lookup(
    _lb: RequireFeature<Leaderboards>,
    _on: RequireFeature<LeaderboardDuplicates>,
    State(state): State<AppState>,
    Public(ctx): Public<{ LEADERBOARDS_READ }>,
    uri: Uri,
) -> Result<Response, ApiError> {
    let name = tail(&uri, "/v1/leaderboards/duplicates/");
    Ok((ctx.rate_limit, ok(duplicate_lookup(&Pg(&state), &name, true).await?)).into_response())
}

async fn renames_backfill(
    _lb: RequireFeature<Leaderboards>,
    State(state): State<AppState>,
    MasterIngest(auth): MasterIngest,
    Query(ClearFirst(clear_first)): Query<ClearFirst>,
) -> Response {
    tokio::spawn(async move { renames::backfill(&Pg(&state), &renames::KEYS, wall_clock, clear_first).await });
    (StatusCode::ACCEPTED, auth.rate_limit, pyjson::json_response(started(clear_first))).into_response()
}

async fn renames_backfill_status(
    _lb: RequireFeature<Leaderboards>,
    State(state): State<AppState>,
    MasterIngest(auth): MasterIngest,
) -> Result<Response, ApiError> {
    Ok((auth.rate_limit, pyjson::json_response(renames_status(&Pg(&state)).await?)).into_response())
}

async fn duplicates_backfill(
    _lb: RequireFeature<Leaderboards>,
    State(state): State<AppState>,
    MasterIngest(auth): MasterIngest,
    Query(ClearFirst(clear_first)): Query<ClearFirst>,
) -> Response {
    tokio::spawn(async move { duplicates::backfill(&Pg(&state), wall_clock, clear_first).await });
    (StatusCode::ACCEPTED, auth.rate_limit, pyjson::json_response(started(clear_first))).into_response()
}

async fn duplicates_backfill_status(
    _lb: RequireFeature<Leaderboards>,
    State(state): State<AppState>,
    MasterIngest(auth): MasterIngest,
) -> Result<Response, ApiError> {
    Ok((auth.rate_limit, pyjson::json_response(duplicates_status(&Pg(&state)).await?)).into_response())
}

async fn site_renames(
    _gate: SiteLeaderboards,
    State(state): State<AppState>,
    Query(q): Query<Page>,
) -> Result<Response, ApiError> {
    ok(renames_list(&Pg(&state), &q, false).await?)
}

async fn site_rename_history(
    _gate: SiteLeaderboards,
    State(state): State<AppState>,
    uri: Uri,
) -> Result<Response, ApiError> {
    ok(rename_history(&Pg(&state), &tail(&uri, "/site/leaderboards/renames/"), false).await?)
}

async fn site_duplicates(
    _gate: SiteLeaderboards,
    State(state): State<AppState>,
    Query(q): Query<DupPage>,
) -> Result<Response, ApiError> {
    ok(duplicates_list(&Pg(&state), &q, false).await?)
}

async fn site_duplicate_lookup(
    _gate: SiteLeaderboards,
    State(state): State<AppState>,
    uri: Uri,
) -> Result<Response, ApiError> {
    ok(duplicate_lookup(&Pg(&state), &tail(&uri, "/site/leaderboards/duplicates/"), false).await?)
}
