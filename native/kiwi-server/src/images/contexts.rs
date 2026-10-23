//! `app/discord/embed_contexts.py` as Image Studio uses it: the bindable kinds,
//! their sample variables and their live variables (English, the language a
//! render request runs in).
//!
//! In Python `daily_bonuses`, `corruxion` and `fluxion` import names
//! `app.trove.rotations` does not have, so their live context raises and the
//! render falls back to the sample; that is what [`live`] returns for them.

use serde_json::{Map, Value, json};

use super::render::Ctx;
use crate::{
    activity::player,
    giveaways::{service as giveaways, store::Mongo},
    og::{board::py_title, data},
    state::AppState,
};

const SITE: &str = "https://trove.aallyn.net";

pub const KINDS: [&str; 13] = [
    "challenge",
    "chaos",
    "daily_bonuses",
    "longshade",
    "wild_mana",
    "stampy",
    "corruxion",
    "fluxion",
    "trove_news",
    "giveaways",
    "activity",
    "server_status",
    "game_update",
];

const KIND_BY_KEY: [(&str, &str); 17] = [
    ("challenge_collection", "challenge"),
    ("challenge_rampage", "challenge"),
    ("challenge_racing", "challenge"),
    ("challenge_target", "challenge"),
    ("challenge_dungeon", "challenge"),
    ("chaos_chest", "chaos"),
    ("daily_bonuses", "daily_bonuses"),
    ("longshade", "longshade"),
    ("wild_mana", "wild_mana"),
    ("stampy", "stampy"),
    ("corruxion", "corruxion"),
    ("fluxion", "fluxion"),
    ("trove_news", "trove_news"),
    ("giveaways", "giveaways"),
    ("activity", "activity"),
    ("server_status", "server_status"),
    ("game_update", "game_update"),
];

/// `_kind`: a bare kind, or the kind an announcement key maps to.
pub fn kind(key: &str) -> Option<&'static str> {
    KINDS.iter().find(|k| **k == key).copied().or_else(|| KIND_BY_KEY.iter().find(|(k, _)| *k == key).map(|(_, v)| *v))
}

pub fn is_bindable(key: &str) -> bool {
    kind(key).is_some()
}

fn label(kind: &str) -> &'static str {
    match kind {
        "challenge" => "Hourly challenge",
        "chaos" => "Chaos Chest",
        "daily_bonuses" => "Daily bonuses",
        "longshade" => "Depth-15 biomes",
        "wild_mana" => "Wild Mana",
        "stampy" => "Stampy",
        "corruxion" => "Corruxion",
        "fluxion" => "Fluxion",
        "trove_news" => "Trove news",
        "giveaways" => "Giveaways",
        "activity" => "Player activity",
        "server_status" => "Server status",
        _ => "Game update",
    }
}

fn obj(v: Value) -> Ctx {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// `sample_context(key)`.
pub fn sample(key: &str) -> Ctx {
    let Some(kind) = kind(key) else { return Map::new() };
    obj(match kind {
        "challenge" => json!({"name": "Racing Challenge", "type": "racing", "type_label": "Racing",
            "starts_at": 1718200000, "ends_at": 1718201200, "status": "live",
            "current_line": "**Racing Challenge** · Racing\nEnds in 18 minutes"}),
        "chaos" => json!({"item_name": "Radiant Mount", "desc_line": "This week's featured item: **Radiant Mount**",
            "starts_at": 1718000000, "ends_at": 1718600000}),
        "daily_bonuses" => json!({"daily_name": "Combat", "daily_weekday": "Friday", "daily_emoji": "⚔️",
            "daily_buffs": "• +50% Combat XP\n• +25% Loot", "weekly_name": "Geode",
            "weekly_emoji": "🗓️", "weekly_buffs": "• +Geode topside drops"}),
        "longshade" | "wild_mana" => json!({"current_biomes": "• Permafrost\n• Cursed Vale\n• Jurassic Jungle",
            "rotates_at": 1718200000, "next_biomes": "• Fae Forest\n• Neon City", "next_at": 1718210000}),
        "stampy" => json!({"biome": "Candoria", "starts_at": 1718200000, "ends_at": 1718372800,
            "state_line": "**Active now** — ends in 1 day"}),
        "corruxion" | "fluxion" => json!({"status": "here", "state": "selling", "starts_at": 1718200000,
            "ends_at": 1718459200, "desc_line": "**Here now!**\nLeaves in 2 days",
            "upcoming": "Jun 20 → Jun 23\nJul 04 → Jul 07"}),
        "trove_news" => json!({"title": "Summer Update is live!", "url": SITE, "image_url": null,
            "summary": "Patch notes, new mounts, and a seasonal event.",
            "more": "• [Dev blog](https://trovegame.com)\n• [Known issues](https://trovegame.com)"}),
        "giveaways" => json!({"open_count": 1, "upcoming_count": 1, "ended_count": 2,
            "open_lines": "**Holiday code drop** — Mount Code\nEnds in 3 days · 42 entered",
            "upcoming_lines": "**Next week** — Mystery Prize", "ended_lines": "**Last draw** — Glim Pack"}),
        "activity" => json!({"label": "Last 7 days", "last_24h": "18,764", "last_7d": "52,310",
            "peak": "21,120", "average": "17,480", "image_url": null}),
        "server_status" => json!({"overall": "online", "overall_text": "Online", "overall_dot": "🟢",
            "eu_line": "🟢 Online", "us_line": "🟢 Online", "pts_line": "🟡 Maintenance",
            "auth_line": "Reachable"}),
        _ => json!({"version_tag": "2024.06.01", "files_added": "12", "files_modified": "8",
            "files_removed": "1", "ordinal": 142, "image_url": null}),
    })
}

/// `bindings()`: the studio's binding picker.
pub fn bindings() -> Value {
    Value::Array(
        KINDS
            .iter()
            .map(|k| {
                let s = sample(k);
                let vars: Vec<&String> = s.keys().filter(|k| *k != "image_url").collect();
                json!({
                    "key": k,
                    "label": label(k),
                    "variables": vars,
                    "sample": s,
                    "has_image": matches!(*k, "trove_news" | "activity" | "game_update"),
                })
            })
            .collect(),
    )
}

fn ts(unix: &Value, style: &str) -> anyhow::Result<String> {
    let i = crate::feeds::pyval::py_int(unix).ok_or_else(|| anyhow::anyhow!("int() of {unix}"))?;
    Ok(format!("<t:{i}:{style}>"))
}

fn get<'a>(v: &'a Value, k: &str) -> &'a Value {
    v.get(k).unwrap_or(&Value::Null)
}

