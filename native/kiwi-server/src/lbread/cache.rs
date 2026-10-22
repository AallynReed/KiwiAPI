//! `app/trove/leaderboards/cache.py`: the Redis snapshot cache in front of the
//! leaderboards reads. Same keys, TTLs and `json.dumps` payloads as Python, so
//! both servers read and refresh one cache; every Redis failure is a miss.

use std::future::Future;

use futures::{StreamExt, stream};
use kiwi_core::lbread::{DAY, TROVE_DAY_OFFSET, cap_to_ready, latest_per_day};
use serde_json::{Value, json};

use super::{
    Live, service,
    store::{Res, Source},
};
use crate::{pyjson, state::AppState};

pub const PREFIX: &str = "lb:cache:";
pub const TTL_SECONDS: i64 = 6 * 3600;
pub const TS_KEY: &str = "lb:cache:timestamps";
pub const TS_CACHE_LIMIT: i64 = 365;
pub const DAYS_KEY: &str = "lb:cache:days";
pub const DAYS_TTL: i64 = 12 * 3600;
pub const READY_KEY: &str = "lb:cache:ready_anchor";
pub const BHIST_DEFAULT_DAYS: i64 = 7;
pub const BHIST_DEFAULT_TOP: i64 = 5;
const BHIST_WARM_CONCURRENCY: usize = 6;

pub fn boards_key(anchor: i64) -> String {
    format!("{PREFIX}boards:{anchor}")
}

pub fn entries_key(anchor: i64, uuid: i64, limit: i64, offset: i64) -> String {
    format!("{PREFIX}entries:{anchor}:{uuid}:{offset}:{limit}")
}

pub fn bhist_key(uuid: i64, days: i64, top: i64) -> String {
    format!("{PREFIX}bhist:{uuid}:{days}:{top}")
}

pub fn cheaters_key(anchor: i64) -> String {
    format!("{PREFIX}cheaters:{anchor}")
}

pub fn activity_key(anchor: i64) -> String {
    format!("{PREFIX}activity:{anchor}")
}

/// The cache's view of Redis plus the one setting it reads.
pub trait Kv: Sync {
    /// `_rget`: the decoded value, `None` on a miss or any failure.
    fn get(&self, key: &str) -> impl Future<Output = Option<Value>> + Send;
    /// `_rset`: `SET key json.dumps(value) EX ttl`, failures logged and dropped.
    fn set(&self, key: &str, value: &Value, ttl: i64) -> impl Future<Output = ()> + Send;
    /// `_retention_days`: `max(1, leaderboards_hot_retention_days)`, 3 when the lookup fails.
    fn retention_days(&self) -> impl Future<Output = i64> + Send;
    /// Whether Redis is configured at all (`get_redis() is None`).
    fn configured(&self) -> bool;
}

/// Python `int(value)` for a decoded setting.
pub fn py_int(v: &Value) -> anyhow::Result<i64> {
    match v {
        Value::Bool(b) => Ok(i64::from(*b)),
        Value::Number(n) => match n.as_i64() {
            Some(i) => Ok(i),
            None => n
                .as_f64()
                .filter(|f| f.is_finite())
                .map(|f| f.trunc() as i64)
                .ok_or_else(|| anyhow::anyhow!("int({n})")),
        },
        Value::String(s) => s.trim().parse().map_err(|_| anyhow::anyhow!("int({s:?})")),
        other => anyhow::bail!("int({other})"),
    }
}

