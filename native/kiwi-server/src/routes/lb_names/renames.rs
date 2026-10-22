//! `app/trove/leaderboards/renames.py`: fingerprints from the store, the live
//! pass the warmer runs ([`detect_latest`]), the archive backfill and the reads.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{LazyLock, Mutex};

use kiwi_core::renames::{self as core, Fps, MatchCfg, Named};
use serde_json::{Map, Value, json};

use super::store::{
    RenameRow, RenameWrite, Store, board_label, float_setting, int_setting, is_lifetime_kind, is_player_board,
    reset_kind,
};
use super::{Clock, StatusDoc, int_secs, lower_key};
use crate::{features, feeds::pyval, pyjson};

pub const METHOD_VERSION: i64 = 1;
pub const STATUS_KEY: &str = "lb:renames:status";
pub const RUNNING_KEY: &str = "lb:renames:running";

/// Name keys by player id, kept for the process like Python's `_KEYS`: ids
/// only ever gain keys, so fingerprints taken across a walk still compare. A
/// stored spelling only ever changes case, so a player's key never moves.
#[derive(Default)]
pub struct Keys {
    key_of: HashMap<i64, i64>,
    index: HashMap<String, i64>,
    keys: Vec<String>,
}

impl Keys {
    fn add(&mut self, id: i64, name: &str) {
        let key = lower_key(name);
        let next = self.keys.len() as i64;
        let k = *self.index.entry(key.clone()).or_insert_with(|| next);
        if k == next {
            self.keys.push(key);
        }
        self.key_of.insert(id, k);
    }
}

pub static KEYS: LazyLock<Mutex<Keys>> = LazyLock::new(Mutex::default);
/// `_last_scanned_pair`.
pub static LAST_PAIR: Mutex<Option<(i64, i64)>> = Mutex::new(None);
static LOCAL_STATUS: LazyLock<Mutex<Map<String, Value>>> =
    LazyLock::new(|| Mutex::new(Map::from_iter([("running".to_string(), Value::Bool(false))])));

#[cfg(test)]
pub(crate) fn reset_local_status() {
    *LOCAL_STATUS.lock().unwrap_or_else(|e| e.into_inner()) =
        Map::from_iter([("running".to_string(), Value::Bool(false))]);
}

