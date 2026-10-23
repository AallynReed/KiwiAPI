//! `GET /site/rotations` and `GET /site/calendar/yearly`: the raw dicts of
//! `app/trove/{server_time,rotations,luxion,chaos,calendar}.py`, not the
//! `/v1` response models.

use axum::{extract::State, response::Response};
use chrono::{DateTime, Utc};
use kiwi_core::rotations::{self as rot, BiomeSlot, Window};
use serde_json::{Map, Value, json};

use super::{cached, flags::SiteGate};
use crate::{
    dt,
    error::ApiError,
    routes::rotations::{
        bodies::{self, LuxionDoc},
        chaos,
        gamedata::Gamedata,
        store,
    },
    state::AppState,
};

fn internal(e: impl std::fmt::Display) -> ApiError {
    tracing::error!("site rotations: {e}");
    ApiError::internal()
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect::<Map<String, Value>>())
}

/// `_sub_biome(key)`.
fn sub_biome(data: &Gamedata, key: &str) -> Value {
    match data.biomes.get(key) {
        Some(sb) if crate::features::py_truthy(sb) => {
            let get = |k: &str, d: &str| sb.get(k).cloned().unwrap_or_else(|| Value::String(d.to_owned()));
            obj(vec![
                ("name", get("name", key)),
                ("final_name", get("final_name", key)),
                ("icon", get("icon", "unknown")),
            ])
        }
        _ => json!({"name": key, "final_name": key, "icon": "unknown"}),
    }
}

/// A rotation slot as its raw dict, `biome` resolving each entry.
fn slot(s: &BiomeSlot, biome: &dyn Fn(&str) -> Value) -> Value {
    json!({"starts_at": s.starts_at, "ends_at": s.ends_at, "biomes": s.biomes.iter().map(|b| biome(b)).collect::<Vec<_>>()})
}

/// `{name: final_name or name, icon}` per biome (`_biome_list`).
fn biome_list(biomes: &Value) -> Value {
    let items = biomes.as_array().map(Vec::as_slice).unwrap_or_default();
    Value::Array(
        items
            .iter()
            .map(|b| {
                let name = match b.get("final_name") {
                    Some(v) if crate::features::py_truthy(v) => v.clone(),
                    _ => b.get("name").cloned().unwrap_or(Value::Null),
                };
                obj(vec![("name", name), ("icon", b.get("icon").cloned().unwrap_or(Value::Null))])
            })
            .collect(),
    )
}

/// `_sched(rows, n)` over raw windows.
fn sched(rows: &[Value], n: usize) -> Value {
    Value::Array(
        rows.iter()
            .take(n)
            .map(|r| {
                let mut e = Map::new();
                e.insert("starts_at".into(), r.get("starts_at").cloned().unwrap_or(Value::Null));
                e.insert("ends_at".into(), r.get("ends_at").cloned().unwrap_or(Value::Null));
                if let Some(state) = r.get("state").filter(|v| crate::features::py_truthy(v)) {
                    e.insert("state".into(), state.clone());
                }
                if let Some(b) = r.get("biomes").filter(|v| crate::features::py_truthy(v)) {
                    e.insert("biomes".into(), biome_list(b));
                }
                Value::Object(e)
            })
            .collect(),
    )
}

/// A merchant window's raw dict (Corruxion's carries no `state`).
fn window(w: &Window) -> Value {
    let mut m = Map::new();
    m.insert("starts_at".into(), w.starts_at.into());
    m.insert("ends_at".into(), w.ends_at.into());
    if let Some(s) = &w.state {
        m.insert("state".into(), s.clone().into());
    }
    Value::Object(m)
}

fn windows(ws: &[Window]) -> Vec<Value> {
    ws.iter().map(window).collect()
}

fn merchant(id: &str, name: &str, active: bool, starts_at: Value, ends_at: Value, extra: Vec<(&str, Value)>) -> Value {
    let mut pairs = vec![
        ("id", id.into()),
        ("name", name.into()),
        ("active", active.into()),
        ("starts_at", starts_at),
        ("ends_at", ends_at),
    ];
    pairs.extend(extra);
    obj(pairs)
}

/// The chaos chest as the dashboard reads it.
pub struct Chaos {
    pub starts_at: i64,
    pub ends_at: i64,
    pub seconds_remaining: i64,
    pub item: Value,
}

/// `_daily_rotation(st)`.
fn daily_rotation(data: &Gamedata, now: i64, reset: i64) -> Value {
    let week: Vec<&Value> = (0..7).filter_map(|i| data.daily.get(&i.to_string())).collect();
    let current = data.daily.get(&rot::daily_buff_index(now).to_string());
    rotation(
        &week,
        current,
        7,
        86400,
        reset,
        &["name", "emoji", "color", "weekday", "normal_buffs", "premium_buffs", "banner"],
    )
}

