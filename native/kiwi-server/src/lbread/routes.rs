//! The read routes: `/v1/leaderboards*` (`app/trove/router.py`) and their
//! `/site/leaderboards/*` twins (`app/site/router.py`). Each handler runs the
//! dependencies in FastAPI's order, then one backend-generic endpoint the
//! golden tests drive with recorded Postgres answers.

use std::future::Future;

use axum::{
    Router,
    extract::{FromRequestParts, State},
    http::{HeaderName, HeaderValue, StatusCode, Uri, header, request::Parts},
    response::{IntoResponse, Response},
    routing::get,
};
use kiwi_core::lbread::{DAY, Stamp, normalize_timestamp};
use mongodb::bson::{Bson, Document, doc};
use serde_json::{Value, json};

use super::{
    Live,
    cache::{self, Kv, py_int},
    parse_uuid, service,
    store::Source,
};
use crate::{
    auth::{Public, Rejection, RequireScope, scopes},
    error::ApiError,
    features::{self, RequireFeature, py_truthy},
    middleware::pyurl::scope_path,
    pyjson::{self, PyJson},
    ratelimit::{self, RateLimitInfo},
    state::AppState,
    validation::{FromParams, Params, Query},
};

pub type Reply = Result<Response, Rejection>;

/// What the endpoints need besides Postgres and Redis.
pub trait Env: Sync {
    /// `runtime_config.get_setting(key)`.
    fn setting(&self, key: &str) -> impl Future<Output = anyhow::Result<Value>> + Send;
    fn now_us(&self) -> i64;
    /// `check_rate_limit(f"lb_archive:{token_id}", *leaderboards_archive_rate_limit)`.
    fn charge_archive(&self, token_id: &str) -> impl Future<Output = Result<RateLimitInfo, Rejection>> + Send;
}

pub trait Backend: Source + Kv + Env {}

impl<T: Source + Kv + Env> Backend for T {}

pub fn unix_now_us() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_micros() as i64).unwrap_or(0)
}

impl Env for Live<'_> {
    async fn setting(&self, key: &str) -> anyhow::Result<Value> {
        crate::runtime_config::get_setting(self.0, key).await
    }

    fn now_us(&self) -> i64 {
        unix_now_us()
    }

    async fn charge_archive(&self, token_id: &str) -> Result<RateLimitInfo, Rejection> {
        let (max, window) = ratelimit::get_rate_limit(self.0, "leaderboards_archive_rate_limit").await;
        ratelimit::check_rate_limit(self.0, &format!("lb_archive:{token_id}"), max, window).await
    }
}

pub fn internal(e: anyhow::Error) -> Rejection {
    tracing::error!("leaderboards: {e:#}");
    ApiError::internal().into()
}

fn secs(us: i64) -> i64 {
    us.div_euclid(1_000_000)
}

fn pydantic(v: &Value) -> Response {
    pyjson::json_response(pyjson::pydantic(v))
}

fn site_json(v: Value, cache_control: &'static str) -> Response {
    ([(header::CACHE_CONTROL, HeaderValue::from_static(cache_control))], PyJson(v)).into_response()
}

/// Python `len()` of a decoded value.
fn py_len(v: &Value) -> Result<usize, Rejection> {
    match v {
        Value::Array(a) => Ok(a.len()),
        Value::Object(o) => Ok(o.len()),
        Value::String(s) => Ok(s.chars().count()),
        other => Err(internal(anyhow::anyhow!("len() of {other}"))),
    }
}

/// Python `float(x)` for `timedelta(days=x)`.
fn py_number(v: &Value) -> anyhow::Result<f64> {
    match v {
        Value::Bool(b) => Ok(f64::from(u8::from(*b))),
        Value::Number(n) => n.as_f64().ok_or_else(|| anyhow::anyhow!("{n}")),
        other => anyhow::bail!("timedelta(days={other})"),
    }
}