/// Drops the in-process scan guard (`renames.reset()`, on a full leaderboards reset).
pub fn reset_guard() {
    *LAST_PAIR.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

pub struct Cfg {
    pub max_gap: i64,
    pub min_boards: i64,
    pub drift: f64,
    pub min_score: f64,
    pub min_conf: f64,
    pub excluded: HashSet<i128>,
    pub lifetime_uuids: Vec<i32>,
    pub board_names: HashMap<i32, String>,
}

/// `_parse_excluded`.
fn parse_excluded(csv: &str) -> HashSet<i128> {
    let mut out = HashSet::new();
    for tok in csv.split(',') {
        let s = kiwi_core::pystr::strip(tok);
        if s.is_empty() {
            continue;
        }
        match kiwi_core::feeds::py::int(s, 10) {
            Some(i) => {
                out.insert(i);
            }
            None => tracing::warn!("renames: ignoring non-numeric excluded-board token {}", kiwi_core::pystr::repr(s)),
        }
    }
    out
}

/// `_load_config`.
pub async fn load_config<S: Store>(s: &S, anchor: Option<i64>) -> anyhow::Result<Cfg> {
    let excluded = parse_excluded(&pyval::py_str(&s.setting("renames_excluded_board_uuids").await?));
    let mut cfg = Cfg {
        max_gap: int_setting(s, "renames_max_gap_seconds").await?,
        min_boards: int_setting(s, "renames_min_boards").await?,
        drift: float_setting(s, "renames_score_drift_pct").await?,
        min_score: float_setting(s, "renames_min_score").await?,
        min_conf: float_setting(s, "renames_min_confidence").await?,
        excluded,
        lifetime_uuids: Vec::new(),
        board_names: HashMap::new(),
    };
    if let Some(anchor) = anchor {
        for b in s.boards_at(anchor).await? {
            if cfg.excluded.contains(&i128::from(b.uuid))
                || !is_player_board(b.uuid)
                || !is_lifetime_kind(reset_kind(b.reset_kind_override.as_deref(), b.uuid))
            {
                continue;
            }
            cfg.lifetime_uuids.push(b.uuid);
            cfg.board_names.insert(b.uuid, board_label(&b));
        }
    }
    Ok(cfg)
}

fn gap_ok(cfg: &Cfg, a: i64, b: i64) -> bool {
    0 < b - a && b - a <= cfg.max_gap
}

/// `_fingerprints`: name key -> lifetime scores at `anchor`, scores of at
/// least `min_score` only. Players without a `player` row are left out, as
/// the Python join does.
pub async fn fingerprints<S: Store>(s: &S, keys: &Mutex<Keys>, anchor: i64, cfg: &Cfg) -> anyhow::Result<Fps> {
    if cfg.lifetime_uuids.is_empty() {
        return Ok(Fps::default());
    }
    let cap = s.capture(anchor, &cfg.lifetime_uuids).await?;
    let missing: Vec<i64> = {
        let k = keys.lock().unwrap_or_else(|e| e.into_inner());
        let mut ids: Vec<i64> = cap
            .boards
            .values()
            .flat_map(|m| m.iter())
            .filter(|(id, score)| **score >= cfg.min_score && !k.key_of.contains_key(id))
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    };
    if !missing.is_empty() {
        let names = s.player_names(&missing).await?;
        let mut k = keys.lock().unwrap_or_else(|e| e.into_inner());
        for (id, name) in names {
            k.add(id, &name);
        }
    }
    let k = keys.lock().unwrap_or_else(|e| e.into_inner());
    Ok(core::fingerprint_known(&cap, cfg.min_score, |id| k.key_of.get(&id).copied()))
}

/// `_diff` + `_match` for one pair, as the rows `upsert_renames` writes. The
/// display names are read fresh: Python rebuilds its name cache whenever a
/// capture holds an unseen player, which is nearly every capture.
pub async fn diff_and_match<S: Store>(
    s: &S,
    keys: &Mutex<Keys>,
    fa: &Fps,
    fb: &Fps,
    (a, b): (i64, i64),
    cfg: &Cfg,
    now: i64,
) -> anyhow::Result<Vec<RenameWrite>> {
    let (only_a, only_b) = core::diff(fa, fb, usize::try_from(cfg.min_boards.max(1)).unwrap_or(usize::MAX));
    if only_a.is_empty() || only_b.is_empty() {
        return Ok(Vec::new());
    }
    let mut ids: Vec<i64> = only_a.iter().chain(&only_b).flat_map(|row| row.2.iter().copied()).collect();
    ids.sort_unstable();
    ids.dedup();
    let names: HashMap<i64, String> = s.player_names(&ids).await?.into_iter().collect();
    let k = keys.lock().unwrap_or_else(|e| e.into_inner());
    let side = |rows: core::Side| -> BTreeMap<String, Named> {
        rows.into_iter()
            .map(|(key, boards, ids)| {
                let display = ids.iter().filter_map(|i| names.get(i)).min().map(String::from).unwrap_or_default();
                (k.keys[key as usize].clone(), Named { display, boards: boards.into_iter().collect() })
            })
            .collect()
    };
    let (da, db) = (side(only_a), side(only_b));
    drop(k);
    let mcfg = MatchCfg { min_boards: cfg.min_boards, drift: cfg.drift, min_conf: cfg.min_conf };
    Ok(to_writes(core::match_pair(&da, &db, &mcfg), a, b, &cfg.board_names, now))
}

/// `_match`'s event dicts as `upsert_renames` parameter rows.
pub fn to_writes(
    matches: Vec<core::Rename>,
    a: i64,
    b: i64,
    board_names: &HashMap<i32, String>,
    now: i64,
) -> Vec<RenameWrite> {
    matches
        .into_iter()
        .map(|m| {
            let boards: Vec<Value> = m
                .boards
                .iter()
                .map(|bm| {
                    json!({
                        "uuid": bm.uuid,
                        "name": board_names.get(&bm.uuid).cloned().unwrap_or_else(|| bm.uuid.to_string()),
                        "score_from": bm.score_from,
                        "score_to": bm.score_to,
                        "drift_pct": bm.drift_pct,
                    })
                })
                .collect();
            let t = &m.terms;
            let evidence = json!({
                "gap_seconds": b - a,
                "boards": boards,
                "terms": {
                    "matched_boards": t.matched_boards,
                    "runnerup_overlap": t.runnerup_overlap,
                    "board_term": t.board_term,
                    "tightness": t.tightness,
                    "exclusivity": t.exclusivity,
                    "rarity": t.rarity,
                    "mean_drift_pct": t.mean_drift_pct,
                    "confidence": t.confidence,
                },
                "summary": m.summary,
            });
            RenameWrite {
                from_name_lower: lower_key(&m.from_name),
                to_name_lower: lower_key(&m.to_name),
                from_name: m.from_name,
                to_name: m.to_name,
                from_anchor: a,
                to_anchor: b,
                confidence: m.confidence,
                matched_boards: m.matched_boards as i32,
                evidence: pyjson::dumps_spaced(&evidence),
                method_version: METHOD_VERSION as i32,
                created_at: now,
            }
        })
        .collect()
}

/// `_process_pair`: the events and both fingerprints (the later one seeds the next pair).
async fn process_pair<S: Store>(
    s: &S,
    keys: &Mutex<Keys>,
    a: i64,
    b: i64,
    cfg: &Cfg,
    now: i64,
    fp_a: Option<Fps>,
) -> anyhow::Result<(Vec<RenameWrite>, Fps)> {
    let fa = match fp_a {
        Some(f) => f,
        None => fingerprints(s, keys, a, cfg).await?,
    };
    let fb = fingerprints(s, keys, b, cfg).await?;
    Ok((diff_and_match(s, keys, &fa, &fb, (a, b), cfg, now).await?, fb))
}

/// `detect_latest`: the warmer's live pass over the newest adjacent pair.
pub async fn detect_latest<S: Store>(
    s: &S,
    keys: &Mutex<Keys>,
    guard: &Mutex<Option<(i64, i64)>>,
    clock: Clock,
    force: bool,
) -> anyhow::Result<Value> {
    if !s.flag(features::RENAMES_FLAG).await? {
        return Ok(json!({"status": "disabled", "detected": 0}));
    }
    let ts = s.latest_anchors(2).await?;
    if ts.len() < 2 {
        return Ok(json!({"status": "insufficient_history", "detected": 0}));
    }
    let (b, a) = (ts[0], ts[1]);
    if !force && *guard.lock().unwrap_or_else(|e| e.into_inner()) == Some((a, b)) {
        return Ok(json!({"status": "already_scanned", "from_anchor": a, "to_anchor": b, "detected": 0}));
    }
    let cfg = load_config(s, Some(b)).await?;
    if cfg.lifetime_uuids.is_empty() {
        return Ok(json!({"status": "no_lifetime_boards", "detected": 0}));
    }
    if !gap_ok(&cfg, a, b) {
        *guard.lock().unwrap_or_else(|e| e.into_inner()) = Some((a, b));
        return Ok(
            json!({"status": "gap_too_large", "from_anchor": a, "to_anchor": b, "gap_seconds": b - a, "detected": 0}),
        );
    }
    let now = int_secs(clock());
    let (events, _) = process_pair(s, keys, a, b, &cfg, now, None).await?;
    if !events.is_empty() {
        s.upsert_renames(&events).await?;
    }
    *guard.lock().unwrap_or_else(|e| e.into_inner()) = Some((a, b));
    tracing::info!("renames: live pass {a}->{b} found {} rename(s)", events.len());
    Ok(json!({"status": "ok", "from_anchor": a, "to_anchor": b, "gap_seconds": b - a, "detected": events.len()}))
}

async fn set_status<S: Store>(s: &S, status: &StatusDoc) {
    *LOCAL_STATUS.lock().unwrap_or_else(|e| e.into_inner()) = status.0.clone();
    s.save_status(STATUS_KEY, &status.dumps()).await;
}

/// `get_status`: the shared backfill progress, else this process's last one.
pub async fn get_status<S: Store>(s: &S) -> Map<String, Value> {
    super::load_status(s, STATUS_KEY, &LOCAL_STATUS).await
}

/// `backfill`: every adjacent pair within the gap, oldest first, single-flight.
pub async fn backfill<S: Store>(s: &S, keys: &Mutex<Keys>, clock: Clock, clear_first: bool) {
    if s.lock(RUNNING_KEY).await == Some(false) {
        tracing::info!("renames backfill already running - skipping duplicate");
        return;
    }
    // Python reads these before its try, so a failure here keeps the lock until its TTL.
    let setup = async {
        let anchors = s.all_anchors_asc().await?;
        let cfg = load_config(s, anchors.last().copied()).await?;
        anyhow::Ok((anchors, cfg))
    };
    let (anchors, cfg) = match setup.await {
        Ok(v) => v,
        Err(e) => return tracing::error!("renames backfill failed: {e:#}"),
    };
    let now = int_secs(clock());
    let mut status = StatusDoc::default();
    status.set("running", true);
    status.set("total", anchors.len().saturating_sub(1));
    status.set("done", 0);
    status.set("detected", 0);
    status.set("skipped_gap", 0);
    status.set("started_at", now);
    status.set("finished_at", Value::Null);
    status.set("last_anchor", Value::Null);
    status.set("clear_first", clear_first);
    status.set("lifetime_boards", cfg.lifetime_uuids.len());
    set_status(s, &status).await;

    let result = walk(s, keys, &anchors, &cfg, now, clear_first, &mut status).await;
    status.set("running", false);
    status.set("phase", "done");
    status.set("finished_at", int_secs(clock()));
    set_status(s, &status).await;
    s.unlock(RUNNING_KEY).await;
    if let Err(e) = result {
        tracing::error!("renames backfill failed: {e:#}");
    }
    tracing::info!(
        "renames backfill done: {} detected across {} pairs ({} skipped for gap)",
        status.int("detected"),
        status.int("done"),
        status.int("skipped_gap")
    );
}

async fn walk<S: Store>(
    s: &S,
    keys: &Mutex<Keys>,
    anchors: &[i64],
    cfg: &Cfg,
    now: i64,
    clear_first: bool,
    status: &mut StatusDoc,
) -> anyhow::Result<()> {
    if clear_first {
        let deleted = s.delete_all_renames().await?;
        status.set("cleared", deleted);
        set_status(s, status).await;
    }
    if cfg.lifetime_uuids.is_empty() {
        tracing::warn!("renames backfill: no lifetime boards resolved - nothing to do");
        return Ok(());
    }
    let mut batch: Vec<RenameWrite> = Vec::new();
    let mut prev: Option<(i64, Fps)> = None;
    for pair in anchors.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if !gap_ok(cfg, a, b) {
            status.bump("skipped_gap", 1);
            status.bump("done", 1);
            prev = None;
            continue;
        }
        let fp_a = match prev.take() {
            Some((anchor, fp)) if anchor == a => Some(fp),
            _ => None,
        };
        match process_pair(s, keys, a, b, cfg, now, fp_a).await {
            Ok((events, fb)) => {
                status.bump("detected", events.len() as i64);
                status.bump("done", 1);
                status.set("last_anchor", b);
                batch.extend(events);
                prev = Some((b, fb));
            }
            Err(e) => {
                tracing::warn!("renames backfill: pair {a}->{b} failed: {e:#}");
                status.bump("done", 1);
                prev = None;
                continue;
            }
        }
        if batch.len() >= 500 {
            s.upsert_renames(&batch).await?;
            batch.clear();
        }
        if status.int("done") % 50 == 0 {
            set_status(s, status).await;
            s.heartbeat(RUNNING_KEY).await;
        }
    }
    if !batch.is_empty() {
        s.upsert_renames(&batch).await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Serving

/// `_rename_row`: the evidence jsonb is usually a string holding JSON
/// (double-encoded on write); a real object passes through.
pub fn row_value(r: &RenameRow) -> Value {
    let mut ev = pyjson::loads(r.evidence.as_bytes()).unwrap_or(Value::Null);
    if let Value::String(inner) = &ev {
        ev = pyjson::loads(inner.as_bytes()).unwrap_or_else(|_| Value::Object(Map::new()));
    }
    if !features::py_truthy(&ev) {
        ev = Value::Object(Map::new());
    }
    json!({
        "id": r.id,
        "from_name": r.from_name,
        "to_name": r.to_name,
        "from_anchor": r.from_anchor,
        "to_anchor": r.to_anchor,
        "confidence": r.confidence,
        "matched_boards": r.matched_boards,
        "evidence": ev,
        "method_version": r.method_version,
        "created_at": r.created_at,
    })
}

/// `serve_list` (the dict `/site` sends).
pub async fn serve_list<S: Store>(s: &S, limit: i64, offset: i64) -> anyhow::Result<Value> {
    if !s.flag(features::RENAMES_FLAG).await? {
        return Ok(
            json!({"enabled": false, "renames": [], "total": 0, "limit": limit, "offset": offset, "method_version": METHOD_VERSION}),
        );
    }
    let (rows, total) = s.list_renames(limit, offset).await?;
    let renames: Vec<Value> = rows.iter().map(row_value).collect();
    Ok(
        json!({"enabled": true, "renames": renames, "total": total, "limit": limit, "offset": offset, "method_version": METHOD_VERSION}),
    )
}

/// `history`: every edge reachable from `name` in both directions, at most
/// `max_nodes` names visited, oldest edge first.
pub async fn history<S: Store>(s: &S, name: &str, max_nodes: usize) -> anyhow::Result<Value> {
    let mut seen_names: HashSet<String> = HashSet::new();
    let mut seen_edges: HashSet<i64> = HashSet::new();
    let mut edges: Vec<RenameRow> = Vec::new();
    let mut frontier = vec![lower_key(name)];
    while seen_names.len() < max_nodes {
        let Some(cur) = frontier.pop() else { break };
        if seen_names.contains(&cur) {
            continue;
        }
        let found = s.renames_for_name(&lower_key(&cur)).await?;
        seen_names.insert(cur);
        for e in found {
            for next in [e.from_name.to_lowercase(), e.to_name.to_lowercase()] {
                if !seen_names.contains(&next) {
                    frontier.push(next);
                }
            }
            if seen_edges.insert(e.id) {
                edges.push(e);
            }
        }
    }
    edges.sort_by_key(|e| (e.to_anchor, e.id));
    let mut aliases: Vec<&String> = edges.iter().flat_map(|e| [&e.from_name, &e.to_name]).collect();
    aliases.sort();
    aliases.dedup();
    let current = edges.last().map_or(name, |e| e.to_name.as_str());
    let rows: Vec<Value> = edges.iter().map(row_value).collect();
    Ok(json!({
        "query": name,
        "aliases": aliases,
        "current_name": current,
        "edges": rows,
        "rename_count": edges.len(),
    }))
}

fn event_model(row: &Value) -> Option<Value> {
    row.get("evidence")?.as_object()?;
    Some(row.clone())
}

/// `RenamesResponse(**serve_list(...))`; `None` where pydantic would reject a row.
pub fn list_model(payload: &Value) -> Option<Value> {
    let renames: Option<Vec<Value>> = payload["renames"].as_array()?.iter().map(event_model).collect();
    Some(json!({
        "renames": renames?,
        "total": payload["total"],
        "limit": payload["limit"],
        "offset": payload["offset"],
        "enabled": payload["enabled"],
        "method_version": payload["method_version"],
    }))
}

/// `RenameHistoryResponse(**history(...))`.
pub fn history_model(payload: &Value) -> Option<Value> {
    let edges: Option<Vec<Value>> = payload["edges"].as_array()?.iter().map(event_model).collect();
    Some(json!({
        "query": payload["query"],
        "current_name": payload["current_name"],
        "aliases": payload["aliases"],
        "edges": edges?,
        "rename_count": payload["rename_count"],
    }))
}

/// `history`'s default bound.
pub const MAX_NODES: usize = 40;
