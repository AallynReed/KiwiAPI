//! `app/core/ratelimit.py`: a Redis sliding window (same `rl:{key}` ZSETs and
//! Lua script, so Python and Rust share budgets), a Mongo fixed-window
//! fallback in `rate_limit_buckets` when Redis is unset, and the exact 429.

use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        LazyLock,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    http::{HeaderName, HeaderValue, StatusCode},
    response::{IntoResponseParts, ResponseParts},
};
use mongodb::{
    bson::{doc, DateTime, Document},
    options::ReturnDocument,
};

use crate::{auth::Rejection, state::AppState};

pub const RATE_LIMIT_COLLECTION: &str = "rate_limit_buckets";

pub const SLIDING_WINDOW_LUA: &str = "
local key = KEYS[1]
local now = tonumber(ARGV[1])
local window = tonumber(ARGV[2])
local limit = tonumber(ARGV[3])
local member = ARGV[4]
redis.call('ZREMRANGEBYSCORE', key, 0, now - window)
redis.call('ZADD', key, now, member)
local count = redis.call('ZCARD', key)
redis.call('PEXPIRE', key, math.ceil(window * 1000))
if count > limit then
  -- This request is over budget; don't let it count against future windows.
  redis.call('ZREM', key, member)
end
return count
";

static SLIDING_WINDOW: LazyLock<redis::Script> = LazyLock::new(|| redis::Script::new(SLIDING_WINDOW_LUA));

/// Limit state after a passing check; also the `X-RateLimit-*` headers Python
/// adds to successful model/dict responses (return it as a response part).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateLimitInfo {
    pub limit: i64,
    pub remaining: i64,
    /// `int(reset.timestamp())`.
    pub reset: i64,
}

impl RateLimitInfo {
    pub fn headers(&self) -> [(HeaderName, HeaderValue); 3] {
        [
            (HeaderName::from_static("x-ratelimit-limit"), HeaderValue::from(self.limit)),
            (HeaderName::from_static("x-ratelimit-remaining"), HeaderValue::from(self.remaining)),
            (HeaderName::from_static("x-ratelimit-reset"), HeaderValue::from(self.reset)),
        ]
    }
}

impl IntoResponseParts for RateLimitInfo {
    type Error = std::convert::Infallible;

    fn into_response_parts(self, mut res: ResponseParts) -> Result<ResponseParts, Self::Error> {
        for (name, value) in self.headers() {
            res.headers_mut().insert(name, value);
        }
        Ok(res)
    }
}

/// The 429 `rate_limited` envelope with `X-RateLimit-*` and `Retry-After`.
pub fn too_many(info: &RateLimitInfo, window_seconds: i64, retry_after: i64) -> Rejection {
    let mut r = Rejection::new(
        StatusCode::TOO_MANY_REQUESTS,
        "rate_limited",
        "Rate limit exceeded. Slow down and try again later.",
    )
    .with_details(serde_json::json!({ "limit": info.limit, "window_seconds": window_seconds }));
    r.headers.extend(info.headers());
    r.with_header("retry-after", retry_after.max(1).to_string())
}

fn now_micros() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_micros() as i64).unwrap_or(0)
}

/// `datetime.fromtimestamp(t)` keeps microseconds, rounded half-even.
fn micros_of(t: f64) -> i64 {
    (t * 1e6).round_ties_even() as i64
}

static SEQ: AtomicU64 = AtomicU64::new(0);

/// Unique-per-process ZSET member suffix (Python's `_counter`).
fn next_seq() -> u64 {
    SEQ.fetch_add(1, Ordering::Relaxed) % 1_000_000 + 1
}

/// Python `str(float)` for an epoch timestamp (shortest round-trip repr).
pub fn py_float_str(x: f64) -> String {
    let s = format!("{x}");
    if s.contains('.') || s.contains('e') || s.contains("inf") || s.contains("NaN") {
        s
    } else {
        format!("{s}.0")
    }
}

/// Header and Retry-After math of the Redis path, split out for tests:
/// `now` is when the script ran, `now2_us` when the 429 was built.
pub fn redis_outcome(
    now: f64,
    window: i64,
    max: i64,
    count: i64,
    now2_us: i64,
) -> Result<RateLimitInfo, (RateLimitInfo, i64)> {
    let reset_us = micros_of(now + window as f64);
    let info = RateLimitInfo { limit: max, remaining: (max - count).max(0), reset: reset_us.div_euclid(1_000_000) };
    if count > max {
        return Err((info, (reset_us - now2_us) / 1_000_000));
    }
    Ok(info)
}

