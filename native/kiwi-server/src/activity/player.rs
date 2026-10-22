//! `app/trove/leaderboards/activity.py`: the active-player estimate, its
//! history and chart series, and the backfill. `estimate_active_players(state,
//! true)` is the warmer's entry.

use std::{
    collections::{HashMap, HashSet},
    sync::{LazyLock, Mutex},
};

use kiwi_core::{
    activity::{Capture, pair},
    activity_calc::{self as calc, ACTIVE_RETENTION, BACKFILL_ANCHOR_MARGIN, Chain, DAY, RollupSource, SeriesRow},
    gems::py_round,
};
use serde_json::{Map, Value, json};

use super::{cache, store, unix_now, unix_now_f64};
use crate::{runtime_config, state::AppState};

pub const METHODOLOGY: &str = "Distinct top-N leaderboard players whose score rose on at least one board \
between two captures (or who newly appear above the previous capture's \
floor, so a shallower dump can't fake a surge). A reset isn't counted as \
activity (the score drops, it doesn't rise). The 24h / 7d figures are the \
distinct UNION of every capture's active set over the period, so they don't \
depend on how often captures land; one a capture gap stretches past 1.5x its \
period is withheld. `estimate` covers only the latest capture window. Lower \
bound: players outside every board's top-N aren't seen.";

const EMPTY_METHODOLOGY: &str = "At least two captures required to compute a score-delta. \
Bot needs to send unique timestamps (not just the daily \
anchor) for this estimate to be available.";

const HISTORY_METHODOLOGY: &str = "One point per consecutive capture pair. `estimate` is the distinct \
top-N leaderboard players whose score rose on at least one board \
between the two captures; `estimate_24h` is the distinct union of \
those players over the 24 hours up to the capture (null where a \
capture gap stretches that span past 36 hours).";

const SERIES_METHODOLOGY: &str = "Distinct players active in the 24 hours up to each capture, from the \
stored per-capture rollups, averaged per time bucket. The busiest and \
quietest captures in the range are plotted at their true values so a \
bucket never flattens the real peak. Buckets are never narrower than \
the capture cadence; a capture whose 24h span a gap stretched past 36 \
hours is left out.";

/// `_ACTIVITY_PERIODS`: period -> (days, minimum bucket seconds).
pub const PERIODS: [(&str, i64, i64); 2] = [("7d", 7, 6 * 3600), ("1m", 30, DAY)];

#[derive(Default)]
struct Memo {
    /// `_CACHE`: `(late, early) -> (stored_at, payload)`.
    cache: HashMap<(i64, i64), (f64, Value)>,
    /// `_LAST_GOOD`.
    last_good: Option<Value>,
}

static MEMO: LazyLock<Mutex<Memo>> = LazyLock::new(Default::default);

fn memo() -> std::sync::MutexGuard<'static, Memo> {
    MEMO.lock().unwrap_or_else(|e| e.into_inner())
}

/// `invalidate_cache()`.
pub fn invalidate_cache() {
    memo().cache.clear();
}

/// `reset_caches()`.
pub fn reset_caches() {
    let mut m = memo();
    m.cache.clear();
    m.last_good = None;
}

/// `_empty()`.
pub fn empty(now: i64) -> Value {
    json!({
        "window_start": null,
        "window_end": null,
        "duration_hours": null,
        "estimate": null,
        "estimate_24h": null,
        "estimate_7d": null,
        "window_24h_start": null,
        "window_7d_start": null,
        "span_24h_hours": null,
        "span_7d_hours": null,
        "by_board": [],
        "boards_analyzed": 0,
        "methodology": EMPTY_METHODOLOGY,
        "computed_at": now,
    })
}

/// `float(get_setting("cheaters_cache_ttl_seconds"))`.
pub async fn cache_ttl(state: &AppState) -> anyhow::Result<f64> {
    let v = runtime_config::get_setting(state, "cheaters_cache_ttl_seconds").await?;
    v.as_f64().ok_or_else(|| anyhow::anyhow!("cheaters_cache_ttl_seconds is not a number: {v}"))
}

