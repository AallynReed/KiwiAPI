//! The embed builders of `app/discord/embeds.py` that the gateway bot posts,
//! as pure functions over the service dicts they read.

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};

use super::{
    i18n::Tr,
    pyfmt::{count as _count, flux, num, or_else, round_num, s, thousands, ts},
};
use crate::{dt, features::py_truthy, feeds::store::News, og::board::py_title};

pub const SITE: &str = "https://trove.aallyn.net";

pub type Obj = Map<String, Value>;

static NULL: Value = Value::Null;

/// `d.get(k)`.
pub fn get<'a>(v: &'a Value, k: &str) -> &'a Value {
    v.get(k).unwrap_or(&NULL)
}

/// `d.get(k, default)`.
pub fn get_or<'a>(v: &'a Value, k: &str, default: &'a Value) -> &'a Value {
    v.get(k).unwrap_or(default)
}

/// `d[k]`.
pub fn req<'a>(v: &'a Value, k: &str) -> Result<&'a Value> {
    v.get(k).with_context(|| format!("KeyError: {k}"))
}

/// `x or {}`.
pub fn or_empty(v: &Value) -> Value {
    if py_truthy(v) { v.clone() } else { Value::Object(Map::new()) }
}

fn items(v: &Value) -> &[Value] {
    match v {
        Value::Array(a) => a,
        _ => &[],
    }
}

pub fn field(name: impl Into<Value>, value: impl Into<Value>, inline: bool) -> Value {
    json!({"name": name.into(), "value": value.into(), "inline": inline})
}

fn obj(v: Value) -> Obj {
    match v {
        Value::Object(m) => m,
        _ => Map::new(),
    }
}

/// `int(dt.timestamp())` of a pydantic datetime string.
pub fn unix(v: &Value) -> Result<i64> {
    let s = v.as_str().context("not a datetime")?;
    Ok(dt::parse_utc(s).context("bad datetime")?.timestamp())
}

// --- status -------------------------------------------------------------------------

pub fn status_color(overall: &Value) -> Option<i64> {
    Some(match overall.as_str()? {
        "online" => 0x46D39A,
        "maintenance" | "down" => 0xF0556A,
        "unknown" => 0x8A93A3,
        _ => return None,
    })
}

pub fn dot(status: &Value) -> &'static str {
    match status.as_str() {
        Some("online") => "\u{1F7E2}",
        Some("maintenance" | "down") => "\u{1F534}",
        _ => "⚪",
    }
}

fn overall_label(overall: &Value) -> &'static str {
    match overall.as_str() {
        Some("online") => "All systems operational",
        Some("maintenance" | "down") => "Major outage",
        _ => "Status unknown",
    }
}

fn env_label(status: &Value) -> &'static str {
    match status.as_str() {
        Some("online") => "Online",
        Some("maintenance" | "down") => "Down",
        _ => "Unknown",
    }
}

pub const REGIONS: [(&str, &str); 3] =
    [("eu", "\u{1F1EA}\u{1F1FA} EU Live"), ("us", "\u{1F1FA}\u{1F1F8} US Live"), ("pts", "\u{1F9EA} PTS")];

fn unknown() -> Value {
    Value::String("unknown".into())
}

/// The overall line: "Partial outage" while a Live region is still up.
pub fn overall_text(tr: &Tr, overall: &Value, envs: &Value) -> String {
    let live_up = ["eu", "us"].iter().any(|k| get(&or_empty(get(envs, k)), "status").as_str() == Some("online"));
    if overall.as_str() == Some("down") && live_up { tr.t("Partial outage") } else { tr.t(overall_label(overall)) }
}