/// `_lb_timestamp`: the normalized anchor of a `/v1` `created_at`.
pub fn lb_timestamp(created_at: i64, now: i64) -> Result<i64, Rejection> {
    if created_at <= 0 {
        return Err(ApiError::bad_request("Missing or invalid 'created_at' (unix seconds at 11:00 UTC).").into());
    }
    match normalize_timestamp(created_at, now, false) {
        Stamp::Anchor(anchor) => Ok(anchor),
        Stamp::Invalid => {
            Err(ApiError::bad_request("created_at must align to a Trove reset (11:00 UTC; 00:00 UTC also accepted).")
                .into())
        }
        Stamp::OutOfRange => Err(internal(anyhow::anyhow!("created_at {created_at} is outside datetime's range"))),
    }
}

/// `archive_query_cutoff`: `int((now - timedelta(days=days)).timestamp())`.
pub fn archive_cutoff(now_us: i64, days: f64) -> i64 {
    secs(now_us - (days * 86_400_000_000.0).round_ties_even() as i64)
}

pub type ArchiveHeaders = [(HeaderName, HeaderValue); 3];

pub fn archive_headers(info: &RateLimitInfo) -> ArchiveHeaders {
    [
        (HeaderName::from_static("x-ratelimit-archive-limit"), HeaderValue::from(info.limit)),
        (HeaderName::from_static("x-ratelimit-archive-remaining"), HeaderValue::from(info.remaining)),
        (HeaderName::from_static("x-ratelimit-archive-reset"), HeaderValue::from(info.reset)),
    ]
}

/// `_enforce_lb_archive_limit`: the tighter bucket for anchors past the threshold.
pub async fn archive_limit<B: Env>(b: &B, token_id: &str, anchor: i64) -> Result<Option<ArchiveHeaders>, Rejection> {
    let days = b.setting("leaderboards_archive_query_threshold_days").await.map_err(internal)?;
    let days = py_number(&days).map_err(internal)?;
    if anchor >= archive_cutoff(b.now_us(), days) {
        return Ok(None);
    }
    Ok(Some(archive_headers(&b.charge_archive(token_id).await?)))
}

// --- query parameters ---------------------------------------------------------

pub struct LimitQ {
    pub limit: i64,
}

impl FromParams for LimitQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        LimitQ { limit: p.query("limit").int().ge(1).le(365).default(60) }
    }
}

pub struct CreatedAtQ {
    pub created_at: i64,
}

impl FromParams for CreatedAtQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        CreatedAtQ { created_at: p.query("created_at").int().required() }
    }
}

pub struct EntriesQ {
    pub created_at: i64,
    pub limit: i64,
    pub offset: i64,
}

impl FromParams for EntriesQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        EntriesQ {
            created_at: p.query("created_at").int().required(),
            limit: p.query("limit").int().ge(1).le(1000).default(100),
            offset: p.query("offset").int().ge(0).default(0),
        }
    }
}

pub struct HistoryQ {
    pub days: i64,
    pub top: i64,
}

impl FromParams for HistoryQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        HistoryQ {
            days: p.query("days").int().ge(1).le(30).default(7),
            top: p.query("top").int().ge(1).le(20).default(5),
        }
    }
}

/// The `/site` routes' `uuid: int` path parameter (pydantic, before the
/// query), `None` past int32.
pub struct SiteUuid(pub Option<i32>);

fn site_uuid(p: &mut Params<'_>) -> Option<i32> {
    i32::try_from(p.path("uuid").int().saturating().required()).ok()
}

impl FromParams for SiteUuid {
    fn from_params(p: &mut Params<'_>) -> Self {
        SiteUuid(site_uuid(p))
    }
}

pub struct SiteQ<T>(pub Option<i32>, pub T);

impl<T: FromParams> FromParams for SiteQ<T> {
    fn from_params(p: &mut Params<'_>) -> Self {
        let uuid = site_uuid(p);
        SiteQ(uuid, T::from_params(p))
    }
}

