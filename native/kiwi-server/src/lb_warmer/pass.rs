//! One warmer pass (`_warm_all`), the boot freshness probe
//! (`_persisted_pass_age`) and the loop's cadence (`_warmup_loop`).

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use serde_json::Value;

use super::{guarded, off_executor};
use crate::{
    activity, cheaters,
    features::py_truthy,
    lb_ingest,
    lbread::{self, store::Source},
    routes::lb_names,
    runtime_config,
    state::AppState,
};

pub const COLD_TIER_PERIOD_SECONDS: f64 = 86400.0;
pub const COLD_TIER_PER_PASS: usize = 3;
const DEFAULT_TTL: f64 = 1800.0;

/// The steps of a pass, in `_warm_all`'s order.
#[allow(async_fn_in_trait)]
pub trait Ops: Sync {
    fn now(&self) -> f64;
    async fn detect(&self) -> anyhow::Result<Arc<Value>>;
    async fn activity(&self) -> anyhow::Result<()>;
    async fn class_activity(&self) -> anyhow::Result<()>;
    async fn renames(&self) -> anyhow::Result<()>;
    async fn duplicates(&self) -> anyhow::Result<()>;
    async fn warm_cache(&self) -> anyhow::Result<()>;
    /// `list_timestamps(limit=1, include_archive=False)[0]`.
    async fn latest(&self) -> anyhow::Result<Option<i64>>;
    async fn set_ready_anchor(&self, anchor: i64);
    async fn warm_board_histories(&self, anchor: i64) -> anyhow::Result<usize>;
    async fn warm_days(&self) -> anyhow::Result<()>;
    fn postgres_enabled(&self) -> bool;
    async fn tier_after_days(&self) -> anyhow::Result<Value>;
    /// The partitions moved.
    async fn tier_cold(&self, after_days: i64, now: i64, limit: usize) -> anyhow::Result<Vec<String>>;
}

/// What the boot freshness probe reads.
#[allow(async_fn_in_trait)]
pub trait Probe: Sync {
    fn now(&self) -> f64;
    async fn ready_anchor(&self) -> Option<i64>;
    async fn persisted(&self, anchor: i64) -> Option<Value>;
    async fn timestamps(&self) -> anyhow::Result<Vec<i64>>;
    async fn ttl(&self) -> anyhow::Result<Value>;
}

/// What one loop iteration needs.
#[allow(async_fn_in_trait)]
pub trait Driver: Sync {
    fn now(&self) -> f64;
    async fn resume_age(&self) -> Option<f64>;
    async fn pass(&self) -> anyhow::Result<()>;
    async fn ttl(&self) -> anyhow::Result<Value>;
}

/// Python `float(v)`; None where it raises.
pub fn py_float(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        Value::String(s) => kiwi_core::pynum::float(s),
        _ => None,
    }
}

/// Python `int(v)`; None where it raises.
pub fn py_int(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().filter(|f| f.is_finite()).map(|f| f.trunc() as i64)),
        Value::Bool(b) => Some(i64::from(*b)),
        Value::String(s) => kiwi_core::pynum::int(s),
        _ => None,
    }
}

async fn soft<T>(what: &str, fut: impl std::future::Future<Output = anyhow::Result<T>>) -> Option<T> {
    match guarded(fut).await {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::error!("leaderboards warmer: {what} failed (non-fatal): {e:#}");
            None
        }
    }
}

/// `_warm_all`: detection, activity, class activity, renames, duplicates (each
/// non-fatal), the page snapshot, the publish, the board charts, the day list
/// and the cold-tier drip. `last_cold` is `_last_cold_tier_ts`.
pub async fn warm_all<O: Ops>(ops: &O, last_cold: &mut f64) -> anyhow::Result<()> {
    let mut steps: Vec<(&str, f64)> = Vec::new();
    let mut t = std::time::Instant::now();
    let mut lap = |name: &'static str, steps: &mut Vec<(&str, f64)>| {
        steps.push((name, t.elapsed().as_secs_f64()));
        t = std::time::Instant::now();
    };
    let res = soft("cheater/cluster detection", ops.detect()).await;
    lap("detect", &mut steps);
    soft("activity estimate", ops.activity()).await;
    lap("activity", &mut steps);
    soft("class activity estimate", ops.class_activity()).await;
    lap("class_activity", &mut steps);
    soft("rename detection", ops.renames()).await;
    lap("renames", &mut steps);
    soft("duplicate detection", ops.duplicates()).await;
    lap("duplicates", &mut steps);
    guarded(ops.warm_cache()).await?;
    lap("cache", &mut steps);
    let mut anchor = res.as_deref().and_then(|r| match r {
        Value::Object(m) => m.get("anchor").and_then(Value::as_i64),
        _ => None,
    });
    if anchor.is_none() {
        anchor = guarded(ops.latest()).await?;
    }
    if let Some(a) = anchor {
        ops.set_ready_anchor(a).await;
        if let Some(n) = soft("board-history pre-warm", ops.warm_board_histories(a)).await {
            tracing::info!("leaderboards warmer: pre-warmed {n} board-history charts");
        }
    }
    lap("histories", &mut steps);
    soft("day-list warm", ops.warm_days()).await;
    lap("days", &mut steps);
    tier_cold(ops, last_cold).await;
    lap("cold", &mut steps);
    let parts: Vec<String> = steps.iter().map(|(n, s)| format!("{n} {s:.2}s")).collect();
    tracing::info!("leaderboards warmer: steps {}", parts.join(", "));
    Ok(())
}

