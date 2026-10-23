//! `app/discord/embed_contexts.context(key)` in a guild's language: the
//! variables a customized announcement renders against.
//!
//! The `daily_bonuses`, `corruxion` and `fluxion` contexts import names
//! `app.trove.rotations` does not have, so they always fail and those guilds
//! get the default embed.

use anyhow::{Result, bail};
use serde_json::{Value, json};

use super::{
    embeds::{SITE, biomes_str, count, dot, get, get_or, or_empty, overall_text, req, unix},
    i18n::Tr,
    pyfmt::{or_else, s, thousands, ts},
    sources::{self, Sources},
};
use crate::{embed_templates::Ctx, features::py_truthy, og::board::py_title};

/// `KIND_BY_KEY`.
pub fn kind(key: &str) -> Option<&'static str> {
    Some(match key {
        "challenge_collection"
        | "challenge_rampage"
        | "challenge_racing"
        | "challenge_target"
        | "challenge_dungeon" => "challenge",
        "chaos_chest" => "chaos",
        "daily_bonuses" => "daily_bonuses",
        "longshade" => "longshade",
        "wild_mana" => "wild_mana",
        "stampy" => "stampy",
        "corruxion" => "corruxion",
        "fluxion" => "fluxion",
        "trove_news" => "trove_news",
        "giveaways" => "giveaways",
        "activity" => "activity",
        "server_status" => "server_status",
        "game_update" => "game_update",
        _ => return None,
    })
}

/// `has_customization`.
pub fn has_customization(key: &str) -> bool {
    kind(key).is_some()
}

fn obj(v: Value) -> Ctx {
    match v {
        Value::Object(m) => m,
        _ => Ctx::new(),
    }
}

fn truthy_or(v: &Value, alt: Value) -> Value {
    if py_truthy(v) { v.clone() } else { alt }
}

pub async fn context<S: Sources>(src: &S, tr: &Tr, key: &str, asset: &str) -> Result<Ctx> {
    let now = src.now_us().div_euclid(1_000_000);
    match kind(key) {
        Some("challenge") => challenge(tr, &src.challenge().await?),
        Some("chaos") => chaos(tr, &src.chaos().await?),
        Some("longshade") => Ok(biome_rotation(&src.biome_rotation()?)),
        Some("wild_mana") => Ok(biome_rotation(&src.wild_mana()?)),
        Some("stampy") => stampy(tr, &src.stampy()?, now),
        Some("trove_news") => news(src).await,
        Some("giveaways") => giveaways(tr, src).await,
        Some("activity") => Ok(activity(src, asset).await),
        Some("server_status") => Ok(status(tr, &src.status().await)),
        Some("game_update") => game_update(src, asset).await,
        Some(k) => bail!("ImportError: {k} context"),
        None => Ok(Ctx::new()),
    }
}

fn challenge(tr: &Tr, c: &Value) -> Result<Ctx> {
    let name = get(c, "name");
    let ctype = if py_truthy(get(c, "type")) { s(get(c, "type")) } else { String::new() };
    let active = py_truthy(get(c, "active"));
    let line = if py_truthy(name) {
        let mut l = format!("**{}**", s(name));
        if !ctype.is_empty() {
            l.push_str(&format!(" · {}", py_title(&ctype)));
        }
        l.push('\n');
        l.push_str(&if active {
            tr.tf("Ends {when}", &[("when", &ts(req(c, "ends_at")?, "R")?)])
        } else {
            tr.tf("Next window {when}", &[("when", &ts(req(c, "starts_at")?, "R")?)])
        });
        l
    } else {
        tr.t("No challenge captured for this window.")
    };
    Ok(obj(json!({
        "name": truthy_or(name, json!("")),
        "type": ctype,
        "type_label": py_title(&ctype),
        "starts_at": get(c, "starts_at"),
        "ends_at": get(c, "ends_at"),
        "status": if active { "live" } else { "upcoming" },
        "current_line": line,
    })))
}

fn chaos(tr: &Tr, c: &Value) -> Result<Ctx> {
    let name = get(&or_empty(get(c, "item")), "name").clone();
    let desc = if py_truthy(&name) {
        tr.tf("This week's featured item: **{name}**", &[("name", &s(&name))])
    } else {
        tr.t("No item has been relayed for this week's chest yet.")
    };
    Ok(obj(json!({
        "item_name": truthy_or(&name, json!("")),
        "desc_line": desc,
        "starts_at": get(c, "starts_at"),
        "ends_at": get(c, "ends_at"),
    })))
}

fn biome_rotation(r: &Value) -> Ctx {
    let cur = or_empty(get(r, "current"));
    let first = match get(r, "upcoming") {
        Value::Array(a) if !a.is_empty() => a[0].clone(),
        _ => Value::Null,
    };
    let nxt = or_empty(&first);
    obj(json!({
        "current_biomes": biomes_str(&cur),
        "rotates_at": get(&cur, "ends_at"),
        "next_biomes": if py_truthy(&nxt) { biomes_str(&nxt) } else { "—".into() },
        "next_at": get(&nxt, "starts_at"),
    }))
}

