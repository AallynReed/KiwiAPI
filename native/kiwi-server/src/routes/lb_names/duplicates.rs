//! `app/trove/leaderboards/duplicates.py`: per-name records from the store, the
//! live pass the warmer runs ([`detect_latest`]), the archive backfill and the
//! reads. Never claims which identity is real.

use std::collections::{BTreeSet, HashMap};
use std::sync::{LazyLock, Mutex};

use futures::{StreamExt, stream};
use kiwi_core::duplicates::{self as core, Point, SeriesSummary};
use serde_json::{Map, Value, json};

use super::store::{DateWrite, DupGroup, DuplicateRow, DuplicateWrite, Store, board_label, int_setting};
use super::{Clock, StatusDoc, int_secs, lower_key, py_slice_end};
use crate::{features, pyjson};

pub const METHOD_VERSION: i64 = 1;
pub const STATUS_KEY: &str = "lb:duplicates:status";
pub const RUNNING_KEY: &str = "lb:duplicates:running";
/// Names whose window rows load at once in the live pass (records keep their order).
const RECORD_CONCURRENCY: usize = 6;

/// `_last_scanned_anchor`.
pub static LAST_ANCHOR: Mutex<Option<i64>> = Mutex::new(None);
static LOCAL_STATUS: LazyLock<Mutex<Map<String, Value>>> =
    LazyLock::new(|| Mutex::new(Map::from_iter([("running".to_string(), Value::Bool(false))])));

#[cfg(test)]
pub(crate) fn reset_local_status() {
    *LOCAL_STATUS.lock().unwrap_or_else(|e| e.into_inner()) =
        Map::from_iter([("running".to_string(), Value::Bool(false))]);
}

/// Drops the in-process scan guard (`duplicates.reset()`, on a full leaderboards reset).
pub fn reset_guard() {
    *LAST_ANCHOR.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

pub struct Cfg {
    pub lookback_days: i64,
    pub min_boards: i64,
    pub frozen_min_captures: i64,
    pub max_names: i64,
    pub board_names: HashMap<i32, String>,
}

/// `_load_config`; board names are cosmetic, so a failed read leaves them out.
pub async fn load_config<S: Store>(s: &S, anchor: Option<i64>) -> anyhow::Result<Cfg> {
    let mut cfg = Cfg {
        lookback_days: int_setting(s, "duplicates_lookback_days").await?,
        min_boards: int_setting(s, "duplicates_min_boards").await?,
        frozen_min_captures: int_setting(s, "duplicates_frozen_min_captures").await?,
        max_names: int_setting(s, "duplicates_max_names").await?,
        board_names: HashMap::new(),
    };
    if let Some(anchor) = anchor {
        match s.boards_at(anchor).await {
            Ok(boards) => cfg.board_names = boards.iter().map(|b| (b.uuid, board_label(b))).collect(),
            Err(e) => tracing::warn!("duplicates: board names unresolved at {anchor}: {e:#}"),
        }
    }
    Ok(cfg)
}

pub fn series_value(s: &SeriesSummary) -> Value {
    json!({
        "slot": s.slot,
        "captures": s.captures,
        "first_anchor": s.first_anchor,
        "last_anchor": s.last_anchor,
        "first_score": s.first_score,
        "last_score": s.last_score,
        "last_rank": s.last_rank,
        "moved": s.moved,
        "frozen": s.frozen,
    })
}

/// `_build_record`: one name's row, its window re-split per duplicated board.
pub async fn build_record<S: Store>(
    s: &S,
    name: &str,
    groups: &[DupGroup],
    cfg: &Cfg,
    anchor: i64,
    now: i64,
    first_anchor_override: Option<i64>,
) -> DuplicateWrite {
    let window_start = anchor - cfg.lookback_days * 86400;
    let rows = match s.player_rows_window(name, window_start, anchor).await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!("duplicates: window rows failed for {}: {e:#}", kiwi_core::pystr::repr(name));
            Vec::new()
        }
    };
    let mut by_board: HashMap<i32, Vec<Point>> = HashMap::new();
    for r in rows {
        by_board.entry(r.leaderboard).or_default().push(Point {
            created_at: r.created_at,
            rank: i64::from(r.rank),
            score: r.score,
        });
    }
    let dup_uuids: BTreeSet<i32> = groups.iter().map(|g| g.board_uuid).collect();
    let mut reports: Vec<(i32, i64, Vec<SeriesSummary>)> = Vec::new();
    for &uuid in &dup_uuids {
        let pts = by_board.get(&uuid).map(Vec::as_slice).unwrap_or_default();
        let split = core::split_series(pts);
        let occurrences = groups.iter().find(|g| g.board_uuid == uuid).map_or(2, |g| i64::from(g.occurrences));
        reports.push((uuid, occurrences, core::summarise_series(&split, cfg.frozen_min_captures)));
    }
    let series_count = groups.iter().map(|g| i64::from(g.occurrences)).max().unwrap_or(2);
    let verdict = core::verdict(reports.iter().map(|r| r.2.as_slice()));
    let first_anchor = first_anchor_override
        .unwrap_or_else(|| reports.iter().flat_map(|r| r.2.iter().map(|s| s.first_anchor)).min().unwrap_or(anchor));
    let boards = dup_uuids.len() as i64;
    let board_values: Vec<Value> = reports
        .iter()
        .take(40)
        .map(|(uuid, occurrences, series)| {
            json!({
                "uuid": uuid,
                "name": cfg.board_names.get(uuid).cloned().unwrap_or_else(|| format!("Board #{uuid}")),
                "occurrences": occurrences,
                "series": series.iter().map(series_value).collect::<Vec<_>>(),
            })
        })
        .collect();
    let evidence = json!({
        "lookback_days": cfg.lookback_days,
        "boards": board_values,
        "summary": core::summary_text(name, boards, verdict, series_count),
    });
    DuplicateWrite {
        name_lower: lower_key(name),
        name: name.to_owned(),
        kind: "same_name".into(),
        verdict: verdict.into(),
        boards: boards as i32,
        max_occurrences: series_count as i32,
        spellings: "[]".into(),
        first_anchor,
        last_anchor: anchor,
        evidence: pyjson::dumps_spaced(&evidence),
        method_version: METHOD_VERSION as i32,
        updated_at: now,
    }
}

