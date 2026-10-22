//! The leaderboards maintenance routes of `app/admin/router.py` (all behind
//! `get_current_superuser`): board reset cadence, the player-aggregate
//! rebuild and last-played backfill, the cold-tier move, and the ingest log.

use std::sync::{LazyLock, Mutex};

use axum::{
    extract::{FromRequestParts, RawPathParams, Request, State},
    response::{IntoResponse, Response},
};
use futures::TryStreamExt;
use kiwi_core::lbmodels::{RESET_KIND_VALUES, effective_reset_kind, is_lifetime_kind};
use mongodb::bson::{Bson, Document, doc};
use serde_json::{Map, Value, json};

use super::{routes::internal, signals, store};
use crate::{
    auth::Superuser,
    dt,
    error::ApiError,
    pyjson::PydanticJson,
    runtime_config::get_setting,
    state::AppState,
    validation::{Field, FromParams, Params, Query, ValidationError, read_body},
};

/// `LeaderboardBoardAdminView`.
pub fn board_view(b: &store::BoardRow) -> Value {
    let eff = effective_reset_kind(b.reset_kind_override.as_deref(), b.uuid);
    json!({
        "uuid": b.uuid,
        "name": b.name,
        "name_id": b.name_id,
        "category": b.category,
        "category_id": b.category_id,
        "effective_reset_kind": eff,
        "reset_kind_override": b.reset_kind_override,
        "has_periodic_reset": !is_lifetime_kind(eff),
    })
}

/// `LeaderboardBoardAdminList`, sorted by `(category, name, uuid)`.
pub fn board_list(mut rows: Vec<store::BoardRow>) -> Value {
    rows.sort_by(|a, b| (&a.category, &a.name, a.uuid).cmp(&(&b.category, &b.name, b.uuid)));
    let items: Vec<Value> = rows.iter().map(board_view).collect();
    json!({"items": items, "count": rows.len()})
}

/// `GET /admin/leaderboards/boards`.
pub async fn list_boards(State(state): State<AppState>, _admin: Superuser) -> Result<PydanticJson, ApiError> {
    let pool = store::pool(&state).map_err(internal)?;
    Ok(PydanticJson(board_list(store::admin_list_boards(pool).await.map_err(internal)?)))
}

/// `PATCH /admin/leaderboards/boards/{uuid}`: the path and body checks.
pub fn validate_patch(
    path: Vec<(String, String)>,
    body: &crate::validation::BodyInput,
) -> Result<(i64, Option<String>), ValidationError> {
    let headers = axum::http::HeaderMap::new();
    let mut p = Params::new(path, None, &headers);
    let uuid = p.path("uuid").int().required();
    let mut v = p.v;
    let value = Field::new(&mut v, vec!["body".into()], body.input())
        .model(|m| m.field("reset_kind_override").str().optional())
        .required();
    v.finish((uuid, value))
}

/// The 400 for an override outside `RESET_KIND_VALUES`.
pub fn bad_override(value: &str) -> ApiError {
    ApiError::bad_request(format!(
        "reset_kind_override must be one of {} or null; got {}",
        RESET_KIND_VALUES.join(", "),
        kiwi_core::pystr::repr(value)
    ))
}

pub async fn patch_board(State(state): State<AppState>, req: Request) -> Response {
    let (mut parts, body) = req.into_parts();
    let body = match read_body(&parts.headers, body).await {
        Ok(b) => b,
        Err(e) => return e.into_response(),
    };
    let admin = match Superuser::from_request_parts(&mut parts, &state).await {
        Ok(Superuser(u)) => u,
        Err(r) => return r.into_response(),
    };
    let path = match RawPathParams::from_request_parts(&mut parts, &state).await {
        Ok(raw) => raw.iter().map(|(k, v)| (k.to_owned(), v.to_owned())).collect(),
        Err(_) => Vec::new(),
    };
    let (uuid, value) = match validate_patch(path, &body) {
        Ok(x) => x,
        Err(e) => return e.into_response(),
    };
    if let Some(v) = value.as_deref()
        && !RESET_KIND_VALUES.contains(&v)
    {
        return bad_override(v).into_response();
    }
    let res = async {
        let pool = store::pool(&state)?;
        store::set_reset_kind_override(pool, uuid, value.as_deref()).await
    }
    .await;
    match res {
        Err(e) => internal(e).into_response(),
        Ok(None) => ApiError::not_found(format!("No leaderboard with uuid={uuid}")).into_response(),
        Ok(Some(row)) => {
            signals::trigger_warmer(&state, &admin);
            PydanticJson(board_view(&row)).into_response()
        }
    }
}