fn stampy(tr: &Tr, st: &Value, now: i64) -> Result<Ctx> {
    let cur = or_empty(get(st, "current"));
    let biome = match get(&cur, "biomes") {
        Value::Array(a) if !a.is_empty() => a[0].clone(),
        _ => json!({}),
    };
    let zero = json!(0);
    let num = |k: &str| get_or(&cur, k, &zero).as_f64();
    let has = py_truthy(&cur);
    let live = has
        && match (num("starts_at"), num("ends_at")) {
            (Some(a), Some(b)) => a <= now as f64 && (now as f64) < b,
            _ => bail!("TypeError: stampy window"),
        };
    let state = if live {
        tr.tf("**Active now** — ends {when}", &[("when", &ts(req(&cur, "ends_at")?, "R")?)])
    } else if has {
        tr.tf("Starts {when}", &[("when", &ts(req(&cur, "starts_at")?, "R")?)])
    } else {
        tr.t("No upcoming Stampy event scheduled.")
    };
    Ok(obj(json!({
        "biome": or_else(or_else(get(&biome, "final_name"), get(&biome, "name")), &json!("?")),
        "starts_at": get(&cur, "starts_at"),
        "ends_at": get(&cur, "ends_at"),
        "state_line": state,
    })))
}

async fn news<S: Sources>(src: &S) -> Result<Ctx> {
    let items = src.news(5).await?;
    let Some(top) = items.first() else {
        return Ok(obj(json!({"title": "", "url": SITE, "summary": "", "more": "—", "image_url": null})));
    };
    let more: Vec<String> = items.iter().skip(1).take(4).map(|n| format!("• [{}]({})", n.title, n.url)).collect();
    Ok(obj(json!({
        "title": top.title,
        "url": top.url,
        "summary": kiwi_core::pystr::prefix(&top.summary, 600),
        "more": if more.is_empty() { "—".into() } else { more.join("\n") },
        "image_url": top.image,
    })))
}

async fn giveaways<S: Sources>(tr: &Tr, src: &S) -> Result<Ctx> {
    let [ongoing, upcoming, ended] = src.giveaways().await?;
    let head = |g: &Value| format!("**{}** — {}", s(get(g, "title")), s(get(g, "prize_name")));
    let mut open = Vec::new();
    for g in ongoing.iter().take(5) {
        let when = ts(&json!(unix(get(g, "ends_at"))?), "R")?;
        open.push(format!(
            "{}\n{}",
            head(g),
            tr.tf("Ends {when} · {n} entered", &[("when", &when), ("n", &s(get(g, "entry_count")))])
        ));
    }
    let short = |rows: &[Value]| rows.iter().take(5).map(head).collect::<Vec<_>>();
    let or = |v: Vec<String>, sep: &str, empty: String| if v.is_empty() { empty } else { v.join(sep) };
    Ok(obj(json!({
        "open_count": ongoing.len(),
        "upcoming_count": upcoming.len(),
        "ended_count": ended.len(),
        "open_lines": or(open, "\n\n", tr.t("None right now.")),
        "upcoming_lines": or(short(&upcoming), "\n", tr.t("Nothing scheduled.")),
        "ended_lines": or(short(&ended), "\n", tr.t("None in the last 7 days.")),
    })))
}

async fn activity<S: Sources>(src: &S, asset: &str) -> Ctx {
    let live = sources::activity_current(src).await;
    let series = sources::activity_series(src).await;
    let peak = or_empty(get(&series, "peak"));
    let cb = super::embeds::activity_cache_buster(&live, &series);
    obj(json!({
        "label": "Last 7 days",
        "last_24h": count(get(&live, "estimate_24h")),
        "last_7d": count(get(&live, "estimate_7d")),
        "peak": count(get(&peak, "active")),
        "average": count(get(&series, "average")),
        "image_url": format!("{asset}/activity/og.png?period=7d&v={cb}"),
    }))
}

fn status(tr: &Tr, snap: &Value) -> Ctx {
    let unk = json!("unknown");
    let overall = get_or(snap, "overall", &unk);
    let envs = or_empty(get(snap, "environments"));
    let line = |k: &str| {
        let env = or_empty(get(&envs, k));
        let st = get_or(&env, "status", &unk);
        format!("{} {}", dot(st), py_title(&s(st)))
    };
    let auth_line = match snap.get("auth") {
        None | Some(Value::Null) => tr.t("Checking…"),
        Some(a) if py_truthy(get(a, "online")) => tr.t("Reachable"),
        Some(_) => tr.t("Unreachable"),
    };
    obj(json!({
        "overall": overall,
        "overall_text": overall_text(tr, overall, &envs),
        "overall_dot": dot(overall),
        "eu_line": line("eu"),
        "us_line": line("us"),
        "pts_line": line("pts"),
        "auth_line": auth_line,
    }))
}

fn grouped(v: &Value) -> Result<String> {
    match crate::feeds::pyval::py_int(v) {
        Some(i) if v.is_i64() || v.is_u64() => Ok(thousands(i)),
        _ => bail!("ValueError: {v} is not an int"),
    }
}

async fn game_update<S: Sources>(src: &S, asset: &str) -> Result<Ctx> {
    let Some(v) = src.latest_update().await? else {
        return Ok(obj(json!({"version_tag": "new build", "files_added": "0", "files_modified": "0",
            "files_removed": "0", "ordinal": null, "image_url": null})));
    };
    let ordinal = get(&v, "ordinal").clone();
    Ok(obj(json!({
        "version_tag": get(&v, "version_tag"),
        "files_added": grouped(get(&v, "files_added"))?,
        "files_modified": grouped(get(&v, "files_modified"))?,
        "files_removed": grouped(get(&v, "files_removed"))?,
        "ordinal": ordinal,
        "image_url": format!("{asset}/announce.png?kind=game_update&v={}", s(&ordinal)),
    })))
}