/// Groups by name in first-seen order.
fn by_name(groups: Vec<DupGroup>) -> Vec<(String, Vec<DupGroup>)> {
    let mut out: Vec<(String, Vec<DupGroup>)> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    for g in groups {
        match at.get(&g.name) {
            Some(&i) => out[i].1.push(g),
            None => {
                at.insert(g.name.clone(), out.len());
                out.push((g.name.clone(), vec![g]));
            }
        }
    }
    out
}

/// `detect_latest`: the warmer's live pass over the newest capture.
pub async fn detect_latest<S: Store>(
    s: &S,
    guard: &Mutex<Option<i64>>,
    clock: Clock,
    force: bool,
) -> anyhow::Result<Value> {
    if !s.flag(features::DUPLICATES_FLAG).await? {
        return Ok(json!({"status": "disabled", "detected": 0}));
    }
    let ts = s.latest_anchors(1).await?;
    let Some(&anchor) = ts.first() else {
        return Ok(json!({"status": "no_captures", "detected": 0}));
    };
    if !force && *guard.lock().unwrap_or_else(|e| e.into_inner()) == Some(anchor) {
        return Ok(json!({"status": "already_scanned", "anchor": anchor, "detected": 0}));
    }
    let cfg = load_config(s, Some(anchor)).await?;
    let named = by_name(s.duplicate_groups_at(anchor).await?);
    let candidates = named.len();
    let mut ordered: Vec<&(String, Vec<DupGroup>)> = named.iter().collect();
    ordered.sort_by_key(|(_, g)| std::cmp::Reverse(g.len()));
    ordered.retain(|(_, g)| g.len() as i64 >= cfg.min_boards);
    let now = int_secs(clock());
    let builds: Vec<_> = ordered[..py_slice_end(ordered.len(), cfg.max_names)]
        .iter()
        .map(|(name, g)| build_record(s, name, g, &cfg, anchor, now, None))
        .collect();
    let records: Vec<DuplicateWrite> = stream::iter(builds).buffered(RECORD_CONCURRENCY).collect().await;
    if !records.is_empty() {
        s.upsert_duplicates(&records).await?;
    }
    *guard.lock().unwrap_or_else(|e| e.into_inner()) = Some(anchor);
    tracing::info!("duplicates: live pass at {anchor} found {} duplicated name(s)", records.len());
    Ok(json!({"status": "ok", "anchor": anchor, "detected": records.len(), "candidates": candidates}))
}

