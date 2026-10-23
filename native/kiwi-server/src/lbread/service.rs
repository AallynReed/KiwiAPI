//! `app/trove/leaderboards/service.py` read side, plus the `/v1` response
//! models those payloads are re-validated through (field order, filtering,
//! int-to-float on float fields).

use std::collections::HashMap;

use kiwi_core::{
    gems::pynum::round_int,
    lbread::{self as core, DAY, HealthItem, Point},
};
use serde_json::{Map, Value, json};

use super::{
    cache::{self, Kv},
    store::{Board, EntryRow, Res, Source, py_lower},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Comparison {
    pub comparable: bool,
    pub prev_anchor: Option<i64>,
    pub reason: &'static str,
}

impl Comparison {
    pub const NO_PRIOR: Comparison = Comparison { comparable: false, prev_anchor: None, reason: "no_prior_snapshot" };

    pub fn to_value(self) -> Value {
        json!({"comparable": self.comparable, "prev_anchor": self.prev_anchor, "reason": self.reason})
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Delta {
    pub is_new: bool,
    pub prev_rank: Option<i64>,
    pub prev_score: Option<f64>,
    pub rank_delta: Option<i64>,
    pub score_delta: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub row: EntryRow,
    /// Set only on a comparable page.
    pub delta: Option<Delta>,
}

impl Entry {
    /// The service dict: delta keys only when the page compares.
    pub fn to_value(&self) -> Value {
        let mut m = Map::new();
        m.insert("player_name".into(), json!(self.row.player_name));
        m.insert("rank".into(), json!(self.row.rank));
        m.insert("score".into(), json!(self.row.score));
        if let Some(d) = self.delta {
            m.insert("is_new".into(), json!(d.is_new));
            m.insert("prev_rank".into(), json!(d.prev_rank));
            m.insert("prev_score".into(), json!(d.prev_score));
            m.insert("rank_delta".into(), json!(d.rank_delta));
            m.insert("score_delta".into(), json!(d.score_delta));
        }
        Value::Object(m)
    }

    /// `LeaderboardEntryOut`.
    pub fn v1_value(&self) -> Value {
        let d = self.delta.unwrap_or(Delta {
            is_new: false,
            prev_rank: None,
            prev_score: None,
            rank_delta: None,
            score_delta: None,
        });
        json!({
            "rank": self.row.rank, "player_name": self.row.player_name, "score": self.row.score,
            "prev_rank": d.prev_rank, "prev_score": d.prev_score, "rank_delta": d.rank_delta,
            "score_delta": d.score_delta, "is_new": d.is_new,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EntriesPage {
    pub items: Vec<Entry>,
    pub total: i64,
    pub comparison: Comparison,
}

impl EntriesPage {
    /// The `lb:cache:entries:*` payload.
    pub fn cache_value(&self) -> Value {
        json!({
            "items": self.items.iter().map(Entry::to_value).collect::<Vec<_>>(),
            "total": self.total,
            "comparison": self.comparison.to_value(),
        })
    }
}

/// `list_entries_with_deltas`: one page plus day-over-day movement when the
/// previous trove-day's capture is comparable (no reset crossed).
pub async fn list_entries_with_deltas<S: Source>(
    src: &S,
    uuid: i32,
    anchor: i64,
    limit: i64,
    offset: i64,
) -> Res<EntriesPage> {
    let (rows, total) = src.list_entries(uuid, anchor, limit, offset).await?;
    let mut items: Vec<Entry> = rows.into_iter().map(|row| Entry { row, delta: None }).collect();
    let Some(prev_anchor) = src.previous_day_anchor(uuid, anchor).await? else {
        return Ok(EntriesPage { items, total, comparison: Comparison::NO_PRIOR });
    };
    let kind = src.board_reset_kind(uuid).await?;
    if !core::reset_boundaries_for_kind(kind, prev_anchor, anchor).is_empty() {
        let comparison = Comparison { comparable: false, prev_anchor: Some(prev_anchor), reason: "crossed_reset" };
        return Ok(EntriesPage { items, total, comparison });
    }
    if !items.is_empty() {
        let names: Vec<String> = items.iter().map(|it| it.row.player_name.clone()).collect();
        let prev = src.prev_rows_for_players(uuid, prev_anchor, &names).await?;
        for it in &mut items {
            it.delta = Some(match prev.get(&py_lower(&it.row.player_name)) {
                None => Delta { is_new: true, prev_rank: None, prev_score: None, rank_delta: None, score_delta: None },
                Some(&(rank, score)) => Delta {
                    is_new: false,
                    prev_rank: Some(rank),
                    prev_score: Some(score),
                    rank_delta: Some(rank - it.row.rank),
                    score_delta: Some(it.row.score - score),
                },
            });
        }
    }
    Ok(EntriesPage {
        items,
        total,
        comparison: Comparison { comparable: true, prev_anchor: Some(prev_anchor), reason: "ok" },
    })
}

fn point_value(p: &Point) -> Value {
    json!({"created_at": p.created_at, "rank": p.rank, "score": p.score, "synthetic": p.synthetic})
}

/// `board_history`: the current top-`top` players' trajectories over `days`
/// days, with the synthetic reset-zero cliffs.
pub async fn board_history<S: Source>(src: &S, uuid: i32, days: i64, top: i64, now: i64) -> Res<Value> {
    history(src, uuid, days, top, now, false).await
}

/// [`board_history`] as the warmer computes it ([`Source::warm_top_series`]).
pub async fn warm_board_history<S: Source>(src: &S, uuid: i32, days: i64, top: i64, now: i64) -> Res<Value> {
    history(src, uuid, days, top, now, true).await
}

async fn history<S: Source>(src: &S, uuid: i32, days: i64, top: i64, now: i64, warm: bool) -> Res<Value> {
    let days = days.clamp(1, 30);
    let top = top.clamp(1, 20);
    let window_start = now - days * DAY;
    let ts = if warm {
        src.warm_top_series(uuid, window_start, top).await?
    } else {
        src.board_top_series(uuid, window_start, top).await?
    };
    let shell = |anchors: &[i64], series: Vec<Value>| {
        json!({"uuid": uuid, "days": days, "window_start": window_start, "window_end": now,
               "anchors": anchors, "series": series})
    };
    if ts.anchors.is_empty() {
        return Ok(shell(&[], Vec::new()));
    }
    if ts.top.is_empty() {
        return Ok(shell(&ts.anchors, Vec::new()));
    }
    let mut names: HashMap<i64, &str> = HashMap::new();
    let mut current_rank: HashMap<i64, i64> = HashMap::new();
    for m in &ts.top {
        names.insert(m.player_id, &m.player_name);
        current_rank.insert(m.player_id, m.rank);
    }
    let mut per_player: HashMap<i64, Vec<(i64, i64, f64)>> = names.keys().map(|&pid| (pid, Vec::new())).collect();
    for r in &ts.rows {
        per_player
            .get_mut(&r.player_id)
            .ok_or_else(|| anyhow::anyhow!("series row for player {} outside the top", r.player_id))?
            .push((r.created_at, r.rank, r.score));
    }
    for pts in per_player.values_mut() {
        pts.sort_by_key(|p| p.0);
    }
    let kind = src.board_reset_kind(uuid).await?;
    let series = ts
        .top
        .iter()
        .map(|m| {
            let points = core::inject_reset_zeros(&per_player[&m.player_id], kind);
            json!({
                "player_name": names[&m.player_id],
                "current_rank": current_rank.get(&m.player_id),
                "points": points.iter().map(point_value).collect::<Vec<_>>(),
            })
        })
        .collect();
    Ok(shell(&ts.anchors, series))
}

pub struct BoardHealth {
    /// Non-finite metrics are `null` here, as pydantic writes them.
    pub value: Value,
    /// False when a metric overflowed, which Starlette's `JSONResponse` refuses (a 500).
    pub finite: bool,
}

/// `board_health`: competitiveness of the latest snapshot's top 50, plus
/// turnover and score inflation when the previous trove-day compares.
pub async fn board_health<S: Source>(src: &S, uuid: i32) -> Res<Option<BoardHealth>> {
    let board = src.get_board(uuid).await?;
    let latest = src.latest_anchor_for_board(uuid).await?;
    let (Some(Board { row, .. }), Some(latest)) = (board, latest) else { return Ok(None) };
    let page = list_entries_with_deltas(src, uuid, latest, 50, 0).await?;
    let c = page.comparison;
    let items: Vec<HealthItem> = page
        .items
        .iter()
        .map(|it| HealthItem {
            score: it.row.score,
            is_new: it.delta.is_some_and(|d| d.is_new),
            score_delta: it.delta.and_then(|d| d.score_delta),
            prev_score: it.delta.and_then(|d| d.prev_score),
        })
        .collect();
    let h = core::compute_board_health(&items, c.comparable);
    let floats = [h.leader_share, h.p1_pn_ratio, h.gini, h.turnover_rate, h.median_score_gain, h.median_score_gain_pct];
    let value = json!({
        "uuid": uuid, "name": row.name, "category": row.category, "reset_kind": row.reset_kind(),
        "anchor": latest, "prev_anchor": c.prev_anchor, "comparable": c.comparable,
        "comparison_reason": c.reason, "population": page.total, "sample_size": items.len(),
        "leader_share": h.leader_share, "p1_pn_ratio": h.p1_pn_ratio, "gini": h.gini,
        "turnover_rate": h.turnover_rate, "new_entrants": h.new_entrants,
        "median_score_gain": h.median_score_gain, "median_score_gain_pct": h.median_score_gain_pct,
    });
    Ok(Some(BoardHealth { value, finite: floats.iter().flatten().all(|f| f.is_finite()) }))
}

pub const TROVE_MASTERY_UUID: i32 = 1;
pub const GEODE_MASTERY_UUID: i32 = 20;
pub const GEODE_LEVEL_CAP: i64 = 100;
const POWER_RANK_UUIDS: std::ops::RangeInclusive<i32> = 1000..=1016;

/// Python `int(round(score))`.
fn round_score(score: f64) -> Res<i64> {
    round_int(score).ok_or_else(|| anyhow::anyhow!("score {score} does not round to an int64"))
}

fn mastery_block(top: Option<&super::store::TopEntry>, cap: Option<i64>) -> Res<Value> {
    let Some(top) = top else { return Ok(Value::Null) };
    let points = round_score(top.score)?.max(0);
    let (level, into, to_next) = core::level_from_points(points);
    let mut m = Map::new();
    m.insert("points".into(), json!(points));
    m.insert("level".into(), json!(cap.map_or(level, |c| level.min(c))));
    m.insert("points_into_level".into(), json!(into));
    m.insert("points_to_next_level".into(), json!(to_next));
    m.insert("player_name".into(), json!(top.player_name));
    m.insert("anchor".into(), json!(top.anchor));
    if let Some(cap) = cap {
        m.insert("level_cap".into(), json!(cap));
        m.insert("uncapped_level".into(), json!(level));
        m.insert("capped".into(), json!(level > cap));
    }
    Ok(Value::Object(m))
}

/// `mastery_records`: the rank-1 of the lifetime mastery and Power Rank boards
/// at the latest published anchor.
pub async fn mastery_records<S: Source, K: Kv>(src: &S, kv: &K) -> Res<Value> {
    let ts = cache::get_timestamps(src, kv, 1).await?;
    let Some(&anchor) = ts.first() else {
        return Ok(json!({"trove_mastery": null, "geode_mastery": null, "power_rank": null}));
    };
    let mut uuids = vec![TROVE_MASTERY_UUID, GEODE_MASTERY_UUID];
    uuids.extend(POWER_RANK_UUIDS);
    let tops = src.top_entries_for_boards(&uuids, anchor).await?;
    let mut power: Option<(i64, i32, &super::store::TopEntry)> = None;
    for uuid in POWER_RANK_UUIDS {
        let Some(top) = tops.get(&i64::from(uuid)) else { continue };
        let value = round_score(top.score)?;
        if power.is_none_or(|(best, _, _)| value > best) {
            power = Some((value, uuid, top));
        }
    }
    let power = power.map_or(Value::Null, |(value, uuid, top)| {
        json!({"value": value, "board_uuid": uuid, "player_name": top.player_name, "anchor": top.anchor})
    });
    Ok(json!({
        "trove_mastery": mastery_block(tops.get(&i64::from(TROVE_MASTERY_UUID)), None)?,
        "geode_mastery": mastery_block(tops.get(&i64::from(GEODE_MASTERY_UUID)), Some(GEODE_LEVEL_CAP))?,
        "power_rank": power,
    }))
}

// --- /v1 response models ------------------------------------------------------

/// `LeaderboardBoardOut`.
pub fn board_v1(board: &Board) -> Value {
    let mut m = board.row.info_v1();
    let contests: Vec<Value> = board.contests.iter().map(|(t, ty)| json!({"time": t, "type": ty})).collect();
    m.insert("contests".into(), Value::Array(contests));
    Value::Object(m)
}

/// `LeaderboardEntriesPage`.
pub fn entries_v1(uuid: i32, anchor: i64, page: &EntriesPage) -> Value {
    json!({
        "uuid": uuid, "created_at": anchor,
        "items": page.items.iter().map(Entry::v1_value).collect::<Vec<_>>(),
        "count": page.items.len(), "total": page.total, "comparison": page.comparison.to_value(),
    })
}

fn pick<'a>(m: &'a Map<String, Value>, key: &str) -> Res<&'a Value> {
    m.get(key).ok_or_else(|| anyhow::anyhow!("field {key:?} required"))
}

/// pydantic's lax `int`: an int, or a float with no fraction.
fn as_int(v: &Value) -> Res<Value> {
    if let Some(i) = v.as_i64() {
        return Ok(json!(i));
    }
    match v.as_f64() {
        Some(f) if f.fract() == 0.0 && f.abs() < 9.2e18 => Ok(json!(f as i64)),
        _ => anyhow::bail!("{v} is not an int"),
    }
}

fn as_opt_int(v: &Value) -> Res<Value> {
    if v.is_null() { Ok(Value::Null) } else { as_int(v) }
}

fn as_float(v: &Value) -> Res<Value> {
    match v {
        Value::Number(n) => Ok(json!(n.as_f64().unwrap_or(f64::NAN))),
        _ => anyhow::bail!("{v} is not a float"),
    }
}

fn as_str(v: &Value) -> Res<Value> {
    if v.is_string() { Ok(v.clone()) } else { anyhow::bail!("{v} is not a str") }
}

fn obj(v: &Value) -> Res<&Map<String, Value>> {
    v.as_object().ok_or_else(|| anyhow::anyhow!("{v} is not an object"))
}

fn list(v: &Value) -> Res<&Vec<Value>> {
    v.as_array().ok_or_else(|| anyhow::anyhow!("{v} is not a list"))
}

/// `BoardHistoryResponse(**payload)`: a cached payload re-validated.
pub fn history_v1(payload: &Value) -> Res<Value> {
    let p = obj(payload)?;
    let anchors = list(pick(p, "anchors")?)?.iter().map(as_int).collect::<Res<Vec<_>>>()?;
    let mut series = Vec::new();
    for s in list(pick(p, "series")?)? {
        let s = obj(s)?;
        let mut points = Vec::new();
        for pt in list(pick(s, "points")?)? {
            let pt = obj(pt)?;
            let synthetic = match pt.get("synthetic") {
                None => Value::Bool(false),
                Some(Value::Bool(b)) => Value::Bool(*b),
                Some(other) => anyhow::bail!("synthetic {other} is not a bool"),
            };
            points.push(json!({
                "created_at": as_int(pick(pt, "created_at")?)?, "rank": as_int(pick(pt, "rank")?)?,
                "score": as_float(pick(pt, "score")?)?, "synthetic": synthetic,
            }));
        }
        series.push(json!({
            "player_name": as_str(pick(s, "player_name")?)?,
            "current_rank": as_opt_int(pick(s, "current_rank")?)?,
            "points": points,
        }));
    }
    Ok(json!({
        "uuid": as_int(pick(p, "uuid")?)?, "days": as_int(pick(p, "days")?)?,
        "window_start": as_int(pick(p, "window_start")?)?, "window_end": as_int(pick(p, "window_end")?)?,
        "anchors": anchors, "series": series,
    }))
}

/// `MasteryRecordsResponse(**payload)`.
pub fn records_v1(payload: &Value) -> Value {
    let block = |v: &Value, keys: &[&str]| match v.as_object() {
        None => Value::Null,
        Some(m) => {
            Value::Object(keys.iter().map(|&k| (k.to_owned(), m.get(k).cloned().unwrap_or(Value::Null))).collect())
        }
    };
    const MASTERY: [&str; 9] = [
        "points",
        "level",
        "points_into_level",
        "points_to_next_level",
        "player_name",
        "anchor",
        "level_cap",
        "uncapped_level",
        "capped",
    ];
    json!({
        "trove_mastery": block(&payload["trove_mastery"], &MASTERY),
        "geode_mastery": block(&payload["geode_mastery"], &MASTERY),
        "power_rank": block(&payload["power_rank"], &["value", "board_uuid", "player_name", "anchor"]),
    })
}