// --- background jobs ------------------------------------------------------------------

/// `_agg_rebuild_status` / `_last_rise_status`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AggStatus {
    pub running: bool,
    pub captures: u64,
    pub first_anchor: Option<i64>,
    pub anchor: Option<i64>,
    pub rows: Option<i64>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub error: Option<String>,
}

impl AggStatus {
    pub fn to_json(&self) -> Value {
        json!({
            "running": self.running, "captures": self.captures, "first_anchor": self.first_anchor,
            "anchor": self.anchor, "rows": self.rows, "started_at": self.started_at,
            "finished_at": self.finished_at, "error": self.error,
        })
    }
}

/// `_cold_tier_status`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ColdStatus {
    pub running: bool,
    pub moved: Option<Vec<String>>,
    pub moved_bytes: Option<i64>,
    pub finished_at: Option<i64>,
    pub error: Option<String>,
}

impl ColdStatus {
    pub fn to_json(&self) -> Value {
        json!({
            "running": self.running, "moved": self.moved, "moved_bytes": self.moved_bytes,
            "finished_at": self.finished_at, "error": self.error,
        })
    }
}

pub const REBUILD_BUSY: &str = "A rebuild is already running.";
pub const LAST_RISE_BUSY: &str = "A last-played backfill is already running.";
pub const COLD_BUSY: &str = "A cold-tier move is already running.";

static REBUILD: LazyLock<Mutex<AggStatus>> = LazyLock::new(Default::default);
static LAST_RISE: LazyLock<Mutex<AggStatus>> = LazyLock::new(Default::default);
static COLD: LazyLock<Mutex<ColdStatus>> = LazyLock::new(Default::default);

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn disabled() -> ApiError {
    ApiError::bad_request("Postgres backend is disabled")
}

type AggJob = for<'a> fn(
    &'a deadpool_postgres::Pool,
    &'a (dyn Fn(u64, i64, i64) + Sync),
) -> std::pin::Pin<Box<dyn Future<Output = store::PgResult<i64>> + Send + 'a>>;

fn start_agg(
    state: &AppState,
    status: &'static Mutex<AggStatus>,
    job: AggJob,
    busy: &str,
    started: &str,
) -> Result<PydanticJson, ApiError> {
    if !state.settings().postgres_enabled() {
        return Err(disabled());
    }
    {
        let mut s = lock(status);
        if s.running {
            return Ok(PydanticJson(json!({"started": false, "message": busy})));
        }
        *s = AggStatus { running: true, started_at: Some(now()), ..AggStatus::default() };
    }
    let state = state.clone();
    tokio::spawn(async move {
        let progress = |done: u64, first: i64, anchor: i64| {
            let mut s = lock(status);
            s.captures = done;
            s.first_anchor = Some(first);
            s.anchor = Some(anchor);
        };
        let res = match store::pool(&state) {
            Ok(pool) => job(pool, &progress).await,
            Err(e) => Err(e),
        };
        let mut s = lock(status);
        match res {
            Ok(rows) => s.rows = Some(rows),
            Err(e) => {
                tracing::error!("player aggregate job failed: {e}");
                s.error = Some(e);
            }
        }
        s.running = false;
        s.finished_at = Some(now());
    });
    Ok(PydanticJson(json!({"started": true, "message": started})))
}

/// `POST /admin/leaderboards/rebuild-player-agg`.
pub async fn start_rebuild(State(state): State<AppState>, _admin: Superuser) -> Result<PydanticJson, ApiError> {
    start_agg(
        &state,
        &REBUILD,
        |pool, progress| Box::pin(store::rebuild_player_board_agg(pool, progress)),
        REBUILD_BUSY,
        "Player aggregate rebuild started - poll /admin/leaderboards/rebuild-player-agg/status.",
    )
}

pub async fn rebuild_status(_admin: Superuser) -> PydanticJson {
    PydanticJson(lock(&REBUILD).to_json())
}

