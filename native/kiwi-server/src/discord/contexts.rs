//! `app/discord/embed_contexts.py`: the customizable announcement kinds, their
//! localized default templates, sample variables (shared with Image Studio)
//! and live variables. `daily_bonuses`, `corruxion` and `fluxion` fail in
//! Python (their source names are imported from the wrong module); their live
//! context is an `Err` here too, so callers fall back the same way.

use anyhow::{Result, anyhow};
use serde_json::{Map, Value, json};

use super::{
    commands::Domain,
    embeds::{self, SITE, biomes_str, title, ts},
};
use crate::{
    embed_templates::{Ctx, EmbedField, EmbedTemplate},
    feeds::{pyval, store::News},
    i18n::Lang,
    images::contexts as studio,
};

/// `KIND_BY_KEY`: announcement key -> context kind.
pub fn kind_by_key(key: &str) -> Option<&'static str> {
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

/// `_kind`: a bare kind or an announcement key.
pub fn kind(key: &str) -> Option<&'static str> {
    studio::kind(key)
}

pub fn has_customization(key: &str) -> bool {
    kind_by_key(key).is_some()
}

pub fn has_image(key: &str) -> bool {
    matches!(kind(key), Some("trove_news" | "activity" | "game_update"))
}

pub fn sample_context(key: &str) -> Ctx {
    studio::sample(key)
}

pub fn variables(key: &str) -> Vec<String> {
    sample_context(key).keys().filter(|k| *k != "image_url").cloned().collect()
}

fn tmpl(
    title: String,
    url: Option<String>,
    color: &str,
    description: Option<String>,
    fields: Vec<EmbedField>,
    footer: String,
    show_image: bool,
) -> EmbedTemplate {
    EmbedTemplate {
        title: Some(title),
        url,
        color: Some(color.into()),
        description,
        fields,
        footer: Some(footer),
        show_image,
        ..Default::default()
    }
}

fn f(name: String, value: &str, inline: bool) -> EmbedField {
    EmbedField { name, value: value.into(), inline }
}

