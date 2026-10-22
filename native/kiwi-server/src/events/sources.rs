//! `app/events/sources.py`: each event type's data (the dict Python builds,
//! datetimes already in their `str()` form), its exactly-once signature, and
//! for the time-driven ones the next boundary the scheduler sleeps to.

use anyhow::{Context, anyhow};
use chrono::{DateTime, Utc};
use kiwi_core::rotations as rot;
use mongodb::{
    Database,
    bson::{Bson, Document, doc},
};
use serde_json::{Map, Value, json};

use crate::{
    dt,
    feeds::pyval::{py_str, truthy},
    routes::{
        rotations::{
            bodies::{self, LuxionDoc},
            chaos,
            gamedata::Gamedata,
            store,
        },
        trove_status,
    },
    state::AppState,
};

pub const DAY: i64 = 86_400;
const GAME_UPDATE_BRANCH: &str = "live-us";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    Challenge,
    Chaos,
    Luxion,
    Corruxion,
    Fluxion,
    Longshade,
    WildMana,
    Stampy,
    DailyBonuses,
    Activity,
    ServerStatus,
    TroveNews,
    Giveaways,
    GameUpdate,
}

/// `SOURCES`, in snapshot order.
pub const SOURCES: [Source; 14] = [
    Source::Challenge,
    Source::Chaos,
    Source::Luxion,
    Source::Corruxion,
    Source::Fluxion,
    Source::Longshade,
    Source::WildMana,
    Source::Stampy,
    Source::DailyBonuses,
    Source::Activity,
    Source::ServerStatus,
    Source::TroveNews,
    Source::Giveaways,
    Source::GameUpdate,
];

pub fn now_us() -> i64 {
    dt::now().timestamp_micros()
}

