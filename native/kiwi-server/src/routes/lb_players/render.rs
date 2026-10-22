//! The two wire shapes: `/site` sends the service dicts as they are
//! (`JSONResponse`), `/v1` through the pydantic models in `app/trove/schemas.py`
//! (field order, undeclared keys dropped).

use serde_json::{Map, Value, json};

use super::{
    service::{Boards, HistRow, PlayerBoards, Profile, Series},
    store::internal,
};
use crate::{error::ApiResult, pyjson};

pub(crate) fn site_row(r: &HistRow) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("player_name".into(), r.row.player_name.clone().into());
    m.insert("rank".into(), r.row.rank.into());
    m.insert("score".into(), r.row.score.into());
    m.insert("leaderboard".into(), r.row.leaderboard.into());
    m.insert("created_at".into(), r.row.created_at.into());
    if let Some((slot, slots)) = r.slot {
        m.insert("slot".into(), slot.into());
        m.insert("slots".into(), slots.into());
    }
    if let Some(d) = &r.deltas {
        m.insert("prev_rank".into(), d.prev_rank.into());
        m.insert("prev_score".into(), d.prev_score.into());
        m.insert("rank_delta".into(), d.rank_delta.into());
        m.insert("score_delta".into(), d.score_delta.into());
    }
    m
}

/// `GET /site/leaderboards/players/{name}/history`.
pub fn site_history(player_name: &str, rows: &[HistRow]) -> String {
    let items: Vec<Value> = rows.iter().map(|r| Value::Object(site_row(r))).collect();
    pyjson::dumps(&json!({"player_name": player_name, "items": items, "count": rows.len()}))
}

/// `LeaderboardPlayerHistory`.
pub fn v1_history(player_name: &str, rows: &[HistRow]) -> String {
    let items: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "rank": r.row.rank,
                "player_name": r.row.player_name,
                "score": r.row.score,
                "prev_rank": null,
                "prev_score": null,
                "rank_delta": null,
                "score_delta": null,
                "is_new": false,
                "leaderboard": r.row.leaderboard,
                "created_at": r.row.created_at,
            })
        })
        .collect();
    pyjson::pydantic(&json!({"player_name": player_name, "items": items, "count": rows.len()}))
}

fn board_name(b: &Boards, uuid: i64) -> Value {
    b.meta(uuid).map_or(Value::Null, |m| m.name.clone().into())
}

/// The summary dict; `last_played` only on the profile.
fn summary(b: &Boards, last_played: Option<Option<i64>>) -> Value {
    let best = b.rows.first();
    let mut m = json!({
        "boards_appeared": b.rows.len(),
        "appearances": b.appearances(),
        "best_rank": best.map(|r| r.best_rank),
        "best_rank_board_uuid": best.map(|r| r.leaderboard),
        "best_rank_board_name": best.map_or(Value::Null, |r| board_name(b, r.leaderboard)),
        "top10_count": b.best_within(10),
        "top100_count": b.best_within(100),
        "latest_anchor": b.latest_anchor(),
    });
    if let (Some(lp), Value::Object(map)) = (last_played, &mut m) {
        map.insert("last_played".into(), lp.into());
    }
    m
}

fn site_boards(b: &Boards) -> Vec<Value> {
    b.rows
        .iter()
        .map(|r| {
            let meta = b.meta(r.leaderboard);
            json!({
                "leaderboard": r.leaderboard,
                "best_rank": r.best_rank,
                "appearances": r.appearances,
                "first_seen": r.first_seen,
                "last_seen": r.last_seen,
                "latest_rank": r.latest_rank,
                "latest_score": r.latest_score,
                "board_name": meta.map(|m| m.name.clone()),
                "board_name_id": meta.map(|m| m.name_id.clone()),
                "category": meta.map(|m| m.category.clone()),
            })
        })
        .collect()
}

/// `PlayerBoardSummary` rows.
fn v1_boards_rows(b: &Boards) -> Vec<Value> {
    b.rows
        .iter()
        .map(|r| {
            let meta = b.meta(r.leaderboard);
            json!({
                "leaderboard": r.leaderboard,
                "board_name": meta.map(|m| m.name.clone()),
                "category": meta.map(|m| m.category.clone()),
                "best_rank": r.best_rank,
                "latest_rank": r.latest_rank,
                "latest_score": r.latest_score,
                "appearances": r.appearances,
                "first_seen": r.first_seen,
                "last_seen": r.last_seen,
            })
        })
        .collect()
}

/// `PlayerBoardsResponse`.
pub fn v1_boards(p: &PlayerBoards) -> String {
    pyjson::pydantic(&json!({
        "player_name": p.player_name,
        "summary": summary(&p.boards, None),
        "boards": v1_boards_rows(&p.boards),
    }))
}