/// `_weekly_rotation(st)`.
fn weekly_rotation(data: &Gamedata, now: i64, reset: i64) -> Value {
    let weeks: Vec<&Value> = (0..4).filter_map(|i| data.weekly.get(&i.to_string())).collect();
    let current = data.weekly.get(&rot::weekly_buff_index(now).to_string());
    rotation(&weeks, current, 4, 7 * 86400, reset, &["name", "emoji", "color", "buffs", "banner"])
}

fn rotation(items: &[&Value], current: Option<&Value>, n: i64, step: i64, reset: i64, keys: &[&str]) -> Value {
    let cur_name = current.and_then(|c| c.get("name"));
    let cur_idx = items.iter().position(|x| x.get("name") == cur_name).unwrap_or(0) as i64;
    Value::Array(
        items
            .iter()
            .enumerate()
            .map(|(i, x)| {
                let d = (i as i64 - cur_idx).rem_euclid(n);
                let mut m: Map<String, Value> =
                    keys.iter().map(|k| ((*k).to_owned(), x.get(*k).cloned().unwrap_or(Value::Null))).collect();
                m.insert("is_current".into(), (d == 0).into());
                m.insert("next_at".into(), if reset != 0 { (reset + (d - 1) * step).into() } else { Value::Null });
                Value::Object(m)
            })
            .collect(),
    )
}

/// `data.get(str(k), {}) or None`.
fn current_buff(data: &Map<String, Value>, k: usize) -> Value {
    match data.get(&k.to_string()) {
        Some(v) if crate::features::py_truthy(v) => v.clone(),
        _ => Value::Null,
    }
}

/// The `/site/rotations` body at `now` (microseconds).
pub fn body(data: &Gamedata, now: i64, now_dt: DateTime<Utc>, lux: Option<&LuxionDoc>, chaos: &Chaos) -> Value {
    let parent = |n: &str| data.parent_biome(n);
    let feed = |slots: Vec<BiomeSlot>, biome: &dyn Fn(&str) -> Value| -> (Value, Vec<Value>) {
        let mut it = slots.iter().map(|s| slot(s, biome));
        (it.next().unwrap_or(Value::Null), it.collect())
    };
    let (mana_cur, mana_up) = feed(rot::wild_mana(now, 8), &parent);
    let (stampy_cur, stampy_up) = feed(rot::stampy(now, 8), &parent);
    let (d15_cur, d15_up) = feed(rot::biome_rotation(now, 16), &|k| sub_biome(data, k));
    let corr = rot::corruxion(now, 8);
    let flux = rot::fluxion(now, 8);

    let field = |v: &Value, k: &str| v.get(k).cloned().unwrap_or(Value::Null);
    let lux_merchant = match lux {
        None => merchant(
            "luxion",
            "Luxion",
            false,
            Value::Null,
            Value::Null,
            vec![("state", Value::Null), ("schedule", json!([])), ("window", Value::Null)],
        ),
        Some(doc) => {
            let run = doc.run();
            let l = rot::luxion(run.day_anchor, run.first_seen, run.ended, now);
            let sched_rows = if l.active { sched(&windows(&l.schedule), 7) } else { json!([]) };
            let lw = l.current_window.as_ref().or(l.next_window.as_ref());
            let lux_window = match lw {
                Some(w) if l.active => json!({"open": l.merchant_open, "starts_at": w.starts_at, "ends_at": w.ends_at}),
                _ => Value::Null,
            };
            merchant(
                "luxion",
                "Luxion",
                l.active,
                if l.active { l.starts_at.into() } else { Value::Null },
                if l.active { l.ends_at.into() } else { Value::Null },
                vec![
                    ("state", if l.merchant_open { "Open".into() } else { Value::Null }),
                    ("schedule", sched_rows),
                    ("window", lux_window),
                ],
            )
        }
    };
    let biomes_of = |cur: &Value| biome_list(cur.get("biomes").unwrap_or(&Value::Null));
    let mut merchants = vec![
        merchant(
            "corruxion",
            "Corruxion",
            corr.active,
            corr.starts_at.into(),
            corr.ends_at.into(),
            vec![("schedule", sched(&windows(&corr.schedule), 6))],
        ),
        merchant(
            "fluxion",
            "Fluxion",
            flux.active,
            flux.starts_at.into(),
            flux.ends_at.into(),
            vec![
                ("state", flux.state.map_or(Value::Null, Value::from)),
                ("schedule", sched(&windows(&flux.schedule), 6)),
            ],
        ),
        lux_merchant,
        merchant(
            "wild_mana",
            "Wild Mana",
            true,
            field(&mana_cur, "starts_at"),
            field(&mana_cur, "ends_at"),
            vec![("biomes", biomes_of(&mana_cur)), ("schedule", sched(&mana_up, 6))],
        ),
        merchant(
            "d15",
            "Long Shade Rotation",
            true,
            field(&d15_cur, "starts_at"),
            field(&d15_cur, "ends_at"),
            vec![("biomes", biomes_of(&d15_cur)), ("schedule", sched(&d15_up, 6))],
        ),
    ];
    if !stampy_cur.is_null() {
        let now_ts = rot::unix(now);
        let (s, e) = (stampy_cur["starts_at"].as_i64().unwrap_or(0), stampy_cur["ends_at"].as_i64().unwrap_or(0));
        merchants.push(merchant(
            "stampy",
            "Stampy",
            s <= now_ts && now_ts < e,
            field(&stampy_cur, "starts_at"),
            field(&stampy_cur, "ends_at"),
            vec![("biomes", biomes_of(&stampy_cur)), ("schedule", sched(&stampy_up, 6))],
        ));
    }
    let st = bodies::server_time(now_dt);
    let daily_reset = st["daily_reset_at"].as_i64().unwrap_or(0);
    let weekly_reset = st["weekly_reset_at"].as_i64().unwrap_or(0);
    json!({
        "server_time": st,
        "daily_buff": current_buff(&data.daily, rot::daily_buff_index(now)),
        "weekly_buff": current_buff(&data.weekly, rot::weekly_buff_index(now)),
        "daily_rotation": daily_rotation(data, now, daily_reset),
        "weekly_rotation": weekly_rotation(data, now, weekly_reset),
        "chaos": {
            "starts_at": chaos.starts_at,
            "ends_at": chaos.ends_at,
            "seconds_remaining": chaos.seconds_remaining,
            "item": chaos.item,
        },
        "merchants": merchants,
    })
}