/// `estimate_active_players(force=)`. Readers (`force=false`) get the
/// published anchor's persisted estimate, else this process's last good one;
/// the warmer (`force=true`) computes and persists the latest pair.
pub async fn estimate_active_players(state: &AppState, force: bool) -> anyhow::Result<Value> {
    let ttl = cache_ttl(state).await?;
    let now = unix_now_f64();
    let pg = state.pg();
    if !force {
        if let Some(anchor) = cache::ready_anchor(state).await
            && let Some(p) = cache::get_activity(state, anchor).await
        {
            memo().last_good = Some(p.clone());
            return Ok(p);
        }
        let stamps = store::list_timestamps(pg, 2, None).await?;
        if stamps.len() < 2 {
            return Ok(empty(unix_now()));
        }
        let mut m = memo();
        if let Some((at, p)) = m.cache.get(&(stamps[0], stamps[1])).cloned()
            && now - at < ttl
        {
            m.last_good = Some(p.clone());
            return Ok(p);
        }
        return Ok(m.last_good.clone().unwrap_or_else(|| empty(unix_now())));
    }

    let stamps = store::list_timestamps(pg, 2, None).await?;
    if stamps.len() < 2 || stamps[0] <= stamps[1] {
        return Ok(empty(unix_now()));
    }
    let (late, early) = (stamps[0], stamps[1]);
    if let Some(p) = cache::get_activity(state, late).await {
        let computed_at = match &p {
            Value::Object(o) => o.get("computed_at").cloned().unwrap_or(Value::from(0)),
            _ => anyhow::bail!("persisted activity payload is not an object"),
        };
        let computed_at = computed_at.as_f64().ok_or_else(|| anyhow::anyhow!("bad computed_at"))?;
        if now - computed_at < ttl {
            let mut m = memo();
            m.cache.insert((late, early), (now, p.clone()));
            m.last_good = Some(p.clone());
            return Ok(p);
        }
    }
    let payload = compute(state, late, early).await?;
    {
        let mut m = memo();
        m.cache.insert((late, early), (now, payload.clone()));
        m.last_good = Some(payload.clone());
    }
    cache::set_activity(state, late, &payload).await;
    memo().cache.retain(|_, (t, _)| now - *t <= ttl * 2.0);
    Ok(payload)
}

/// `_rollup`: the distinct union over `(early, late]`, `None` when unmeasurable
/// or the count fails.
async fn rollup(state: &AppState, early: Option<i64>, late: i64, period: i64) -> Option<i64> {
    let early = early.filter(|e| calc::rollup_measurable(*e, late, period))?;
    match store::count_active_since(state.pg(), early, late).await {
        Ok(n) => Some(n),
        Err(e) => {
            tracing::error!("activity: {period}s rollup failed for late={late}: {e:#}");
            None
        }
    }
}

/// `_compute(anchor_late, anchor_early)`: the estimate for one pair, persisted
/// (active set, rollups, `activity_estimate` row).
pub async fn compute(state: &AppState, late: i64, early: i64) -> anyhow::Result<Value> {
    let pg = state.pg();
    let boards = store::list_boards_at(pg, late).await?;
    let stamps_desc = store::list_timestamps(pg, 1000, Some(late - ACTIVE_RETENTION)).await?;
    let early_24h = calc::pick_early_anchor(&stamps_desc, late, DAY);
    let early_7d = calc::pick_early_anchor(&stamps_desc, late, 7 * DAY);

    let player_boards: Vec<&store::Board> = boards.iter().filter(|b| calc::is_player_board(b.uuid)).collect();
    let uuids: Vec<i32> = player_boards.iter().map(|b| b.uuid).collect();
    let late_cap = store::capture(pg, late, &uuids).await?;
    let early_cap = store::capture(pg, early, &uuids).await?;
    let p = pair(&early_cap, &late_cap, &uuids);
    let names: HashSet<String> = store::player_names(pg, &p.active).await?.into_values().collect();

    let now_ts = unix_now();
    let lowered = store::lowered(names.iter().map(String::as_str));
    let recorded = async {
        store::record_active_window(pg, late, &lowered).await?;
        store::prune_active_windows(pg, late - ACTIVE_RETENTION).await?;
        anyhow::Ok(())
    };
    if let Err(e) = recorded.await {
        tracing::error!("activity: failed to record active window late={late}: {e:#}");
    }
    let w = Window {
        late,
        early,
        boards: &player_boards,
        per_board: &p.per_board,
        estimate: names.len() as i64,
        early_24h,
        early_7d,
        estimate_24h: rollup(state, early_24h, late, DAY).await,
        estimate_7d: rollup(state, early_7d, late, 7 * DAY).await,
        now: now_ts,
    };
    if let Err(e) = store::upsert_estimate(pg, &w.row()).await {
        tracing::error!("activity: failed to persist estimate for window_end={late}: {e:#}");
    }
    Ok(w.payload())
}

