//! Response bodies for the deterministic routes, as the Python response models
//! dump them. Pure: instants are microseconds, data comes in as arguments.

use chrono::{DateTime, Utc};
use kiwi_core::rotations::{self as rot, BiomeSlot, LuxionRun, Merchant, Window};
use serde_json::{Map, Value, json};

use super::gamedata::{self, Gamedata, Invalid, Out};
use crate::dt;

pub fn server_time(now: DateTime<Utc>) -> Value {
    let st = rot::server_time(now.timestamp_micros());
    json!({
        "now_unix": st.now_unix,
        "now_iso": dt::isoformat(&now),
        "trove_day": rot::WEEKDAYS[st.trove_weekday],
        "daily_reset_at": st.daily_reset_at,
        "weekly_reset_at": st.weekly_reset_at,
    })
}

fn rotation_of(data: &Map<String, Value>, n: usize) -> Vec<&Value> {
    (0..n).filter_map(|i| data.get(&i.to_string())).collect()
}

pub fn daily_buffs(data: &Gamedata, now: i64) -> Out {
    let current = data.daily.get(&rot::daily_buff_index(now).to_string()).ok_or(Invalid)?;
    let week = rotation_of(&data.daily, 7).into_iter().map(gamedata::daily_buff).collect::<Out<Vec<_>>>()?;
    Ok(json!({"current": gamedata::daily_buff(current)?, "week": week}))
}

pub fn weekly_buffs(data: &Gamedata, now: i64) -> Out {
    let current = data.weekly.get(&rot::weekly_buff_index(now).to_string()).ok_or(Invalid)?;
    let rotation = rotation_of(&data.weekly, 4).into_iter().map(gamedata::weekly_buff).collect::<Out<Vec<_>>>()?;
    Ok(json!({"current": gamedata::weekly_buff(current)?, "rotation": rotation}))
}

/// The `MerchantWindow` model (`state` is always present, maybe null).
pub fn window(w: &Window) -> Value {
    json!({"starts_at": w.starts_at, "ends_at": w.ends_at, "state": w.state})
}

fn merchant(m: &Merchant) -> Value {
    let mut out = Map::new();
    out.insert("active".into(), m.active.into());
    if let Some(state) = m.state {
        out.insert("state".into(), state.into());
    }
    out.insert("starts_at".into(), m.starts_at.into());
    out.insert("ends_at".into(), m.ends_at.into());
    out.insert("seconds_remaining".into(), m.seconds_remaining.into());
    out.insert("schedule".into(), m.schedule.iter().map(window).collect());
    Value::Object(out)
}

pub fn corruxion(now: i64) -> Value {
    merchant(&rot::corruxion(now, 8))
}

pub fn fluxion(now: i64) -> Value {
    merchant(&rot::fluxion(now, 8))
}

pub fn gardening(now: i64) -> Value {
    let g = rot::gardening(now);
    let win =
        |h: &rot::Harvest| json!({"name": h.name, "active": h.active, "starts_at": h.starts_at, "ends_at": h.ends_at});
    json!({
        "two_day": win(&g.two_day),
        "three_day": win(&g.three_day),
        "upcoming": g.upcoming.iter().map(win).collect::<Vec<_>>(),
    })
}

fn slot(s: &BiomeSlot, biome: impl Fn(&str) -> Out) -> Out {
    let biomes = s.biomes.iter().map(|b| biome(b)).collect::<Out<Vec<_>>>()?;
    Ok(json!({"starts_at": s.starts_at, "ends_at": s.ends_at, "biomes": biomes}))
}

fn feed(slots: &[BiomeSlot], biome: impl Fn(&str) -> Out + Copy) -> Out {
    let current = match slots.first() {
        Some(s) => slot(s, biome)?,
        None => Value::Null,
    };
    let upcoming = slots.iter().skip(1).map(|s| slot(s, biome)).collect::<Out<Vec<_>>>()?;
    Ok(json!({"current": current, "upcoming": upcoming}))
}