/// `default_template(key)` in `l`.
pub fn default_template(l: Lang, key: &str) -> Option<EmbedTemplate> {
    let t = |s: &str| l.t(s);
    Some(match kind(key)? {
        "challenge" => tmpl(
            t("⚔️ Hourly challenge"),
            None,
            "#FF9800",
            Some(t("Trove's hourly challenge - 20 minutes at the top of each hour.")),
            vec![f(t("Current"), "{current_line}", false)],
            t("Times shown in your local zone"),
            false,
        ),
        "chaos" => tmpl(
            t("🎁 Chaos Chest"),
            None,
            "#B794F6",
            Some("{desc_line}".into()),
            vec![
                f(t("Rotates"), "{ends_at:R}\n{ends_at:f}", true),
                f(t("This week"), "{starts_at:d} → {ends_at:d}", true),
            ],
            t("Trove Chaos Chest · rotates weekly (Tue 11:00 UTC)"),
            false,
        ),
        "daily_bonuses" => tmpl(
            t("✨ Trove bonuses"),
            None,
            "#FFB86B",
            None,
            vec![
                f(
                    format!(
                        "{{daily_emoji}} {}",
                        l.f("Daily — {name} ({weekday})", &[("name", "{daily_name}"), ("weekday", "{daily_weekday}")])
                    ),
                    "{daily_buffs}",
                    false,
                ),
                f(
                    format!("{{weekly_emoji}} {}", l.f("Weekly — {name}", &[("name", "{weekly_name}")])),
                    "{weekly_buffs}",
                    false,
                ),
            ],
            t("Daily resets 11:00 UTC · weekly Monday"),
            false,
        ),
        "longshade" => tmpl(
            t("🌑 Depth 15 biomes"),
            None,
            "#9B8AFB",
            Some(t("Current Depth-15 adventure biome rotation (long shade).")),
            vec![
                f(t("Current biomes"), "{current_biomes}", true),
                f(t("Rotates"), "{rotates_at:R}", true),
                f(t("Up next"), "{next_biomes}", false),
            ],
            t("Rotates every 3 hours · Trove server time (UTC−11)"),
            false,
        ),
        "wild_mana" => tmpl(
            t("🌿 Wild Mana"),
            None,
            "#46D39A",
            Some(t("Weekly Wild Mana biome rotation.")),
            vec![
                f(t("This week's biomes"), "{current_biomes}", true),
                f(t("Rotates"), "{rotates_at:R}", true),
                f(t("Next week"), "{next_biomes}", false),
            ],
            t("Rotates weekly · Trove server time (UTC−11)"),
            false,
        ),
        "stampy" => tmpl(
            t("🐘 Stampy"),
            None,
            "#FFB86B",
            Some("{state_line}".into()),
            vec![f(t("Biome"), "{biome}", true), f(t("Window"), "{starts_at:d} → {ends_at:d}", true)],
            t("Stampy event · 48-hour window"),
            false,
        ),
        "corruxion" => tmpl(
            t("🐲 Corruxion"),
            None,
            "#9B5DE5",
            Some("{desc_line}".into()),
            vec![f(t("Upcoming visits"), "{upcoming}", false)],
            t("Corruxion merchant · 14-day cycle, 3-day visits"),
            false,
        ),
        "fluxion" => tmpl(
            t("💧 Fluxion"),
            None,
            "#4CC9F0",
            Some("{desc_line}".into()),
            vec![f(t("Upcoming stages"), "{upcoming}", false)],
            t("Fluxion merchant · alternates voting / selling"),
            false,
        ),
        "trove_news" => tmpl(
            "📰 {title}".into(),
            Some("{url}".into()),
            "#5EC6FF",
            Some("{summary}".into()),
            vec![f(t("More"), "{more}", false)],
            t("Latest from trovegame.com"),
            true,
        ),
        "giveaways" => tmpl(
            t("🎉 Giveaways"),
            Some(format!("{SITE}/giveaways")),
            "#FF5C80",
            None,
            vec![
                f(t("🟢 Open now"), "{open_lines}", false),
                f(t("🔜 Upcoming"), "{upcoming_lines}", false),
                f(t("🏁 Ended · last 7 days"), "{ended_lines}", false),
            ],
            "trove.aallyn.net/giveaways".into(),
            false,
        ),
        "activity" => tmpl(
            l.f("Player activity — {label}", &[("label", "{label}")]),
            Some(format!("{SITE}/activity")),
            "#4CC9F0",
            Some(t("Active players per day and per week, estimated from the leaderboard captures.")),
            vec![
                f(t("Last 24h"), "{last_24h}", true),
                f(t("Last 7d"), "{last_7d}", true),
                f(t("Peak"), "{peak}", true),
                f(t("Average"), "{average}", true),
            ],
            "trove.aallyn.net/activity · Trove server time (UTC−11)".into(),
            true,
        ),
        "server_status" => tmpl(
            t("Trove server status"),
            Some(format!("{SITE}/status")),
            "#4CC9F0",
            Some("{overall_dot} **{overall_text}**".into()),
            vec![
                f("EU".into(), "{eu_line}", true),
                f("US".into(), "{us_line}", true),
                f("PTS".into(), "{pts_line}", true),
                f(t("Account auth"), "{auth_line}", false),
            ],
            "trove.aallyn.net/status".into(),
            false,
        ),
        _ => tmpl(
            l.f("🧩 New Trove update — {version}", &[("version", "{version_tag}")]),
            Some(format!("{SITE}/updates")),
            "#5EC6FF",
            Some(t("A new build is live on the US servers. See what changed:")),
            vec![
                f(t("Added"), "{files_added}", true),
                f(t("Modified"), "{files_modified}", true),
                f(t("Removed"), "{files_removed}", true),
            ],
            t("Browse the changed files at trove.aallyn.net/updates"),
            true,
        ),
    })
}

// --- live contexts (pure over the fetched data) --------------------------------------------

fn obj(v: Value) -> Ctx {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

fn get<'a>(v: &'a Value, k: &str) -> &'a Value {
    v.get(k).unwrap_or(&Value::Null)
}

fn or_empty(v: &Value) -> Value {
    if pyval::truthy(v) { v.clone() } else { json!({}) }
}

pub fn ctx_challenge(l: Lang, c: &Value) -> Result<Ctx> {
    let name = get(c, "name");
    let ctype = if pyval::truthy(get(c, "type")) { pyval::py_str(get(c, "type")) } else { String::new() };
    let active = pyval::truthy(get(c, "active"));
    let line = if pyval::truthy(name) {
        let mut line = format!("**{}**", pyval::py_str(name));
        if !ctype.is_empty() {
            line.push_str(&format!(" · {}", title(&ctype)));
        }
        line.push('\n');
        line.push_str(&if active {
            l.f("Ends {when}", &[("when", &ts(get(c, "ends_at"), "R")?)])
        } else {
            l.f("Next window {when}", &[("when", &ts(get(c, "starts_at"), "R")?)])
        });
        line
    } else {
        l.t("No challenge captured for this window.")
    };
    Ok(obj(json!({
        "name": if pyval::truthy(name) { name.clone() } else { json!("") },
        "type": ctype,
        "type_label": title(&ctype),
        "starts_at": get(c, "starts_at"),
        "ends_at": get(c, "ends_at"),
        "status": if active { "live" } else { "upcoming" },
        "current_line": line,
    })))
}