// --- /v1 endpoints --------------------------------------------------------------

pub async fn v1_timestamps<B: Backend>(b: &B, limit: i64) -> Reply {
    let items = b.list_timestamps(limit, None).await.map_err(internal)?;
    Ok(pydantic(&json!({"items": items, "count": items.len()})))
}

pub async fn v1_boards<B: Backend>(b: &B, token_id: &str, created_at: i64) -> Reply {
    let anchor = lb_timestamp(created_at, secs(b.now_us()))?;
    let archive = archive_limit(b, token_id, anchor).await?;
    let rows = b.list_boards_at(anchor).await.map_err(internal)?;
    let items: Vec<Value> = rows.iter().map(|r| Value::Object(r.info_v1())).collect();
    Ok((archive, pydantic(&json!({"created_at": anchor, "items": items, "count": items.len()}))).into_response())
}

pub async fn v1_board<B: Backend>(b: &B, uuid: i32) -> Reply {
    match b.get_board(uuid).await.map_err(internal)? {
        Some(board) => Ok(pydantic(&service::board_v1(&board))),
        None => Err(ApiError::not_found(format!("No leaderboard with uuid {uuid}")).into()),
    }
}

pub async fn v1_entries<B: Backend>(b: &B, token_id: &str, uuid: i32, q: &EntriesQ) -> Reply {
    let anchor = lb_timestamp(q.created_at, secs(b.now_us()))?;
    let archive = archive_limit(b, token_id, anchor).await?;
    let page = service::list_entries_with_deltas(b, uuid, anchor, q.limit, q.offset).await.map_err(internal)?;
    Ok((archive, pydantic(&service::entries_v1(uuid, anchor, &page))).into_response())
}

pub async fn v1_history<B: Backend>(b: &B, token_id: &str, uuid: i32, q: &HistoryQ) -> Reply {
    let now = secs(b.now_us());
    let archive = archive_limit(b, token_id, now - q.days * DAY).await?;
    let payload = cache::get_board_history(b, b, uuid, q.days, q.top, now).await.map_err(internal)?;
    let body = service::history_v1(&payload).map_err(internal)?;
    Ok((archive, pydantic(&body)).into_response())
}

/// A board id past the `INTEGER` column: Python reaches the first query that
/// binds it and asyncpg refuses the argument.
fn past_int32() -> Rejection {
    internal(anyhow::anyhow!("board uuid out of int32 range"))
}

/// `uuid` is `None` past int32: the steps before the first query still run.
pub async fn v1_entries_any<B: Backend>(b: &B, token_id: &str, uuid: Option<i32>, q: &EntriesQ) -> Reply {
    let Some(uuid) = uuid else {
        let anchor = lb_timestamp(q.created_at, secs(b.now_us()))?;
        archive_limit(b, token_id, anchor).await?;
        return Err(past_int32());
    };
    v1_entries(b, token_id, uuid, q).await
}

pub async fn v1_history_any<B: Backend>(b: &B, token_id: &str, uuid: Option<i32>, q: &HistoryQ) -> Reply {
    let Some(uuid) = uuid else {
        archive_limit(b, token_id, secs(b.now_us()) - q.days * DAY).await?;
        return Err(past_int32());
    };
    v1_history(b, token_id, uuid, q).await
}

pub async fn site_entries_any<B: Backend>(b: &B, uuid: Option<i32>, q: &EntriesQ) -> Reply {
    let Some(uuid) = uuid else {
        guard_archive(b, q.created_at).await?;
        return Err(past_int32());
    };
    site_entries(b, uuid, q).await
}

pub async fn v1_health<B: Backend>(b: &B, uuid: i32) -> Reply {
    match service::board_health(b, uuid).await.map_err(internal)? {
        Some(h) => Ok(pydantic(&h.value)),
        None => Err(ApiError::not_found(format!("No leaderboard data for uuid {uuid}")).into()),
    }
}

