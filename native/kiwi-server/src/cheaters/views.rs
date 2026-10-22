//! `POST /v1/leaderboards/views/recompute`: drop the page-view snapshots of
//! `app/trove/leaderboards/cache.py` and re-warm them (latest capture per hot
//! day, the first board's first page, every board's default chart, the ready
//! pointer) with the same keys, values and expiries.

use std::collections::HashMap;

use futures::{StreamExt, stream};
use kiwi_core::cheaters::{Entry, is_player_board};
use serde_json::{Map, Value, json};

use super::store::{BoardRow, PREFIX, READY_KEY, Store, TS_KEY, VIEW_TTL, retention_days, rget, rset};

const TS_CACHE_LIMIT: i64 = 365;
const BHIST_DAYS: i64 = 7;
const BHIST_TOP: i64 = 5;
const BHIST_CONCURRENCY: usize = 6;
const DAY: i64 = 86400;
const RESET: i64 = 11 * 3600;

fn boards_key(anchor: i64) -> String {
    format!("{PREFIX}boards:{anchor}")
}

fn entries_key(anchor: i64, uuid: i64, limit: i64, offset: i64) -> String {
    format!("{PREFIX}entries:{anchor}:{uuid}:{offset}:{limit}")
}

fn bhist_key(uuid: i64, days: i64, top: i64) -> String {
    format!("{PREFIX}bhist:{uuid}:{days}:{top}")
}

/// `invalidate_views`: keys dropped (0 without Redis or on any failure).
pub async fn invalidate_views<S: Store>(s: &S) -> i64 {
    if !s.has_redis() {
        return 0;
    }
    let run = async {
        let mut keys = vec![TS_KEY.to_string(), READY_KEY.to_string()];
        for pat in ["boards:*", "entries:*", "bhist:*"] {
            keys.extend(s.redis_scan(&format!("{PREFIX}{pat}")).await?);
        }
        s.redis_del(&keys).await?;
        anyhow::Ok(keys.len() as i64)
    };
    run.await.unwrap_or_else(|e| {
        tracing::warn!("lb cache: invalidate views failed: {e:#}");
        0
    })
}

/// `invalidate_cheaters`: drop every persisted detection snapshot.
pub async fn invalidate_cheaters<S: Store>(s: &S) -> i64 {
    if !s.has_redis() {
        return 0;
    }
    let run = async {
        let keys = s.redis_scan(&format!("{PREFIX}cheaters:*")).await?;
        if !keys.is_empty() {
            s.redis_del(&keys).await?;
        }
        anyhow::Ok(keys.len() as i64)
    };
    run.await.unwrap_or_else(|e| {
        tracing::warn!("lb cache: invalidate cheaters failed: {e:#}");
        0
    })
}

/// A `list_boards_at` row as Python's dict.
pub fn board_json(r: &BoardRow) -> Value {
    let b = &r.board;
    json!({
        "uuid": b.uuid,
        "name_id": b.name_id,
        "name": b.name,
        "category_id": b.category_id,
        "category": b.category,
        "contest_type": b.contest_type,
        "reset_kind": b.reset_kind,
        "reset_kind_override": r.reset_kind_override,
        "player_board": is_player_board(b.uuid),
    })
}

/// `service.reset_boundaries_for_kind`: resets inside `(t_start, t_end]`.
pub fn reset_boundaries(kind: &str, t_start: i64, t_end: i64) -> Vec<i64> {
    if !matches!(kind, "daily" | "weekly") || t_end <= t_start {
        return Vec::new();
    }
    let mut c = t_start.div_euclid(DAY) * DAY + RESET;
    if c <= t_start {
        c += DAY;
    }
    let step = if kind == "weekly" {
        while (c.div_euclid(DAY) + 3).rem_euclid(7) != 0 {
            c += DAY;
        }
        7 * DAY
    } else {
        DAY
    };
    let mut out = Vec::new();
    while c <= t_end {
        out.push(c);
        c += step;
    }
    out
}

fn entry_json(e: &Entry) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("player_name".into(), e.player_name.clone().into());
    m.insert("rank".into(), e.rank.into());
    m.insert("score".into(), kiwi_core::cheaters::fv(e.score));
    m
}

