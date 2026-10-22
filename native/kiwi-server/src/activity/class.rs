//! `app/trove/leaderboards/class_activity.py`: per-class estimates from the
//! Effort boards (`estimate_class_activity` is the warmer's entry), the donut
//! headcount, the chart series and the backfill.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{LazyLock, Mutex},
};

use kiwi_core::{
    activity::{Capture, ClassSet, ResetWindow, class_sets},
    activity_calc::{
        self as calc, BACKFILL_ANCHOR_MARGIN, CLASS_ACTIVE_RETENTION, Chain, ClassCount, ClassSeriesRow, ClassSets,
        DAY, ResetKind, RollupSource, SHORT_RESET_WINDOW, Thresholds,
    },
    gems::py_round,
};
use serde_json::{Value, json};

use super::{cache, player::backfill_pairs, store, unix_now, unix_now_f64};
use crate::{
    auth::pyutil::int_of, error::ApiError, gamedata::Gamedata, routes::stats::board_class, runtime_config,
    state::AppState,
};

pub const METHODOLOGY: &str = "Per class, distinct top-N players whose score rose (or who first appear) on \
its Effort board, counted over the 24 hours up to each capture (the union of \
every capture window in that span). (Paragon boards are \
excluded as ambiguous - counts are Effort-only.) The default 'clean' \
(established) view keeps only players who, snapshot at the window end, clear \
every configured floor - Power Rank (1000+i board), Effort (4000+i) and XP \
(board 21005) - filtering new characters and throwaway alts; the 'All' view \
counts everyone. \
Effort boards reset weekly (Mon 11:00 UTC); a window crossing the reset is \
unmeasurable, so the 24 hours after it have no point. 'Share' is share of \
class activity (a \
player active on several classes counts in each), not distinct players. Lower \
bound: players outside a board's top-N aren't seen.";

pub const DONUT_METHODOLOGY: &str = "Per-class player share from the latest leaderboard snapshot - a direct \
headcount, NOT the activity pipeline. RAW counts the players present on a \
class's Effort board at the most recent capture (Paragon is excluded as \
ambiguous); the 'established' view keeps only those clearing every floor - \
Power Rank (1000+i), Effort (4000+i) and XP (board 21005). 'Share' is a \
class's count divided by \
the total across classes (a player on several classes counts in each), so \
shares sum to 100% but aren't distinct players. Each class also carries the \
Effort ADDED in the last 24 hours (this capture vs the newest one at least 24h \
earlier) - the sum of positive score gains over players on its Effort board, \
per view; skipped when a weekly reset falls in that span. Lower bound: players \
outside a board's top-N aren't seen.";

/// `_SERIES_PERIODS`: period -> (days, minimum bucket); `None` = all / dynamic.
pub const PERIODS: [(&str, Option<i64>, Option<i64>); 6] = [
    ("7d", Some(7), Some(6 * 3600)),
    ("1m", Some(30), Some(DAY)),
    ("3m", Some(90), Some(DAY)),
    ("6m", Some(180), Some(2 * DAY)),
    ("1y", Some(365), Some(7 * DAY)),
    ("all", None, None),
];

#[derive(Default)]
struct Memo {
    last_good: Option<Value>,
    last_good_donut: Option<Value>,
}

static MEMO: LazyLock<Mutex<Memo>> = LazyLock::new(Default::default);

fn memo() -> std::sync::MutexGuard<'static, Memo> {
    MEMO.lock().unwrap_or_else(|e| e.into_inner())
}

/// `reset_caches()`.
pub fn reset_caches() {
    let mut m = memo();
    m.last_good = None;
    m.last_good_donut = None;
}

/// `_setting_int(key, fallback)`: a config failure falls back to the default.
async fn setting_int(state: &AppState, key: &str, fallback: i64) -> i64 {
    match runtime_config::get_setting(state, key).await {
        Ok(v) => int_of(&v).and_then(|i| i64::try_from(i).ok()).unwrap_or(fallback),
        Err(_) => fallback,
    }
}

/// `_clean_thresholds()`.
pub async fn thresholds(state: &AppState) -> Thresholds {
    let s = state.settings();
    Thresholds {
        power_rank: setting_int(state, "class_activity_power_rank_threshold", s.class_activity_power_rank_threshold)
            .await,
        effort: setting_int(state, "class_activity_effort_threshold", s.class_activity_effort_threshold).await,
        xp: setting_int(state, "class_activity_xp_threshold", s.class_activity_xp_threshold).await,
    }
}