fn truthy(v: &Value) -> bool {
    crate::features::py_truthy(v)
}

fn text(v: &Value) -> String {
    crate::feeds::pyval::py_str(v)
}

fn biomes(rot: &Value) -> String {
    let lines: Vec<String> = match get(rot, "biomes") {
        Value::Array(a) => a
            .iter()
            .map(|b| {
                let name = get(b, "final_name");
                format!("• {}", text(if truthy(name) { name } else { get(b, "name") }))
            })
            .collect(),
        _ => Vec::new(),
    };
    if lines.is_empty() { "—".into() } else { lines.join("\n") }
}

fn or_empty(v: &Value) -> Value {
    if truthy(v) { v.clone() } else { Value::Object(Map::new()) }
}

fn biome_rotation(r: &Value) -> Ctx {
    let cur = or_empty(get(r, "current"));
    let first = match get(r, "upcoming") {
        Value::Array(a) if !a.is_empty() => a[0].clone(),
        _ => Value::Null,
    };
    let nxt = or_empty(&first);
    let has_next = truthy(&nxt);
    obj(json!({
        "current_biomes": biomes(&cur),
        "rotates_at": get(&cur, "ends_at"),
        "next_biomes": if has_next { biomes(&nxt) } else { "—".into() },
        "next_at": get(&nxt, "starts_at"),
    }))
}

fn count(v: &Value) -> String {
    match v {
        Value::Bool(b) => (*b as i64).to_string(),
        Value::Number(n) => {
            let r = match n.as_i64() {
                Some(i) => i as i128,
                None => n.as_f64().map_or(0, |f| f.round_ties_even() as i128),
            };
            crate::og::text::thousands(r as i64)
        }
        _ => "—".into(),
    }
}

fn thousands(v: &Value) -> String {
    match v.as_i64() {
        Some(i) => crate::og::text::thousands(i),
        None => text(v),
    }
}

fn dot(status: &str) -> &'static str {
    match status {
        "online" => "\u{1F7E2}",
        "maintenance" | "down" => "\u{1F534}",
        _ => "⚪",
    }
}

fn overall_text(overall: &str) -> &'static str {
    match overall {
        "online" => "All systems operational",
        "maintenance" | "down" => "Major outage",
        _ => "Status unknown",
    }
}