impl Source {
    pub const fn kind(self) -> &'static str {
        match self {
            Source::Challenge => "challenge",
            Source::Chaos => "chaos",
            Source::Luxion => "luxion",
            Source::Corruxion => "corruxion",
            Source::Fluxion => "fluxion",
            Source::Longshade => "longshade",
            Source::WildMana => "wild_mana",
            Source::Stampy => "stampy",
            Source::DailyBonuses => "daily_bonuses",
            Source::Activity => "activity",
            Source::ServerStatus => "server_status",
            Source::TroveNews => "trove_news",
            Source::Giveaways => "giveaways",
            Source::GameUpdate => "game_update",
        }
    }

    /// `SOURCES_BY_TYPE.get(name)`.
    pub fn by_type(name: &str) -> Option<Source> {
        SOURCES.into_iter().find(|s| s.kind() == name)
    }

    /// `SCHEDULED_SOURCES`: the ones with a `next_at_fn`.
    pub const fn scheduled(self) -> bool {
        matches!(
            self,
            Source::Corruxion
                | Source::Fluxion
                | Source::Longshade
                | Source::WildMana
                | Source::Stampy
                | Source::DailyBonuses
                | Source::Activity
        )
    }

    /// `data_fn()`.
    pub async fn data(self, state: &AppState) -> anyhow::Result<Value> {
        let now = now_us();
        match self {
            Source::Challenge => challenge(state, now).await,
            Source::Chaos => chaos_chest(state, now).await,
            Source::Luxion => {
                let doc = store::latest_luxion(mongo(state)?).await?;
                Ok(luxion_data(doc.as_ref(), now))
            }
            Source::ServerStatus => {
                Ok(trove_status::shared_snapshot(state).await.unwrap_or_else(trove_status::unknown_snapshot))
            }
            Source::TroveNews => Ok(news_data(latest_news(mongo(state)?).await?.as_ref())),
            Source::Giveaways => Ok(giveaways_data(newest_open_giveaway(mongo(state)?).await?.as_ref())),
            Source::GameUpdate => Ok(game_update_data(latest_version(mongo(state)?).await?.as_ref())),
            timed => timed.timed_data(Gamedata::get(), now),
        }
    }

    /// The time-driven sources' data at `now` (µs) over the baseline `data`.
    pub fn timed_data(self, data: &Gamedata, now: i64) -> anyhow::Result<Value> {
        let invalid = || anyhow!("{} gamedata failed validation", self.kind());
        Ok(match self {
            Source::Corruxion => merchant(&rot::corruxion(now, 8)),
            Source::Fluxion => merchant(&rot::fluxion(now, 8)),
            Source::Longshade => bodies::biomes(data, now).map_err(|_| invalid())?,
            Source::WildMana => bodies::wild_mana(data, now).map_err(|_| invalid())?,
            Source::Stampy => bodies::stampy(data, now).map_err(|_| invalid())?,
            Source::DailyBonuses => daily_data(data, now),
            Source::Activity => json!({"window_start": rot::server_time(now).daily_reset_at - DAY}),
            other => return Err(anyhow!("{} is not time-driven", other.kind())),
        })
    }

    /// `sig_fn(data)`: what must change for the event to fire again; `None`
    /// means nothing to announce. `now` (µs) stands for `time.time()`.
    pub fn sig(self, d: &Value, now: i64) -> Option<String> {
        match self {
            Source::Challenge => {
                let name = get(d, "name").filter(|v| truthy(v))?;
                Some(format!("{}:{}", opt_str(get(d, "starts_at")), py_str(name)))
            }
            Source::Chaos => {
                let item = get(d, "item").filter(|v| truthy(v));
                let name = item.and_then(|i| get(i, "name")).filter(|v| truthy(v))?;
                Some(format!("{}:{}", opt_str(get(d, "starts_at")), py_str(name)))
            }
            Source::Luxion => luxion_sig(d, rot::unix(now)),
            Source::Corruxion | Source::Fluxion => {
                get(d, "active").filter(|v| truthy(v)).map(|_| py_str(&d["starts_at"]))
            }
            Source::Longshade | Source::WildMana => {
                let cur = get(d, "current").filter(|v| truthy(v));
                cur.and_then(|c| get(c, "starts_at")).filter(|v| truthy(v)).map(py_str)
            }
            Source::Stampy => {
                let cur = get(d, "current").filter(|v| truthy(v))?;
                let (s, e) = (cur["starts_at"].as_i64()?, cur["ends_at"].as_i64()?);
                let now = rot::unix(now);
                (s <= now && now < e).then(|| s.to_string())
            }
            Source::DailyBonuses => Some((rot::server_time(now).daily_reset_at - DAY).to_string()),
            Source::Activity => Some(py_str(&d["window_start"])),
            Source::ServerStatus => match get(d, "overall") {
                None => None,
                Some(Value::String(s)) if s == "unknown" => None,
                Some(v) => Some(format!("status:{}", py_str(v))),
            },
            Source::TroveNews => {
                let item = get(d, "item").filter(|v| truthy(v))?;
                get(item, "url").filter(|v| truthy(v)).map(py_str)
            }
            Source::Giveaways => get(d, "newest").filter(|v| truthy(v)).map(|n| py_str(&n["created_at"])),
            Source::GameUpdate => get(d, "version")
                .filter(|v| truthy(v))
                .map(|v| format!("{}:{}", py_str(&v["branch"]), py_str(&v["ordinal"]))),
        }
    }

    /// `next_at_fn()`: unix seconds of the next boundary, from fresh data at
    /// `now` (µs). `None` for the sources nothing schedules.
    pub fn next_at(self, data: &Gamedata, now: i64) -> Option<i64> {
        match self {
            Source::Corruxion | Source::Fluxion => {
                let d = self.timed_data(data, now).ok()?;
                let key = if get(&d, "active").is_some_and(truthy) { "ends_at" } else { "starts_at" };
                d[key].as_i64()
            }
            Source::Longshade | Source::WildMana => {
                let d = self.timed_data(data, now).ok()?;
                get(&d, "current").filter(|v| truthy(v)).and_then(|c| get(c, "ends_at")).and_then(Value::as_i64)
            }
            Source::Stampy => {
                let d = self.timed_data(data, now).ok()?;
                let now = rot::unix(now);
                match get(&d, "current").filter(|v| truthy(v)) {
                    None => Some(now + 3600),
                    Some(cur) => {
                        let s = cur["starts_at"].as_i64()?;
                        if now < s { Some(s) } else { cur["ends_at"].as_i64() }
                    }
                }
            }
            Source::DailyBonuses | Source::Activity => Some(rot::server_time(now).daily_reset_at),
            _ => None,
        }
    }
}

/// `d.get(key)` on a dict.
fn get<'a>(d: &'a Value, key: &str) -> Option<&'a Value> {
    d.as_object()?.get(key)
}

fn opt_str(v: Option<&Value>) -> String {
    v.map_or_else(|| "None".to_owned(), py_str)
}

/// `_luxion_sig`: the run start plus the latest opened window.
fn luxion_sig(d: &Value, now: i64) -> Option<String> {
    let starts = get(d, "starts_at").filter(|v| truthy(v))?;
    if !get(d, "active").is_some_and(truthy) {
        return None;
    }
    let schedule = get(d, "schedule").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let opened = schedule.iter().rfind(|w| w["starts_at"].as_i64().is_some_and(|s| s <= now));
    Some(match opened {
        Some(w) => format!("{}:{}", py_str(starts), py_str(&w["starts_at"])),
        None => py_str(starts),
    })
}