pub fn status_embed(tr: &Tr, snap: &Value) -> Result<Obj> {
    let unk = unknown();
    let overall = get_or(snap, "overall", &unk);
    let envs = or_empty(get(snap, "environments"));
    let mut fields = Vec::new();
    for (key, label) in REGIONS {
        let env = or_empty(get(&envs, key));
        let st = get_or(&env, "status", &unk);
        let mut value = format!("{} {}", dot(st), tr.t(env_label(st)));
        let game = or_empty(get(&env, "game"));
        let latency = get(&game, "latency_ms");
        if st.as_str() == Some("online")
            && let Some(ms) = round_num(latency)
        {
            value.push_str(&format!(" · {ms} ms"));
        }
        fields.push(field(label, value, true));
    }
    let auth_value = match snap.get("auth") {
        None | Some(Value::Null) => format!("{} {}", dot(&unk), tr.t("Checking…")),
        Some(a) if py_truthy(get(a, "online")) => format!("{} {}", dot(&json!("online")), tr.t("Reachable")),
        Some(_) => format!("{} {}", dot(&json!("down")), tr.t("Unreachable")),
    };
    fields.push(field(tr.t("Account auth"), auth_value, false));
    let mut embed = obj(json!({
        "title": tr.t("Trove server status"),
        "url": format!("{SITE}/status"),
        "color": status_color(overall).unwrap_or(0x8A93A3),
        "description": format!("{}  **{}**", dot(overall), overall_text(tr, overall, &envs)),
        "fields": fields,
        "footer": {"text": "trove.aallyn.net/status"},
    }));
    let checked = get(snap, "checked_at");
    if py_truthy(checked) {
        let secs = checked.as_f64().context("checked_at")?;
        let at = dt::from_timestamp(secs).context("checked_at")?;
        embed.insert("timestamp".into(), dt::isoformat(&at).into());
    }
    Ok(embed)
}

// --- activity -----------------------------------------------------------------------

pub fn activity_embed(tr: &Tr, live: &Value, series: &Value, asset: &str) -> Result<Obj> {
    let period = "7d";
    let label = tr.t("Last 7 days");
    let mut fields = vec![
        field(tr.t("Last 24h"), num(get(live, "estimate_24h")), true),
        field(tr.t("Last 7d"), num(get(live, "estimate_7d")), true),
    ];
    let peak = or_empty(get(series, "peak"));
    if let Some(active) = round_num(get(&peak, "active")) {
        let when =
            if py_truthy(get(&peak, "t")) { format!(" ({})", ts(req(&peak, "t")?, "d")?) } else { String::new() };
        fields.push(field(tr.tf("Peak · {label}", &[("label", &label)]), format!("{}{when}", thousands(active)), true));
    }
    if let Some(avg) = round_num(get(series, "average")) {
        fields.push(field(tr.t("Average"), thousands(avg), true));
    }
    let cb = activity_cache_buster(live, series);
    Ok(obj(json!({
        "title": tr.tf("Player activity — {label}", &[("label", &label)]),
        "url": format!("{SITE}/activity?period={period}"),
        "color": 0x4CC9F0,
        "description": tr.t("Active players per day and per week, estimated from the leaderboard captures."),
        "fields": fields,
        "image": {"url": format!("{asset}/activity/og.png?period={period}&v={cb}")},
        "footer": {"text": "trove.aallyn.net/activity · Trove server time (UTC−11)"},
    })))
}

/// `live.get("computed_at") or series.get("window_end") or 0`.
pub fn activity_cache_buster(live: &Value, series: &Value) -> String {
    let zero = json!(0);
    s(or_else(get(live, "computed_at"), or_else(get(series, "window_end"), &zero)))
}

// --- rotations ----------------------------------------------------------------------

pub fn chaos_embed(tr: &Tr, c: &Value) -> Result<Obj> {
    let item = or_empty(get(c, "item"));
    let name = get(&item, "name");
    let desc = if py_truthy(name) {
        tr.tf("This week's featured item: **{name}**", &[("name", &s(name))])
    } else {
        tr.t("No item has been relayed for this week's chest yet.")
    };
    let (start, end) = (req(c, "starts_at")?, req(c, "ends_at")?);
    Ok(obj(json!({
        "title": tr.t("🎁 Chaos Chest"),
        "color": 0xB794F6,
        "description": desc,
        "fields": [
            field(tr.t("Rotates"), format!("{}\n{}", ts(end, "R")?, ts(end, "f")?), true),
            field(tr.t("This week"), format!("{} → {}", ts(start, "d")?, ts(end, "d")?), true),
        ],
        "footer": {"text": tr.t("Trove Chaos Chest · rotates weekly (Tue 11:00 UTC)")},
    })))
}

fn bullets(v: &Value) -> String {
    let out: Vec<String> = items(v).iter().map(|b| format!("• {}", s(b))).collect();
    if out.is_empty() { "—".into() } else { out.join("\n") }
}