/// One computed window, ready to persist and serve.
pub struct Window<'a> {
    pub late: i64,
    pub early: i64,
    /// The player boards at `late`, in `list_boards_at` order.
    pub boards: &'a [&'a store::Board],
    /// Active players per board (boards with none may be absent).
    pub per_board: &'a HashMap<i32, usize>,
    pub estimate: i64,
    pub early_24h: Option<i64>,
    pub early_7d: Option<i64>,
    pub estimate_24h: Option<i64>,
    pub estimate_7d: Option<i64>,
    pub now: i64,
}

impl Window<'_> {
    fn by_board(&self) -> Vec<(i64, &store::Board)> {
        let mut out: Vec<(i64, &store::Board)> = self
            .boards
            .iter()
            .filter_map(|b| self.per_board.get(&b.uuid).filter(|n| **n > 0).map(|&n| (n as i64, *b)))
            .collect();
        out.sort_by_key(|(n, _)| -n);
        out
    }

    fn duration(&self) -> f64 {
        py_round((self.late - self.early) as f64 / 3600.0, 2)
    }

    /// The `activity_estimate` row `_compute` upserts.
    pub fn row(&self) -> store::Estimate {
        store::Estimate {
            window_end: self.late,
            window_start: self.early,
            duration_hours: self.duration(),
            estimate: self.estimate as i32,
            boards_analyzed: self.by_board().len() as i32,
            computed_at: self.now,
            estimate_24h: self.estimate_24h.map(|n| n as i32),
        }
    }

    /// `_compute`'s payload.
    pub fn payload(&self) -> Value {
        let by_board = self.by_board();
        let span = |e: Option<i64>| e.map(|e| py_round((self.late - e) as f64 / 3600.0, 1));
        let boards: Vec<Value> = by_board
            .iter()
            .map(|(n, b)| json!({"uuid": b.uuid, "name": b.name, "category": b.category, "active_players": n}))
            .collect();
        json!({
            "window_start": self.early,
            "window_end": self.late,
            "duration_hours": self.duration(),
            "estimate": self.estimate,
            "estimate_24h": self.estimate_24h,
            "estimate_7d": self.estimate_7d,
            "window_24h_start": self.early_24h,
            "window_7d_start": self.early_7d,
            "span_24h_hours": span(self.early_24h),
            "span_7d_hours": span(self.early_7d),
            "by_board": boards,
            "boards_analyzed": by_board.len(),
            "methodology": METHODOLOGY,
            "computed_at": self.now,
        })
    }
}

/// `estimate_active_players_history(days=)` over already-loaded rows.
pub fn history_payload(days: i64, now: i64, rows: &[store::Estimate]) -> Value {
    let points: Vec<Value> = rows
        .iter()
        .map(|r| {
            let d = r.duration_hours;
            let per_hour = if d > 0.0 { f64::from(r.estimate) / d } else { 0.0 };
            json!({
                "window_end": r.window_end,
                "window_start": r.window_start,
                "duration_hours": py_round(d, 2),
                "estimate": r.estimate,
                "estimate_24h": r.estimate_24h,
                "estimate_per_hour": py_round(per_hour, 1),
            })
        })
        .collect();
    json!({
        "days": days,
        "window_start": now - days * 86_400,
        "window_end": now,
        "points": points,
        "methodology": HISTORY_METHODOLOGY,
    })
}