/// `chaos.get_chaos_chest()` as `/site/rotations` keeps it.
async fn chaos_chest(state: &AppState, now: i64) -> Result<Chaos, ApiError> {
    let week = rot::chaos_chest_window(now);
    let db = state.mongo();
    let chest = match store::chaos_for_week(db, week.0).await.map_err(internal)? {
        Some(capture) => chaos::build(Some(&chaos::capture_item(&capture.name, week)), Some(capture.captured_at), now),
        None => match store::chaos_feed(db).await.map_err(internal)? {
            Some((cached, fetched_at)) => chaos::build(cached.as_ref(), Some(fetched_at), now),
            None => chaos::build(None, None, now),
        },
    }
    .map_err(|_| internal("chaos chest item failed validation"))?;
    let resolved = match chaos::needs_blueprint(&chest) {
        Some(name) => chaos::resolve_blueprint(state, name).await,
        None => None,
    };
    let full = chaos::body(&chest, resolved, &state.settings().api_url);
    Ok(Chaos {
        starts_at: chest.starts_at,
        ends_at: chest.ends_at,
        seconds_remaining: chest.seconds_remaining,
        item: full.get("item").cloned().unwrap_or(Value::Null),
    })
}

pub async fn route(_: SiteGate, State(state): State<AppState>) -> Result<Response, ApiError> {
    let now_dt = dt::now();
    let now = now_dt.timestamp_micros();
    let lux = store::latest_luxion(state.mongo()).await.map_err(internal)?;
    let chaos = chaos_chest(&state, now).await?;
    Ok(cached(body(Gamedata::get(), now, now_dt, lux.as_ref(), &chaos), "public, max-age=30"))
}

/// `yearly_calendar(...)` as its raw dict: each event carries only its own extras.
pub fn yearly_body(data: &Gamedata, now: i64, runs: &[rot::LuxionRun]) -> Result<Value, ApiError> {
    let v = bodies::calendar(data, now, runs).map_err(|_| internal("weekly buff failed validation"))?;
    let events: Vec<Value> = v["events"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|e| {
            let keep: &[&str] = match e["type"].as_str().unwrap_or("") {
                "weekly_buff" | "gardening_2" | "gardening_3" => &["color"],
                "fluxion" => &["state", "color"],
                "stampy" | "mana" => &["biomes"],
                _ => &[],
            };
            let mut m = Map::new();
            for k in ["type", "name", "starts_at", "ends_at"].iter().chain(keep) {
                m.insert((*k).to_owned(), e.get(*k).cloned().unwrap_or(Value::Null));
            }
            Value::Object(m)
        })
        .collect();
    Ok(json!({
        "starts_at": v["starts_at"],
        "ends_at": v["ends_at"],
        "generated_at": v["generated_at"],
        "count": events.len(),
        "events": events,
    }))
}

pub async fn yearly(_: SiteGate, State(state): State<AppState>) -> Result<Response, ApiError> {
    let runs: Vec<rot::LuxionRun> =
        store::luxion_runs(state.mongo()).await.map_err(internal)?.iter().map(|d| d.run()).collect();
    Ok(cached(yearly_body(Gamedata::get(), dt::now().timestamp_micros(), &runs)?, "public, max-age=300"))
}