/// `server_time.corruxion()` / `fluxion()` as Python builds the dicts:
/// Corruxion carries no `state` at all.
pub fn merchant(m: &rot::Merchant) -> Value {
    let mut out = Map::new();
    out.insert("active".into(), m.active.into());
    if let Some(state) = m.state {
        out.insert("state".into(), state.into());
    }
    out.insert("starts_at".into(), m.starts_at.into());
    out.insert("ends_at".into(), m.ends_at.into());
    out.insert("seconds_remaining".into(), m.seconds_remaining.into());
    let schedule = m
        .schedule
        .iter()
        .map(|w| {
            let mut win = Map::new();
            win.insert("starts_at".into(), w.starts_at.into());
            win.insert("ends_at".into(), w.ends_at.into());
            if let Some(state) = &w.state {
                win.insert("state".into(), state.clone().into());
            }
            Value::Object(win)
        })
        .collect();
    out.insert("schedule".into(), Value::Array(schedule));
    Value::Object(out)
}

/// `server_time.daily_buffs()`: the raw baseline entries.
pub fn daily_data(data: &Gamedata, now: i64) -> Value {
    let current = data.daily.get(&rot::daily_buff_index(now).to_string()).cloned().unwrap_or_else(|| json!({}));
    let week: Vec<Value> = (0..7).filter_map(|i| data.daily.get(&i.to_string()).cloned()).collect();
    json!({"current": current, "week": week})
}

fn str_dt(t: &DateTime<Utc>) -> Value {
    Value::String(dt::str_dt(t))
}

/// `get_current_challenge()`.
pub fn challenge_data(now: i64, capture: Option<(&str, DateTime<Utc>)>) -> Value {
    let mut v = bodies::challenge_current(now, capture);
    v["captured_at"] = capture.map_or(Value::Null, |(_, at)| str_dt(&at));
    v
}

/// `get_luxion()`.
pub fn luxion_data(doc: Option<&LuxionDoc>, now: i64) -> Value {
    let mut v = bodies::luxion(doc, now);
    if let Some(doc) = doc {
        v["first_seen_at"] = str_dt(&doc.first_seen_at);
        v["last_seen_at"] = str_dt(&doc.last_seen_at);
    }
    v
}

/// `get_chaos_chest()` once the item's blueprint is resolved.
pub fn chaos_data(chest: &chaos::Chest, resolved: Option<String>, api_url: &str) -> Value {
    let mut v = chaos::body(chest, resolved, api_url);
    v["fetched_at"] = chest.fetched_at.as_ref().map_or(Value::Null, str_dt);
    v
}

pub fn mongo(state: &AppState) -> anyhow::Result<&Database> {
    state.inner.db.mongo.as_ref().context("MONGO_URI not configured")
}

fn invalid(what: &str) -> anyhow::Error {
    anyhow!("{what} failed validation")
}

async fn challenge(state: &AppState, now: i64) -> anyhow::Result<Value> {
    let window = rot::challenge_window(now);
    let doc = store::challenge_for_window(mongo(state)?, window.starts_at).await?;
    Ok(challenge_data(now, doc.as_ref().map(|d| (d.name.as_str(), d.captured_at))))
}

async fn chaos_chest(state: &AppState, now: i64) -> anyhow::Result<Value> {
    let db = mongo(state)?;
    let week = rot::chaos_chest_window(now);
    let built = match store::chaos_for_week(db, week.0).await? {
        Some(c) => chaos::build(Some(&chaos::capture_item(&c.name, week)), Some(c.captured_at), now),
        None => match store::chaos_feed(db).await? {
            Some((cached, fetched_at)) => chaos::build(cached.as_ref(), Some(fetched_at), now),
            None => chaos::build(None, None, now),
        },
    };
    let chest = built.map_err(|_| invalid("chaos chest item"))?;
    let resolved = match chaos::needs_blueprint(&chest) {
        Some(name) => chaos::resolve_blueprint(state, name).await,
        None => None,
    };
    Ok(chaos_data(&chest, resolved, &state.settings().api_url))
}

// --- trove_news, giveaways, game_update ---------------------------------------------

pub struct NewsTop {
    pub title: String,
    pub url: String,
    pub published_at: Option<DateTime<Utc>>,
}