pub fn ctx_chaos(l: Lang, c: &Value) -> Ctx {
    let name = get(&or_empty(get(c, "item")), "name").clone();
    obj(json!({
        "item_name": if pyval::truthy(&name) { name } else { json!("") },
        "desc_line": embeds::chaos_desc(l, c),
        "starts_at": get(c, "starts_at"),
        "ends_at": get(c, "ends_at"),
    }))
}

pub fn ctx_biome_rotation(r: &Value) -> Ctx {
    let cur = or_empty(get(r, "current"));
    let first = match get(r, "upcoming") {
        Value::Array(a) if !a.is_empty() => a[0].clone(),
        _ => Value::Null,
    };
    let nxt = or_empty(&first);
    obj(json!({
        "current_biomes": biomes_str(&cur),
        "rotates_at": get(&cur, "ends_at"),
        "next_biomes": if pyval::truthy(&nxt) { biomes_str(&nxt) } else { "—".into() },
        "next_at": get(&nxt, "starts_at"),
    }))
}

pub fn ctx_stampy(l: Lang, st: &Value, now: i64) -> Result<Ctx> {
    let cur = or_empty(get(st, "current"));
    let biome = match get(&cur, "biomes") {
        Value::Array(a) if !a.is_empty() => a[0].clone(),
        _ => json!({}),
    };
    let has = pyval::truthy(&cur);
    let n = |k: &str| get(&cur, k).as_i64().unwrap_or(0);
    let state = if has && n("starts_at") <= now && now < n("ends_at") {
        l.f("**Active now** — ends {when}", &[("when", &ts(get(&cur, "ends_at"), "R")?)])
    } else if has {
        l.f("Starts {when}", &[("when", &ts(get(&cur, "starts_at"), "R")?)])
    } else {
        l.t("No upcoming Stampy event scheduled.")
    };
    let name = [get(&biome, "final_name"), get(&biome, "name")]
        .into_iter()
        .find(|v| pyval::truthy(v))
        .cloned()
        .unwrap_or_else(|| json!("?"));
    Ok(obj(json!({
        "biome": name,
        "starts_at": get(&cur, "starts_at"),
        "ends_at": get(&cur, "ends_at"),
        "state_line": state,
    })))
}