async fn set_status<S: Store>(s: &S, status: &StatusDoc) {
    *LOCAL_STATUS.lock().unwrap_or_else(|e| e.into_inner()) = status.0.clone();
    s.save_status(STATUS_KEY, &status.dumps()).await;
}

/// `get_status`.
pub async fn get_status<S: Store>(s: &S) -> Map<String, Value> {
    super::load_status(s, STATUS_KEY, &LOCAL_STATUS).await
}

struct Tally {
    first: i64,
    last: i64,
    groups: Vec<DupGroup>,
}

struct Beat<'a, S> {
    s: &'a S,
    clock: Clock,
    last: f64,
}

impl<S: Store> Beat<'_, S> {
    /// `_heartbeat`: refresh the lock TTL at most every 120s of wall time.
    async fn beat(&mut self) {
        let t = (self.clock)();
        if t - self.last < 120.0 {
            return;
        }
        self.last = (self.clock)();
        self.s.heartbeat(RUNNING_KEY).await;
    }
}

/// `backfill`: date every duplication over the whole archive, then evidence
/// the names still current at the newest scanned anchor. Single-flight.
pub async fn backfill<S: Store>(s: &S, clock: Clock, clear_first: bool) {
    if s.lock(RUNNING_KEY).await == Some(false) {
        tracing::info!("duplicates backfill already running - skipping duplicate");
        return;
    }
    let mut status = StatusDoc::default();
    status.set("running", true);
    status.set("phase", "starting");
    status.set("total", 0);
    status.set("done", 0);
    status.set("detected", 0);
    status.set("started_at", int_secs(clock()));
    status.set("finished_at", Value::Null);
    status.set("last_anchor", Value::Null);
    status.set("clear_first", clear_first);
    let result = run(s, clock, clear_first, &mut status).await;
    if let Err(e) = &result {
        status.set("error", format!("{e:#}"));
        tracing::error!("duplicates backfill failed: {e:#}");
    }
    status.set("running", false);
    status.set("phase", if result.is_err() { "failed" } else { "done" });
    status.set("finished_at", int_secs(clock()));
    set_status(s, &status).await;
    s.unlock(RUNNING_KEY).await;
    let phase = if result.is_err() { "failed" } else { "done" };
    tracing::info!("duplicates backfill {phase}: {} name(s) recorded", status.int("detected"));
}