/// `bonuses_embed` over `daily_buffs()["current"]` and `weekly_buffs()["current"]`.
pub fn bonuses_embed(tr: &Tr, daily: &Value, weekly: &Value) -> Result<Obj> {
    let d = or_empty(daily);
    let w = or_empty(weekly);
    let mut daily_val = bullets(get(&d, "normal_buffs"));
    let premium = get(&d, "premium_buffs");
    if py_truthy(premium) {
        let joined: Vec<String> = items(premium).iter().map(s).collect();
        daily_val.push('\n');
        daily_val.push_str(&tr.tf("*Patron:* {buffs}", &[("buffs", &joined.join(", "))]));
    }
    let mut color: i128 = 0xFFB86B;
    if let Some(Value::String(c)) = d.get("color")
        && !c.is_empty()
        && let Some(v) = kiwi_core::feeds::py::int(c, 16)
    {
        color = v;
    }
    let dflt = |k: &str, def: &str| d.get(k).map_or_else(|| def.to_owned(), s);
    let wdflt = |k: &str, def: &str| w.get(k).map_or_else(|| def.to_owned(), s);
    let mut embed = obj(json!({
        "title": tr.t("✨ Trove bonuses"),
        "color": i64::try_from(color).unwrap_or(0xFFB86B),
        "fields": [
            field(
                tr.tf("{emoji} Daily — {name} ({weekday})", &[
                    ("emoji", &dflt("emoji", "📅")),
                    ("name", &dflt("name", "?")),
                    ("weekday", &dflt("weekday", "")),
                ]),
                daily_val,
                false,
            ),
            field(
                tr.tf("{emoji} Weekly — {name}", &[("emoji", &wdflt("emoji", "🗓️")), ("name", &wdflt("name", "?"))]),
                bullets(get(&w, "buffs")),
                false,
            ),
        ],
        "footer": {"text": tr.t("Daily resets 11:00 UTC · weekly Monday")},
    }));
    let icon = get(&d, "icon");
    if py_truthy(icon) {
        embed.insert("thumbnail".into(), json!({"url": icon}));
    }
    Ok(embed)
}

pub fn challenge_embed(tr: &Tr, cur: &Value) -> Result<Obj> {
    let cur_val = if py_truthy(get(cur, "name")) {
        let mut v = format!("**{}**", s(req(cur, "name")?));
        if py_truthy(get(cur, "type")) {
            v.push_str(&format!(" · {}", py_title(&s(req(cur, "type")?))));
        }
        v.push('\n');
        if py_truthy(get(cur, "active")) {
            v.push_str(&tr.tf("Ends {when}", &[("when", &ts(req(cur, "ends_at")?, "R")?)]));
        } else {
            v.push_str(&tr.tf("Next window {when}", &[("when", &ts(req(cur, "starts_at")?, "R")?)]));
        }
        v
    } else {
        format!(
            "{}\n{}",
            tr.t("No challenge captured for this window."),
            tr.tf(
                "Window {start} → {end}",
                &[("start", &ts(req(cur, "starts_at")?, "t")?), ("end", &ts(req(cur, "ends_at")?, "t")?),]
            )
        )
    };
    Ok(obj(json!({
        "title": tr.t("⚔️ Hourly challenge"),
        "color": 0xFF9800,
        "description": tr.t("Trove's hourly challenge - 20 minutes at the top of each hour."),
        "fields": [field(tr.t("Current"), cur_val, false)],
        "footer": {"text": tr.t("Times shown in your local zone")},
    })))
}

/// `_biomes_str`.
pub fn biomes_str(rot: &Value) -> String {
    let lines: Vec<String> = items(get(rot, "biomes"))
        .iter()
        .map(|b| format!("• {}", s(or_else(get(b, "final_name"), get(b, "name")))))
        .collect();
    if lines.is_empty() { "—".into() } else { lines.join("\n") }
}

fn first(v: &Value) -> Option<&Value> {
    items(v).first()
}

fn biome_feed(tr: &Tr, r: &Value, labels: (&str, &str, &str, &str, &str), next_style: &str, color: i64) -> Result<Obj> {
    let (cur_label, next_label, title, description, footer) = labels;
    let cur = or_empty(get(r, "current"));
    let zero = json!(0);
    let mut fields = vec![
        field(tr.t(cur_label), biomes_str(&cur), true),
        field(tr.t("Rotates"), ts(get_or(&cur, "ends_at", &zero), "R")?, true),
    ];
    if let Some(nxt) = first(get(r, "upcoming")).filter(|n| py_truthy(n)) {
        fields.push(field(
            tr.tf(next_label, &[("when", &ts(req(nxt, "starts_at")?, next_style)?)]),
            biomes_str(nxt),
            false,
        ));
    }
    Ok(obj(json!({
        "title": tr.t(title),
        "color": color,
        "description": tr.t(description),
        "fields": fields,
        "footer": {"text": tr.t(footer)},
    })))
}