fn sigil(p: &Profile) -> Value {
    p.sigil.as_ref().map_or(Value::Null, |s| {
        json!({
            "power_rank": s.power_rank,
            "power_rank_board": s.power_rank_board,
            "power_rank_class": s.power_rank_class,
            "trove_mastery": s.trove_mastery,
            "geode_mastery": s.geode_mastery,
            "mastery": s.trove_mastery + s.geode_mastery,
        })
    })
}

fn classes(p: &Profile) -> Vec<Value> {
    p.classes
        .iter()
        .map(|c| {
            json!({
                "class_index": c.index,
                "name": c.name,
                "icon": c.icon,
                "power_rank_board": c.power_rank,
                "effort_board": c.effort,
                "paragon_board": c.paragon,
            })
        })
        .collect()
}

/// `GET /site/leaderboards/players/{name}/profile`.
pub fn site_profile(p: &Profile) -> String {
    let recent: Vec<Value> = p
        .recent
        .iter()
        .map(|r| {
            let mut m = site_row(r);
            m.insert("board_name".into(), board_name(&p.boards, r.row.leaderboard));
            Value::Object(m)
        })
        .collect();
    pyjson::dumps(&json!({
        "player_name": p.player_name,
        "verified": p.verified,
        "summary": summary(&p.boards, Some(p.last_played)),
        "boards": site_boards(&p.boards),
        "recent": recent,
        "renames": p.renames,
        "alt_clusters": p.alt_clusters,
        "duplicate": p.duplicate,
        "sigil": sigil(p),
        "classes": classes(p),
    }))
}

/// `PlayerProfileResponse`; a non-str class name fails its validation (500).
pub fn v1_profile(p: &Profile) -> ApiResult<String> {
    let names_ok =
        p.classes.iter().map(|c| &c.name).chain(p.sigil.iter().map(|s| &s.power_rank_class)).all(Value::is_string);
    if !names_ok {
        return Err(internal("class name is not a str"));
    }
    let recent: Vec<Value> = p
        .recent
        .iter()
        .map(|r| {
            let d = r.deltas.as_ref();
            json!({
                "player_name": r.row.player_name,
                "leaderboard": r.row.leaderboard,
                "board_name": board_name(&p.boards, r.row.leaderboard),
                "rank": r.row.rank,
                "score": r.row.score,
                "created_at": r.row.created_at,
                "prev_rank": d.map(|d| d.prev_rank),
                "prev_score": d.map(|d| d.prev_score),
                "rank_delta": d.map(|d| d.rank_delta),
                "score_delta": d.map(|d| d.score_delta),
                "is_new": null,
            })
        })
        .collect();
    Ok(pyjson::pydantic(&json!({
        "player_name": p.player_name,
        "verified": p.verified,
        "summary": summary(&p.boards, Some(p.last_played)),
        "boards": v1_boards_rows(&p.boards),
        "recent": recent,
        "sigil": sigil(p),
        "classes": classes(p),
    })))
}

fn points(points: &[(kiwi_core::lbplayers::Point, bool)]) -> Vec<Value> {
    points
        .iter()
        .map(|(p, synthetic)| json!({"created_at": p.created_at, "rank": p.rank, "score": p.score, "synthetic": synthetic}))
        .collect()
}

/// `GET /site/leaderboards/players/{name}/series`.
pub fn site_series(s: &Series) -> String {
    pyjson::dumps(&site_series_value(s))
}

/// The `player_history_series` dict.
pub(crate) fn site_series_value(s: &Series) -> Value {
    let mut m = json!({
        "player_name": s.player_name,
        "canonical_name": s.canonical_name,
        "days": s.days,
        "window_start": s.window_start,
        "window_end": s.window_end,
    });
    let Value::Object(map) = &mut m else { unreachable!("json object") };
    match &s.series {
        None => {
            map.insert("anchors".into(), json!([]));
            map.insert("series".into(), json!([]));
        }
        Some(lines) => {
            map.insert("duplicate_boards".into(), s.duplicate_boards.into());
            map.insert("anchors".into(), s.anchors.clone().into());
            let series: Vec<Value> = lines
                .iter()
                .map(|l| {
                    json!({
                        "uuid": l.uuid,
                        "name": l.name,
                        "best_rank": l.best_rank,
                        "slot": l.slot,
                        "slots": l.slots,
                        "points": points(&l.points),
                    })
                })
                .collect();
            map.insert("series".into(), series.into());
        }
    }
    m
}

/// `PlayerHistorySeriesResponse`.
pub fn v1_series(s: &Series) -> String {
    let series: Vec<Value> = s
        .series
        .iter()
        .flatten()
        .map(|l| json!({"uuid": l.uuid, "name": l.name, "best_rank": l.best_rank, "points": points(&l.points)}))
        .collect();
    pyjson::pydantic(&json!({
        "player_name": s.player_name,
        "canonical_name": s.canonical_name,
        "days": s.days,
        "window_start": s.window_start,
        "window_end": s.window_end,
        "anchors": s.anchors,
        "series": series,
    }))
}