pub fn news_data(top: Option<&NewsTop>) -> Value {
    match top {
        None => json!({"item": null}),
        Some(n) => json!({"item": {
            "title": n.title,
            "url": n.url,
            "published_at": n.published_at.as_ref().map(dt::isoformat),
        }}),
    }
}

pub struct GiveawayTop {
    pub id: String,
    pub title: String,
    pub created_at: i64,
}

pub fn giveaways_data(top: Option<&GiveawayTop>) -> Value {
    match top {
        None => json!({"newest": null}),
        Some(g) => json!({"newest": {"id": g.id, "title": g.title, "created_at": g.created_at}}),
    }
}

pub struct Version {
    pub branch: String,
    pub ordinal: i64,
    pub version_tag: String,
    pub files_added: i64,
    pub files_modified: i64,
    pub files_removed: i64,
    pub bytes_added: i64,
    pub completed_at: Option<DateTime<Utc>>,
}

pub fn game_update_data(v: Option<&Version>) -> Value {
    match v {
        None => json!({"version": null}),
        Some(v) => json!({"version": {
            "branch": v.branch,
            "ordinal": v.ordinal,
            "version_tag": v.version_tag,
            "files_added": v.files_added,
            "files_modified": v.files_modified,
            "files_removed": v.files_removed,
            "bytes_added": v.bytes_added,
            "completed_at": v.completed_at.as_ref().map(dt::isoformat),
        }}),
    }
}

fn string(d: &Document, key: &str) -> anyhow::Result<String> {
    match d.get(key) {
        Some(Bson::String(s)) => Ok(s.clone()),
        _ => Err(invalid(key)),
    }
}

fn int_or(d: &Document, key: &str, default: Option<i64>) -> anyhow::Result<i64> {
    match (d.get(key), default) {
        (None, Some(def)) => Ok(def),
        (Some(v), _) => store::as_int(v).map_err(|_| invalid(key)),
        (None, None) => Err(invalid(key)),
    }
}

fn opt_datetime(d: &Document, key: &str) -> anyhow::Result<Option<DateTime<Utc>>> {
    match d.get(key) {
        None | Some(Bson::Null) => Ok(None),
        Some(Bson::DateTime(t)) => Ok(Some(dt::from_bson(*t))),
        _ => Err(invalid(key)),
    }
}

async fn newest(db: &Database, coll: &str, filter: Document, sort: Document) -> anyhow::Result<Option<Document>> {
    Ok(db.collection::<Document>(coll).find_one(filter).sort(sort).await?)
}

/// `latest_news(1)`: the newest article (the first one is never a duplicate).
pub async fn latest_news(db: &Database) -> anyhow::Result<Option<NewsTop>> {
    let Some(d) = newest(db, "trove_news", doc! {}, doc! { "published_at": -1 }).await? else { return Ok(None) };
    Ok(Some(NewsTop {
        title: string(&d, "title")?,
        url: string(&d, "url")?,
        published_at: opt_datetime(&d, "published_at")?,
    }))
}

pub async fn newest_open_giveaway(db: &Database) -> anyhow::Result<Option<GiveawayTop>> {
    let Some(d) = newest(db, "giveaways", doc! { "status": "open" }, doc! { "created_at": -1 }).await? else {
        return Ok(None);
    };
    let id = match d.get("_id") {
        Some(Bson::ObjectId(oid)) => oid.to_hex(),
        _ => return Err(invalid("_id")),
    };
    let created_at = match d.get("created_at") {
        Some(Bson::DateTime(t)) => t.timestamp_millis() / 1000,
        None => dt::now().timestamp(),
        _ => return Err(invalid("created_at")),
    };
    Ok(Some(GiveawayTop { id, title: string(&d, "title")?, created_at }))
}

/// `updates.read.latest_version("live-us")`.
pub async fn latest_version(db: &Database) -> anyhow::Result<Option<Version>> {
    let filter = doc! { "branch": GAME_UPDATE_BRANCH, "status": "complete" };
    let Some(d) = newest(db, "update_versions", filter, doc! { "ordinal": -1 }).await? else { return Ok(None) };
    Ok(Some(Version {
        branch: string(&d, "branch")?,
        ordinal: int_or(&d, "ordinal", None)?,
        version_tag: string(&d, "version_tag")?,
        files_added: int_or(&d, "files_added", Some(0))?,
        files_modified: int_or(&d, "files_modified", Some(0))?,
        files_removed: int_or(&d, "files_removed", Some(0))?,
        bytes_added: int_or(&d, "bytes_added", Some(0))?,
        completed_at: opt_datetime(&d, "completed_at")?,
    }))
}