impl Kv for Live<'_> {
    async fn get(&self, key: &str) -> Option<Value> {
        let mut conn = self.0.inner.db.redis.clone()?;
        let raw: Option<Vec<u8>> = match redis::cmd("GET").arg(key).query_async(&mut conn).await {
            Ok(raw) => raw,
            Err(e) => {
                tracing::warn!("lb cache: get {key} failed: {e}");
                return None;
            }
        };
        match pyjson::loads(&raw?) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!("lb cache: get {key} failed: {e:?}");
                None
            }
        }
    }

    async fn set(&self, key: &str, value: &Value, ttl: i64) {
        let Some(mut conn) = self.0.inner.db.redis.clone() else { return };
        let res: redis::RedisResult<()> =
            redis::cmd("SET").arg(key).arg(pyjson::dumps_spaced(value)).arg("EX").arg(ttl).query_async(&mut conn).await;
        if let Err(e) = res {
            tracing::warn!("lb cache: set {key} failed: {e}");
        }
    }

    async fn retention_days(&self) -> i64 {
        match crate::runtime_config::get_setting(self.0, "leaderboards_hot_retention_days").await {
            Ok(v) => py_int(&v).map(|d| d.max(1)).unwrap_or(3),
            Err(_) => 3,
        }
    }

    fn configured(&self) -> bool {
        self.0.inner.db.redis.is_some()
    }
}

fn int_list(v: &Value) -> Res<Vec<i64>> {
    v.as_array()
        .and_then(|a| a.iter().map(Value::as_i64).collect::<Option<Vec<i64>>>())
        .ok_or_else(|| anyhow::anyhow!("cached anchor list is not a list of ints"))
}

/// `get_ready_anchor`: only an int (or a bool, which Python's `isinstance` counts) is a pointer.
pub async fn get_ready_anchor<K: Kv>(kv: &K) -> Option<i64> {
    match kv.get(READY_KEY).await? {
        Value::Bool(b) => Some(i64::from(b)),
        v => v.as_i64(),
    }
}

/// `get_timestamps`: the cached anchor list, held at the published ready anchor.
pub async fn get_timestamps<S: Source, K: Kv>(src: &S, kv: &K, limit: usize) -> Res<Vec<i64>> {
    let cached = match kv.get(TS_KEY).await {
        Some(v) => int_list(&v)?,
        None => {
            let fresh = src.list_timestamps(TS_CACHE_LIMIT, None).await?;
            kv.set(TS_KEY, &json!(fresh), TTL_SECONDS).await;
            fresh
        }
    };
    let mut out = cap_to_ready(cached, get_ready_anchor(kv).await);
    out.truncate(limit);
    Ok(out)
}

/// `get_boards`: the `list_boards_at` dicts for one anchor.
pub async fn get_boards<S: Source, K: Kv>(src: &S, kv: &K, anchor: i64) -> Res<Value> {
    if let Some(cached) = kv.get(&boards_key(anchor)).await {
        return Ok(cached);
    }
    let fresh = Value::Array(src.list_boards_at(anchor).await?.iter().map(|b| b.to_value()).collect());
    kv.set(&boards_key(anchor), &fresh, TTL_SECONDS).await;
    Ok(fresh)
}

/// `get_entries`: `(items, total, comparison)` as cached.
pub async fn get_entries<S: Source, K: Kv>(
    src: &S,
    kv: &K,
    uuid: i32,
    anchor: i64,
    limit: i64,
    offset: i64,
) -> Res<(Value, Value, Value)> {
    let key = entries_key(anchor, i64::from(uuid), limit, offset);
    if let Some(cached) = kv.get(&key).await {
        let field = |k: &str| cached.get(k).cloned().ok_or_else(|| anyhow::anyhow!("cached entries lack {k:?}"));
        let comparison = cached.get("comparison").cloned().unwrap_or_else(|| service::Comparison::NO_PRIOR.to_value());
        return Ok((field("items")?, field("total")?, comparison));
    }
    let page = service::list_entries_with_deltas(src, uuid, anchor, limit, offset).await?;
    let payload = page.cache_value();
    kv.set(&key, &payload, TTL_SECONDS).await;
    Ok((payload["items"].clone(), payload["total"].clone(), payload["comparison"].clone()))
}

