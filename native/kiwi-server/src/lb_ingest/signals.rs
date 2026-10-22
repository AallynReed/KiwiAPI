//! What an ingest leaves behind besides the rows: the master ingest log
//! (`app/admin/ingest_log.py`), the Redis snapshot invalidation and reset
//! (`cache.py`), the `leaderboard` event (`app/events/bus.py`), and the warmer
//! wake-up.

use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use tokio::sync::Notify;

use crate::{
    auth::{
        jwt,
        models::{ApiToken, User},
    },
    dt, jobs, pyjson,
    state::AppState,
};

pub const INGEST_LOG: &str = "ingest_log";

/// `IngestLogEntry` as Beanie inserts it (`_id` first, as pymongo sends it);
/// `success` is `error is None` at every caller here.
pub fn log_doc(
    id: ObjectId,
    endpoint: &str,
    now: chrono::DateTime<chrono::Utc>,
    user: &User,
    token: Option<&ApiToken>,
    summary: Document,
    error: Option<&str>,
) -> Document {
    doc! {
        "_id": id,
        "endpoint": endpoint,
        "timestamp": dt::to_bson(now),
        "user_id": user.id,
        "user_email": &user.email,
        "success": error.is_none(),
        "summary": summary,
        "error": error.map_or(Bson::Null, |e| Bson::String(e.to_owned())),
        "auth_via": if token.is_some() { "token" } else { "session" },
        "token_name": token.map_or(Bson::Null, |t| Bson::String(t.name.clone())),
    }
}

/// `ingest_log.record`: never fails the caller.
pub async fn record(
    state: &AppState,
    endpoint: &str,
    user: &User,
    token: Option<&ApiToken>,
    summary: Document,
    error: Option<&str>,
) {
    let Some(db) = state.inner.db.mongo.as_ref() else { return };
    let row = log_doc(ObjectId::new(), endpoint, dt::now(), user, token, summary, error);
    if let Err(e) = db.collection::<Document>(INGEST_LOG).insert_one(row).await {
        tracing::error!("ingest_log.record: failed to write entry for {endpoint}: {e}");
    }
}

const CACHE_PREFIX: &str = "lb:cache:";

/// `SCAN MATCH` as redis-py's `scan_iter` walks it (no COUNT, repeats kept).
async fn scan(con: &mut redis::aio::ConnectionManager, pattern: &str) -> redis::RedisResult<Vec<String>> {
    let mut keys = Vec::new();
    let mut cursor: u64 = 0;
    loop {
        let (next, batch): (u64, Vec<String>) =
            redis::cmd("SCAN").arg(cursor).arg("MATCH").arg(pattern).query_async(con).await?;
        keys.extend(batch);
        if next == 0 {
            return Ok(keys);
        }
        cursor = next;
    }
}

/// `cache.invalidate_anchor`.
pub async fn invalidate_anchor(state: &AppState, anchor: i64) {
    let Some(mut con) = state.inner.db.redis.clone() else { return };
    let res: redis::RedisResult<()> = async {
        let mut keys = vec![
            format!("{CACHE_PREFIX}boards:{anchor}"),
            format!("{CACHE_PREFIX}cheaters:{anchor}"),
            format!("{CACHE_PREFIX}activity:{anchor}"),
        ];
        keys.extend(scan(&mut con, &format!("{CACHE_PREFIX}entries:{anchor}:*")).await?);
        redis::cmd("DEL").arg(&keys).query_async::<()>(&mut con).await
    }
    .await;
    if let Err(e) = res {
        tracing::warn!("lb cache: invalidate anchor={anchor} failed: {e}");
    }
}

/// `cache.reset_all`: every `lb:cache:*` key dropped; how many were listed.
pub async fn reset_cache(state: &AppState) -> i64 {
    let Some(mut con) = state.inner.db.redis.clone() else { return 0 };
    let res: redis::RedisResult<i64> = async {
        let keys = scan(&mut con, &format!("{CACHE_PREFIX}*")).await?;
        if !keys.is_empty() {
            redis::cmd("DEL").arg(&keys).query_async::<()>(&mut con).await?;
        }
        Ok(keys.len() as i64)
    }
    .await;
    res.unwrap_or_else(|e| {
        tracing::warn!("lb cache: reset_all failed: {e}");
        0
    })
}

/// `bus.publish(type, signature, data)`'s Redis side: the compare-and-set on
/// `events:sig:<type>` and the channel message. `leaderboard` is neither a
/// webhook nor a DM-subscription type, so nothing else is queued.
pub fn event_payload(kind: &str, data: Value, ts: i64) -> String {
    pyjson::dumps_spaced(&json!({"type": kind, "data": data, "ts": ts}))
}

pub async fn publish_event(state: &AppState, kind: &str, signature: &str, data: Value) -> bool {
    let Some(mut con) = state.inner.db.redis.clone() else { return false };
    let payload = event_payload(kind, data, chrono::Utc::now().timestamp());
    let res: redis::RedisResult<bool> = async {
        let prev: Option<String> =
            redis::cmd("SET").arg(format!("events:sig:{kind}")).arg(signature).arg("GET").query_async(&mut con).await?;
        if prev.as_deref() == Some(signature) {
            return Ok(false);
        }
        redis::cmd("PUBLISH").arg(&state.settings().events_channel).arg(payload).query_async::<()>(&mut con).await?;
        Ok(true)
    }
    .await;
    res.unwrap_or_else(|e| {
        tracing::warn!("event publish failed ({kind}): {e}");
        false
    })
}

/// Wakes a Rust-owned leaderboards warmer (`RUST_OWNS=leaderboards_warmer`).
pub static WARMER_WAKE: Notify = Notify::const_new();

/// `detection.trigger_warmer()`. While Python owns the warmer it lives in a
/// worker's event loop with no outside signal, so wake it the one way
/// Python offers: `POST /v1/leaderboards/warm` as the same master (a
/// short-lived session token, so no rate limit, cooldown or usage row).
pub fn trigger_warmer(state: &AppState, user: &User) {
    if jobs::owns("leaderboards_warmer") {
        WARMER_WAKE.notify_one();
        return;
    }
    let token = jwt::create_access_token_at(
        &state.settings().secret_key,
        &user.id.to_hex(),
        user.token_version,
        None,
        chrono::Duration::seconds(60),
        chrono::Utc::now(),
    );
    let state = state.clone();
    tokio::spawn(async move {
        let uri = format!("{}/v1/leaderboards/warm", state.settings().py_upstream);
        let req = axum::http::Request::post(uri)
            .header("authorization", format!("Bearer {token}"))
            .header("content-length", "0")
            .body(axum::body::Body::empty());
        let Ok(req) = req else { return };
        match state.inner.upstream.request(req).await {
            Ok(res) if res.status().is_success() => {}
            Ok(res) => tracing::warn!("leaderboards warmer wake answered {}", res.status()),
            Err(e) => tracing::warn!("leaderboards warmer wake failed: {e}"),
        }
    });
}