const RECORDS_CACHE: &str = "public, max-age=900";

pub async fn v1_records<B: Backend>(b: &B) -> Reply {
    let payload = service::mastery_records(b, b).await.map_err(internal)?;
    let cc = [(header::CACHE_CONTROL, HeaderValue::from_static(RECORDS_CACHE))];
    Ok((cc, pydantic(&service::records_v1(&payload))).into_response())
}

// --- /site endpoints --------------------------------------------------------------

/// `_lb_browse_window_days`.
async fn browse_window<B: Env>(b: &B) -> Result<i64, Rejection> {
    let v = b.setting("leaderboards_anon_retention_days").await.map_err(internal)?;
    Ok(py_int(&v).map_err(internal)?.max(1))
}

/// `_guard_lb_archive_anchor`.
async fn guard_archive<B: Env>(b: &B, anchor: i64) -> Result<(), Rejection> {
    let window = browse_window(b).await?;
    if anchor < secs(b.now_us()) - window * DAY {
        return Err(ApiError::forbidden(format!("Captures older than {window} days aren't available.")).into());
    }
    Ok(())
}

async fn flag<B: Env>(b: &B, key: &str) -> Result<bool, Rejection> {
    Ok(py_truthy(&b.setting(key).await.map_err(internal)?))
}

pub async fn site_config<B: Backend>(b: &B) -> Reply {
    let days = py_int(&b.setting("leaderboards_hot_retention_days").await.map_err(internal)?).map_err(internal)?;
    let picker_days = browse_window(b).await?;
    let body = json!({
        "hot_retention_days": days,
        "picker_days": picker_days,
        "graph_max_age_days": picker_days,
        "cheater_detection_enabled": flag(b, features::CHEATER_DETECTION_FLAG).await?,
        "alt_clusters_enabled": flag(b, features::ALT_CLUSTERS_FLAG).await?,
        "renames_enabled": flag(b, features::RENAMES_FLAG).await?,
        "duplicates_enabled": flag(b, features::DUPLICATES_FLAG).await?,
    });
    Ok(site_json(body, "public, max-age=15"))
}

pub async fn site_timestamps<B: Backend>(b: &B, limit: i64) -> Reply {
    let items = cache::get_timestamps(b, b, usize::try_from(limit).unwrap_or(0)).await.map_err(internal)?;
    Ok(site_json(json!({"items": items, "count": items.len()}), "public, max-age=30"))
}

pub async fn site_days<B: Backend>(b: &B) -> Reply {
    let window = browse_window(b).await?;
    let anchors = cache::get_days(b, b, 400).await.map_err(internal)?;
    let cutoff = secs(b.now_us()) - window * DAY;
    let anchors: Vec<i64> = anchors.into_iter().filter(|&a| a >= cutoff).collect();
    Ok(site_json(json!({"items": anchors, "count": anchors.len()}), "public, max-age=120"))
}

pub async fn site_boards<B: Backend>(b: &B, created_at: i64) -> Reply {
    guard_archive(b, created_at).await?;
    let rows = cache::get_boards(b, b, created_at).await.map_err(internal)?;
    let count = py_len(&rows)?;
    Ok(site_json(json!({"created_at": created_at, "items": rows, "count": count}), "public, max-age=60"))
}

pub async fn site_entries<B: Backend>(b: &B, uuid: i32, q: &EntriesQ) -> Reply {
    guard_archive(b, q.created_at).await?;
    let (items, total, comparison) =
        cache::get_entries(b, b, uuid, q.created_at, q.limit, q.offset).await.map_err(internal)?;
    let count = py_len(&items)?;
    let body = json!({
        "uuid": uuid, "created_at": q.created_at, "items": items, "count": count,
        "total": total, "comparison": comparison,
    });
    Ok(site_json(body, "public, max-age=30"))
}

