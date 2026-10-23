//! `app/bot/announcements.py`: the announcement-type registry the dashboard
//! lists, and each type's edge-trigger anchor (the string persisted as
//! `last_anchor`, so it must match Python's byte for byte).

use futures::TryStreamExt;
use mongodb::bson::{Bson, Document, doc};
use serde_json::Value;

use super::{commands::Domain, embeds};
use crate::{feeds::pyval, i18n::Lang, og::data, routes::rotations::bodies, state::AppState};

pub struct AnnouncementType {
    pub key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub category: &'static str,
    pub auto_manage: bool,
}

const fn t(
    key: &'static str,
    label: &'static str,
    description: &'static str,
    category: &'static str,
    auto_manage: bool,
) -> AnnouncementType {
    AnnouncementType { key, label, description, category, auto_manage }
}

/// `ANNOUNCEMENT_TYPES`, in order.
pub const TYPES: [AnnouncementType; 16] = [
    t("challenge_collection", "Hourly: Collection", "Post Collection hourly challenges only.", "Rotations", true),
    t("challenge_rampage", "Hourly: Rampage", "Post Rampage Alert hourly challenges only.", "Rotations", true),
    t("challenge_dungeon", "Hourly: Dungeon", "Post Dungeon / biome hourly challenges only.", "Rotations", true),
    t(
        "chaos_chest",
        "Chaos Chest",
        "Post the week's featured Chaos Chest item at each Tuesday rotation.",
        "Rotations",
        true,
    ),
    t("daily_bonuses", "Daily bonuses", "Post the day's bonus at each daily reset (11:00 UTC).", "Rotations", true),
    t("longshade", "Depth-15 biomes", "Post the Depth-15 delve biome rotation every 3 hours.", "Rotations", true),
    t("wild_mana", "Wild Mana", "Post the weekly Wild Mana biome rotation.", "Rotations", true),
    t("stampy", "Stampy event", "Post when the Stampy event begins each week.", "Rotations", true),
    t("corruxion", "Corruxion merchant", "Post when the Corruxion merchant arrives.", "Rotations", true),
    t(
        "fluxion",
        "Fluxion merchant",
        "Post when the Fluxion merchant arrives (voting / selling stage).",
        "Rotations",
        true,
    ),
    t(
        "luxion",
        "Trials of Luxion",
        "Post when the Trials of Luxion start, then again as each rotation opens.",
        "Rotations",
        true,
    ),
    t("trove_news", "Trove news", "Post each new Trove news article from the official feed.", "Feeds", false),
    t("giveaways", "Giveaways", "Post when a new giveaway opens on trove.aallyn.net.", "Feeds", false),
    t("activity", "Player activity", "Post a daily player-activity snapshot at the daily reset.", "Feeds", false),
    t(
        "server_status",
        "Server status",
        "Post when Trove's overall server status changes (online / down).",
        "Feeds",
        true,
    ),
    t(
        "game_update",
        "Game updates",
        "Post when a new Trove build goes live (file-change summary image + link).",
        "Feeds",
        false,
    ),
];

pub fn by_key(key: &str) -> Option<&'static AnnouncementType> {
    TYPES.iter().find(|a| a.key == key)
}

pub fn keys() -> Vec<&'static str> {
    TYPES.iter().map(|a| a.key).collect()
}

fn s(v: &Value) -> String {
    pyval::py_str(v)
}

fn get<'a>(v: &'a Value, k: &str) -> &'a Value {
    v.get(k).unwrap_or(&Value::Null)
}

fn truthy(v: &Value) -> bool {
    pyval::truthy(v)
}

/// `str(server_time()["daily_reset_at"] - DAY)`.
fn daily_window(now_us: i64) -> String {
    (kiwi_core::rotations::server_time(now_us).daily_reset_at - 86400).to_string()
}

fn current_start(r: &Value) -> Option<String> {
    let cur = get(r, "current");
    let st = get(cur, "starts_at");
    truthy(st).then(|| s(st))
}

