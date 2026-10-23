//! What an ingest leaves behind besides the rows: the master ingest log
//! (`app/admin/ingest_log.py`), the Redis snapshot invalidation and reset
//! (`cache.py`), the `leaderboard` event (`app/events/bus.py`), and the warmer
//! wake-up.

use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use serde_json::{Value, json};

use crate::{
    auth::models::{ApiToken, User},
    dt, pyjson,
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

/// The channel message `bus.publish` sends.
pub fn event_payload(kind: &str, data: Value, ts: i64) -> String {
    pyjson::dumps_spaced(&json!({"type": kind, "data": data, "ts": ts}))
}

/// `bus.publish(type, signature, data)` on the events bus.
pub async fn publish_event(state: &AppState, kind: &str, signature: &str, data: Value) -> bool {
    crate::events::publish(state, kind, signature, data).await
}