/// `get_board_history`: the per-board chart, read-through.
pub async fn get_board_history<S: Source, K: Kv>(
    src: &S,
    kv: &K,
    uuid: i32,
    days: i64,
    top: i64,
    now: i64,
) -> Res<Value> {
    let key = bhist_key(i64::from(uuid), days, top);
    if let Some(cached) = kv.get(&key).await {
        return Ok(cached);
    }
    let fresh = service::board_history(src, uuid, days, top, now).await?;
    let ttl = kv.retention_days().await * DAY;
    kv.set(&key, &fresh, ttl).await;
    Ok(fresh)
}

/// `get_days`: the latest anchor per trove-day, newest first.
pub async fn get_days<S: Source, K: Kv>(src: &S, kv: &K, limit: i64) -> Res<Vec<i64>> {
    if let Some(cached) = kv.get(DAYS_KEY).await {
        let mut days = int_list(&cached)?;
        days.truncate(usize::try_from(limit).unwrap_or(0));
        return Ok(days);
    }
    let days = src.list_days(limit).await?;
    kv.set(DAYS_KEY, &json!(days), DAYS_TTL).await;
    Ok(days)
}

// --- warm and publish (the warmer's side) -------------------------------------

pub async fn set_board_history<K: Kv>(kv: &K, uuid: i64, days: i64, top: i64, payload: &Value) {
    let ttl = kv.retention_days().await * DAY;
    kv.set(&bhist_key(uuid, days, top), payload, ttl).await;
}

/// `set_ready_anchor`: publish `anchor` to the page.
pub async fn set_ready_anchor<K: Kv>(kv: &K, anchor: i64) {
    let ttl = kv.retention_days().await * DAY;
    kv.set(READY_KEY, &json!(anchor), ttl).await;
}

/// `warm`: rewrite the anchor list, then the boards and first page of the
/// newest capture of each of the last `hot_retention_days` trove-days.
pub async fn warm<S: Source, K: Kv>(src: &S, kv: &K) -> Res<()> {
    let timestamps = src.list_timestamps(TS_CACHE_LIMIT, None).await?;
    kv.set(TS_KEY, &json!(timestamps), TTL_SECONDS).await;
    if timestamps.is_empty() {
        return Ok(());
    }
    let days = usize::try_from(kv.retention_days().await).unwrap_or(1);
    for anchor in latest_per_day(&timestamps, days) {
        let boards = src.list_boards_at(anchor).await?;
        let value = Value::Array(boards.iter().map(|b| b.to_value()).collect());
        kv.set(&boards_key(anchor), &value, TTL_SECONDS).await;
        if let Some(first) = boards.first() {
            let uuid = i32::try_from(first.uuid)?;
            let page = service::list_entries_with_deltas(src, uuid, anchor, 100, 0).await?;
            kv.set(&entries_key(anchor, first.uuid, 100, 0), &page.cache_value(), TTL_SECONDS).await;
        }
    }
    Ok(())
}

/// `warm_days`: rebuild the per-day list only when it is missing or a new trove-day began.
pub async fn warm_days<S: Source, K: Kv>(src: &S, kv: &K) -> Res<()> {
    let latest = src.list_timestamps(1, None).await?;
    let Some(&newest) = latest.first() else { return Ok(()) };
    let day = |a: i64| (a - TROVE_DAY_OFFSET).div_euclid(DAY);
    if let Some(cached) = kv.get(DAYS_KEY).await
        && let Some(first) = cached.as_array().and_then(|a| a.first())
        && first.as_i64().is_some_and(|c| day(c) == day(newest))
    {
        return Ok(());
    }
    let days = src.list_days(400).await?;
    kv.set(DAYS_KEY, &json!(days), DAYS_TTL).await;
    Ok(())
}

