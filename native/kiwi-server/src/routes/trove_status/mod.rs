//! `GET /v1/misc/trove-status` and `/history` (`app/trove/status.py`, `app/trove/router.py`).
//!
//! The current status is the prober snapshot `get_status_shared` reads: this
//! process's prober when it runs here, else the Redis mirror every Python
//! prober writes, else the "unknown" shell.

pub mod prober;
#[cfg(test)]
mod tests;

use axum::{
    Router,
    extract::State,
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
    routing::get,
};
use futures::TryStreamExt;
use kiwi_core::status::{self as core, Segment};
use mongodb::bson::{Bson, Document, doc};
use serde_json::{Map, Value, json};

use crate::{
    auth::{Public, scopes},
    error::ApiError,
    features::{RequireFeature, ServerStatus},
    pyjson::{self, PydanticJson},
    state::AppState,
    validation::{FromParams, Params, Query},
};

pub const REDIS_KEY: &str = "kiwi:status:current";
pub const COLLECTION: &str = "trove_status_events";
const OVERALL: [&str; 4] = ["online", "maintenance", "down", "unknown"];

pub fn router() -> Router<AppState> {
    Router::new().route("/v1/misc/trove-status", get(current)).route("/v1/misc/trove-status/history", get(history))
}

fn cached(cache_control: &'static str, body: Value) -> Response {
    ([(header::CACHE_CONTROL, HeaderValue::from_static(cache_control))], PydanticJson(body)).into_response()
}

async fn current(
    _: RequireFeature<ServerStatus>,
    Public(ctx): Public<{ scopes::MISC_READ }>,
    State(state): State<AppState>,
) -> Result<Response, ApiError> {
    let body = status_response(shared_snapshot(&state).await.as_ref())?;
    Ok((ctx.rate_limit, cached("public, max-age=30", body)).into_response())
}

pub struct HistoryQuery {
    pub env: String,
    pub days: i64,
}

impl FromParams for HistoryQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        HistoryQuery {
            env: p.query("env").str().one_of_pattern(&["live", "pts"]).default("live".into()),
            days: p.query("days").int().ge(1).le(90).default(30),
        }
    }
}

async fn history(
    _: RequireFeature<ServerStatus>,
    Public(ctx): Public<{ scopes::MISC_READ }>,
    State(state): State<AppState>,
    Query(q): Query<HistoryQuery>,
) -> Result<Response, ApiError> {
    let now = unix_now();
    let env = core::history_env(&q.env);
    let rows = load_segments(&state, env).await.map_err(|e| {
        tracing::error!("trove status history: {e:#}");
        ApiError::internal()
    })?;
    Ok((ctx.rate_limit, history_reply(env, q.days, now, &rows)).into_response())
}

pub fn history_reply(env: &str, days: i64, now: i64, rows: &[Segment]) -> Response {
    cached("public, max-age=60", history_value(&core::history(env, days, now, rows)))
}

pub fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// `get_status()` before the first probe.
pub fn unknown_snapshot() -> Value {
    let env = || json!({"status": "unknown", "online": false, "game": null});
    json!({
        "overall": "unknown",
        "auth": null,
        "environments": {"eu": env(), "us": env(), "pts": env()},
        "checked_at": null,
    })
}

/// `get_status_shared()` minus the shell: `None` means no snapshot anywhere.
pub async fn shared_snapshot(state: &AppState) -> Option<Value> {
    if let Some(v) = prober::current() {
        return Some(v);
    }
    let mut conn = state.inner.db.redis.clone()?;
    let raw: Option<Vec<u8>> = match redis::cmd("GET").arg(REDIS_KEY).query_async(&mut conn).await {
        Ok(raw) => raw,
        Err(e) => {
            tracing::warn!("status: failed to read shared snapshot from Redis: {e}");
            return None;
        }
    };
    let text = String::from_utf8(raw?).ok().filter(|s| !s.is_empty() && !s.starts_with('\u{feff}'))?;
    match pyjson::loads(text.as_bytes()) {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!("status: shared snapshot is not JSON: {e:?}");
            None
        }
    }
}