pub fn ctx_news(items: &[News]) -> Ctx {
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

fn unix_of(v: &Value) -> Result<i64> {
    let s = v.as_str().ok_or_else(|| anyhow!("not a datetime"))?;
    Ok(crate::dt::parse_utc(s).ok_or_else(|| anyhow!("bad datetime"))?.timestamp())
}

pub fn ctx_giveaways(l: Lang, ongoing: &[Value], upcoming: &[Value], ended: &[Value]) -> Result<Ctx> {
    let mut open = Vec::new();
    for g in ongoing.iter().take(5) {
        open.push(format!(
            "**{}** — {}\n{}",
            pyval::py_str(get(g, "title")),
            pyval::py_str(get(g, "prize_name")),
            l.f(
                "Ends {when} · {n} entered",
                &[
                    ("when", &format!("<t:{}:R>", unix_of(get(g, "ends_at"))?)),
                    ("n", &pyval::py_str(get(g, "entry_count"))),
                ]
            )
        ));
    }
    let short = |rows: &[Value]| -> Vec<String> {
        rows.iter()
            .take(5)
            .map(|g| format!("**{}** — {}", pyval::py_str(get(g, "title")), pyval::py_str(get(g, "prize_name"))))
            .collect()
    };
    let or = |v: Vec<String>, sep: &str, empty: String| if v.is_empty() { empty } else { v.join(sep) };
    Ok(obj(json!({
        "open_count": ongoing.len(),
        "upcoming_count": upcoming.len(),
        "ended_count": ended.len(),
        "open_lines": or(open, "\n\n", l.t("None right now.")),
        "upcoming_lines": or(short(upcoming), "\n", l.t("Nothing scheduled.")),
        "ended_lines": or(short(ended), "\n", l.t("None in the last 7 days.")),
    })))
}

fn count(v: &Value) -> Result<String> {
    match v {
        Value::Bool(b) => Ok(u8::from(*b).to_string()),
        Value::Number(n) => {
            let r = match n.as_i64() {
                Some(i) => i128::from(i),
                None => {
                    let x = n.as_f64().unwrap_or(f64::NAN);
                    if !x.is_finite() {
                        return Err(anyhow!("round() of {x}"));
                    }
                    x.round_ties_even() as i128
                }
            };
            Ok(embeds::thousands(r))
        }
        _ => Ok("—".into()),
    }
}

pub fn ctx_activity(asset: &str, live: &Value, series: &Value) -> Result<Ctx> {
    let peak = or_empty(get(series, "peak"));
    let cb = [get(live, "computed_at"), get(series, "window_end")]
        .into_iter()
        .find(|v| pyval::truthy(v))
        .cloned()
        .unwrap_or(json!(0));
    Ok(obj(json!({
        "label": "Last 7 days",
        "last_24h": count(get(live, "estimate_24h"))?,
        "last_7d": count(get(live, "estimate_7d"))?,
        "peak": count(get(&peak, "active"))?,
        "average": count(get(series, "average"))?,
        "image_url": format!("{asset}/activity/og.png?period=7d&v={}", pyval::py_str(&cb)),
    })))
}

pub fn ctx_status(l: Lang, snap: &Value) -> Ctx {
    let overall = match snap.get("overall") {
        None => "unknown".to_owned(),
        Some(v) => pyval::py_str(v),
    };
    let envs = or_empty(get(snap, "environments"));
    let line = |k: &str| {
        let env = or_empty(get(&envs, k));
        let st = match env.get("status") {
            None => "unknown".to_owned(),
            Some(v) => pyval::py_str(v),
        };
        format!("{} {}", embeds::dot(&st), title(&st))
    };
    let auth_line = match snap.get("auth") {
        None | Some(Value::Null) => l.t("Checking…"),
        Some(a) if pyval::truthy(get(a, "online")) => l.t("Reachable"),
        Some(_) => l.t("Unreachable"),
    };
    obj(json!({
        "overall": overall,
        "overall_text": embeds::overall_text(l, &overall, &envs),
        "overall_dot": embeds::dot(&overall),
        "eu_line": line("eu"),
        "us_line": line("us"),
        "pts_line": line("pts"),
        "auth_line": auth_line,
    }))
}

pub fn ctx_game_update(asset: &str, v: Option<&Value>) -> Ctx {
    let Some(v) = v else {
        return obj(json!({"version_tag": "new build", "files_added": "0", "files_modified": "0",
            "files_removed": "0", "ordinal": null, "image_url": null}));
    };
    let th = |k: &str| match get(v, k) {
        Value::Number(n) if n.as_i64().is_some() => embeds::thousands(i128::from(n.as_i64().unwrap_or(0))),
        other => pyval::py_str(other),
    };
    let ordinal = get(v, "ordinal").clone();
    obj(json!({
        "version_tag": get(v, "version_tag"),
        "files_added": th("files_added"),
        "files_modified": th("files_modified"),
        "files_removed": th("files_removed"),
        "ordinal": ordinal,
        "image_url": format!("{asset}/announce.png?kind=game_update&v={}", pyval::py_str(&ordinal)),
    }))
}

/// The `announcement-meta` body (`{"meta": [...]}`), in English like the route.
pub fn announcement_meta(keys: &[&str]) -> Value {
    let meta: Vec<Value> = keys
        .iter()
        .filter(|k| has_customization(k))
        .map(|k| {
            json!({
                "key": k,
                "variables": variables(k),
                "default_template": default_template(Lang::default(), k).map_or(Value::Null, |t| t.to_value()),
                "sample": sample_context(k),
                "has_image": has_image(k),
            })
        })
        .collect();
    json!({"meta": meta})
}

/// `context(key)`: the live variables in `l`, `Err` where Python raises.
pub async fn context<D: Domain>(d: &D, l: Lang, key: &str) -> Result<Ctx> {
    let now = d.now_us().div_euclid(1_000_000);
    match kind(key) {
        None => Ok(Map::new()),
        Some("challenge") => ctx_challenge(l, &d.challenge().await?),
        Some("chaos") => Ok(ctx_chaos(l, &d.chaos().await?)),
        Some("longshade") => Ok(ctx_biome_rotation(&d.biome_rotation()?)),
        Some("wild_mana") => Ok(ctx_biome_rotation(&d.wild_mana()?)),
        Some("stampy") => ctx_stampy(l, &d.stampy()?, now),
        Some("trove_news") => Ok(ctx_news(&d.news(5).await)),
        Some("giveaways") => {
            let (o, u, e) = d.giveaways().await?;
            ctx_giveaways(l, &o, &u, &e)
        }
        Some("activity") => {
            let (live, series) = d.activity("7d").await?;
            ctx_activity(&d.asset_url(), &live, &series)
        }
        Some("server_status") => Ok(ctx_status(l, &d.status().await)),
        Some("game_update") => Ok(ctx_game_update(&d.asset_url(), d.latest_update().await?.as_ref())),
        Some(k) => Err(anyhow!("ImportError: {k}")),
    }
}