pub async fn site_history<B: Backend>(b: &B, uuid: i32, q: &HistoryQ) -> Reply {
    let payload = cache::get_board_history(b, b, uuid, q.days, q.top, secs(b.now_us())).await.map_err(internal)?;
    Ok(site_json(payload, "public, max-age=60"))
}

pub async fn site_health<B: Backend>(b: &B, uuid: i32) -> Reply {
    match service::board_health(b, uuid).await.map_err(internal)? {
        Some(h) if !h.finite => Err(internal(anyhow::anyhow!("board {uuid} health is not JSON compliant"))),
        Some(h) => Ok(site_json(h.value, "public, max-age=60")),
        None => {
            Err(ApiError::http(StatusCode::NOT_FOUND, Some(&format!("No leaderboard data for uuid {uuid}"))).into())
        }
    }
}

pub async fn site_records<B: Backend>(b: &B) -> Reply {
    Ok(site_json(service::mastery_records(b, b).await.map_err(internal)?, RECORDS_CACHE))
}

pub const ICON_BRANCH: &str = "live-us";

/// `_LB_ICON_NAME_RE.match`: `^[a-z0-9_]+$`, where `$` also takes one trailing newline.
pub fn icon_name_ok(name: &str) -> bool {
    let core = name.strip_suffix('\n').unwrap_or(name);
    !core.is_empty() && core.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
}

/// `ContentStore(root).path_for(sha)`.
pub fn blob_path(root: &str, sha: &str) -> std::path::PathBuf {
    let shard: String = sha.chars().take(2).collect();
    std::path::Path::new(root).join("objects").join(shard).join(sha)
}