/// `(class_name(i), class_icon(i))` for every class index.
pub fn class_labels(gd: &Gamedata) -> Result<Vec<(Value, Value)>, ApiError> {
    (0..calc::CLASS_COUNT).map(|i| board_class(gd, i)).collect()
}

fn labels(state: &AppState) -> anyhow::Result<Vec<(Value, Value)>> {
    class_labels(Gamedata::global(state.settings())).map_err(|e| anyhow::anyhow!("classes index: {e}"))
}

fn label(labels: &[(Value, Value)], i: i64) -> (Value, Value) {
    usize::try_from(i)
        .ok()
        .and_then(|i| labels.get(i).cloned())
        .unwrap_or_else(|| (Value::from(format!("Class {i}")), Value::Null))
}

/// `_empty()`.
pub fn empty(now: i64) -> Value {
    json!({
        "window_start": null, "window_end": null, "duration_hours": null,
        "total_active": null, "total_active_clean": null,
        "total_effort_added": null, "total_effort_added_clean": null,
        "power_rank_threshold": 0, "effort_threshold": 0, "xp_threshold": 0,
        "classes": [], "methodology": METHODOLOGY,
        "computed_at": now,
    })
}

/// `_build_current(...)`.
#[allow(clippy::too_many_arguments)]
pub fn build_current(
    window_start: Option<i64>,
    window_end: Option<i64>,
    duration_h: Option<f64>,
    counts: &BTreeMap<i64, ClassCount>,
    computed_at: i64,
    t: &Thresholds,
    labels: &[(Value, Value)],
    methodology: &str,
) -> Value {
    let cur = calc::build_current(counts);
    let classes: Vec<Value> = cur
        .classes
        .iter()
        .map(|c| {
            let (name, icon) = label(labels, c.class_index);
            json!({
                "class_index": c.class_index,
                "name": name,
                "icon": icon,
                "active_players": c.active_players,
                "share": c.share,
                "active_players_clean": c.active_players_clean,
                "share_clean": c.share_clean,
                "effort_added": c.effort_added,
                "effort_added_clean": c.effort_added_clean,
            })
        })
        .collect();
    json!({
        "window_start": window_start,
        "window_end": window_end,
        "duration_hours": duration_h.map(|d| py_round(d, 2)),
        "total_active": cur.total_active,
        "total_active_clean": cur.total_active_clean,
        "total_effort_added": cur.total_effort_added,
        "total_effort_added_clean": cur.total_effort_added_clean,
        "power_rank_threshold": t.power_rank,
        "effort_threshold": t.effort,
        "xp_threshold": t.xp,
        "classes": classes,
        "methodology": methodology,
        "computed_at": computed_at,
    })
}

/// `_sets_for(...)` on decoded captures: per class, active names and clean
/// names (`None` = unmeasurable), in class order.
async fn sets_for(
    state: &AppState,
    early: &Capture,
    late: &Capture,
    gates: &Capture,
    early_ts: i64,
    late_ts: i64,
    t: &Thresholds,
) -> anyhow::Result<ClassSets> {
    let crossed = calc::reset_crossed(ResetKind::Weekly, early_ts, late_ts);
    let short = crossed && late_ts - early_ts <= SHORT_RESET_WINDOW;
    let rows: Vec<ClassSet> = class_sets(early, late, gates, &ResetWindow { crossed, short }, &t.floors());
    let mut ids: Vec<i64> = rows.iter().flat_map(|(_, a, _)| a.iter().copied()).collect();
    ids.sort_unstable();
    ids.dedup();
    let names = store::player_names(state.pg(), &ids).await?;
    let name = |p: &i64| names.get(p).cloned().ok_or_else(|| anyhow::anyhow!("player {p} has no name"));
    let mut out = ClassSets::new();
    for (i, active, clean) in rows {
        let a = active.iter().map(name).collect::<anyhow::Result<HashSet<_>>>()?;
        let c = clean.map(|c| c.iter().map(name).collect::<anyhow::Result<HashSet<_>>>()).transpose()?;
        out.insert(i, (a, c));
    }
    Ok(out)
}