/// `estimate_active_players_history(days=)`.
pub async fn history(state: &AppState, days: i64) -> anyhow::Result<Value> {
    let days = days.clamp(1, 30);
    let now = unix_now();
    let rows = store::get_estimates(state.pg(), Some(now - days * 86_400)).await?;
    Ok(history_payload(days, now, &rows))
}

/// `(period or "7d").lower()`, unknown periods falling back to `7d`.
pub fn normalize_period(period: &str, known: &[&str]) -> String {
    let p = if period.is_empty() { "7d".to_owned() } else { period.to_lowercase() };
    if known.contains(&p.as_str()) { p } else { "7d".to_owned() }
}

/// The `activity_series` payload over the stored rows of its window.
pub fn series_payload(period: &str, min_bucket: i64, window_start: i64, now: i64, rows: &[store::Estimate]) -> Value {
    let rows: Vec<SeriesRow> = rows
        .iter()
        .filter_map(|r| {
            r.estimate_24h.map(|e| SeriesRow {
                window_end: r.window_end,
                duration_hours: r.duration_hours,
                estimate_24h: i64::from(e),
            })
        })
        .collect();
    let s = calc::activity_series(&rows, min_bucket);
    let points: Vec<Value> = s
        .points
        .iter()
        .map(|p| json!({"t": p.t, "active": p.active, "avg": p.avg, "peak": p.peak, "samples": p.samples}))
        .collect();
    let extreme = |e: &Option<calc::Extreme>| e.as_ref().map(|e| json!({"t": e.t, "active": e.active}));
    json!({
        "period": period,
        "bucket_seconds": s.bucket,
        "window_start": window_start,
        "window_end": now,
        "points": points,
        "peak": extreme(&s.peak),
        "low": extreme(&s.low),
        "average": s.average,
        "latest": s.latest,
        "methodology": SERIES_METHODOLOGY,
    })
}

/// `activity_series(period=)`: read-through `lb:cache:activity_series:{p}`.
pub async fn series(state: &AppState, period: &str) -> anyhow::Result<Value> {
    let period = normalize_period(period, &["7d", "1m"]);
    let key = cache::series_key(&period);
    if let Some(cached) = cache::get(state, &key).await {
        return Ok(cached);
    }
    let (_, days, bucket) = PERIODS.iter().find(|p| p.0 == period).copied().unwrap_or(PERIODS[0]);
    let now = unix_now();
    let window_start = now - days * 86_400;
    let rows = store::get_estimates(state.pg(), Some(window_start)).await?;
    let payload = series_payload(&period, bucket, window_start, now, &rows);
    cache::set(state, &key, &payload, cache::SERIES_TTL).await;
    Ok(payload)
}

// ---------------------------------------------------------------------------
// Backfill

fn summary(pairs: &[(&str, Value)]) -> Value {
    Value::Object(pairs.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect::<Map<_, _>>())
}

fn short_summary(skipped: usize, total: usize, note: &str) -> Value {
    summary(&[
        ("computed", 0.into()),
        ("skipped", skipped.into()),
        ("failed", 0.into()),
        ("total", total.into()),
        ("note", note.into()),
    ])
}

/// `(early, late)` anchors.
pub type Pair = (i64, i64);

/// The consecutive `(early, late)` pairs a backfill over `[lo, hi]` replays
/// (`late >= lo - 1 day`), and those it stores (`late >= lo`).
pub fn backfill_pairs(stamps_asc: &[i64], lo: i64, hi: Option<i64>) -> (Vec<Pair>, Vec<Pair>) {
    let pairs: Vec<(i64, i64)> = stamps_asc
        .windows(2)
        .map(|w| (w[0], w[1]))
        .filter(|&(_, late)| late >= lo - DAY && hi.is_none_or(|h| late <= h))
        .collect();
    let stored = pairs.iter().copied().filter(|p| p.1 >= lo).collect();
    (pairs, stored)
}