/// `_maybe_tier_cold_partitions`: at most once a day, a few partitions a pass.
async fn tier_cold<O: Ops>(ops: &O, last_cold: &mut f64) {
    if !ops.postgres_enabled() {
        return;
    }
    let now = ops.now();
    if now - *last_cold < COLD_TIER_PERIOD_SECONDS {
        return;
    }
    *last_cold = now;
    let moved = soft("cold-tier move", async {
        let raw = ops.tier_after_days().await?;
        let after = py_int(&raw).ok_or_else(|| anyhow::anyhow!("int({raw})"))?;
        ops.tier_cold(after, now as i64, COLD_TIER_PER_PASS).await
    })
    .await;
    if let Some(moved) = moved.filter(|m| !m.is_empty()) {
        tracing::info!("leaderboards warmer: cold-tiered {} partition(s) ({})", moved.len(), moved.join(", "));
    }
}

/// `_persisted_pass_age`: how old the published anchor's persisted pass is,
/// when it is still inside the TTL and no newer capture has landed.
pub async fn persisted_pass_age<P: Probe>(p: &P) -> Option<f64> {
    let probe = async {
        let Some(anchor) = p.ready_anchor().await else { return Ok(None) };
        let Some(persisted) = p.persisted(anchor).await.filter(py_truthy) else { return Ok(None) };
        if p.timestamps().await?.first().is_some_and(|&latest| latest > anchor) {
            return Ok(None);
        }
        let raw = p.ttl().await?;
        let ttl = py_float(&raw).ok_or_else(|| anyhow::anyhow!("float({raw})"))?;
        let Value::Object(m) = &persisted else { anyhow::bail!("the persisted pass is not a dict") };
        let computed_at = match m.get("computed_at").filter(|v| py_truthy(v)) {
            None => 0.0,
            Some(v) => py_float(v).ok_or_else(|| anyhow::anyhow!("float({v})"))?,
        };
        let age = p.now() - computed_at;
        anyhow::Ok((0.0 <= age && age < ttl).then_some(age))
    };
    probe.await.unwrap_or_else(|e| {
        tracing::warn!("leaderboards warmer: freshness probe failed - running the full pass: {e:#}");
        None
    })
}

/// `max(30.0, ttl - elapsed - (resumed or 0.0))`.
pub fn next_sleep(ttl: f64, elapsed: f64, resumed: Option<f64>) -> f64 {
    let v = ttl - elapsed - resumed.unwrap_or(0.0);
    if v > 30.0 { v } else { 30.0 }
}

/// One `_warmup_loop` iteration up to the sleep; returns the sleep in seconds.
pub async fn iteration<D: Driver>(d: &D, first_pass: bool) -> f64 {
    let start = d.now();
    let resumed = if first_pass { d.resume_age().await } else { None };
    match resumed {
        Some(age) => tracing::info!(
            "leaderboards warmer: resuming - the published anchor's persisted pass is {age:.0}s old, \
             nothing has landed since (boot pass skipped)"
        ),
        None => match guarded(d.pass()).await {
            Ok(()) => tracing::info!("leaderboards warmer: caches refreshed in {:.2}s", d.now() - start),
            Err(e) => tracing::error!("leaderboards warmer: iteration failed: {e:#}"),
        },
    }
    let ttl = d.ttl().await.ok().as_ref().and_then(py_float).unwrap_or(DEFAULT_TTL);
    next_sleep(ttl, d.now() - start, resumed)
}

/// The warmer's widest fan-out (the co-movement loads).
const WARMER_PG_MAX: usize = 8;

/// The state a pass runs on: its own Postgres connections, so it never queues
/// behind request traffic on the shared pool (each Python worker had a pool of
/// its own), planning every statement for its actual parameters as asyncpg's
/// short-lived statement cache did.
fn warmer_state(state: &AppState) -> AppState {
    match crate::db::pg_pool(WARMER_PG_MAX, Some("-c plan_cache_mode=force_custom_plan")) {
        Ok(Some(pool)) => state.with_pg(pool),
        Ok(None) => state.clone(),
        Err(e) => {
            tracing::warn!("leaderboards warmer: own Postgres pool unavailable, sharing the app's: {e:#}");
            state.clone()
        }
    }
}