/// `_counts_of(sets)`.
pub fn counts_of(sets: &ClassSets) -> BTreeMap<i64, ClassCount> {
    sets.iter()
        .map(|(&i, (a, c))| {
            (i, ClassCount { raw: a.len() as i64, clean: c.as_ref().map(|c| c.len() as i64), ..Default::default() })
        })
        .collect()
}

/// `_rows(early, late, counts, rollups, now)`.
pub fn rows(
    early: i64,
    late: i64,
    counts: &BTreeMap<i64, ClassCount>,
    rollups: &BTreeMap<i64, (i64, Option<i64>)>,
    now: i64,
) -> Vec<store::ClassEstimate> {
    counts
        .iter()
        .map(|(&i, c)| {
            let r = rollups.get(&i);
            store::ClassEstimate {
                class_index: i as i32,
                window_end: late,
                window_start: early,
                duration_hours: py_round((late - early) as f64 / 3600.0, 2),
                estimate: c.raw as i32,
                estimate_clean: c.clean.map(|n| n as i32),
                computed_at: now,
                estimate_24h: r.map(|r| r.0 as i32),
                estimate_24h_clean: r.and_then(|r| r.1).map(|n| n as i32),
            }
        })
        .collect()
}

/// `record_class_active_window` rows: `(class, lower(name), clean)` with each
/// `(class, lower)` once and `clean` OR-ed, as the upsert folds them.
pub fn active_rows(sets: &ClassSets) -> Vec<(i32, String, bool)> {
    let mut merged: BTreeMap<(i32, String), bool> = BTreeMap::new();
    for (&i, (names, clean)) in sets {
        for n in names.iter().filter(|n| !n.is_empty()) {
            let is_clean = clean.as_ref().is_some_and(|c| c.contains(n));
            *merged.entry((i as i32, n.to_lowercase())).or_insert(false) |= is_clean;
        }
    }
    merged.into_iter().map(|((i, n), c)| (i, n, c)).collect()
}

/// The span `_stored_rollups_24h` reads: `(early, windows in (early, late])`,
/// `None` when it isn't measurable.
pub fn rollup_span(stamps: &[i64], late: i64) -> Option<(i64, i64)> {
    let mut asc = stamps.to_vec();
    asc.sort_unstable();
    let early = calc::day_start_anchor(&asc, late).filter(|e| calc::rollup_measurable(*e, late, DAY))?;
    Some((early, stamps.iter().filter(|&&s| early < s && s <= late).count() as i64))
}

/// The classes measurable in every window of the span (clean only when every
/// window had a clean count).
pub fn pick_rollups(n: i64, rollups: &BTreeMap<i64, store::ClassRollup>) -> BTreeMap<i64, (i64, Option<i64>)> {
    rollups
        .iter()
        .filter(|(_, r)| r.windows >= n)
        .map(|(&i, r)| (i, (r.raw, (r.clean_windows >= n).then_some(r.clean))))
        .collect()
}

/// `_stored_rollups_24h(stamps, late)`.
async fn stored_rollups_24h(
    state: &AppState,
    stamps: &[i64],
    late: i64,
) -> anyhow::Result<BTreeMap<i64, (i64, Option<i64>)>> {
    let Some((early, n)) = rollup_span(stamps, late) else { return Ok(BTreeMap::new()) };
    Ok(pick_rollups(n, &store::class_rollup_since(state.pg(), early, late).await?))
}

/// The capture the donut's Effort-added compares against: the newest at least
/// a day older, unless a gap or a weekly reset spoils the span.
pub fn donut_early(stamps: &[i64], anchor: i64) -> Option<i64> {
    let mut asc = stamps.to_vec();
    asc.sort_unstable();
    calc::day_start_anchor(&asc, anchor)
        .filter(|&e| calc::rollup_measurable(e, anchor, DAY) && !calc::reset_crossed(ResetKind::Weekly, e, anchor))
}

