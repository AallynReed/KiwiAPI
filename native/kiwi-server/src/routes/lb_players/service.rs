//! `app/trove/leaderboards/service.py`'s player reads (`player_history`,
//! `player_boards`, `player_profile`, `player_history_series`) and the
//! profile extras they pull from `renames`, `duplicates` and `detection`.

use std::collections::{BTreeSet, HashMap, HashSet};

use kiwi_core::{
    feeds::py::int as py_int,
    lbplayers::{self as core, Point},
    pystr,
};
use serde_json::{Map, Value};

use super::store::{AggRow, BoardMeta, DuplicateRow, EntryRow, RenameEdge, Store, internal, py_lower};
use crate::{
    error::ApiResult,
    features::{ALT_CLUSTERS_FLAG, CHEATER_DETECTION_FLAG, DUPLICATES_FLAG, py_truthy},
    gamedata::Gamedata,
    routes::stats::classes::class_display_name,
};

/// `int(time.time())` for a present-day clock.
pub(crate) fn now_secs<S: Store>(s: &S) -> i64 {
    s.now().floor() as i64
}

/// Python `int(value)` of a runtime setting; `None` where it raises.
fn setting_int(v: &Value) -> Option<i64> {
    match v {
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().filter(|f| f.is_finite()).map(|f| f.trunc() as i64)),
        Value::Bool(b) => Some((*b).into()),
        Value::String(s) => py_int(s, 10).map(|i| i.clamp(i64::MIN.into(), i64::MAX.into()) as i64),
        _ => None,
    }
}

/// `hot_window_days`: the tiering age, the static default if the lookup fails.
pub(crate) async fn hot_window_days<S: Store>(s: &S) -> i64 {
    let days = match s.setting("leaderboards_pg_tier_after_days").await {
        Ok(v) => setting_int(&v).unwrap_or_else(|| s.default_tier_days()),
        Err(_) => s.default_tier_days(),
    };
    days.max(1)
}

/// `hot_window_start`: the oldest hot partition's trove-day start.
pub(crate) async fn hot_window_start<S: Store>(s: &S) -> i64 {
    let days = hot_window_days(s).await;
    core::cold_keep_from(now_secs(s), days)
}

/// `is_archive_query(anchor)`; `Err` where the setting lookup raises.
pub(crate) async fn is_archive_query<S: Store>(s: &S, anchor: i64) -> ApiResult<bool> {
    let v = s.setting("leaderboards_archive_query_threshold_days").await.map_err(internal)?;
    let days = v.as_f64().ok_or_else(|| internal("archive threshold is not a number"))?;
    Ok(anchor < (s.now() - days * 86_400.0).trunc() as i64)
}

/// The day-over-day movement `_attach_player_history_deltas` adds.
#[derive(Clone, Debug, PartialEq)]
pub struct Deltas {
    pub prev_rank: i64,
    pub prev_score: f64,
    pub rank_delta: i64,
    pub score_delta: f64,
}

/// A player-history row with the keys Python adds in place.
#[derive(Clone, Debug, PartialEq)]
pub struct HistRow {
    pub row: EntryRow,
    /// `(slot, slots)`.
    pub slot: Option<(i64, i64)>,
    pub deltas: Option<Deltas>,
}

fn point(r: &EntryRow) -> Point {
    Point { created_at: r.created_at, rank: r.rank, score: r.score }
}

/// Indices per board, boards in order of first appearance (a Python dict).
fn group_by_board<'a>(rows: impl Iterator<Item = &'a EntryRow>) -> Vec<(i64, Vec<usize>)> {
    let mut out: Vec<(i64, Vec<usize>)> = Vec::new();
    let mut at: HashMap<i64, usize> = HashMap::new();
    for (i, r) in rows.enumerate() {
        let slot = *at.entry(r.leaderboard).or_insert_with(|| {
            out.push((r.leaderboard, Vec::new()));
            out.len() - 1
        });
        out[slot].1.push(i);
    }
    out
}