async fn challenge(state: &AppState, now_us: i64) -> anyhow::Result<Ctx> {
    let c = data::challenge(state, now_us).await?;
    let name = get(&c, "name");
    let ctype = if truthy(get(&c, "type")) { text(get(&c, "type")) } else { String::new() };
    let active = truthy(get(&c, "active"));
    let line = if truthy(name) {
        let mut l = format!("**{}**", text(name));
        if !ctype.is_empty() {
            l.push_str(&format!(" · {}", py_title(&ctype)));
        }
        l.push('\n');
        if active {
            l.push_str(&format!("Ends {}", ts(get(&c, "ends_at"), "R")?));
        } else {
            l.push_str(&format!("Next window {}", ts(get(&c, "starts_at"), "R")?));
        }
        l
    } else {
        "No challenge captured for this window.".into()
    };
    Ok(obj(json!({
        "name": if truthy(name) { name.clone() } else { Value::String(String::new()) },
        "type": ctype,
        "type_label": py_title(&ctype),
        "starts_at": get(&c, "starts_at"),
        "ends_at": get(&c, "ends_at"),
        "status": if active { "live" } else { "upcoming" },
        "current_line": line,
    })))
}

async fn chaos(state: &AppState, now_us: i64) -> anyhow::Result<Ctx> {
    let c = data::chaos_chest(state, now_us).await?;
    let name = get(&or_empty(get(&c, "item")), "name").clone();
    let desc = if truthy(&name) {
        format!("This week's featured item: **{}**", text(&name))
    } else {
        "No item has been relayed for this week's chest yet.".into()
    };
    Ok(obj(json!({
        "item_name": if truthy(&name) { name } else { Value::String(String::new()) },
        "desc_line": desc,
        "starts_at": get(&c, "starts_at"),
        "ends_at": get(&c, "ends_at"),
    })))
}

fn stampy(now_us: i64) -> anyhow::Result<Ctx> {
    let s = data::stampy(now_us)?;
    let cur = or_empty(get(&s, "current"));
    let biome = match get(&cur, "biomes") {
        Value::Array(a) if !a.is_empty() => a[0].clone(),
        _ => Value::Object(Map::new()),
    };
    let now = now_us.div_euclid(1_000_000);
    let has = truthy(&cur);
    let num = |k: &str| get(&cur, k).as_i64();
    let state = if has && num("starts_at").unwrap_or(0) <= now && now < num("ends_at").unwrap_or(0) {
        format!("**Active now** — ends {}", ts(get(&cur, "ends_at"), "R")?)
    } else if has {
        format!("Starts {}", ts(get(&cur, "starts_at"), "R")?)
    } else {
        "No upcoming Stampy event scheduled.".into()
    };
    let fname = get(&biome, "final_name");
    let name = if truthy(fname) {
        fname.clone()
    } else if truthy(get(&biome, "name")) {
        get(&biome, "name").clone()
    } else {
        Value::String("?".into())
    };
    Ok(obj(json!({
        "biome": name,
        "starts_at": get(&cur, "starts_at"),
        "ends_at": get(&cur, "ends_at"),
        "state_line": state,
    })))
}

async fn news(state: &AppState) -> Ctx {
    let docs = crate::feeds::store::news_page(state.mongo(), None, 15).await;
    let items = crate::feeds::routes::dedupe_news(docs, 5);
    let Some(top) = items.first() else {
        return obj(json!({"title": "", "url": SITE, "summary": "", "more": "—", "image_url": null}));
    };
    let more: Vec<String> = items.iter().skip(1).take(4).map(|n| format!("• [{}]({})", n.title, n.url)).collect();
    obj(json!({
        "title": top.title,
        "url": top.url,
        "summary": kiwi_core::pystr::prefix(&top.summary, 600),
        "more": if more.is_empty() { "—".into() } else { more.join("\n") },
        "image_url": top.image,
    }))
}

fn unix_of(v: &Value) -> anyhow::Result<i64> {
    let s = v.as_str().ok_or_else(|| anyhow::anyhow!("no ends_at"))?;
    Ok(crate::dt::parse_utc(s).ok_or_else(|| anyhow::anyhow!("bad ends_at"))?.timestamp())
}

async fn giveaway_lines(state: &AppState) -> anyhow::Result<Ctx> {
    let store = Mongo { db: state.mongo(), email_enabled: false };
    let err = |e: crate::error::ApiError| anyhow::anyhow!("{}", e.body());
    let ongoing = giveaways::list_ongoing(&store).await.map_err(err)?;
    let upcoming = giveaways::list_upcoming(&store).await.map_err(err)?;
    let ended = giveaways::list_ended(&store, 7).await.map_err(err)?;
    let mut open = Vec::new();
    for g in ongoing.iter().take(5) {
        let when = format!("<t:{}:R>", unix_of(get(g, "ends_at"))?);
        open.push(format!(
            "**{}** — {}\nEnds {when} · {} entered",
            text(get(g, "title")),
            text(get(g, "prize_name")),
            text(get(g, "entry_count"))
        ));
    }
    let short = |rows: &[Value]| -> Vec<String> {
        rows.iter().take(5).map(|g| format!("**{}** — {}", text(get(g, "title")), text(get(g, "prize_name")))).collect()
    };
    let or = |v: Vec<String>, sep: &str, empty: &str| if v.is_empty() { empty.to_owned() } else { v.join(sep) };
    Ok(obj(json!({
        "open_count": ongoing.len(),
        "upcoming_count": upcoming.len(),
        "ended_count": ended.len(),
        "open_lines": or(open, "\n\n", "None right now."),
        "upcoming_lines": or(short(&upcoming), "\n", "Nothing scheduled."),
        "ended_lines": or(short(&ended), "\n", "None in the last 7 days."),
    })))
}