/// `site_lb_board_icon`; `meta` is `get_file_meta(branch, path)["content_sha256"]`.
pub async fn board_icon<F, Fut>(name: &str, store_dir: &str, meta: F) -> Reply
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = anyhow::Result<Option<String>>>,
{
    let not_found = |detail: &str| Rejection::from(ApiError::http(StatusCode::NOT_FOUND, Some(detail)));
    if !icon_name_ok(name) {
        return Err(not_found("bad icon name"));
    }
    let sha = meta(format!("ui/leaderboard_icons/{name}.png")).await.map_err(internal)?;
    let Some(sha) = sha.filter(|s| !s.is_empty()) else { return Err(not_found("icon not found")) };
    let blob = blob_path(store_dir, &sha);
    if !tokio::fs::metadata(&blob).await.is_ok_and(|m| m.is_file()) {
        return Err(not_found("blob missing"));
    }
    let data = tokio::fs::read(&blob).await.map_err(|e| internal(e.into()))?;
    let headers = [
        (header::CONTENT_TYPE, HeaderValue::from_static("image/png")),
        (header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=604800")),
    ];
    Ok((headers, data).into_response())
}

/// `updates_read.get_file_meta(branch, path)`'s blob hash, with its
/// `last_known_meta` fallback for a removed path.
pub async fn file_meta_sha(db: &mongodb::Database, branch: &str, path: &str) -> anyhow::Result<Option<String>> {
    if let Some(d) = db.collection::<Document>("update_state").find_one(doc! {"branch": branch, "path": path}).await? {
        return match d.get("content_sha256") {
            Some(Bson::String(s)) => Ok(Some(s.clone())),
            other => anyhow::bail!("update_state {path}: content_sha256 is {other:?}"),
        };
    }
    let changes = db.collection::<Document>("update_changes");
    let newest = doc! {"ordinal": -1};
    let Some(last) = changes.find_one(doc! {"branch": branch, "path": path}).sort(newest.clone()).await? else {
        return Ok(None);
    };
    if last.get_str("type").ok() != Some("removed") {
        return Ok(None);
    }
    let ordinal = last.get("ordinal").cloned().unwrap_or(Bson::Null);
    let filter = doc! {
        "branch": branch, "path": path, "ordinal": {"$lt": ordinal}, "content_sha256": {"$ne": Bson::Null},
    };
    let prev = changes.find_one(filter).sort(newest).await?;
    Ok(prev.and_then(|p| p.get_str("content_sha256").ok().map(str::to_owned)))
}

// --- handlers ---------------------------------------------------------------------

type LbOn = RequireFeature<features::Leaderboards>;
type Scoped = RequireScope<{ scopes::LEADERBOARDS_READ }>;

/// `_resolve_feature_flags` for these paths: the bare 404 while leaderboards are off.
pub struct SiteLbOn;

impl FromRequestParts<AppState> for SiteLbOn {
    type Rejection = ApiError;

    async fn from_request_parts(_parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        match features::is_enabled(state, features::LEADERBOARDS_FLAG).await {
            Ok(true) => Ok(SiteLbOn),
            Ok(false) => Err(ApiError::http(StatusCode::NOT_FOUND, None)),
            Err(e) => {
                tracing::error!("site feature flags: {e:#}");
                Err(ApiError::internal())
            }
        }
    }
}

/// The board id of a `/v1/leaderboards/{uuid:int}/...` path (Starlette's
/// `[0-9]+` on the decoded path, see [`crate::starlette`]), `None` past int32.
pub fn uuid_of(uri: &Uri) -> Option<i32> {
    scope_path(uri).split('/').nth(3).and_then(parse_uuid)
}

fn with_limit(info: RateLimitInfo, reply: Reply) -> Response {
    match reply {
        Ok(resp) => (info, resp).into_response(),
        Err(e) => e.into_response(),
    }
}

fn reply(r: Reply) -> Response {
    r.unwrap_or_else(IntoResponse::into_response)
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/leaderboards", get(h_v1_boards))
        .route("/v1/leaderboards/timestamps", get(h_v1_timestamps))
        .route("/v1/leaderboards/records", get(h_v1_records))
        .route("/v1/leaderboards/{uuid}", get(h_v1_board))
        .route("/v1/leaderboards/{uuid}/entries", get(h_v1_entries))
        .route("/v1/leaderboards/{uuid}/history", get(h_v1_history))
        .route("/v1/leaderboards/{uuid}/health", get(h_v1_health))
        .route("/site/leaderboards/config", get(h_site_config))
        .route("/site/leaderboards/timestamps", get(h_site_timestamps))
        .route("/site/leaderboards/days", get(h_site_days))
        .route("/site/leaderboards/boards", get(h_site_boards))
        .route("/site/leaderboards/records", get(h_site_records))
        .route("/site/leaderboards/{uuid}/entries", get(h_site_entries))
        .route("/site/leaderboards/{uuid}/history", get(h_site_history))
        .route("/site/leaderboards/{uuid}/health", get(h_site_health))
        .route("/site/leaderboards/board-icon/{name}", get(h_site_board_icon))
}

async fn h_v1_timestamps(
    _: LbOn,
    State(state): State<AppState>,
    RequireScope(ctx): Scoped,
    Query(q): Query<LimitQ>,
) -> Response {
    with_limit(ctx.rate_limit, v1_timestamps(&Live(&state), q.limit).await)
}

async fn h_v1_boards(
    _: LbOn,
    State(state): State<AppState>,
    RequireScope(ctx): Scoped,
    Query(q): Query<CreatedAtQ>,
) -> Response {
    with_limit(ctx.rate_limit, v1_boards(&Live(&state), &ctx.token.id.to_hex(), q.created_at).await)
}

async fn h_v1_board(_: LbOn, State(state): State<AppState>, RequireScope(ctx): Scoped, uri: Uri) -> Response {
    let Some(uuid) = uuid_of(&uri) else { return past_int32().into_response() };
    with_limit(ctx.rate_limit, v1_board(&Live(&state), uuid).await)
}

async fn h_v1_entries(
    _: LbOn,
    State(state): State<AppState>,
    RequireScope(ctx): Scoped,
    uri: Uri,
    Query(q): Query<EntriesQ>,
) -> Response {
    with_limit(ctx.rate_limit, v1_entries_any(&Live(&state), &ctx.token.id.to_hex(), uuid_of(&uri), &q).await)
}

async fn h_v1_history(
    _: LbOn,
    State(state): State<AppState>,
    RequireScope(ctx): Scoped,
    uri: Uri,
    Query(q): Query<HistoryQ>,
) -> Response {
    with_limit(ctx.rate_limit, v1_history_any(&Live(&state), &ctx.token.id.to_hex(), uuid_of(&uri), &q).await)
}

async fn h_v1_health(_: LbOn, State(state): State<AppState>, RequireScope(ctx): Scoped, uri: Uri) -> Response {
    let Some(uuid) = uuid_of(&uri) else { return past_int32().into_response() };
    with_limit(ctx.rate_limit, v1_health(&Live(&state), uuid).await)
}

async fn h_v1_records(
    _: LbOn,
    State(state): State<AppState>,
    Public(ctx): Public<{ scopes::LEADERBOARDS_READ }>,
) -> Response {
    with_limit(ctx.rate_limit, v1_records(&Live(&state)).await)
}

async fn h_site_config(_: SiteLbOn, State(state): State<AppState>) -> Response {
    reply(site_config(&Live(&state)).await)
}

async fn h_site_timestamps(_: SiteLbOn, State(state): State<AppState>, Query(q): Query<LimitQ>) -> Response {
    reply(site_timestamps(&Live(&state), q.limit).await)
}

async fn h_site_days(_: SiteLbOn, State(state): State<AppState>) -> Response {
    reply(site_days(&Live(&state)).await)
}

async fn h_site_boards(_: SiteLbOn, State(state): State<AppState>, Query(q): Query<CreatedAtQ>) -> Response {
    reply(site_boards(&Live(&state), q.created_at).await)
}

async fn h_site_records(_: SiteLbOn, State(state): State<AppState>) -> Response {
    reply(site_records(&Live(&state)).await)
}

async fn h_site_entries(_: SiteLbOn, State(state): State<AppState>, Query(q): Query<SiteQ<EntriesQ>>) -> Response {
    reply(site_entries_any(&Live(&state), q.0, &q.1).await)
}

async fn h_site_history(_: SiteLbOn, State(state): State<AppState>, Query(q): Query<SiteQ<HistoryQ>>) -> Response {
    match q.0 {
        Some(uuid) => reply(site_history(&Live(&state), uuid, &q.1).await),
        None => past_int32().into_response(),
    }
}

async fn h_site_health(_: SiteLbOn, State(state): State<AppState>, Query(SiteUuid(uuid)): Query<SiteUuid>) -> Response {
    match uuid {
        Some(uuid) => reply(site_health(&Live(&state), uuid).await),
        None => past_int32().into_response(),
    }
}

/// The `{name}` segment as Starlette routes it: percent-decoded, bad UTF-8 replaced.
pub fn icon_name_of(uri: &Uri) -> String {
    let raw = uri.path().rsplit('/').next().unwrap_or("");
    percent_encoding::percent_decode_str(raw).decode_utf8_lossy().into_owned()
}

async fn h_site_board_icon(_: SiteLbOn, State(state): State<AppState>, uri: Uri) -> Response {
    let name = icon_name_of(&uri);
    let store_dir = state.settings().trove_update_store_dir.clone();
    let meta = |path: String| async move {
        let db = state.inner.db.mongo.as_ref().ok_or_else(|| anyhow::anyhow!("MONGO_URI not configured"))?;
        file_meta_sha(db, ICON_BRANCH, &path).await
    };
    reply(board_icon(&name, &store_dir, meta).await)
}