/// `_attach_duplicate_slots`.
fn attach_duplicate_slots(rows: &mut [HistRow]) {
    for (_, idx) in group_by_board(rows.iter().map(|r| &r.row)) {
        let anchors: HashSet<i64> = idx.iter().map(|&i| rows[i].row.created_at).collect();
        if anchors.len() == idx.len() {
            for &i in &idx {
                rows[i].slot = Some((0, 1));
            }
            continue;
        }
        let pts: Vec<Point> = idx.iter().map(|&i| point(&rows[i].row)).collect();
        let split = core::split_series(&pts);
        let total = split.len() as i64;
        for s in &split {
            for &p in &s.points {
                rows[idx[p]].slot = Some((s.slot as i64, total));
            }
        }
    }
}

/// `_attach_player_history_deltas`.
async fn attach_history_deltas<S: Store>(s: &S, name: &str, rows: &mut [HistRow]) -> ApiResult<()> {
    let docs = s.player_rows_window(name, hot_window_start(s).await).await?;
    if docs.is_empty() {
        return Ok(());
    }
    type Series = Vec<(i64, i64, f64)>;
    let mut by_board: HashMap<i64, HashMap<usize, Series>> = HashMap::new();
    let mut slot_of: HashMap<(i64, i64, i64), usize> = HashMap::new();
    let mut slots_on: HashMap<i64, i64> = HashMap::new();
    let groups = group_by_board(docs.iter());
    for (board, idx) in &groups {
        let pts: Vec<Point> = idx.iter().map(|&i| point(&docs[i])).collect();
        let split = core::split_series(&pts);
        slots_on.insert(*board, split.len() as i64);
        let mut per_slot = HashMap::new();
        for sp in &split {
            let mut series: Series =
                sp.points.iter().map(|&p| (pts[p].created_at, pts[p].rank, pts[p].score)).collect();
            series.sort_by(|a, b| {
                b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal))
            });
            per_slot.insert(sp.slot, series);
            for &p in &sp.points {
                slot_of.insert((*board, pts[p].created_at, pts[p].rank), sp.slot);
            }
        }
        by_board.insert(*board, per_slot);
    }
    let order: Vec<i64> = groups.iter().map(|(b, _)| *b).collect();
    let kinds = s.board_kinds(&order).await?;
    for r in rows.iter_mut() {
        let board = r.row.leaderboard;
        let Some(per_slot) = by_board.get(&board).filter(|m| !m.is_empty()) else { continue };
        let slot = slot_of.get(&(board, r.row.created_at, r.row.rank)).copied().unwrap_or(0);
        r.slot = Some((slot as i64, slots_on.get(&board).copied().unwrap_or(1)));
        let Some(series) = per_slot.get(&slot).filter(|v| !v.is_empty()) else { continue };
        let day = core::trove_day_start(r.row.created_at);
        let Some(&(prev_at, prev_rank, prev_score)) = series.iter().find(|p| p.0 < day) else { continue };
        let kind = kinds.get(&board).map_or("default", String::as_str);
        if !core::reset_boundaries_for_kind(kind, prev_at, r.row.created_at).is_empty() {
            continue;
        }
        r.deltas = Some(Deltas {
            prev_rank,
            prev_score,
            rank_delta: prev_rank - r.row.rank,
            score_delta: r.row.score - prev_score,
        });
    }
    Ok(())
}

/// `player_history`: newest appearances, floored at the hot window.
pub(crate) async fn player_history<S: Store>(
    s: &S,
    player_name: &str,
    limit: i64,
    uuid: Option<i128>,
    with_deltas: bool,
    window_start: Option<i64>,
) -> ApiResult<Vec<HistRow>> {
    let hot = hot_window_start(s).await;
    let window_start = window_start.map_or(hot, |w| w.max(hot));
    let name = pystr::strip(player_name);
    let rows = s.player_rows(name, limit, uuid, Some(window_start)).await?;
    let mut out: Vec<HistRow> = rows.into_iter().map(|row| HistRow { row, slot: None, deltas: None }).collect();
    if !out.is_empty() {
        attach_duplicate_slots(&mut out);
        if with_deltas {
            attach_history_deltas(s, name, &mut out).await?;
        }
    }
    Ok(out)
}