/// `POST /admin/leaderboards/backfill-last-rise`.
pub async fn start_last_rise(State(state): State<AppState>, _admin: Superuser) -> Result<PydanticJson, ApiError> {
    start_agg(
        &state,
        &LAST_RISE,
        |pool, progress| Box::pin(store::backfill_last_rise(pool, progress)),
        LAST_RISE_BUSY,
        "Last-played backfill started - poll /admin/leaderboards/backfill-last-rise/status.",
    )
}

pub async fn last_rise_status(_admin: Superuser) -> PydanticJson {
    PydanticJson(lock(&LAST_RISE).to_json())
}

async fn tier_after_days(state: &AppState) -> Result<i64, String> {
    let v = get_setting(state, "leaderboards_pg_tier_after_days").await.map_err(|e| e.to_string())?;
    Ok(match &v {
        Value::Bool(b) => i64::from(*b),
        Value::Number(n) => n.as_i64().unwrap_or_else(|| n.as_f64().map_or(0, |f| f.trunc() as i64)),
        Value::String(s) => kiwi_core::pynum::int(s).ok_or_else(|| format!("invalid literal for int(): {s}"))?,
        _ => return Err(format!("int() of {v}")),
    })
}

/// `POST /admin/leaderboards/tier-cold`: drain every aged partition.
pub async fn start_cold(State(state): State<AppState>, _admin: Superuser) -> Result<PydanticJson, ApiError> {
    if !state.settings().postgres_enabled() {
        return Err(disabled());
    }
    {
        let mut s = lock(&COLD);
        if s.running {
            return Ok(PydanticJson(json!({"started": false, "message": COLD_BUSY})));
        }
        s.running = true;
        s.error = None;
    }
    tokio::spawn(async move {
        let res = async {
            let after_days = tier_after_days(&state).await?;
            store::tier_cold_partitions(store::pool(&state)?, after_days, now(), None).await
        }
        .await;
        let mut s = lock(&COLD);
        match res {
            Ok(moved) => {
                let (names, bytes) = moved.unwrap_or_default();
                s.moved = Some(names);
                s.moved_bytes = Some(bytes);
            }
            Err(e) => {
                tracing::error!("cold-tier move failed: {e}");
                s.error = Some(e);
            }
        }
        s.running = false;
        s.finished_at = Some(now());
    });
    Ok(PydanticJson(
        json!({"started": true, "message": "Cold-tier move started - poll /admin/leaderboards/tier-cold/status."}),
    ))
}

/// `tier_status(after_days, now)` from the partitions (None: no `cold` tablespace).
pub fn tier_layout(parts: Option<&[store::Partition]>, after_days: i64, now: i64) -> Value {
    let Some(parts) = parts else { return json!({"cold_tablespace": false}) };
    let keep_from = kiwi_core::lbmodels::cold_keep_from(now, after_days);
    let hot: Vec<&store::Partition> = parts.iter().filter(|p| p.tablespace == "pg_default").collect();
    let cold: Vec<&store::Partition> = parts.iter().filter(|p| p.tablespace == store::COLD_TABLESPACE).collect();
    let eligible: Vec<&&store::Partition> = hot.iter().filter(|p| p.lo < keep_from).collect();
    json!({
        "cold_tablespace": true,
        "after_days": after_days,
        "keep_from": keep_from,
        "partitions": parts.len(),
        "hot_partitions": hot.len(),
        "cold_partitions": cold.len(),
        "hot_bytes": hot.iter().map(|p| p.bytes).sum::<i64>(),
        "cold_bytes": cold.iter().map(|p| p.bytes).sum::<i64>(),
        "eligible_partitions": eligible.len(),
        "eligible_bytes": eligible.iter().map(|p| p.bytes).sum::<i64>(),
    })
}

/// `GET /admin/leaderboards/tier-cold/status`.
pub async fn cold_status(State(state): State<AppState>, _admin: Superuser) -> PydanticJson {
    let mut out = Map::new();
    out.insert("last_run".into(), lock(&COLD).to_json());
    if state.settings().postgres_enabled() {
        let layout = async {
            let after_days = tier_after_days(&state).await?;
            let parts = store::tier_partitions(store::pool(&state)?).await?;
            Ok::<Value, String>(tier_layout(parts.as_deref(), after_days, now()))
        }
        .await;
        let layout = layout.unwrap_or_else(|e| {
            tracing::error!("tier_status failed: {e}");
            json!({"error": "tier status unavailable"})
        });
        out.insert("layout".into(), layout);
    }
    PydanticJson(Value::Object(out))
}