/// The anchor fn of `key`: `Ok(None)` when there's nothing to announce.
pub async fn current_anchor(state: &AppState, key: &str, now_us: i64) -> anyhow::Result<Option<String>> {
    let now = now_us.div_euclid(1_000_000);
    Ok(match key {
        "challenge_collection" | "challenge_rampage" | "challenge_dungeon" => {
            let cat = key.trim_start_matches("challenge_");
            let cur = data::challenge(state, now_us).await?;
            (truthy(get(&cur, "active")) && truthy(get(&cur, "name")) && get(&cur, "type").as_str() == Some(cat))
                .then(|| s(get(&cur, "starts_at")))
        }
        "chaos_chest" => {
            let c = data::chaos_chest(state, now_us).await?;
            truthy(get(get(&c, "item"), "name")).then(|| s(get(&c, "starts_at")))
        }
        "daily_bonuses" | "activity" => Some(daily_window(now_us)),
        "longshade" => current_start(&data::biome_rotation(now_us)?),
        "wild_mana" => current_start(&data::wild_mana(now_us)?),
        "stampy" => {
            let st = data::stampy(now_us)?;
            let cur = get(&st, "current");
            let (a, b) = (get(cur, "starts_at").as_i64(), get(cur, "ends_at").as_i64());
            match (truthy(cur), a, b) {
                (true, Some(a), Some(b)) if a <= now && now < b => Some(a.to_string()),
                _ => None,
            }
        }
        "corruxion" => {
            let c = bodies::corruxion(now_us);
            truthy(get(&c, "active")).then(|| s(get(&c, "starts_at")))
        }
        "fluxion" => {
            let f = bodies::fluxion(now_us);
            truthy(get(&f, "active")).then(|| s(get(&f, "starts_at")))
        }
        "luxion" => {
            let l = data::luxion(state, now_us).await?;
            if !(truthy(get(&l, "active")) && truthy(get(&l, "starts_at"))) {
                return Ok(None);
            }
            let opened: Vec<&Value> = match get(&l, "schedule") {
                Value::Array(a) => {
                    a.iter().filter(|w| get(w, "starts_at").as_i64().is_some_and(|st| st <= now)).collect()
                }
                _ => Vec::new(),
            };
            Some(match opened.last() {
                Some(w) => format!("{}:{}", s(get(&l, "starts_at")), s(get(w, "starts_at"))),
                None => s(get(&l, "starts_at")),
            })
        }
        "trove_news" => {
            let docs = crate::feeds::store::news_page(state.mongo(), None, 3).await;
            let items = crate::feeds::routes::dedupe_news(docs, 1);
            match items.first() {
                None => None,
                Some(top) if !top.url.is_empty() => Some(top.url.clone()),
                Some(top) => top.published_at.map(|p| crate::dt::isoformat(&p)),
            }
        }
        "giveaways" => {
            let coll = state.mongo().collection::<Document>("giveaways");
            let rows: Vec<Document> =
                coll.find(doc! {"status": "open"}).sort(doc! {"created_at": -1}).limit(1).await?.try_collect().await?;
            match rows.first().map(|d| d.get("created_at")) {
                Some(Some(Bson::DateTime(at))) => Some(at.timestamp_millis().div_euclid(1000).to_string()),
                Some(_) => anyhow::bail!("giveaway created_at"),
                None => None,
            }
        }
        "server_status" => {
            let snap = data::status(state).await;
            let overall = match snap.get("overall") {
                None => "unknown".to_owned(),
                Some(v) => s(v),
            };
            (overall != "unknown").then(|| format!("status:{overall}"))
        }
        "game_update" => {
            latest_live_us(state).await?.map(|v| format!("{}:{}", s(get(&v, "branch")), s(get(&v, "ordinal"))))
        }
        _ => None,
    })
}

/// `updates.read.latest_version("live-us")` as JSON.
pub async fn latest_live_us(state: &AppState) -> anyhow::Result<Option<Value>> {
    let coll = state.mongo().collection::<Document>("update_versions");
    let rows: Vec<Document> = coll
        .find(doc! {"branch": "live-us", "status": "complete"})
        .sort(doc! {"ordinal": -1})
        .limit(1)
        .await?
        .try_collect()
        .await?;
    Ok(rows.into_iter().next().map(|d| Bson::Document(d).into_relaxed_extjson()))
}

/// `atype.build_embed()`: the type's default embed in `l` (`None`: unknown key).
pub async fn build_embed<D: Domain>(d: &D, l: Lang, key: &str) -> anyhow::Result<Option<Value>> {
    let now = d.now_us().div_euclid(1_000_000);
    let asset = d.asset_url();
    Ok(Some(match key {
        "challenge_collection" | "challenge_rampage" | "challenge_dungeon" => {
            embeds::challenge_embed(l, &d.challenge().await?)?
        }
        "chaos_chest" => embeds::chaos_embed(l, &d.chaos().await?)?,
        "daily_bonuses" => embeds::bonuses_embed(l, &d.daily_buffs(), &d.weekly_buffs())?,
        "longshade" => embeds::longshade_embed(l, &d.biome_rotation()?)?,
        "wild_mana" => embeds::wild_mana_embed(l, &d.wild_mana()?)?,
        "stampy" => embeds::stampy_embed(l, &d.stampy()?, now)?,
        "corruxion" => embeds::corruxion_embed(l, &d.corruxion())?,
        "fluxion" => embeds::fluxion_embed(l, &d.fluxion())?,
        "luxion" => embeds::luxion_embed(l, &asset, &d.luxion().await?, now)?,
        "trove_news" => embeds::trove_news_embed(l, &d.news(5).await),
        "giveaways" => {
            let (o, u, e) = d.giveaways().await?;
            embeds::giveaways_embed(l, &o, &u, &e)?
        }
        "activity" => {
            let (live, series) = d.activity("7d").await?;
            embeds::activity_embed(l, &asset, "7d", &live, &series)?
        }
        "server_status" => embeds::status_embed(l, &d.status().await)?,
        "game_update" => embeds::game_update_embed(l, &asset, d.latest_update().await?.as_ref()),
        _ => return Ok(None),
    }))
}

/// `atype.caption()`: only Luxion has one.
pub async fn caption<D: Domain>(d: &D, l: Lang, key: &str) -> anyhow::Result<Option<Value>> {
    if key != "luxion" {
        return Ok(None);
    }
    let now = d.now_us().div_euclid(1_000_000);
    Ok(Some(embeds::luxion_caption(l, &d.luxion().await?, now)?))
}