/// `service.list_entries_with_deltas` as the `{items, total, comparison}` it caches.
pub async fn entries_with_deltas<S: Store>(
    s: &S,
    uuid: i64,
    anchor: i64,
    limit: i64,
    offset: i64,
) -> anyhow::Result<Value> {
    let (entries, total) = s.list_entries(uuid, anchor, limit, offset).await?;
    let mut items: Vec<Map<String, Value>> = entries.iter().map(entry_json).collect();
    let pack = |items: Vec<Map<String, Value>>, comparison: Value| json!({"items": items, "total": total, "comparison": comparison});
    let Some(prev_anchor) = s.previous_day_anchor(uuid, anchor).await? else {
        return Ok(pack(items, json!({"comparable": false, "prev_anchor": null, "reason": "no_prior_snapshot"})));
    };
    let kind = s.board_reset_kind(uuid).await?;
    if !reset_boundaries(&kind, prev_anchor, anchor).is_empty() {
        return Ok(pack(items, json!({"comparable": false, "prev_anchor": prev_anchor, "reason": "crossed_reset"})));
    }
    if !items.is_empty() {
        let names: Vec<String> = entries.iter().map(|e| e.player_name.clone()).collect();
        let prev = s.prev_rows_for_players(uuid, prev_anchor, &names).await?;
        for (it, e) in items.iter_mut().zip(&entries) {
            match prev.get(&e.player_name.to_lowercase()) {
                None => {
                    it.insert("is_new".into(), true.into());
                    for k in ["prev_rank", "prev_score", "rank_delta", "score_delta"] {
                        it.insert(k.into(), Value::Null);
                    }
                }
                Some(&(rank, score)) => {
                    it.insert("is_new".into(), false.into());
                    it.insert("prev_rank".into(), rank.into());
                    it.insert("prev_score".into(), kiwi_core::cheaters::fv(score));
                    it.insert("rank_delta".into(), (rank - e.rank).into());
                    it.insert("score_delta".into(), kiwi_core::cheaters::fv(e.score - score));
                }
            }
        }
    }
    Ok(pack(items, json!({"comparable": true, "prev_anchor": prev_anchor, "reason": "ok"})))
}

/// `service._inject_reset_zeros` over `(created_at, rank, score)` points.
fn reset_zeros(points: &[(i64, i64, f64)], kind: &str) -> Vec<Value> {
    let point = |t: i64, r: i64, sc: f64, synthetic: bool| json!({"created_at": t, "rank": r, "score": kiwi_core::cheaters::fv(sc), "synthetic": synthetic});
    if !matches!(kind, "daily" | "weekly") || points.len() < 2 {
        return points.iter().map(|&(t, r, sc)| point(t, r, sc, false)).collect();
    }
    let mut out = Vec::new();
    for (i, &(t, r, sc)) in points.iter().enumerate() {
        out.push(point(t, r, sc, false));
        let Some(&(next_t, _, _)) = points.get(i + 1) else { continue };
        let (mut last_score, mut last_rank) = (sc, r);
        for b in reset_boundaries(kind, t, next_t) {
            if last_score > 0.0 {
                out.push(point(b - 1, last_rank, last_score, true));
                if b < next_t {
                    out.push(point(b, 0, 0.0, true));
                }
            }
            last_score = 0.0;
            last_rank = 0;
        }
    }
    out
}

/// `service.board_history` at `now`.
pub async fn board_history<S: Store>(s: &S, uuid: i64, days: i64, top: i64) -> anyhow::Result<Value> {
    let days = days.clamp(1, 30);
    let top = top.clamp(1, 20);
    let now = s.now() as i64;
    let window_start = now - days * DAY;
    let shell = |anchors: &[i64], series: Vec<Value>| {
        json!({"uuid": uuid, "days": days, "window_start": window_start, "window_end": now,
               "anchors": anchors, "series": series})
    };
    let (anchors, top_meta, rows) = s.board_top_series(uuid, window_start, top).await?;
    if anchors.is_empty() || top_meta.is_empty() {
        return Ok(shell(&anchors, Vec::new()));
    }
    let mut per_player: HashMap<i64, Vec<(i64, i64, f64)>> = top_meta.iter().map(|m| (m.0, Vec::new())).collect();
    for &(pid, t, r, sc) in &rows {
        if let Some(v) = per_player.get_mut(&pid) {
            v.push((t, r, sc));
        }
    }
    for pts in per_player.values_mut() {
        pts.sort_by_key(|p| p.0);
    }
    let names: HashMap<i64, &str> = top_meta.iter().map(|m| (m.0, m.1.as_str())).collect();
    let ranks: HashMap<i64, i64> = top_meta.iter().map(|m| (m.0, m.2)).collect();
    let kind = s.board_reset_kind(uuid).await?;
    let series = top_meta
        .iter()
        .map(|m| {
            json!({
                "player_name": names[&m.0],
                "current_rank": ranks[&m.0],
                "points": reset_zeros(&per_player[&m.0], &kind),
            })
        })
        .collect();
    Ok(shell(&anchors, series))
}