// --- ingest log ---------------------------------------------------------------------

pub struct LogQuery {
    pub limit: i64,
    pub endpoint: Option<String>,
}

impl FromParams for LogQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        LogQuery {
            limit: p.query("limit").int().ge(1).le(200).default(20),
            endpoint: p.query("endpoint").str().optional(),
        }
    }
}

/// A stored value as pydantic serializes the `dict[str, Any]` it loads.
pub fn bson_json(b: &Bson) -> Value {
    match b {
        Bson::Double(f) if f.is_finite() => json!(f),
        Bson::String(s) => Value::String(s.clone()),
        Bson::Boolean(v) => Value::Bool(*v),
        Bson::Int32(n) => json!(n),
        Bson::Int64(n) => json!(n),
        Bson::Array(a) => Value::Array(a.iter().map(bson_json).collect()),
        Bson::Document(d) => Value::Object(d.iter().map(|(k, v)| (k.clone(), bson_json(v))).collect()),
        Bson::DateTime(d) => Value::String(dt::pydantic(&dt::from_bson(*d))),
        Bson::ObjectId(o) => Value::String(o.to_hex()),
        _ => Value::Null,
    }
}

/// One `list_ingest_log` row, or Err where Beanie would refuse the document.
pub fn log_row(d: &Document) -> Result<Value, String> {
    let s = |k: &str| d.get_str(k).map(str::to_owned).map_err(|e| format!("{k}: {e}"));
    let opt_s = |k: &str| match d.get(k) {
        None | Some(Bson::Null) => Ok(None),
        Some(Bson::String(v)) => Ok(Some(v.clone())),
        Some(other) => Err(format!("{k}: {other}")),
    };
    let ts = d.get_datetime("timestamp").map_err(|e| format!("timestamp: {e}"))?;
    let success = match d.get("success") {
        Some(Bson::Boolean(b)) => *b,
        Some(Bson::Int32(n @ (0 | 1))) => *n == 1,
        Some(Bson::Int64(n @ (0 | 1))) => *n == 1,
        other => return Err(format!("success: {other:?}")),
    };
    let summary = d.get_document("summary").map_err(|e| format!("summary: {e}"))?;
    d.get_object_id("user_id").map_err(|e| format!("user_id: {e}"))?;
    let auth_via = opt_s("auth_via")?.unwrap_or_else(|| "session".into());
    if auth_via != "token" && auth_via != "session" {
        return Err(format!("auth_via: {auth_via}"));
    }
    Ok(json!({
        "endpoint": s("endpoint")?,
        "timestamp": dt::isoformat(&dt::from_bson(*ts)),
        "user_email": s("user_email")?,
        "success": success,
        "summary": bson_json(&Bson::Document(summary.clone())),
        "error": opt_s("error")?,
        "auth_via": auth_via,
        "token_name": opt_s("token_name")?,
    }))
}

/// `ingest_log.recent`'s query: an empty endpoint filters nothing.
pub fn log_filter(endpoint: Option<String>) -> Document {
    match endpoint.filter(|e| !e.is_empty()) {
        Some(e) => doc! {"endpoint": e},
        None => doc! {},
    }
}

/// `GET /admin/ingest/log`.
pub async fn ingest_log(
    State(state): State<AppState>,
    _admin: Superuser,
    Query(q): Query<LogQuery>,
) -> Result<PydanticJson, ApiError> {
    let filter = log_filter(q.endpoint);
    let db = state.inner.db.mongo.as_ref().ok_or_else(|| internal("MONGO_URI not configured"))?;
    let cursor = db
        .collection::<Document>(signals::INGEST_LOG)
        .find(filter)
        .sort(doc! {"timestamp": -1})
        .limit(q.limit.clamp(1, 200))
        .await
        .map_err(internal)?;
    let docs: Vec<Document> = cursor.try_collect().await.map_err(internal)?;
    let rows = docs.iter().map(log_row).collect::<Result<Vec<_>, _>>().map_err(internal)?;
    Ok(PydanticJson(Value::Array(rows)))
}