pub fn longshade_embed(tr: &Tr, r: &Value) -> Result<Obj> {
    biome_feed(
        tr,
        r,
        (
            "Current biomes",
            "Up next ({when})",
            "🌑 Depth 15 biomes",
            "Current Depth-15 adventure biome rotation (long shade).",
            "Rotates every 3 hours · Trove server time (UTC−11)",
        ),
        "t",
        0x9B8AFB,
    )
}

pub fn wild_mana_embed(tr: &Tr, r: &Value) -> Result<Obj> {
    biome_feed(
        tr,
        r,
        (
            "This week's biomes",
            "Next week ({when})",
            "🌿 Wild Mana",
            "Weekly Wild Mana biome rotation.",
            "Rotates weekly · Trove server time (UTC−11)",
        ),
        "d",
        0x46D39A,
    )
}

/// Python comparison of two JSON numbers.
fn le(a: &Value, b: &Value) -> Result<bool> {
    Ok(a.as_f64().context("number")? <= b.as_f64().context("number")?)
}

pub fn stampy_embed(tr: &Tr, st: &Value, now: i64) -> Result<Obj> {
    let cur = get(st, "current");
    if !py_truthy(cur) {
        return Ok(obj(json!({
            "title": tr.t("🐘 Stampy"),
            "color": 0xFFB86B,
            "description": tr.t("No upcoming Stampy event scheduled."),
            "footer": {"text": tr.t("Stampy event · weekly, 48-hour window")},
        })));
    }
    let biome = first(get(cur, "biomes")).cloned().unwrap_or(json!({}));
    let q = json!("?");
    let bname = or_else(or_else(get(&biome, "final_name"), get(&biome, "name")), &q).clone();
    let (start, end) = (req(cur, "starts_at")?, req(cur, "ends_at")?);
    let nowv = json!(now);
    let state = if le(start, &nowv)? && !le(end, &nowv)? {
        tr.tf("**Active now** — ends {when}", &[("when", &ts(end, "R")?)])
    } else {
        format!("{} · {}", tr.tf("Starts {when}", &[("when", &ts(start, "R")?)]), ts(start, "f")?)
    };
    Ok(obj(json!({
        "title": tr.t("🐘 Stampy"),
        "color": 0xFFB86B,
        "description": state,
        "fields": [
            field(tr.t("Biome"), bname, true),
            field(tr.t("Window"), format!("{} → {}", ts(start, "d")?, ts(end, "d")?), true),
        ],
        "footer": {"text": tr.t("Stampy event · 48-hour window")},
    })))
}

fn schedule_tail(m: &Value) -> Vec<Value> {
    items(or_else(get(m, "schedule"), &json!([]))).iter().skip(1).take(4).cloned().collect()
}

pub fn corruxion_embed(tr: &Tr, c: &Value) -> Result<Obj> {
    let (start, end) = (req(c, "starts_at")?, req(c, "ends_at")?);
    let desc = if py_truthy(req(c, "active")?) {
        format!(
            "{}\n{} · {}",
            tr.t("**Corruxion is here!**"),
            tr.tf("Leaves {when}", &[("when", &ts(end, "R")?)]),
            ts(end, "f")?
        )
    } else {
        format!(
            "{}\n{} · {}\n{}",
            tr.t("Corruxion is away."),
            tr.tf("Arrives {when}", &[("when", &ts(start, "R")?)]),
            ts(start, "f")?,
            tr.tf("Stays until {when}", &[("when", &ts(end, "f")?)])
        )
    };
    let mut fields = Vec::new();
    let upcoming = schedule_tail(c);
    if !upcoming.is_empty() {
        let lines = upcoming
            .iter()
            .map(|w| Ok(format!("{} → {}", ts(req(w, "starts_at")?, "d")?, ts(req(w, "ends_at")?, "d")?)))
            .collect::<Result<Vec<_>>>()?;
        fields.push(field(tr.t("Upcoming visits"), lines.join("\n"), false));
    }
    Ok(obj(json!({
        "title": tr.t("🐲 Corruxion"),
        "color": 0x9B5DE5,
        "description": desc,
        "fields": fields,
        "footer": {"text": tr.t("Corruxion merchant · 14-day cycle, 3-day visits")},
    })))
}