pub fn biomes(data: &Gamedata, now: i64) -> Out {
    feed(&rot::biome_rotation(now, 16), |k| data.sub_biome(k))
}

pub fn wild_mana(data: &Gamedata, now: i64) -> Out {
    feed(&rot::wild_mana(now, 8), |n| Ok(data.parent_biome(n)))
}

pub fn stampy(data: &Gamedata, now: i64) -> Out {
    feed(&rot::stampy(now, 8), |n| Ok(data.parent_biome(n)))
}

/// `ChallengeCurrentOut`: the window plus the capture for it, if any.
pub fn challenge_current(now: i64, capture: Option<(&str, DateTime<Utc>)>) -> Value {
    let w = rot::challenge_window(now);
    let name = capture.map(|(n, _)| n);
    json!({
        "starts_at": w.starts_at,
        "ends_at": w.ends_at,
        "active": w.active,
        "is_friday_window": w.is_friday_window,
        "seconds_remaining": w.seconds_remaining,
        "name": name,
        "type": rot::classify_challenge(name),
        "captured_at": capture.map(|(_, at)| dt::pydantic(&at)),
    })
}

/// `data.get(str(k), {})` -> `(name, color)` of a calendar `weekly_buff`.
fn weekly_entry(weekly: &Map<String, Value>, k: i64) -> Out<(String, Option<String>)> {
    let buff = match weekly.get(&k.to_string()) {
        None => None,
        Some(Value::Object(b)) => Some(b),
        Some(_) => return Err(Invalid),
    };
    let name = match buff.and_then(|b| b.get("name")) {
        None => "Weekly Buff".to_owned(),
        Some(Value::String(s)) => s.clone(),
        Some(_) => return Err(Invalid),
    };
    let color = match buff.and_then(|b| b.get("color")) {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => return Err(Invalid),
    };
    Ok((name, color))
}

/// `YearlyCalendar` over the weekly-buff baseline and the captured Luxion runs.
pub fn calendar(data: &Gamedata, now: i64, runs: &[LuxionRun]) -> Out {
    let n = data.weekly.len() as i64;
    let buffs: Vec<Out<(String, Option<String>)>> = (0..n).map(|k| weekly_entry(&data.weekly, k)).collect();
    let failed = std::cell::Cell::new(false);
    let lookup = |k: i64| match &buffs[k as usize] {
        Ok(pair) => pair.clone(),
        Err(Invalid) => {
            failed.set(true);
            (String::new(), None)
        }
    };
    let cal = rot::yearly_calendar(now, Some((n, &lookup)), runs);
    if failed.get() {
        return Err(Invalid);
    }
    let events: Vec<Value> = cal
        .events
        .iter()
        .map(|e| {
            json!({
                "type": e.kind,
                "name": e.name,
                "starts_at": e.starts_at,
                "ends_at": e.ends_at,
                "color": e.color,
                "state": e.state,
                "biomes": e.biomes.as_ref().map(|bs| bs.iter().map(|b| json!({"name": b, "icon": data.parent_icon(b)})).collect::<Vec<_>>()),
            })
        })
        .collect();
    Ok(json!({
        "starts_at": cal.starts_at,
        "ends_at": cal.ends_at,
        "generated_at": cal.generated_at,
        "count": events.len(),
        "events": events,
    }))
}

/// A stored Luxion appearance (`LuxionAppearance`).
#[derive(Clone, Debug)]
pub struct LuxionDoc {
    pub started_at: i64,
    pub first_seen_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
}

/// `luxion._ts`: `int(dt.timestamp())`.
pub fn ts(d: &DateTime<Utc>) -> i64 {
    rot::unix(d.timestamp_micros())
}

impl LuxionDoc {
    pub fn run(&self) -> LuxionRun {
        LuxionRun {
            day_anchor: self.started_at,
            first_seen: Some(ts(&self.first_seen_at)),
            ended: self.ended_at.as_ref().map(ts),
        }
    }
}