async fn pair_names(
    state: &AppState,
    early: &Capture,
    late: &Capture,
    boards: &[i32],
) -> anyhow::Result<(HashSet<String>, usize)> {
    let p = pair(early, late, boards);
    let names = store::player_names(state.pg(), &p.active).await?.into_values().collect();
    Ok((names, p.boards))
}

async fn replay_rollup_24h(
    state: &AppState,
    chain: &mut Chain<HashSet<String>>,
    stamps_asc: &[i64],
    anchor: i64,
    active_floor: i64,
) -> anyhow::Result<Option<i64>> {
    Ok(match calc::replay_source(chain, stamps_asc, anchor, active_floor) {
        RollupSource::None => None,
        RollupSource::Chain => Some(calc::chain_union_len(chain)),
        RollupSource::Table(early) => Some(store::count_active_since(state.pg(), early, anchor).await?),
    })
}

/// `backfill_history(since_ts=, until_ts=, force=)`.
pub async fn backfill_history(
    state: &AppState,
    since_ts: i64,
    until_ts: Option<i64>,
    force: bool,
) -> anyhow::Result<Value> {
    let pg = state.pg();
    let lo = since_ts;
    let floor = (lo > 0).then(|| (lo - BACKFILL_ANCHOR_MARGIN).max(0));
    let mut stamps_asc = store::list_timestamps(pg, 1_000_000, floor).await?;
    if stamps_asc.is_empty() {
        return Ok(short_summary(0, 0, "no anchors stored"));
    }
    stamps_asc.sort_unstable();
    let (pairs, stored) = backfill_pairs(&stamps_asc, lo, until_ts);
    if stored.is_empty() {
        return Ok(short_summary(0, 0, "no pairs in window"));
    }
    let existing: HashSet<i64> = if force {
        HashSet::new()
    } else {
        store::get_estimates(pg, Some(stored[0].1)).await?.iter().map(|r| r.window_end).collect()
    };
    let todo_stored: Vec<(i64, i64)> = stored.iter().copied().filter(|p| !existing.contains(&p.1)).collect();
    if todo_stored.is_empty() {
        return Ok(short_summary(stored.len(), stored.len(), "all pairs already stored - use force=True to recompute"));
    }
    let todo: Vec<(i64, i64)> = pairs.iter().copied().filter(|p| p.1 < lo).chain(todo_stored.iter().copied()).collect();
    let boards: Vec<i32> =
        store::all_board_uuids(pg).await?.into_iter().filter(|u| calc::is_player_board(*u)).collect();
    let mut needed: Vec<i64> = todo.iter().flat_map(|&(e, l)| [e, l]).collect();
    needed.sort_unstable();
    needed.dedup();
    let early_of: HashMap<i64, i64> = todo.iter().map(|&(e, l)| (l, e)).collect();
    let median_interval = calc::median(&calc::intervals_hours(&stamps_asc));
    let span_days = py_round((stamps_asc[stamps_asc.len() - 1] - stamps_asc[0]) as f64 / 86_400.0, 1);
    tracing::info!(
        "activity backfill: {} pairs over {} anchors, {} boards; cadence median={:.2}h; data spans {:.1} days ({} total anchors)",
        todo.len(),
        needed.len(),
        boards.len(),
        median_interval.unwrap_or(0.0),
        span_days,
        stamps_asc.len()
    );
    let started = unix_now_f64();
    let active_floor = unix_now() - ACTIVE_RETENTION;
    let (mut computed, mut failed, mut unmeasured) = (0usize, 0usize, 0usize);
    let mut prev: Option<(i64, Capture)> = None;
    let mut chain: Chain<HashSet<String>> = Chain::new();
    for (i, &anchor) in needed.iter().enumerate() {
        let cur = store::capture(pg, anchor, &boards).await?;
        if let Some(&early) = early_of.get(&anchor) {
            let step = async {
                let loaded;
                let early_cap = match &prev {
                    Some((a, c)) if *a == early => c,
                    _ => {
                        loaded = store::capture(pg, early, &boards).await?;
                        &loaded
                    }
                };
                let (names, nboards) = pair_names(state, early_cap, &cur, &boards).await?;
                if anchor >= active_floor {
                    store::record_active_window(pg, anchor, &store::lowered(names.iter().map(String::as_str))).await?;
                }
                let count = names.len();
                calc::chain_push(&mut chain, early, anchor, names);
                let est_24h = replay_rollup_24h(state, &mut chain, &stamps_asc, anchor, active_floor).await?;
                if anchor >= lo {
                    store::upsert_estimate(
                        pg,
                        &store::Estimate {
                            window_end: anchor,
                            window_start: early,
                            duration_hours: py_round((anchor - early) as f64 / 3600.0, 2),
                            estimate: count as i32,
                            boards_analyzed: nboards as i32,
                            computed_at: unix_now(),
                            estimate_24h: est_24h.map(|n| n as i32),
                        },
                    )
                    .await?;
                    computed += 1;
                    unmeasured += usize::from(est_24h.is_none());
                }
                anyhow::Ok(())
            };
            if let Err(e) = step.await {
                failed += 1;
                chain.clear();
                tracing::error!("activity backfill: pair late={anchor} failed: {e:#}");
            }
        }
        prev = Some((anchor, cur));
        if (i + 1) % 20 == 0 {
            tracing::info!(
                "activity backfill: streamed {}/{} anchors, {computed} computed ({:.1}s)",
                i + 1,
                needed.len(),
                unix_now_f64() - started
            );
        }
    }
    if computed > 0 {
        invalidate_cache();
        cache::invalidate_all_activity(state).await;
    }
    let out = summary(&[
        ("computed", computed.into()),
        ("skipped", (stored.len() - todo_stored.len()).into()),
        ("unmeasured_24h", unmeasured.into()),
        ("failed", failed.into()),
        ("total", stored.len().into()),
        ("anchors", stamps_asc.len().into()),
        ("span_days", span_days.into()),
        ("median_interval_hours", median_interval.map(|m| py_round(m, 2)).into()),
        ("elapsed_seconds", py_round(unix_now_f64() - started, 2).into()),
    ]);
    tracing::info!("activity backfill done: {out}");
    Ok(out)
}