/// `estimate_class_activity()`: compute and persist the latest pair's
/// per-class rows with their 24h rollups; the /current-shaped payload.
pub async fn estimate_class_activity(state: &AppState) -> anyhow::Result<Value> {
    let pg = state.pg();
    let stamps_desc = store::list_timestamps(pg, 500, Some(unix_now() - CLASS_ACTIVE_RETENTION)).await?;
    let fallback = || memo().last_good.clone().unwrap_or_else(|| empty(unix_now()));
    if stamps_desc.len() < 2 || stamps_desc[0] <= stamps_desc[1] {
        return Ok(fallback());
    }
    let (late, early) = (stamps_desc[0], stamps_desc[1]);
    let duration_h = (late - early) as f64 / 3600.0;
    let t = thresholds(state).await;
    let boards = calc::effort_board_uuids();
    let late_cap = store::capture(pg, late, &boards).await?;
    let early_cap = store::capture(pg, early, &boards).await?;
    let gates = store::capture(pg, late, &t.gate_uuids()).await?;
    let sets = sets_for(state, &early_cap, &late_cap, &gates, early, late, &t).await?;
    let counts = counts_of(&sets);
    let now = unix_now();
    if !counts.is_empty() {
        let persisted = async {
            store::record_class_active_window(pg, late, &active_rows(&sets)).await?;
            store::prune_class_active_windows(pg, late - CLASS_ACTIVE_RETENTION).await?;
            store::upsert_class_estimates(pg, &rows(early, late, &counts, &BTreeMap::new(), now)).await?;
            let rollups = stored_rollups_24h(state, &stamps_desc, late).await?;
            store::upsert_class_estimates(pg, &rows(early, late, &counts, &rollups, now)).await?;
            anyhow::Ok(())
        };
        if let Err(e) = persisted.await {
            tracing::error!("class activity: persist failed for window_end={late}: {e:#}");
        }
    }
    let labels = if counts.is_empty() { Vec::new() } else { labels(state)? };
    let payload = build_current(Some(early), Some(late), Some(duration_h), &counts, now, &t, &labels, METHODOLOGY);
    if !counts.is_empty() {
        memo().last_good = Some(payload.clone());
    }
    Ok(payload)
}

/// The donut payload from one snapshot (and the capture a day earlier, when
/// the Effort-added span is measurable).
pub fn donut_payload(
    anchor: i64,
    effort: &Capture,
    gates: &Capture,
    early: Option<&Capture>,
    t: &Thresholds,
    now: i64,
    labels: &[(Value, Value)],
) -> Option<Value> {
    let mut counts = calc::snapshot_counts(effort, gates, t);
    if counts.is_empty() {
        return None;
    }
    if let Some(early) = early {
        for (i, (raw, clean)) in calc::effort_deltas(effort, early, gates, t) {
            if let Some(c) = counts.get_mut(&i) {
                c.effort_raw = Some(raw);
                c.effort_clean = clean;
            }
        }
    }
    Some(build_current(Some(anchor), Some(anchor), None, &counts, now, t, labels, DONUT_METHODOLOGY))
}

/// `class_activity_current()`: read-through `lb:cache:activity_class_current`.
pub async fn current(state: &AppState) -> anyhow::Result<Value> {
    if let Some(cached) = cache::get(state, cache::CLASS_CURRENT_KEY).await {
        return Ok(cached);
    }
    let pg = state.pg();
    let stamps = store::list_timestamps(pg, 500, Some(unix_now() - CLASS_ACTIVE_RETENTION)).await?;
    let fallback = || memo().last_good_donut.clone().unwrap_or_else(|| empty(unix_now()));
    let Some(&anchor) = stamps.first() else { return Ok(fallback()) };
    let t = thresholds(state).await;
    let boards = calc::effort_board_uuids();
    let effort = store::capture(pg, anchor, &boards).await?;
    let gates = store::capture(pg, anchor, &t.gate_uuids()).await?;
    if calc::snapshot_counts(&effort, &gates, &t).is_empty() {
        return Ok(fallback());
    }
    let early_cap = match donut_early(&stamps, anchor) {
        Some(e) => Some(store::capture(pg, e, &boards).await?),
        None => None,
    };
    let labels = labels(state)?;
    let payload = donut_payload(anchor, &effort, &gates, early_cap.as_ref(), &t, unix_now(), &labels)
        .ok_or_else(|| anyhow::anyhow!("class activity counts vanished"))?;
    memo().last_good_donut = Some(payload.clone());
    cache::set(state, cache::CLASS_CURRENT_KEY, &payload, cache::SERIES_TTL).await;
    Ok(payload)
}