/// `warm_board_histories`: the default chart (7d / top 5) of every board at
/// `anchor`, six at a time; one board failing never stops the rest.
pub async fn warm_board_histories<S: Source, K: Kv>(
    src: &S,
    kv: &K,
    anchor: i64,
    now: impl Fn() -> i64 + Sync,
) -> Res<usize> {
    if !kv.configured() {
        return Ok(0);
    }
    let boards = get_boards(src, kv, anchor).await?;
    let uuids: Vec<i64> =
        boards.as_array().map(|a| a.iter().filter_map(|b| b["uuid"].as_i64()).collect()).unwrap_or_default();
    let now = &now;
    let warmed = stream::iter(uuids)
        .map(|uuid| async move {
            let res = async {
                let payload =
                    service::board_history(src, i32::try_from(uuid)?, BHIST_DEFAULT_DAYS, BHIST_DEFAULT_TOP, now())
                        .await?;
                set_board_history(kv, uuid, BHIST_DEFAULT_DAYS, BHIST_DEFAULT_TOP, &payload).await;
                anyhow::Ok(())
            }
            .await;
            if let Err(e) = &res {
                tracing::warn!("lb cache: warm board history uuid={uuid} failed: {e:#}");
            }
            res.is_ok()
        })
        .buffer_unordered(BHIST_WARM_CONCURRENCY)
        .filter(|ok| std::future::ready(*ok))
        .count()
        .await;
    Ok(warmed)
}

// --- invalidation (ingest / admin) ---------------------------------------------

async fn scan(conn: &mut redis::aio::ConnectionManager, pattern: &str) -> redis::RedisResult<Vec<String>> {
    let mut keys = Vec::new();
    let mut cursor: u64 = 0;
    loop {
        let (next, batch): (u64, Vec<String>) =
            redis::cmd("SCAN").arg(cursor).arg("MATCH").arg(pattern).query_async(conn).await?;
        keys.extend(batch);
        if next == 0 {
            return Ok(keys);
        }
        cursor = next;
    }
}

async fn delete_keys(state: &AppState, fixed: Vec<String>, patterns: &[String], what: &str) -> usize {
    let Some(mut conn) = state.inner.db.redis.clone() else { return 0 };
    let res = async {
        let mut keys = fixed;
        for p in patterns {
            keys.extend(scan(&mut conn, p).await?);
        }
        if !keys.is_empty() {
            let _: i64 = redis::cmd("DEL").arg(&keys).query_async(&mut conn).await?;
        }
        redis::RedisResult::Ok(keys.len())
    }
    .await;
    res.unwrap_or_else(|e| {
        tracing::warn!("lb cache: {what} failed: {e}");
        0
    })
}

/// `invalidate_anchor`: every key of one anchor, after an ingest touched it.
pub async fn invalidate_anchor(state: &AppState, anchor: i64) {
    let fixed = vec![boards_key(anchor), cheaters_key(anchor), activity_key(anchor)];
    delete_keys(state, fixed, &[format!("{PREFIX}entries:{anchor}:*")], "invalidate anchor").await;
}

/// `invalidate_views`: the page-view snapshots and the ready pointer.
pub async fn invalidate_views(state: &AppState) -> usize {
    let patterns = ["boards:*", "entries:*", "bhist:*"].map(|p| format!("{PREFIX}{p}"));
    delete_keys(state, vec![TS_KEY.into(), READY_KEY.into()], &patterns, "invalidate views").await
}

/// `invalidate_cheaters`.
pub async fn invalidate_cheaters(state: &AppState) -> usize {
    delete_keys(state, Vec::new(), &[format!("{PREFIX}cheaters:*")], "invalidate cheaters").await
}

/// `invalidate_all_activity` (also the class-activity keys).
pub async fn invalidate_all_activity(state: &AppState) -> usize {
    delete_keys(state, Vec::new(), &[format!("{PREFIX}activity*")], "invalidate all activity").await
}

/// `reset_all`: every `lb:cache:*` key.
pub async fn reset_all(state: &AppState) -> usize {
    delete_keys(state, Vec::new(), &[format!("{PREFIX}*")], "reset_all").await
}