async fn run<S: Store>(s: &S, clock: Clock, clear_first: bool, status: &mut StatusDoc) -> anyhow::Result<()> {
    set_status(s, status).await;
    let anchors = s.all_anchors_asc().await?;
    let cfg = load_config(s, anchors.last().copied()).await?;
    status.set("phase", "dating");
    status.set("total", anchors.len());
    set_status(s, status).await;
    if clear_first {
        status.set("cleared", s.delete_all_duplicates().await?);
        set_status(s, status).await;
    }

    let mut tally: Vec<(String, Tally)> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    let mut dirty: Vec<usize> = Vec::new();
    let mut newest_scanned: Option<i64> = None;
    let mut beat = Beat { s, clock, last: clock() };

    async fn flush_dates<S: Store>(
        s: &S,
        clock: Clock,
        tally: &[(String, Tally)],
        dirty: &mut Vec<usize>,
    ) -> anyhow::Result<()> {
        if dirty.is_empty() {
            return Ok(());
        }
        let stamp = int_secs(clock());
        let rows: Vec<DateWrite> = dirty
            .iter()
            .map(|&i| {
                let (name, t) = &tally[i];
                DateWrite {
                    name_lower: lower_key(name),
                    name: name.clone(),
                    boards: t.groups.len() as i32,
                    max_occurrences: t.groups.iter().map(|g| g.occurrences).max().unwrap_or(2),
                    first_anchor: t.first,
                    last_anchor: t.last,
                    updated_at: stamp,
                }
            })
            .collect();
        s.upsert_duplicate_dates(&rows).await?;
        dirty.clear();
        Ok(())
    }

    for &anchor in &anchors {
        let groups = match s.duplicate_groups_at(anchor).await {
            Ok(g) => g,
            Err(e) => {
                tracing::warn!("duplicates backfill: anchor {anchor} failed: {e:#}");
                status.bump("done", 1);
                beat.beat().await;
                continue;
            }
        };
        newest_scanned = Some(anchor);
        for (name, g) in by_name(groups) {
            if (g.len() as i64) < cfg.min_boards {
                continue;
            }
            let i = match at.get(&name) {
                Some(&i) => {
                    tally[i].1.last = anchor;
                    tally[i].1.groups = g;
                    i
                }
                None => {
                    at.insert(name.clone(), tally.len());
                    tally.push((name, Tally { first: anchor, last: anchor, groups: g }));
                    tally.len() - 1
                }
            };
            if !dirty.contains(&i) {
                dirty.push(i);
            }
        }
        status.set("detected", tally.len());
        status.bump("done", 1);
        status.set("last_anchor", anchor);
        if status.int("done") % 25 == 0 {
            set_status(s, status).await;
        }
        beat.beat().await;
        if status.int("done") % 100 == 0 {
            flush_dates(s, clock, &tally, &mut dirty).await?;
        }
    }
    flush_dates(s, clock, &tally, &mut dirty).await?;

    let mut current: Vec<&(String, Tally)> = tally.iter().filter(|(_, t)| Some(t.last) == newest_scanned).collect();
    let skipped = tally.len() - current.len();
    current.sort_by_key(|(_, t)| std::cmp::Reverse(t.groups.len()));
    if current.len() as i64 > cfg.max_names {
        tracing::warn!(
            "duplicates backfill: {} current name(s) exceed duplicates_max_names ({}) - the rest are dated but get no evidence",
            current.len(),
            cfg.max_names
        );
    }
    let ordered = &current[..py_slice_end(current.len(), cfg.max_names)];
    if let Some(&last) = anchors.last()
        && newest_scanned != Some(last)
    {
        tracing::warn!(
            "duplicates backfill: newest anchor {last} failed to scan - 'current' is keyed on {newest_scanned:?} instead"
        );
    }
    tracing::info!(
        "duplicates backfill: {} name(s) dated, {} current get evidence, {skipped} historical dated without evidence",
        tally.len(),
        ordered.len()
    );
    status.set("skipped_historical", skipped);
    status.set("phase", "evidence");
    status.set("total", ordered.len());
    status.set("done", 0);
    set_status(s, status).await;
    let mut batch: Vec<DuplicateWrite> = Vec::new();
    for (name, t) in ordered {
        let built = build_record(s, name, &t.groups, &cfg, t.last, int_secs(clock()), Some(t.first)).await;
        batch.push(built);
        status.bump("done", 1);
        if batch.len() >= 50 {
            s.upsert_duplicates(&batch).await?;
            batch.clear();
            set_status(s, status).await;
        }
        beat.beat().await;
    }
    if !batch.is_empty() {
        s.upsert_duplicates(&batch).await?;
    }
    status.set("detected", tally.len());
    status.set("evidenced", ordered.len());
    Ok(())
}

// ---------------------------------------------------------------------------
// Serving