async fn activity(state: &AppState) -> anyhow::Result<Ctx> {
    let live = player::estimate_active_players(state, false).await?;
    let series = player::series(state, "7d").await?;
    let peak = or_empty(get(&series, "peak"));
    let cb = [get(&live, "computed_at"), get(&series, "window_end")]
        .into_iter()
        .find(|v| truthy(v))
        .cloned()
        .unwrap_or(Value::from(0));
    let asset = state.settings().asset_url();
    Ok(obj(json!({
        "label": "Last 7 days",
        "last_24h": count(get(&live, "estimate_24h")),
        "last_7d": count(get(&live, "estimate_7d")),
        "peak": count(get(&peak, "active")),
        "average": count(get(&series, "average")),
        "image_url": format!("{asset}/activity/og.png?period=7d&v={}", text(&cb)),
    })))
}

async fn server_status(state: &AppState) -> Ctx {
    let snap = data::status(state).await;
    let overall = snap.get("overall").map_or("unknown".to_owned(), text);
    let envs = or_empty(get(&snap, "environments"));
    let line = |k: &str| {
        let env = or_empty(get(&envs, k));
        let st = env.get("status").map_or("unknown".to_owned(), text);
        format!("{} {}", dot(&st), py_title(&st))
    };
    let auth_line = match snap.get("auth") {
        None | Some(Value::Null) => "Checking…",
        Some(a) if truthy(get(a, "online")) => "Reachable",
        Some(_) => "Unreachable",
    };
    let env_online = |k: &str| get(&or_empty(get(&envs, k)), "status").as_str() == Some("online");
    let text_ = if overall == "down" && (env_online("eu") || env_online("us")) {
        "Partial outage"
    } else {
        overall_text(&overall)
    };
    obj(json!({
        "overall": overall,
        "overall_text": text_,
        "overall_dot": dot(&overall),
        "eu_line": line("eu"),
        "us_line": line("us"),
        "pts_line": line("pts"),
        "auth_line": auth_line,
    }))
}

async fn game_update(state: &AppState) -> anyhow::Result<Ctx> {
    use futures::TryStreamExt;
    use mongodb::bson::{Document, doc};
    let coll = state.mongo().collection::<Document>("update_versions");
    let docs: Vec<Document> = coll
        .find(doc! {"branch": "live-us", "status": "complete"})
        .sort(doc! {"ordinal": -1})
        .limit(1)
        .await?
        .try_collect()
        .await?;
    let Some(v) = docs.first() else {
        return Ok(obj(json!({"version_tag": "new build", "files_added": "0", "files_modified": "0",
            "files_removed": "0", "ordinal": null, "image_url": null})));
    };
    let val = |k: &str| -> Value {
        match v.get(k) {
            Some(b) => b.clone().into_relaxed_extjson(),
            None => Value::Null,
        }
    };
    let ordinal = val("ordinal");
    Ok(obj(json!({
        "version_tag": val("version_tag"),
        "files_added": thousands(&val("files_added")),
        "files_modified": thousands(&val("files_modified")),
        "files_removed": thousands(&val("files_removed")),
        "ordinal": ordinal,
        "image_url": format!("{}/announce.png?kind=game_update&v={}", state.settings().asset_url(), text(&ordinal)),
    })))
}

/// `_resolve_context`: the live variables, or the sample when the source fails.
pub async fn live(state: &AppState, key: &str, now_us: i64) -> Ctx {
    let Some(kind) = kind(key) else { return Map::new() };
    let got = match kind {
        "challenge" => challenge(state, now_us).await,
        "chaos" => chaos(state, now_us).await,
        "longshade" => data::biome_rotation(now_us).map(|r| biome_rotation(&r)),
        "wild_mana" => data::wild_mana(now_us).map(|r| biome_rotation(&r)),
        "stampy" => stampy(now_us),
        "trove_news" => Ok(news(state).await),
        "giveaways" => giveaway_lines(state).await,
        "activity" => activity(state).await,
        "server_status" => Ok(server_status(state).await),
        "game_update" => game_update(state).await,
        _ => Err(anyhow::anyhow!("ImportError")),
    };
    got.unwrap_or_else(|e| {
        tracing::debug!("image studio context {kind}: {e:#}");
        sample(kind)
    })
}