/// The aggregate roll-up both `player_boards` and `player_profile` build.
#[derive(Clone, Debug, PartialEq)]
pub struct Boards {
    pub rows: Vec<AggRow>,
    pub meta: HashMap<i64, BoardMeta>,
}

impl Boards {
    pub fn meta(&self, uuid: i64) -> Option<&BoardMeta> {
        self.meta.get(&uuid)
    }

    /// `(b["best_rank"] or 1e9) <= limit` counted.
    pub fn best_within(&self, limit: i64) -> i64 {
        self.rows.iter().filter(|b| b.best_rank != 0 && b.best_rank <= limit).count() as i64
    }

    pub fn latest_anchor(&self) -> Option<i64> {
        self.rows.iter().map(|b| b.last_seen).max()
    }

    pub fn appearances(&self) -> i64 {
        self.rows.iter().map(|b| b.appearances).sum()
    }
}

/// `player_boards`.
pub struct PlayerBoards {
    pub player_name: String,
    pub boards: Boards,
}

pub(crate) async fn player_boards<S: Store>(s: &S, name: &str) -> ApiResult<PlayerBoards> {
    let name = pystr::strip(name);
    let (rows, stored) = tokio::join!(s.player_board_summary(name), s.player_canonical_name(name));
    let (rows, stored) = (rows?, stored?);
    let uuids: Vec<i64> = rows.iter().map(|b| b.leaderboard).collect::<BTreeSet<_>>().into_iter().collect();
    let meta = if uuids.is_empty() { HashMap::new() } else { s.board_meta(&uuids).await? };
    let player_name = stored.filter(|n| !n.is_empty()).unwrap_or_else(|| name.to_owned());
    Ok(PlayerBoards { player_name, boards: Boards { rows, meta } })
}

/// One class of `_profile_classes`.
pub struct ProfileClass {
    pub index: i64,
    pub name: Value,
    pub icon: Option<String>,
    pub power_rank: Option<i64>,
    pub effort: Option<i64>,
    pub paragon: Option<i64>,
}

/// `_profile_sigil`.
pub struct ProfileSigil {
    pub power_rank: i64,
    pub power_rank_board: i64,
    pub power_rank_class: Value,
    pub trove_mastery: i64,
    pub geode_mastery: i64,
}

/// Everything `player_profile` returns.
pub struct Profile {
    pub player_name: String,
    pub verified: bool,
    pub last_played: Option<i64>,
    pub boards: Boards,
    pub recent: Vec<HistRow>,
    pub renames: Option<Value>,
    pub alt_clusters: Vec<Value>,
    pub duplicate: Option<Value>,
    pub sigil: Option<ProfileSigil>,
    pub classes: Vec<ProfileClass>,
}

/// `class_name(i)` and `class_icon(i)`.
fn class_meta(gd: &Gamedata, i: i64) -> ApiResult<(Value, Option<String>)> {
    let tech = usize::try_from(i).ok().and_then(|i| kiwi_core::stats::BOARD_CLASS_ORDER.get(i));
    let name = match tech {
        Some(t) => class_display_name(gd, t)?,
        None => None,
    };
    Ok(match name {
        Some(n) => (n, tech.map(|t| format!("/static/class-icons/{t}.png"))),
        None => (Value::String(format!("Class {i}")), None),
    })
}

fn profile_classes(gd: &Gamedata, rows: &[AggRow]) -> ApiResult<Vec<ProfileClass>> {
    let scores: HashMap<i64, f64> = rows.iter().map(|b| (b.leaderboard, b.latest_score)).collect();
    core::profile_classes(&scores)
        .into_iter()
        .map(|c| {
            let (name, icon) = class_meta(gd, c.index)?;
            Ok(ProfileClass {
                index: c.index,
                name,
                icon,
                power_rank: c.power_rank,
                effort: c.effort,
                paragon: c.paragon,
            })
        })
        .collect()
}