/// The loop: a short boot delay, then a pass every TTL or as soon as
/// [`super::trigger`] wakes it.
pub async fn run(state: AppState) {
    let warm = Warm(warmer_state(&state));
    tokio::time::sleep(Duration::from_secs(2)).await;
    let mut first_pass = true;
    loop {
        let secs = iteration(&warm, first_pass).await;
        first_pass = false;
        let wake = cheaters::store::warmer_wake().notified();
        match Duration::try_from_secs_f64(secs) {
            Ok(d) => {
                let _ = tokio::time::timeout(d, wake).await;
            }
            Err(_) => wake.await,
        }
    }
}

// ---------------------------------------------------------------------------
// Production

/// `_last_cold_tier_ts`.
static LAST_COLD: Mutex<f64> = Mutex::new(0.0);

pub fn unix_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// The warmer over the live stores.
#[derive(Clone)]
pub struct Warm(pub AppState);

impl Ops for Warm {
    fn now(&self) -> f64 {
        unix_now()
    }

    async fn detect(&self) -> anyhow::Result<Arc<Value>> {
        cheaters::warm(&self.0).await
    }

    async fn activity(&self) -> anyhow::Result<()> {
        activity::player::estimate_active_players(&self.0, true).await.map(drop)
    }

    async fn class_activity(&self) -> anyhow::Result<()> {
        activity::class::estimate_class_activity(&self.0).await.map(drop)
    }

    async fn renames(&self) -> anyhow::Result<()> {
        lb_names::live::renames(&self.0, false).await.map(drop)
    }

    async fn duplicates(&self) -> anyhow::Result<()> {
        lb_names::live::duplicates(&self.0, false).await.map(drop)
    }

    async fn warm_cache(&self) -> anyhow::Result<()> {
        let l = lbread::Live(&self.0);
        lbread::cache::warm(&l, &l).await
    }

    async fn latest(&self) -> anyhow::Result<Option<i64>> {
        Ok(lbread::Live(&self.0).list_timestamps(1, None).await?.first().copied())
    }

    async fn set_ready_anchor(&self, anchor: i64) {
        lbread::cache::set_ready_anchor(&lbread::Live(&self.0), anchor).await;
    }

    async fn warm_board_histories(&self, anchor: i64) -> anyhow::Result<usize> {
        let l = lbread::Live(&self.0);
        lbread::cache::warm_board_histories(&l, &l, anchor, || unix_now() as i64).await
    }

    async fn warm_days(&self) -> anyhow::Result<()> {
        let l = lbread::Live(&self.0);
        lbread::cache::warm_days(&l, &l).await
    }

    fn postgres_enabled(&self) -> bool {
        self.0.settings().postgres_enabled()
    }

    async fn tier_after_days(&self) -> anyhow::Result<Value> {
        runtime_config::get_setting(&self.0, "leaderboards_pg_tier_after_days").await
    }

    async fn tier_cold(&self, after_days: i64, now: i64, limit: usize) -> anyhow::Result<Vec<String>> {
        let pool = lb_ingest::store::pool(&self.0).map_err(anyhow::Error::msg)?;
        let res = lb_ingest::store::tier_cold_partitions(pool, after_days, now, Some(limit))
            .await
            .map_err(anyhow::Error::msg)?;
        Ok(res.map(|(moved, _)| moved).unwrap_or_default())
    }
}

impl Probe for Warm {
    fn now(&self) -> f64 {
        unix_now()
    }

    async fn ready_anchor(&self) -> Option<i64> {
        cheaters::store::ready_anchor(&cheaters::store::Live(&self.0)).await
    }

    async fn persisted(&self, anchor: i64) -> Option<Value> {
        cheaters::store::rget(&cheaters::store::Live(&self.0), &cheaters::store::cheaters_key(anchor)).await
    }

    async fn timestamps(&self) -> anyhow::Result<Vec<i64>> {
        lbread::Live(&self.0).list_timestamps(1, None).await
    }

    async fn ttl(&self) -> anyhow::Result<Value> {
        runtime_config::get_setting(&self.0, "cheaters_cache_ttl_seconds").await
    }
}

impl Driver for Warm {
    fn now(&self) -> f64 {
        unix_now()
    }

    async fn resume_age(&self) -> Option<f64> {
        persisted_pass_age(self).await
    }

    async fn pass(&self) -> anyhow::Result<()> {
        let warm = self.clone();
        off_executor(move || async move {
            let mut last = *LAST_COLD.lock().unwrap_or_else(|e| e.into_inner());
            let res = warm_all(&warm, &mut last).await;
            *LAST_COLD.lock().unwrap_or_else(|e| e.into_inner()) = last;
            res
        })
        .await?
    }

    async fn ttl(&self) -> anyhow::Result<Value> {
        runtime_config::get_setting(&self.0, "cheaters_cache_ttl_seconds").await
    }
}