/// `_duplicate_row`: json columns are usually strings holding JSON.
pub fn row_value(r: &DuplicateRow) -> Value {
    fn decode(text: &str, fallback: Value) -> Value {
        match pyjson::loads(text.as_bytes()) {
            Ok(Value::String(inner)) => pyjson::loads(inner.as_bytes()).unwrap_or(fallback),
            Ok(Value::Null) | Err(_) => fallback,
            Ok(v) => v,
        }
    }
    let mut evidence = decode(&r.evidence, Value::Object(Map::new()));
    if !features::py_truthy(&evidence) {
        evidence = Value::Object(Map::new());
    }
    json!({
        "name": r.name,
        "kind": r.kind,
        "verdict": r.verdict,
        "boards": r.boards,
        "max_occurrences": r.max_occurrences,
        "spellings": decode(&r.spellings, Value::Array(Vec::new())),
        "first_anchor": r.first_anchor,
        "last_anchor": r.last_anchor,
        "evidence": evidence,
        "method_version": r.method_version,
        "updated_at": r.updated_at,
    })
}

/// `serve_list` (the dict `/site` sends).
pub async fn serve_list<S: Store>(s: &S, limit: i64, offset: i64, kind: Option<&str>) -> anyhow::Result<Value> {
    if !s.flag(features::DUPLICATES_FLAG).await? {
        return Ok(
            json!({"enabled": false, "duplicates": [], "total": 0, "current": 0, "limit": limit, "offset": offset, "method_version": METHOD_VERSION}),
        );
    }
    let (rows, total, current) = s.list_duplicates(limit, offset, kind.filter(|k| !k.is_empty())).await?;
    let latest = s.latest_duplicate_anchor().await?;
    let duplicates: Vec<Value> = rows.iter().map(row_value).collect();
    Ok(json!({
        "enabled": true,
        "duplicates": duplicates,
        "total": total,
        "current": current,
        "latest_anchor": latest,
        "limit": limit,
        "offset": offset,
        "method_version": METHOD_VERSION,
    }))
}

/// `for_name`.
pub async fn for_name<S: Store>(s: &S, name: &str) -> anyhow::Result<Value> {
    if !s.flag(features::DUPLICATES_FLAG).await? {
        return Ok(json!({"query": name, "found": false, "enabled": false}));
    }
    let Some(row) = s.duplicate_for_name(name).await? else {
        return Ok(json!({"query": name, "found": false, "enabled": true}));
    };
    let mut out = Map::new();
    out.insert("query".into(), name.into());
    out.insert("found".into(), true.into());
    out.insert("enabled".into(), true.into());
    if let Value::Object(fields) = row_value(&row) {
        out.extend(fields);
    }
    Ok(Value::Object(out))
}

fn group_model(row: &Value) -> Option<Value> {
    row["evidence"].as_object()?;
    row["spellings"].as_array()?.iter().all(Value::is_string).then(|| row.clone())
}

/// `DuplicatesResponse(**serve_list(...))`; `None` where pydantic would reject a row.
pub fn list_model(payload: &Value) -> Option<Value> {
    let rows: Option<Vec<Value>> = payload["duplicates"].as_array()?.iter().map(group_model).collect();
    Some(json!({
        "duplicates": rows?,
        "total": payload["total"],
        "current": payload["current"],
        "latest_anchor": payload.get("latest_anchor").cloned().unwrap_or(Value::Null),
        "limit": payload["limit"],
        "offset": payload["offset"],
        "enabled": payload["enabled"],
        "method_version": payload["method_version"],
    }))
}

/// `DuplicateLookupResponse(**for_name(...))`.
pub fn lookup_model(payload: &Value) -> Option<Value> {
    if payload["found"] == Value::Bool(true) {
        group_model(payload)?;
    }
    let field = |k: &str, default: Value| payload.get(k).cloned().unwrap_or(default);
    Some(json!({
        "query": payload["query"],
        "found": field("found", false.into()),
        "enabled": field("enabled", true.into()),
        "name": field("name", Value::Null),
        "kind": field("kind", Value::Null),
        "verdict": field("verdict", Value::Null),
        "boards": field("boards", Value::Null),
        "max_occurrences": field("max_occurrences", Value::Null),
        "spellings": field("spellings", json!([])),
        "first_anchor": field("first_anchor", Value::Null),
        "last_anchor": field("last_anchor", Value::Null),
        "evidence": field("evidence", json!({})),
        "method_version": field("method_version", Value::Null),
        "updated_at": field("updated_at", Value::Null),
    }))
}