/// The `class_activity_series` payload over the stored rows.
#[allow(clippy::too_many_arguments)]
pub fn series_payload(
    period: &str,
    bucket: Option<i64>,
    window_start: i64,
    now: i64,
    rows: &[store::ClassEstimate],
    t: &Thresholds,
    labels: &[(Value, Value)],
) -> Value {
    let rows: Vec<ClassSeriesRow> = rows
        .iter()
        .map(|r| ClassSeriesRow {
            class_index: i64::from(r.class_index),
            window_end: r.window_end,
            duration_hours: r.duration_hours,
            estimate_24h: r.estimate_24h.map(i64::from),
            estimate_24h_clean: r.estimate_24h_clean.map(i64::from),
        })
        .collect();
    let s = calc::class_series(&rows, bucket, now, window_start);
    let classes: Vec<Value> = s
        .lines
        .iter()
        .enumerate()
        .map(|(i, line)| {
            let (name, icon) = label(labels, i as i64);
            json!({
                "class_index": i,
                "name": name,
                "icon": icon,
                "values": line.values,
                "values_clean": line.values_clean,
            })
        })
        .collect();
    json!({
        "period": period,
        "bucket_seconds": s.bucket,
        "window_start": window_start,
        "window_end": now,
        "power_rank_threshold": t.power_rank,
        "effort_threshold": t.effort,
        "xp_threshold": t.xp,
        "buckets": s.buckets,
        "classes": classes,
        "methodology": METHODOLOGY,
    })
}

/// `class_activity_series(period=)`: read-through
/// `lb:cache:activity_class_series:{p}`.
pub async fn series(state: &AppState, period: &str) -> anyhow::Result<Value> {
    let known: Vec<&str> = PERIODS.iter().map(|p| p.0).collect();
    let period = super::player::normalize_period(period, &known);
    let key = cache::class_series_key(&period);
    if let Some(cached) = cache::get(state, &key).await {
        return Ok(cached);
    }
    let (_, days, bucket) = PERIODS.iter().find(|p| p.0 == period).copied().unwrap_or(PERIODS[0]);
    let now = unix_now();
    let (window_start, rows) = match days {
        Some(d) => {
            let ws = now - d * 86_400;
            (ws, store::get_class_estimates(state.pg(), Some(ws)).await?)
        }
        None => {
            let rows = store::get_class_estimates(state.pg(), None).await?;
            (rows.first().map_or(now, |r| r.window_end), rows)
        }
    };
    let labels = labels(state)?;
    let t = thresholds(state).await;
    let payload = series_payload(&period, bucket, window_start, now, &rows, &t, &labels);
    cache::set(state, &key, &payload, cache::SERIES_TTL).await;
    Ok(payload)
}

// ---------------------------------------------------------------------------
// Backfill

async fn replay_rollups_24h(
    state: &AppState,
    chain: &mut Chain<ClassSets>,
    stamps_asc: &[i64],
    anchor: i64,
    active_floor: i64,
) -> anyhow::Result<BTreeMap<i64, (i64, Option<i64>)>> {
    Ok(match calc::replay_source(chain, stamps_asc, anchor, active_floor) {
        RollupSource::None => BTreeMap::new(),
        RollupSource::Chain => calc::chain_class_rollups(chain),
        RollupSource::Table(_) => stored_rollups_24h(state, stamps_asc, anchor).await?,
    })
}

fn short_summary(skipped: usize, total: usize, note: &str) -> Value {
    json!({"computed": 0, "skipped": skipped, "failed": 0, "total": total, "note": note})
}