pub fn luxion(doc: Option<&LuxionDoc>, now: i64) -> Value {
    let Some(doc) = doc else {
        return json!({
            "active": false, "merchant_open": false, "starts_at": null, "ends_at": null,
            "seconds_remaining": 0, "current_window": null, "next_window": null, "schedule": [],
            "first_seen_at": null, "last_seen_at": null,
        });
    };
    let run = doc.run();
    let l = rot::luxion(run.day_anchor, run.first_seen, run.ended, now);
    json!({
        "active": l.active,
        "merchant_open": l.merchant_open,
        "starts_at": l.starts_at,
        "ends_at": l.ends_at,
        "seconds_remaining": l.seconds_remaining,
        "current_window": l.current_window.as_ref().map(window),
        "next_window": l.next_window.as_ref().map(window),
        "schedule": l.schedule.iter().map(window).collect::<Vec<_>>(),
        "first_seen_at": dt::pydantic(&doc.first_seen_at),
        "last_seen_at": dt::pydantic(&doc.last_seen_at),
    })
}

pub fn luxion_history_item(doc: &LuxionDoc) -> Value {
    let run = doc.run();
    let (started_at, ends_at) = rot::run_bounds(run.day_anchor, run.first_seen, run.ended);
    json!({
        "started_at": started_at,
        "ends_at": ends_at,
        "first_seen_at": dt::pydantic(&doc.first_seen_at),
        "last_seen_at": dt::pydantic(&doc.last_seen_at),
        "ended_at": doc.ended_at.as_ref().map(dt::pydantic),
    })
}

pub fn page(items: Vec<Value>, total: u64) -> Value {
    let count = items.len();
    json!({"items": items, "count": count, "total": total})
}

pub fn chaos_history_item(week_anchor: i64, name: &str, captured_at: &DateTime<Utc>) -> Value {
    json!({
        "week_anchor": week_anchor,
        "week_ends_at": week_anchor + 7 * 86_400,
        "name": name,
        "captured_at": dt::pydantic(captured_at),
    })
}

pub struct ChallengeDoc {
    pub window_anchor: i64,
    pub window_ends_at: i64,
    pub name: String,
    pub is_friday_window: bool,
    pub captured_at: DateTime<Utc>,
}

pub fn challenge_history_item(d: &ChallengeDoc) -> Out {
    Ok(json!({
        "window_anchor": d.window_anchor,
        "window_ends_at": d.window_ends_at,
        "name": d.name,
        "type": rot::classify_challenge(Some(&d.name)).ok_or(Invalid)?,
        "is_friday_window": d.is_friday_window,
        "captured_at": dt::pydantic(&d.captured_at),
    }))
}

/// `get_delves`: the asked-for week (else the current one). Only an explicit
/// `?week=` that is missing is a 404.
pub fn delve_rotation(week: Option<i64>, current: i64, doc: Option<&super::store::DelveDoc>) -> Result<Value, String> {
    let wk = week.unwrap_or(current);
    let Some(doc) = doc else {
        if let Some(week) = week {
            return Err(format!("No delve data for week {week}"));
        }
        return Ok(json!({"week": wk, "is_current": true, "total": 0, "count": 0, "fetched_at": null, "depths": []}));
    };
    Ok(json!({
        "week": doc.week,
        "is_current": doc.week == current,
        "total": doc.total,
        "count": doc.depth_count,
        "fetched_at": dt::pydantic(&doc.fetched_at),
        "depths": doc.depths,
    }))
}

/// `DelveWeekList` from `list_weeks` rows `(week, total, count, fetched_at)`.
pub fn delve_week_list(current: i64, rows: &[(i64, i64, i64, DateTime<Utc>)]) -> Value {
    let items: Vec<Value> = rows
        .iter()
        .map(|(week, total, count, fetched_at)| {
            json!({"week": week, "total": total, "count": count, "fetched_at": dt::pydantic(fetched_at)})
        })
        .collect();
    let count = items.len();
    json!({"current_week": current, "items": items, "count": count})
}