/// `check_rate_limit(key, max_requests, window_seconds)`.
pub async fn check_rate_limit(state: &AppState, key: &str, max: i64, window: i64) -> Result<RateLimitInfo, Rejection> {
    match state.inner.db.redis.clone() {
        Some(mut conn) => {
            let now = now_micros() as f64 / 1e6;
            let member = format!("{now:.6}:{}", next_seq());
            let count: i64 = SLIDING_WINDOW
                .key(format!("rl:{key}"))
                .arg(py_float_str(now))
                .arg(window.to_string())
                .arg(max.to_string())
                .arg(member)
                .invoke_async(&mut conn)
                .await
                .map_err(Rejection::internal)?;
            redis_outcome(now, window, max, count, now_micros()).map_err(|(info, retry)| too_many(&info, window, retry))
        }
        None => mongo_fixed_window(state, key, max, window).await,
    }
}

/// Window start and end (both epoch seconds) of the fixed window holding `now_us`.
pub fn fixed_window(now_us: i64, window: i64) -> (i64, i64) {
    let start = (now_us as f64 / 1e6 / window as f64).floor() as i64 * window;
    (start, start + window)
}

async fn mongo_fixed_window(state: &AppState, key: &str, max: i64, window: i64) -> Result<RateLimitInfo, Rejection> {
    let db = state.inner.db.mongo.as_ref().ok_or_else(|| Rejection::internal("rate limit: no Redis and no Mongo"))?;
    let now_us = now_micros();
    let (start, reset) = fixed_window(now_us, window);
    let doc = db
        .collection::<Document>(RATE_LIMIT_COLLECTION)
        .find_one_and_update(
            doc! { "_id": format!("{key}:{start}") },
            doc! { "$inc": { "count": 1_i32 }, "$setOnInsert": { "expires_at": DateTime::from_millis(reset * 1000) } },
        )
        .upsert(true)
        .return_document(ReturnDocument::After)
        .await
        .map_err(Rejection::internal)?;
    let count =
        doc.and_then(|d| d.get("count").and_then(|c| c.as_i32().map(i64::from).or_else(|| c.as_i64()))).unwrap_or(1);
    let info = RateLimitInfo { limit: max, remaining: (max - count).max(0), reset };
    if count > max {
        return Err(too_many(&info, window, (reset * 1_000_000 - now_us) / 1_000_000));
    }
    Ok(info)
}

/// Registry defaults (`app/core/config.py`) for the families
/// `runtime_config.get_rate_limit` serves; used only if the lookup fails.
fn default_limit(prefix: &str) -> (i64, i64) {
    use crate::auth::settings::env_i64;
    let (max, window) = match prefix {
        "api_rate_limit" => (120, 60),
        "public_anon_rate_limit" => (30, 60),
        "signup_rate_limit" => (5, 3600),
        "login_rate_limit" => (10, 300),
        "forgot_password_rate_limit" => (5, 3600),
        "ingest_cooldown" => (1, 300),
        "leaderboards_archive_rate_limit" | "market_archive_rate_limit" => (10, 60),
        _ => (60, 60),
    };
    let up = prefix.to_ascii_uppercase();
    (env_i64(&format!("{up}_MAX"), max), env_i64(&format!("{up}_WINDOW_SECONDS"), window))
}

/// `runtime_config.get_rate_limit(prefix)`: (`{prefix}_max`, `{prefix}_window_seconds`).
pub async fn get_rate_limit(state: &AppState, prefix: &str) -> (i64, i64) {
    let (dmax, dwin) = default_limit(prefix);
    let max = setting_i64(state, &format!("{prefix}_max"), dmax).await;
    let window = setting_i64(state, &format!("{prefix}_window_seconds"), dwin).await;
    (max, window)
}

async fn setting_i64(state: &AppState, key: &str, default: i64) -> i64 {
    match crate::runtime_config::get_setting(state, key).await {
        Ok(v) => v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)).unwrap_or(default),
        Err(err) => {
            tracing::warn!("runtime_config {key}: {err}; using default {default}");
            default
        }
    }
}