pub fn fluxion_embed(tr: &Tr, f: &Value) -> Result<Obj> {
    let (start, end) = (req(f, "starts_at")?, req(f, "ends_at")?);
    let desc = if py_truthy(req(f, "active")?) {
        format!(
            "{}\n{} · {}",
            tr.tf("**Fluxion is here** — currently in the **{state}** stage.", &[("state", &s(req(f, "state")?))]),
            tr.tf("Leaves {when}", &[("when", &ts(end, "R")?)]),
            ts(end, "f")?
        )
    } else {
        let nxt = first(get(f, "schedule")).cloned().unwrap_or(json!({}));
        let q = json!("?");
        let stage = s(get_or(&nxt, "state", &q));
        format!(
            "{}\n{} · {}\n{}",
            tr.t("Fluxion is away."),
            tr.tf("Next up: the **{stage}** stage, {when}", &[("stage", &stage), ("when", &ts(start, "R")?)]),
            ts(start, "f")?,
            tr.tf("Runs until {when}", &[("when", &ts(end, "f")?)])
        )
    };
    let mut fields = Vec::new();
    let upcoming = schedule_tail(f);
    if !upcoming.is_empty() {
        let lines = upcoming
            .iter()
            .map(|w| {
                Ok(format!(
                    "{} → {} · {}",
                    ts(req(w, "starts_at")?, "d")?,
                    ts(req(w, "ends_at")?, "d")?,
                    s(req(w, "state")?)
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        fields.push(field(tr.t("Upcoming stages"), lines.join("\n"), false));
    }
    Ok(obj(json!({
        "title": tr.t("💧 Fluxion"),
        "color": 0x4CC9F0,
        "description": desc,
        "fields": fields,
        "footer": {"text": tr.t("Fluxion merchant · alternates voting / selling")},
    })))
}

/// `_luxion_rotations`: unfinished rotations, soonest first, at most seven.
pub fn luxion_rotations(schedule: &Value, now: i64) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for w in items(schedule) {
        let ends = or_else(get(w, "ends_at"), &json!(0)).as_f64().context("ends_at")?;
        if ends > now as f64 {
            let (start, end) = (req(w, "starts_at")?, req(w, "ends_at")?);
            out.push(format!("{} - {}  ·  {}", ts(start, "F")?, ts(end, "F")?, ts(start, "R")?));
        }
    }
    out.truncate(7);
    Ok(out)
}

pub fn luxion_embed(tr: &Tr, l: &Value, now: i64, asset: &str) -> Result<Obj> {
    let schedule = or_else(get(l, "schedule"), &json!([])).clone();
    let desc = if py_truthy(get(l, "active")) {
        let win = get(l, "current_window");
        let mut desc = if py_truthy(win) {
            let end = req(win, "ends_at")?;
            format!(
                "{}\n{} · {}",
                tr.t("**A rotation is open!**"),
                tr.tf("Closes {when}", &[("when", &ts(end, "R")?)]),
                ts(end, "f")?
            )
        } else {
            let mut opened = false;
            for w in items(&schedule) {
                if le(req(w, "starts_at")?, &json!(now))? {
                    opened = true;
                    break;
                }
            }
            let mut d =
                if opened { tr.t("**The Trials of Luxion are live**") } else { tr.t("**Trials of Luxion started**") };
            let nxt = get(l, "next_window");
            if py_truthy(nxt) {
                let start = req(nxt, "starts_at")?;
                let when = ts(start, "R")?;
                let line = if opened {
                    tr.tf("Next rotation {when}", &[("when", &when)])
                } else {
                    tr.tf("First rotation {when}", &[("when", &when)])
                };
                d.push_str(&format!("\n{line} · {}", ts(start, "f")?));
            }
            d
        };
        desc.push('\n');
        desc.push_str(&tr.tf("Ends {when}", &[("when", &ts(req(l, "ends_at")?, "R")?)]));
        desc
    } else {
        tr.t("The Trials of Luxion are away — they return roughly every 4 weeks (dev-set, unpredictable).")
    };
    let mut fields = Vec::new();
    let rotations = luxion_rotations(&schedule, now)?;
    if !rotations.is_empty() {
        fields.push(field(tr.t("Next rotations"), rotations.join("\n"), false));
    }
    let mut embed = obj(json!({
        "title": tr.t("🐉 Trials of Luxion"),
        "color": 0xF5C542,
        "description": desc,
        "fields": fields,
        "footer": {"text": tr.t("Trials of Luxion · a 3-hour rotation every 27 hours")},
    }));
    if py_truthy(get(l, "active")) {
        let v = s(or_else(get(l, "starts_at"), &json!(0)));
        embed.insert("image".into(), json!({"url": format!("{asset}/announce.png?kind=luxion&v={v}")}));
    }
    Ok(embed)
}

/// `luxion_caption`: the rotation times that ride along with the banner.
pub fn luxion_caption(tr: &Tr, l: &Value, now: i64) -> Result<Obj> {
    if !py_truthy(get(l, "active")) {
        return Ok(Map::new());
    }
    let win = get(l, "current_window");
    let mut desc = if py_truthy(win) {
        tr.tf("**A rotation is open** — closes {when}", &[("when", &ts(req(win, "ends_at")?, "R")?)])
    } else {
        tr.t("**Trials of Luxion started**")
    };
    let rotations = luxion_rotations(or_else(get(l, "schedule"), &json!([])), now)?;
    if !rotations.is_empty() {
        desc.push_str(&format!("\n\n{}\n{}", tr.t("Next rotations:"), rotations.join("\n")));
    }
    Ok(obj(json!({"description": desc})))
}

// --- feeds --------------------------------------------------------------------------

fn gw_section(tr: &Tr, rows: &[Value], fmt: impl Fn(&Value) -> Result<String>, empty: String) -> Result<String> {
    if rows.is_empty() {
        return Ok(empty);
    }
    let parts = rows.iter().take(5).map(fmt).collect::<Result<Vec<_>>>()?;
    let mut out = parts.join("\n\n");
    if rows.len() > 5 {
        out.push('\n');
        out.push_str(&tr.tf("…and {n} more", &[("n", &(rows.len() - 5).to_string())]));
    }
    Ok(out)
}

fn gw_head(g: &Value) -> String {
    format!("**{}** — {}", s(get(g, "title")), s(get(g, "prize_name")))
}

pub fn giveaways_embed(tr: &Tr, ongoing: &[Value], upcoming: &[Value], ended: &[Value]) -> Result<Obj> {
    let open = gw_section(
        tr,
        ongoing,
        |g| {
            let when = ts(&json!(unix(get(g, "ends_at"))?), "R")?;
            Ok(format!(
                "{}\n{}",
                gw_head(g),
                tr.tf("Ends {when} · {n} entered", &[("when", &when), ("n", &s(get(g, "entry_count")))])
            ))
        },
        tr.t("None right now."),
    )?;
    let soon = gw_section(
        tr,
        upcoming,
        |g| {
            let when = ts(&json!(unix(get(g, "starts_at"))?), "R")?;
            Ok(format!("{}\n{}", gw_head(g), tr.tf("Starts {when}", &[("when", &when)])))
        },
        tr.t("Nothing scheduled."),
    )?;
    let done = gw_section(
        tr,
        ended,
        |g| {
            let winner = get(g, "winner_username");
            let won = if py_truthy(winner) {
                format!(" · {}", tr.tf("won by {name}", &[("name", &s(winner))]))
            } else {
                String::new()
            };
            let when = ts(&json!(unix(get(g, "ends_at"))?), "R")?;
            Ok(format!("{}\n{}{won}", gw_head(g), tr.tf("Ended {when}", &[("when", &when)])))
        },
        tr.t("None in the last 7 days."),
    )?;
    Ok(obj(json!({
        "title": tr.t("🎉 Giveaways"),
        "url": format!("{SITE}/giveaways"),
        "color": 0xFF5C80,
        "fields": [
            field(tr.tf("🟢 Open now ({n})", &[("n", &ongoing.len().to_string())]), open, false),
            field(tr.tf("🔜 Upcoming ({n})", &[("n", &upcoming.len().to_string())]), soon, false),
            field(tr.tf("🏁 Ended · last 7 days ({n})", &[("n", &ended.len().to_string())]), done, false),
        ],
        "footer": {"text": "trove.aallyn.net/giveaways"},
    })))
}

pub fn trove_news_embed(tr: &Tr, news: &[News]) -> Result<Obj> {
    let Some(top) = news.first() else {
        return Ok(obj(json!({
            "title": tr.t("📰 Trove news"),
            "color": 0x5EC6FF,
            "description": tr.t("No news cached yet — check back shortly."),
        })));
    };
    let fields: Vec<Value> = news
        .iter()
        .skip(1)
        .take(4)
        .map(|n| {
            let when = n.published_at.map_or(String::new(), |p| format!(" · <t:{}:R>", p.timestamp()));
            field(kiwi_core::pystr::prefix(&n.title, 250), format!("[{}]({}){when}", tr.t("Read more"), n.url), false)
        })
        .collect();
    let mut embed = obj(json!({
        "title": kiwi_core::pystr::prefix(&format!("📰 {}", top.title), 256),
        "url": top.url,
        "color": 0x5EC6FF,
        "description": kiwi_core::pystr::prefix(&top.summary, 600),
        "fields": fields,
        "footer": {"text": tr.t("Latest from trovegame.com")},
    }));
    if let Some(img) = top.image.as_ref().filter(|i| !i.is_empty()) {
        embed.insert("image".into(), json!({"url": img}));
    }
    if let Some(p) = top.published_at {
        embed.insert("timestamp".into(), dt::isoformat(&p).into());
    }
    Ok(embed)
}

/// `game_update_embed` over the latest live-us `UpdateVersion` (as JSON).
pub fn game_update_embed(tr: &Tr, v: Option<&Value>, asset: &str) -> Result<Obj> {
    let Some(v) = v else {
        return Ok(obj(json!({
            "title": tr.t("🧩 Trove update"),
            "color": 0x5EC6FF,
            "url": format!("{SITE}/updates"),
            "description": tr.t("No update has been archived yet."),
            "footer": {"text": "trove.aallyn.net/updates"},
        })));
    };
    Ok(obj(json!({
        "title": tr.tf("🧩 New Trove update — {version}", &[("version", &s(get(v, "version_tag")))]),
        "color": 0x5EC6FF,
        "url": format!("{SITE}/updates"),
        "description": tr.t("A new build is live on the US servers. See what changed:"),
        "fields": [
            field(tr.t("Added"), num(get(v, "files_added")), true),
            field(tr.t("Modified"), num(get(v, "files_modified")), true),
            field(tr.t("Removed"), num(get(v, "files_removed")), true),
        ],
        "image": {"url": format!("{asset}/announce.png?kind=game_update&v={}", s(get(v, "ordinal")))},
        "footer": {"text": tr.t("Browse the changed files at trove.aallyn.net/updates")},
    })))
}

/// `live_board_embed`; `now_us` is the clock both `time.time()` and
/// `datetime.now()` read.
pub fn live_board_embed(tr: &Tr, overall: &Value, now_us: i64, asset: &str) -> Obj {
    let minute = now_us.div_euclid(60_000_000);
    let suffix = if tr.lang == "en" { String::new() } else { format!("&lang={}", tr.lang) };
    let now = chrono::DateTime::from_timestamp_micros(now_us).unwrap_or_default();
    obj(json!({
        "title": tr.t("🥝 Trove Now"),
        "url": SITE,
        "color": status_color(overall).unwrap_or(0x46D39A),
        "image": {"url": format!("{asset}/board.png?v={minute}{suffix}")},
        "footer": {"text": tr.t("Live · updates every minute · trove.aallyn.net")},
        "timestamp": dt::isoformat(&now),
    }))
}

/// `market_watch_embed`.
pub fn market_watch_embed(tr: &Tr, name: &str, summary: &Value, max_price_each: Option<f64>) -> Obj {
    let cheapest = get(summary, "min_each");
    let ea = tr.t("flux ea.");
    let target = match max_price_each {
        Some(m) => tr.tf("dropped to {p} flux or less", &[("p", &flux(&json!(m)))]),
        None => tr.t("is now listed"),
    };
    obj(json!({
        "title": tr.tf("🔔 Market watch — {name}", &[("name", name)]),
        "color": 0xF0A500,
        "url": format!("{SITE}/market"),
        "description": tr.tf("**{name}** {target}: cheapest **{price}** flux each.", &[
            ("name", name),
            ("target", &target),
            ("price", &flux(cheapest)),
        ]),
        "fields": [
            field(tr.t("Cheapest"), format!("{} {ea}", flux(cheapest)), true),
            field(tr.t("Listings"), num(get(summary, "count")), true),
        ],
        "footer": {"text": "trove.aallyn.net/market"},
    }))
}

/// `_count` (re-exported for the contexts).
pub fn count(v: &Value) -> String {
    _count(v)
}
