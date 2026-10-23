//! `app/bot/announcements.py`: each type's edge-trigger anchor (the string
//! stored in `last_anchor`, so it must match Python's byte for byte), its
//! expiry and its embed.

use anyhow::Result;
use serde_json::{Value, json};

use crate::{
    discord_common::{
        embeds::{self, Obj, get, req},
        i18n::Tr,
        pyfmt::s,
        sources::{self, Sources},
    },
    features::py_truthy,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Challenge(&'static str),
    Chaos,
    Daily,
    Longshade,
    WildMana,
    Stampy,
    Corruxion,
    Fluxion,
    Luxion,
    News,
    Giveaways,
    Activity,
    Status,
    GameUpdate,
}

pub struct AnnouncementType {
    pub key: &'static str,
    pub kind: Kind,
    pub auto_manage: bool,
    /// Has an expiry function (`expiry is not None`).
    pub expires: bool,
}

/// `ANNOUNCED_CHALLENGE_CATEGORIES`.
pub const CHALLENGE_CATEGORIES: [&str; 3] = ["collection", "rampage", "dungeon"];

const fn t(key: &'static str, kind: Kind, auto_manage: bool, expires: bool) -> AnnouncementType {
    AnnouncementType { key, kind, auto_manage, expires }
}

/// `ANNOUNCEMENT_TYPES`, in registry order.
pub const TYPES: [AnnouncementType; 16] = [
    t("challenge_collection", Kind::Challenge("collection"), true, true),
    t("challenge_rampage", Kind::Challenge("rampage"), true, true),
    t("challenge_dungeon", Kind::Challenge("dungeon"), true, true),
    t("chaos_chest", Kind::Chaos, true, true),
    t("daily_bonuses", Kind::Daily, true, true),
    t("longshade", Kind::Longshade, true, true),
    t("wild_mana", Kind::WildMana, true, true),
    t("stampy", Kind::Stampy, true, true),
    t("corruxion", Kind::Corruxion, true, true),
    t("fluxion", Kind::Fluxion, true, true),
    t("luxion", Kind::Luxion, true, true),
    t("trove_news", Kind::News, false, false),
    t("giveaways", Kind::Giveaways, false, false),
    t("activity", Kind::Activity, false, false),
    t("server_status", Kind::Status, true, false),
    t("game_update", Kind::GameUpdate, false, false),
];

pub fn by_key(key: &str) -> Option<&'static AnnouncementType> {
    TYPES.iter().find(|t| t.key == key)
}

/// `_EVENT_TO_ANNOUNCEMENT`.
pub fn for_event(event: &str) -> Vec<&'static str> {
    match event {
        "challenge" => vec!["challenge_collection", "challenge_rampage", "challenge_dungeon"],
        "chaos" => vec!["chaos_chest"],
        "luxion" | "corruxion" | "fluxion" | "longshade" | "wild_mana" | "stampy" | "daily_bonuses" | "activity"
        | "server_status" | "trove_news" | "giveaways" | "game_update" => {
            vec![by_key(event).map_or("", |t| t.key)]
        }
        _ => Vec::new(),
    }
}

const DAY: i64 = 86_400;

fn current(r: &Value) -> Value {
    embeds::or_empty(get(r, "current"))
}

/// `str(cur["starts_at"]) if cur.get("starts_at") else None`.
fn start_anchor(cur: &Value) -> Option<String> {
    let st = get(cur, "starts_at");
    py_truthy(st).then(|| s(st))
}

fn num(v: &Value) -> Result<f64> {
    v.as_f64().ok_or_else(|| anyhow::anyhow!("TypeError: not a number: {v}"))
}

fn daily_reset(st: &Value) -> Result<i64> {
    req(st, "daily_reset_at")?.as_i64().ok_or_else(|| anyhow::anyhow!("daily_reset_at"))
}

