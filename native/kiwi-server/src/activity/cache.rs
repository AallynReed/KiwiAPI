//! The activity keys of `app/trove/leaderboards/cache.py`: values are Python
//! `json.dumps` text, every operation best-effort.

use serde_json::Value;

use crate::{auth::pyutil::int_of, pyjson, runtime_config, state::AppState};

pub const PREFIX: &str = "lb:cache:";
pub const READY_KEY: &str = "lb:cache:ready_anchor";
pub const CLASS_CURRENT_KEY: &str = "lb:cache:activity_class_current";
/// `_ACTIVITY_SERIES_TTL`.
pub const SERIES_TTL: i64 = 300;

pub fn activity_key(anchor: i64) -> String {
    format!("{PREFIX}activity:{anchor}")
}

pub fn series_key(period: &str) -> String {
    format!("{PREFIX}activity_series:{period}")
}

pub fn class_series_key(period: &str) -> String {
    format!("{PREFIX}activity_class_series:{period}")
}

/// `_rget`: the decoded value, `None` on a miss or any failure.
pub async fn get(state: &AppState, key: &str) -> Option<Value> {
    let mut conn = state.inner.db.redis.clone()?;
    let raw: Option<Vec<u8>> = match redis::cmd("GET").arg(key).query_async(&mut conn).await {
        Ok(raw) => raw,
        Err(e) => {
            tracing::warn!("lb cache: get {key} failed: {e}");
            return None;
        }
    };
    let text = String::from_utf8(raw?).ok()?;
    if text.starts_with('\u{feff}') {
        return None;
    }
    pyjson::loads(text.as_bytes()).ok()
}

/// `_rset`: `SET key json.dumps(value) EX ttl`.
pub async fn set(state: &AppState, key: &str, value: &Value, ttl: i64) {
    let Some(mut conn) = state.inner.db.redis.clone() else { return };
    let r: redis::RedisResult<()> =
        redis::cmd("SET").arg(key).arg(pyjson::dumps_spaced(value)).arg("EX").arg(ttl).query_async(&mut conn).await;
    if let Err(e) = r {
        tracing::warn!("lb cache: set {key} failed: {e}");
    }
}

/// `_retention_seconds()`: hot-retention days (min 1, else 3) in seconds.
pub async fn retention_seconds(state: &AppState) -> i64 {
    let days = match runtime_config::get_setting(state, "leaderboards_hot_retention_days").await {
        Ok(v) => int_of(&v).map(|d| d.clamp(1, i128::from(i64::MAX / 86_400)) as i64).unwrap_or(3),
        Err(_) => 3,
    };
    days * 86_400
}

/// `get_ready_anchor()`: only an int (a bool counts, as in Python).
pub async fn ready_anchor(state: &AppState) -> Option<i64> {
    match get(state, READY_KEY).await? {
        Value::Number(n) => n.as_i64(),
        Value::Bool(b) => Some(i64::from(b)),
        _ => None,
    }
}

pub async fn get_activity(state: &AppState, anchor: i64) -> Option<Value> {
    get(state, &activity_key(anchor)).await
}

pub async fn set_activity(state: &AppState, anchor: i64, payload: &Value) {
    let ttl = retention_seconds(state).await;
    set(state, &activity_key(anchor), payload, ttl).await;
}

/// `invalidate_all_activity()`: drop every `lb:cache:activity*` key.
pub async fn invalidate_all_activity(state: &AppState) -> usize {
    let Some(mut conn) = state.inner.db.redis.clone() else { return 0 };
    let pattern = format!("{PREFIX}activity*");
    let mut keys: Vec<Vec<u8>> = Vec::new();
    let mut cursor: u64 = 0;
    loop {
        let r: redis::RedisResult<(u64, Vec<Vec<u8>>)> =
            redis::cmd("SCAN").arg(cursor).arg("MATCH").arg(&pattern).query_async(&mut conn).await;
        match r {
            Ok((next, batch)) => {
                keys.extend(batch);
                cursor = next;
                if cursor == 0 {
                    break;
                }
            }
            Err(e) => {
                tracing::warn!("lb cache: invalidate all activity failed: {e}");
                return 0;
            }
        }
    }
    if !keys.is_empty() {
        let r: redis::RedisResult<()> = redis::cmd("DEL").arg(&keys).query_async(&mut conn).await;
        if let Err(e) = r {
            tracing::warn!("lb cache: invalidate all activity failed: {e}");
            return 0;
        }
    }
    keys.len()
}