/// `reset_estimates()`: wipe the stored history and every activity cache.
pub async fn reset_estimates(state: &AppState) -> anyhow::Result<u64> {
    let deleted = store::delete_all_estimates(state.pg()).await?;
    let active = store::delete_all_active_windows(state.pg()).await?;
    reset_caches();
    let cleared = cache::invalidate_all_activity(state).await;
    tracing::warn!(
        "activity: RESET cleared {deleted} stored estimates, {active} active-window rows, {cleared} redis snapshots"
    );
    Ok(deleted)
}

/// `backfill_history_chunked(total_days=, chunk_days=, force=, reset=)`
/// (`chunk_days` is accepted and ignored, as in Python).
pub async fn backfill_history_chunked(
    state: &AppState,
    total_days: i64,
    force: bool,
    reset: bool,
) -> anyhow::Result<Value> {
    let mut force = force;
    let mut reset_deleted = 0;
    if reset {
        reset_deleted = reset_estimates(state).await?;
        force = true;
    }
    let now = unix_now();
    let since = if total_days <= 0 { 0 } else { now - total_days * 86_400 };
    let started = unix_now_f64();
    let mut out = backfill_history(state, since, Some(now), force).await?;
    if let Value::Object(o) = &mut out {
        o.insert("total_days".into(), total_days.into());
        o.insert("reset_deleted".into(), reset_deleted.into());
        o.insert("elapsed_seconds".into(), py_round(unix_now_f64() - started, 2).into());
    }
    tracing::info!("activity backfill (chunked) done: {out}");
    Ok(out)
}