/// `TroveStatusResponse(**payload)` as FastAPI dumps it; a snapshot the model
/// rejects is the 500 Python answers.
pub fn status_response(snapshot: Option<&Value>) -> Result<Value, ApiError> {
    let Some(snapshot) = snapshot else { return Ok(unknown_snapshot()) };
    let bad = || {
        tracing::error!("trove status snapshot fails TroveStatusResponse: {snapshot}");
        ApiError::internal()
    };
    let m = snapshot.as_object().ok_or_else(bad)?;
    let overall = m.get("overall").and_then(Value::as_str).filter(|o| OVERALL.contains(o)).ok_or_else(bad)?;
    let auth = m.get("auth").filter(|a| a.is_null() || a.is_object()).ok_or_else(bad)?;
    let environments = m.get("environments").filter(|e| e.is_object()).ok_or_else(bad)?;
    let checked_at = match m.get("checked_at").ok_or_else(bad)? {
        Value::Null => Value::Null,
        Value::Number(n) if n.is_i64() || n.is_u64() => Value::Number(n.clone()),
        Value::Number(n) => match n.as_f64() {
            Some(f) if f.is_finite() && f.fract() == 0.0 && f.abs() < 9.2e18 => Value::from(f as i64),
            _ => return Err(bad()),
        },
        _ => return Err(bad()),
    };
    let mut out = Map::new();
    out.insert("overall".into(), Value::from(overall));
    out.insert("auth".into(), auth.clone());
    out.insert("environments".into(), environments.clone());
    out.insert("checked_at".into(), checked_at);
    Ok(Value::Object(out))
}

/// `TroveStatusHistoryResponse` of a computed history.
pub fn history_value(h: &core::History) -> Value {
    let segments: Vec<Value> = h
        .segments
        .iter()
        .map(|s| {
            json!({
                "status": s.status,
                "online": s.online,
                "started_at": s.started_at,
                "ended_at": s.ended_at,
                "duration_seconds": s.duration_seconds,
            })
        })
        .collect();
    let outages: Vec<Value> = h
        .outages()
        .map(|s| {
            json!({
                "status": s.status,
                "started_at": s.started_at,
                "ended_at": s.ended_at,
                "duration_seconds": s.duration_seconds,
            })
        })
        .collect();
    json!({
        "env": h.env,
        "days": h.days,
        "window_start": h.window_start,
        "window_end": h.window_end,
        "uptime": h.uptime,
        "covered_seconds": h.covered_seconds,
        "segments": segments,
        "outages": outages,
    })
}

/// A pydantic `int` field read from BSON (integral doubles pass, as in lax mode).
fn bson_int(v: Option<&Bson>) -> Option<i64> {
    match v? {
        Bson::Int32(i) => Some(i64::from(*i)),
        Bson::Int64(i) => Some(*i),
        Bson::Double(f) if f.is_finite() && f.fract() == 0.0 && f.abs() < 9.2e18 => Some(*f as i64),
        _ => None,
    }
}

/// A stored event validated as `TroveStatusEvent`.
pub fn segment_from_doc(d: &Document) -> anyhow::Result<Segment> {
    let invalid = || anyhow::anyhow!("invalid TroveStatusEvent {d}");
    let status = d.get_str("status").map_err(|_| invalid())?.to_owned();
    d.get_str("env").map_err(|_| invalid())?;
    let online = match d.get("online") {
        Some(Bson::Boolean(b)) => *b,
        Some(Bson::Int32(i @ (0 | 1))) => *i == 1,
        Some(Bson::Int64(i @ (0 | 1))) => *i == 1,
        _ => return Err(invalid()),
    };
    let started_at = bson_int(d.get("started_at")).ok_or_else(invalid)?;
    let ended_at = match d.get("ended_at") {
        None | Some(Bson::Null) => None,
        v => Some(bson_int(v).ok_or_else(invalid)?),
    };
    Ok(Segment { status, online, started_at, ended_at })
}

async fn load_segments(state: &AppState, env: &str) -> anyhow::Result<Vec<Segment>> {
    let db = state.inner.db.mongo.as_ref().ok_or_else(|| anyhow::anyhow!("MONGO_URI not configured"))?;
    let mut cursor =
        db.collection::<Document>(COLLECTION).find(doc! { "env": env }).sort(doc! { "started_at": 1 }).await?;
    let mut rows = Vec::new();
    while let Some(d) = cursor.try_next().await? {
        rows.push(segment_from_doc(&d)?);
    }
    Ok(rows)
}