/// `_latest_per_day`: the newest anchor of each trove-day, newest first.
fn latest_per_day(anchors: &[i64], days: i64) -> Vec<i64> {
    let mut by_day: Vec<(i64, i64)> = Vec::new();
    for &a in anchors {
        let key = (a - RESET).div_euclid(DAY);
        match by_day.iter_mut().find(|(k, _)| *k == key) {
            Some((_, v)) if a > *v => *v = a,
            Some(_) => {}
            None if a > -1 => by_day.push((key, a)),
            None => {}
        }
    }
    let mut out: Vec<i64> = by_day.into_iter().map(|(_, v)| v).collect();
    out.sort_by(|a, b| b.cmp(a));
    out.truncate(days.max(0) as usize);
    out
}

/// `cache.warm`.
pub async fn warm<S: Store>(s: &S) -> anyhow::Result<()> {
    let timestamps = s.list_timestamps(TS_CACHE_LIMIT).await?;
    rset(s, TS_KEY, &json!(timestamps), VIEW_TTL).await;
    if timestamps.is_empty() {
        return Ok(());
    }
    let days = retention_days(s).await;
    for anchor in latest_per_day(&timestamps, days) {
        let boards = s.list_boards_at(anchor).await?;
        let list: Vec<Value> = boards.iter().map(board_json).collect();
        rset(s, &boards_key(anchor), &Value::Array(list), VIEW_TTL).await;
        if let Some(first) = boards.first() {
            let uuid = first.board.uuid;
            let page = entries_with_deltas(s, uuid, anchor, 100, 0).await?;
            rset(s, &entries_key(anchor, uuid, 100, 0), &page, VIEW_TTL).await;
        }
    }
    Ok(())
}

/// `cache.get_boards`: the cached list, else Postgres (and cache it).
async fn get_boards<S: Store>(s: &S, anchor: i64) -> anyhow::Result<Value> {
    if let Some(v) = rget(s, &boards_key(anchor)).await {
        return Ok(v);
    }
    let fresh = Value::Array(s.list_boards_at(anchor).await?.iter().map(board_json).collect());
    rset(s, &boards_key(anchor), &fresh, VIEW_TTL).await;
    Ok(fresh)
}

/// `cache.warm_board_histories`: charts warmed.
pub async fn warm_board_histories<S: Store>(s: &S, anchor: i64) -> anyhow::Result<i64> {
    if !s.has_redis() {
        return Ok(0);
    }
    let boards = get_boards(s, anchor).await?;
    let Some(list) = boards.as_array().filter(|l| !l.is_empty()) else { return Ok(0) };
    let mut uuids = Vec::with_capacity(list.len());
    for b in list {
        match b.get("uuid").and_then(Value::as_i64) {
            Some(u) => uuids.push(u),
            None => anyhow::bail!("cached board without a uuid"),
        }
    }
    let results: Vec<bool> = stream::iter(uuids)
        .map(|uuid| async move {
            match board_history(s, uuid, BHIST_DAYS, BHIST_TOP).await {
                Ok(payload) => {
                    let ttl = retention_days(s).await * DAY;
                    rset(s, &bhist_key(uuid, BHIST_DAYS, BHIST_TOP), &payload, ttl).await;
                    true
                }
                Err(e) => {
                    tracing::warn!("lb cache: warm board history uuid={uuid} failed: {e:#}");
                    false
                }
            }
        })
        .buffer_unordered(BHIST_CONCURRENCY)
        .collect()
        .await;
    Ok(results.into_iter().filter(|ok| *ok).count() as i64)
}

/// The route's work: `{recomputed, keys_cleared, anchor, board_charts_warmed}`.
pub async fn recompute<S: Store>(s: &S) -> anyhow::Result<Value> {
    let cleared = invalidate_views(s).await;
    warm(s).await?;
    let ts = s.list_timestamps(1).await?;
    let anchor = ts.first().copied();
    let mut warmed = 0;
    if let Some(a) = anchor {
        warmed = warm_board_histories(s, a).await?;
        let ttl = retention_days(s).await * DAY;
        rset(s, READY_KEY, &json!(a), ttl).await;
    }
    Ok(json!({"recomputed": true, "keys_cleared": cleared, "anchor": anchor, "board_charts_warmed": warmed}))
}