/// `backfill_class_history(since_ts=, until_ts=, force=)`.
pub async fn backfill_class_history(
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
        store::get_class_estimates(pg, Some(stored[0].1)).await?.iter().map(|r| r.window_end).collect()
    };
    let todo_stored: Vec<(i64, i64)> = stored.iter().copied().filter(|p| !existing.contains(&p.1)).collect();
    if todo_stored.is_empty() {
        return Ok(short_summary(stored.len(), stored.len(), "all pairs already stored - use force=True"));
    }
    let todo: Vec<(i64, i64)> = pairs.iter().copied().filter(|p| p.1 < lo).chain(todo_stored.iter().copied()).collect();
    let boards = calc::effort_board_uuids();
    let t = thresholds(state).await;
    let gate_uuids = t.gate_uuids();
    let mut needed: Vec<i64> = todo.iter().flat_map(|&(e, l)| [e, l]).collect();
    needed.sort_unstable();
    needed.dedup();
    let early_of: HashMap<i64, i64> = todo.iter().map(|&(e, l)| (l, e)).collect();
    let span_days = py_round((stamps_asc[stamps_asc.len() - 1] - stamps_asc[0]) as f64 / 86_400.0, 1);
    tracing::info!(
        "class activity backfill: {} pairs over {} anchors, {} classes; spans {:.1} days",
        todo.len(),
        needed.len(),
        calc::CLASS_COUNT,
        span_days
    );
    let started = unix_now_f64();
    let active_floor = unix_now() - CLASS_ACTIVE_RETENTION;
    let (mut computed, mut failed, mut empty_skipped) = (0usize, 0usize, 0usize);
    let mut prev: Option<(i64, Capture)> = None;
    let mut chain: Chain<ClassSets> = Chain::new();
    for (idx, &anchor) in needed.iter().enumerate() {
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
                let gates = store::capture(pg, anchor, &gate_uuids).await?;
                let sets = sets_for(state, early_cap, &cur, &gates, early, anchor, &t).await?;
                if anchor >= active_floor {
                    store::record_class_active_window(pg, anchor, &active_rows(&sets)).await?;
                }
                let counts = counts_of(&sets);
                calc::chain_push(&mut chain, early, anchor, sets);
                if anchor >= lo {
                    if !counts.is_empty() {
                        store::upsert_class_estimates(pg, &rows(early, anchor, &counts, &BTreeMap::new(), unix_now()))
                            .await?;
                        let rollups = replay_rollups_24h(state, &mut chain, &stamps_asc, anchor, active_floor).await?;
                        if !rollups.is_empty() {
                            store::upsert_class_estimates(pg, &rows(early, anchor, &counts, &rollups, unix_now()))
                                .await?;
                        }
                        computed += 1;
                    } else {
                        store::delete_class_estimate(pg, anchor).await?;
                        empty_skipped += 1;
                    }
                }
                anyhow::Ok(())
            };
            if let Err(e) = step.await {
                failed += 1;
                chain.clear();
                tracing::error!("class backfill: pair late={anchor} failed: {e:#}");
            }
        }
        prev = Some((anchor, cur));
        if (idx + 1) % 20 == 0 {
            tracing::info!(
                "class activity backfill: {}/{} anchors, {computed} computed ({:.1}s)",
                idx + 1,
                needed.len(),
                unix_now_f64() - started
            );
        }
    }
    if computed > 0 {
        cache::invalidate_all_activity(state).await;
    }
    let out = json!({
        "computed": computed,
        "skipped": stored.len() - todo_stored.len(),
        "empty_skipped": empty_skipped,
        "failed": failed,
        "total": stored.len(),
        "anchors": stamps_asc.len(),
        "span_days": span_days,
        "elapsed_seconds": py_round(unix_now_f64() - started, 2),
    });
    tracing::info!("class activity backfill done: {out}");
    Ok(out)
}

/// `reset_class_estimates()`.
pub async fn reset_class_estimates(state: &AppState) -> anyhow::Result<u64> {
    let deleted = store::delete_all_class_estimates(state.pg()).await?;
    store::delete_all_class_active_windows(state.pg()).await?;
    reset_caches();
    cache::invalidate_all_activity(state).await;
    tracing::warn!("class activity: RESET cleared {deleted} stored estimates");
    Ok(deleted)
}

/// `backfill_class_history_chunked(total_days=, force=, reset=)`.
pub async fn backfill_class_history_chunked(
    state: &AppState,
    total_days: i64,
    force: bool,
    reset: bool,
) -> anyhow::Result<Value> {
    let mut force = force;
    let mut reset_deleted = 0;
    if reset {
        reset_deleted = reset_class_estimates(state).await?;
        force = true;
    }
    let now = unix_now();
    let since = if total_days <= 0 { 0 } else { now - total_days * 86_400 };
    let mut out = backfill_class_history(state, since, Some(now), force).await?;
    if let Value::Object(o) = &mut out {
        o.insert("total_days".into(), total_days.into());
        o.insert("reset_deleted".into(), reset_deleted.into());
    }
    tracing::info!("class activity backfill (chunked) done: {out}");
    Ok(out)
}