/// The type's current anchor, or `None` when there is nothing to announce.
pub async fn anchor<S: Sources>(src: &S, kind: Kind) -> Result<Option<String>> {
    let now = src.now_us().div_euclid(1_000_000);
    Ok(match kind {
        Kind::Challenge(cat) => {
            let cur = src.challenge().await?;
            let live = py_truthy(get(&cur, "active"))
                && py_truthy(get(&cur, "name"))
                && get(&cur, "type").as_str() == Some(cat);
            if live { Some(s(req(&cur, "starts_at")?)) } else { None }
        }
        Kind::Chaos => {
            let c = src.chaos().await?;
            if py_truthy(get(&embeds::or_empty(get(&c, "item")), "name")) {
                Some(s(req(&c, "starts_at")?))
            } else {
                None
            }
        }
        Kind::Daily | Kind::Activity => Some((daily_reset(&src.server_time())? - DAY).to_string()),
        Kind::Longshade => start_anchor(&current(&src.biome_rotation()?)),
        Kind::WildMana => start_anchor(&current(&src.wild_mana()?)),
        Kind::Stampy => {
            let cur = get(&src.stampy()?, "current").clone();
            if py_truthy(&cur) {
                let (a, b) = (req(&cur, "starts_at")?, req(&cur, "ends_at")?);
                (num(a)? <= now as f64 && (now as f64) < num(b)?).then(|| s(a))
            } else {
                None
            }
        }
        Kind::Corruxion | Kind::Fluxion => {
            let m = if kind == Kind::Corruxion { src.corruxion() } else { src.fluxion() };
            if py_truthy(get(&m, "active")) { Some(s(req(&m, "starts_at")?)) } else { None }
        }
        Kind::Luxion => {
            let l = src.luxion().await?;
            if !(py_truthy(get(&l, "active")) && py_truthy(get(&l, "starts_at"))) {
                return Ok(None);
            }
            let mut opened = None;
            if let Value::Array(sched) = get(&l, "schedule") {
                for w in sched {
                    if num(req(w, "starts_at")?)? <= now as f64 {
                        opened = Some(w.clone());
                    }
                }
            }
            let start = s(req(&l, "starts_at")?);
            Some(match opened {
                Some(w) => format!("{start}:{}", s(req(&w, "starts_at")?)),
                None => start,
            })
        }
        Kind::News => {
            let items = src.news(1).await?;
            match items.first() {
                None => None,
                Some(top) if !top.url.is_empty() => Some(top.url.clone()),
                Some(top) => top.published_at.map(|p| crate::dt::isoformat(&p)),
            }
        }
        Kind::Giveaways => src.newest_open_giveaway().await?.map(|c| c.timestamp().to_string()),
        Kind::Status => {
            let snap = src.status().await;
            let overall = snap.get("overall").cloned().unwrap_or_else(|| json!("unknown"));
            if overall == "unknown" { None } else { Some(format!("status:{}", s(&overall))) }
        }
        Kind::GameUpdate => {
            src.latest_update().await?.map(|v| format!("{}:{}", s(get(&v, "branch")), s(get(&v, "ordinal"))))
        }
    })
}

/// The occurrence's end (unix), when the type has an expiry.
pub async fn expiry<S: Sources>(src: &S, kind: Kind) -> Result<Option<Value>> {
    let end = |v: &Value| get(v, "ends_at").clone();
    Ok(Some(match kind {
        Kind::Challenge(_) => end(&src.challenge().await?),
        Kind::Chaos => end(&src.chaos().await?),
        Kind::Daily => req(&src.server_time(), "daily_reset_at")?.clone(),
        Kind::Longshade => end(&current(&src.biome_rotation()?)),
        Kind::WildMana => end(&current(&src.wild_mana()?)),
        Kind::Stampy => end(&current(&src.stampy()?)),
        Kind::Corruxion => end(&src.corruxion()),
        Kind::Fluxion => end(&src.fluxion()),
        Kind::Luxion => end(&src.luxion().await?),
        _ => return Ok(None),
    }))
}

/// The type's rich embed (`build_embed`) in `tr`'s language.
pub async fn build_embed<S: Sources>(src: &S, kind: Kind, tr: &Tr, asset: &str) -> Result<Obj> {
    let now = src.now_us().div_euclid(1_000_000);
    match kind {
        Kind::Challenge(_) => embeds::challenge_embed(tr, &src.challenge().await?),
        Kind::Chaos => embeds::chaos_embed(tr, &src.chaos().await?),
        Kind::Daily => embeds::bonuses_embed(tr, &src.daily_buff(), &src.weekly_buff()),
        Kind::Longshade => embeds::longshade_embed(tr, &src.biome_rotation()?),
        Kind::WildMana => embeds::wild_mana_embed(tr, &src.wild_mana()?),
        Kind::Stampy => embeds::stampy_embed(tr, &src.stampy()?, now),
        Kind::Corruxion => embeds::corruxion_embed(tr, &src.corruxion()),
        Kind::Fluxion => embeds::fluxion_embed(tr, &src.fluxion()),
        Kind::Luxion => embeds::luxion_embed(tr, &src.luxion().await?, now, asset),
        Kind::News => embeds::trove_news_embed(tr, &src.news(5).await?),
        Kind::Giveaways => {
            let [a, b, c] = src.giveaways().await?;
            embeds::giveaways_embed(tr, &a, &b, &c)
        }
        Kind::Activity => {
            let live = sources::activity_current(src).await;
            let series = sources::activity_series(src).await;
            embeds::activity_embed(tr, &live, &series, asset)
        }
        Kind::Status => embeds::status_embed(tr, &src.status().await),
        Kind::GameUpdate => embeds::game_update_embed(tr, src.latest_update().await?.as_ref(), asset),
    }
}

/// The `caption` merged over the banner (Luxion only).
pub async fn caption<S: Sources>(src: &S, kind: Kind, tr: &Tr) -> Result<Option<Obj>> {
    if kind != Kind::Luxion {
        return Ok(None);
    }
    let now = src.now_us().div_euclid(1_000_000);
    embeds::luxion_caption(tr, &src.luxion().await?, now).map(Some)
}
