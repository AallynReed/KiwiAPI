//! `app/tokens/usage.py`: coalesced `request_count` / `last_used_at` writes.
//! The first request per interval flushes the Redis-accumulated delta.

use std::sync::LazyLock;

use mongodb::bson::{doc, oid::ObjectId, DateTime, Document};

use super::{models::API_TOKENS, pyutil::bson_int};
use crate::state::AppState;

pub const FLUSH_LUA: &str = "
local pending = KEYS[1]
local gate = KEYS[2]
local interval = tonumber(ARGV[1])
local n = redis.call('INCR', pending)
local claimed = redis.call('SET', gate, '1', 'NX', 'EX', interval)
if claimed then
  redis.call('SET', pending, '0')
  redis.call('EXPIRE', pending, interval * 4)
  return n
end
return -1
";

static FLUSH: LazyLock<redis::Script> = LazyLock::new(|| redis::Script::new(FLUSH_LUA));

/// `record_token_use(token, ip)`; the IP is deliberately never stored.
pub async fn record_token_use(state: &AppState, token_id: ObjectId) {
    let now = DateTime::now();
    let delta = match state.inner.db.redis.clone() {
        None => 1,
        Some(mut conn) => {
            let interval = super::auth_settings().token_touch_interval_seconds;
            let res: redis::RedisResult<i64> = FLUSH
                .key(format!("tok:pending:{token_id}"))
                .key(format!("tok:gate:{token_id}"))
                .arg(interval.to_string())
                .invoke_async(&mut conn)
                .await;
            res.unwrap_or_else(|err| {
                tracing::warn!("token-use debounce failed; writing directly: {err}");
                1
            })
        }
    };
    if delta <= 0 {
        return;
    }
    let Some(db) = state.inner.db.mongo.as_ref() else {
        return;
    };
    let update = doc! { "$inc": { "request_count": bson_int(delta) }, "$set": { "last_used_at": now } };
    if let Err(err) = db.collection::<Document>(API_TOKENS).update_one(doc! { "_id": token_id }, update).await {
        tracing::warn!("token-use write failed: {err}");
    }
}