fn profile_sigil(gd: &Gamedata, rows: &[AggRow]) -> ApiResult<Option<ProfileSigil>> {
    let scores: Vec<(i64, f64)> = rows.iter().map(|b| (b.leaderboard, b.latest_score)).collect();
    let Some(sig) = core::profile_sigil(&scores).map_err(|_| internal("sigil: non-finite score"))? else {
        return Ok(None);
    };
    let (class, _) = class_meta(gd, kiwi_core::stats::class_index_for_board(sig.power_rank_board))?;
    Ok(Some(ProfileSigil {
        power_rank: sig.power_rank,
        power_rank_board: sig.power_rank_board,
        power_rank_class: class,
        trove_mastery: sig.trove_mastery,
        geode_mastery: sig.geode_mastery,
    }))
}

/// `_profile_last_played`: the setting lookup may raise, the read may not.
async fn profile_last_played<S: Store>(s: &S, name: &str) -> ApiResult<Option<i64>> {
    let csv = match s.setting("last_played_excluded_board_uuids").await.map_err(internal)? {
        Value::String(v) => v,
        v if !py_truthy(&v) => String::new(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    };
    let excluded = core::parse_board_csv(&csv);
    Ok(s.player_last_played(name, &excluded).await.unwrap_or(None))
}

fn rename_edge(e: &RenameEdge) -> Value {
    serde_json::json!({
        "id": e.id,
        "from_name": e.from_name,
        "to_name": e.to_name,
        "from_anchor": e.from_anchor,
        "to_anchor": e.to_anchor,
        "confidence": e.confidence,
        "matched_boards": e.matched_boards,
        "evidence": e.evidence,
        "method_version": e.method_version,
        "created_at": e.created_at,
    })
}

/// `renames.history(name)`'s edge walk, both directions, 40 names at most.
async fn rename_edges<S: Store>(s: &S, name: &str) -> ApiResult<Vec<RenameEdge>> {
    let mut seen_names: HashSet<String> = HashSet::new();
    let mut seen_edges: HashSet<i64> = HashSet::new();
    let mut edges: Vec<RenameEdge> = Vec::new();
    let mut frontier = vec![py_lower(pystr::strip(name))];
    while seen_names.len() < 40 {
        let Some(cur) = frontier.pop() else { break };
        if !seen_names.insert(cur.clone()) {
            continue;
        }
        for e in s.renames_for_name(&cur).await? {
            for nxt in [py_lower(&e.from_name), py_lower(&e.to_name)] {
                if !seen_names.contains(&nxt) {
                    frontier.push(nxt);
                }
            }
            if seen_edges.insert(e.id) {
                edges.push(e);
            }
        }
    }
    edges.sort_by_key(|e| (e.to_anchor, e.id));
    Ok(edges)
}

/// `_profile_renames`: `None` without renames or on any failure.
async fn profile_renames<S: Store>(s: &S, name: &str) -> Option<Value> {
    let edges = rename_edges(s, name).await.ok()?;
    let last = edges.last()?;
    let aliases: BTreeSet<&str> = edges.iter().flat_map(|e| [e.from_name.as_str(), e.to_name.as_str()]).collect();
    Some(serde_json::json!({
        "aliases": aliases.into_iter().collect::<Vec<_>>(),
        "current_name": last.to_name,
        "count": edges.len(),
        "edges": edges.iter().map(rename_edge).collect::<Vec<_>>(),
    }))
}

/// `_profile_duplicate`: `duplicates.for_name` folded to the profile block.
async fn profile_duplicate<S: Store>(s: &S, name: &str) -> ApiResult<Option<Value>> {
    let lookup = async {
        if !py_truthy(&s.setting(DUPLICATES_FLAG).await.ok()?) {
            return None;
        }
        s.duplicate_for_name(name).await.ok()?
    };
    let Some(DuplicateRow {
        kind,
        verdict,
        boards,
        max_occurrences,
        spellings,
        first_anchor,
        last_anchor,
        evidence,
        ..
    }) = lookup.await
    else {
        return Ok(None);
    };
    let Value::Object(ev) = evidence else { return Err(internal("duplicate evidence is not a dict")) };
    Ok(Some(serde_json::json!({
        "kind": kind,
        "verdict": verdict,
        "boards": boards,
        "max_occurrences": max_occurrences,
        "spellings": spellings,
        "first_anchor": first_anchor,
        "last_anchor": last_anchor,
        "summary": ev.get("summary").cloned().unwrap_or(Value::Null),
    })))
}

/// `detect_possible_cheaters()`'s serving path as far as the profile reads
/// it: the persisted snapshot of the published anchor, `None` otherwise.
async fn served_cheaters<S: Store>(s: &S) -> Option<Value> {
    let cheater = py_truthy(&s.setting(CHEATER_DETECTION_FLAG).await.ok()?);
    let clusters = py_truthy(&s.setting(ALT_CLUSTERS_FLAG).await.ok()?);
    if !cheater && !clusters {
        return None;
    }
    let anchor = match s.ready_anchor().await {
        Some(a) => a,
        None => s.latest_anchor().await.ok()??,
    };
    s.cheaters_payload(anchor).await
}

const CLUSTER_KEEP: [&str; 10] = [
    "stem",
    "label",
    "method",
    "confidence",
    "member_count",
    "members",
    "members_truncated",
    "board_count",
    "corroborated_by",
    "summary",
];

/// Python iteration of a JSON value (`for x in v`), `Err` where it raises.
fn py_iter(v: &Value) -> ApiResult<Vec<Value>> {
    match v {
        _ if !py_truthy(v) => Ok(Vec::new()),
        Value::Array(a) => Ok(a.clone()),
        Value::Object(m) => Ok(m.keys().map(|k| Value::String(k.clone())).collect()),
        Value::String(s) => Ok(s.chars().map(|c| Value::String(c.to_string())).collect()),
        _ => Err(internal("not iterable")),
    }
}

/// `_profile_alt_clusters(name, [])`.
async fn profile_alt_clusters<S: Store>(s: &S, name: &str) -> ApiResult<Vec<Value>> {
    let Some(payload) = served_cheaters(s).await else { return Ok(Vec::new()) };
    let Value::Object(payload) = payload else { return Err(internal("cheaters payload is not a dict")) };
    let wanted = py_lower(name);
    let mut out = Vec::new();
    for c in py_iter(payload.get("clusters").unwrap_or(&Value::Null))? {
        let Value::Object(c) = c else { return Err(internal("cluster is not a dict")) };
        let mut hit = false;
        for m in py_iter(c.get("members").unwrap_or(&Value::Null))? {
            let m = match &m {
                v if !py_truthy(v) => String::new(),
                Value::String(m) => m.clone(),
                _ => return Err(internal("cluster member is not a str")),
            };
            if py_lower(&m) == wanted {
                hit = true;
                break;
            }
        }
        if hit {
            let kept: Map<String, Value> =
                CLUSTER_KEEP.iter().filter_map(|k| c.get(*k).map(|v| ((*k).to_owned(), v.clone()))).collect();
            out.push(Value::Object(kept));
        }
    }
    Ok(out)
}

/// `player_profile`. `extras` adds what only `/site` shows (renames and alt
/// clusters); `/v1` drops both, so it skips their lookups.
pub(crate) async fn player_profile<S: Store>(s: &S, gd: &Gamedata, name: &str, extras: bool) -> ApiResult<Profile> {
    let name = pystr::strip(name);
    let claimed = py_lower(pystr::strip(name));
    let (rows, stored, verified, renames, alt_clusters, last_played, recent, duplicate) = tokio::join!(
        s.player_board_summary(name),
        s.player_canonical_name(name),
        s.claim_verified(&claimed),
        async {
            match extras {
                true => profile_renames(s, name).await,
                false => None,
            }
        },
        async {
            match extras {
                true => profile_alt_clusters(s, name).await,
                false => Ok(Vec::new()),
            }
        },
        profile_last_played(s, name),
        player_history(s, name, 200, None, true, None),
        profile_duplicate(s, name),
    );
    let (rows, stored, alt_clusters, last_played, recent, duplicate) =
        (rows?, stored?, alt_clusters?, last_played?, recent?, duplicate?);
    let player_name = match stored.filter(|n| !n.is_empty()) {
        Some(n) => n,
        None => recent.first().map_or_else(|| name.to_owned(), |r| r.row.player_name.clone()),
    };
    let uuids: Vec<i64> = recent
        .iter()
        .map(|r| r.row.leaderboard)
        .chain(rows.iter().map(|b| b.leaderboard))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let meta = if uuids.is_empty() { HashMap::new() } else { s.board_meta(&uuids).await? };
    let sigil = if duplicate.is_some() { None } else { profile_sigil(gd, &rows)? };
    let classes = profile_classes(gd, &rows)?;
    Ok(Profile {
        player_name,
        verified,
        last_played,
        boards: Boards { rows, meta },
        recent,
        renames,
        alt_clusters,
        duplicate,
        sigil,
        classes,
    })
}

/// One line of `player_history_series`.
pub struct SeriesLine {
    pub uuid: i64,
    pub name: String,
    pub best_rank: i64,
    pub slot: i64,
    pub slots: i64,
    pub points: Vec<(Point, bool)>,
}

/// `player_history_series`; `series` is `None` for the early return (no rows).
pub struct Series {
    pub player_name: String,
    pub canonical_name: String,
    pub days: i64,
    pub window_start: i64,
    pub window_end: i64,
    pub duplicate_boards: i64,
    pub anchors: Vec<i64>,
    pub series: Option<Vec<SeriesLine>>,
}

pub(crate) async fn player_history_series<S: Store>(s: &S, player_name: &str, days: i64) -> ApiResult<Series> {
    let days = days.clamp(1, 30);
    let now = now_secs(s);
    let window_start = (now - days * core::DAY).max(hot_window_start(s).await);
    let name = pystr::strip(player_name).to_owned();
    let all_rows = s.player_rows_window(&name, window_start).await?;
    let mut out = Series {
        player_name: name.clone(),
        canonical_name: name,
        days,
        window_start,
        window_end: now,
        duplicate_boards: 0,
        anchors: Vec::new(),
        series: None,
    };
    let Some(newest) = all_rows.iter().reduce(|a, b| if b.created_at > a.created_at { b } else { a }) else {
        return Ok(out);
    };
    out.canonical_name = newest.player_name.clone();
    let mut by_board: Vec<(i64, Vec<Point>)> = group_by_board(all_rows.iter())
        .into_iter()
        .map(|(b, idx)| (b, idx.iter().map(|&i| point(&all_rows[i])).collect()))
        .collect();
    for (_, pts) in by_board.iter_mut() {
        pts.sort_by_key(|p| p.created_at);
    }
    out.anchors = all_rows.iter().map(|r| r.created_at).collect::<BTreeSet<_>>().into_iter().collect();
    let order: Vec<i64> = by_board.iter().map(|(b, _)| *b).collect();
    let meta = s.board_meta(&order).await?;
    let mut lines = Vec::new();
    for (uuid, pts) in &by_board {
        let (name, kind) = match meta.get(uuid) {
            Some(m) => (m.name.clone(), m.reset_kind.as_str()),
            None => (format!("Board #{uuid}"), "default"),
        };
        let split = core::split_series(pts);
        if split.len() > 1 {
            out.duplicate_boards += 1;
        }
        for sp in &split {
            let spts: Vec<Point> = sp.points.iter().map(|&i| pts[i]).collect();
            lines.push(SeriesLine {
                uuid: *uuid,
                name: name.clone(),
                best_rank: spts.iter().map(|p| p.rank).min().unwrap_or_default(),
                slot: sp.slot as i64,
                slots: split.len() as i64,
                points: core::inject_reset_zeros(&spts, kind),
            });
        }
    }
    lines.sort_by_key(|l| (l.best_rank, l.uuid, l.slot));
    out.series = Some(lines);
    Ok(out)
}
