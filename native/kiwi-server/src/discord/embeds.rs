//! `app/discord/embeds.py`: the Discord embeds of the slash commands and the
//! announcements, built from the domain dicts they read. Pure: the caller
//! fetches the data; an `Err` is an exception the Python builder would raise.

use anyhow::{Result, anyhow};
use serde_json::{Map, Value, json};

use crate::{feeds::pyval, i18n::Lang};

pub const SITE: &str = "https://trove.aallyn.net";

// --- Python formatting -----------------------------------------------------------------

/// `int(v)` (`TypeError`/`ValueError` as `Err`).
pub fn int_of(v: &Value) -> Result<i128> {
    pyval::py_int(v).ok_or_else(|| anyhow!("int() of {v}"))
}

/// `discord_ts(unix, style)`.
pub fn ts(v: &Value, style: &str) -> Result<String> {
    Ok(format!("<t:{}:{style}>", int_of(v)?))
}

fn ts_i(v: i64, style: &str) -> String {
    format!("<t:{v}:{style}>")
}

/// `f"{n:,}"` of an int.
pub fn thousands(n: i128) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3 + 1);
    if n < 0 {
        out.push('-');
    }
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `round(x)` of an int or float; `Err` where Python raises.
fn round_num(v: &Value) -> Result<i128> {
    match v {
        Value::Bool(b) => Ok(i128::from(*b)),
        Value::Number(n) => match (n.as_i64(), n.as_u64()) {
            (Some(i), _) => Ok(i128::from(i)),
            (_, Some(u)) => Ok(i128::from(u)),
            _ => {
                let f = n.as_f64().unwrap_or(f64::NAN);
                if !f.is_finite() {
                    return Err(anyhow!("cannot round {f}"));
                }
                Ok(f.round_ties_even() as i128)
            }
        },
        _ => Err(anyhow!("round() of {v}")),
    }
}

fn is_num(v: Option<&Value>) -> bool {
    matches!(v, Some(Value::Number(_) | Value::Bool(_)))
}

/// `_num`: thousands for an int, `—` for anything else.
pub fn num(v: Option<&Value>) -> String {
    match v {
        Some(Value::Bool(b)) => u8::from(*b).to_string(),
        Some(Value::Number(n)) if !n.is_f64() => {
            thousands(n.as_i64().map_or_else(|| n.as_u64().unwrap_or(0) as i128, i128::from))
        }
        _ => "—".into(),
    }
}

/// `float(v)`; `None` where Python raises `TypeError`/`ValueError`.
fn py_float(v: &Value) -> Option<f64> {
    match v {
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::Number(n) => n.as_f64(),
        Value::String(s) => kiwi_core::pynum::float(s),
        _ => None,
    }
}

/// `_flux`: whole flux with thousands separators, `—` when not a number.
pub fn flux(v: Option<&Value>) -> Result<String> {
    let Some(f) = v.and_then(py_float) else { return Ok("—".into()) };
    if f.is_nan() {
        return Ok("—".into());
    }
    if f.is_infinite() {
        return Err(anyhow!("OverflowError: cannot convert float infinity to integer"));
    }
    Ok(thousands(f.round_ties_even() as i128))
}

/// `f"{x:g}"`.
pub fn fmt_g(x: f64) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if x == 0.0 {
        return if x.is_sign_negative() { "-0".into() } else { "0".into() };
    }
    let sci = format!("{x:.5e}");
    let (mant, exp) = sci.split_once('e').unwrap_or((&sci, "0"));
    let e: i32 = exp.parse().unwrap_or(0);
    let strip = |s: &str| -> String {
        if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_owned() } else { s.to_owned() }
    };
    if !(-4..6).contains(&e) {
        format!("{}e{}{:02}", strip(mant), if e < 0 { '-' } else { '+' }, e.abs())
    } else {
        strip(&format!("{x:.*}", (5 - e) as usize))
    }
}

/// `str.title()`.
pub fn title(s: &str) -> String {
    crate::og::board::py_title(s)
}

/// `s.splitlines()[0]` (`None` for an empty string: an `IndexError`).
pub fn first_line(s: &str) -> Option<&str> {
    if s.is_empty() {
        return None;
    }
    let end = s
        .char_indices()
        .find(|(_, c)| {
            matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}')
        })
        .map_or(s.len(), |(i, _)| i);
    Some(&s[..end])
}

fn prefix(s: &str, n: usize) -> String {
    kiwi_core::pystr::prefix(s, n).to_owned()
}

fn get<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    v.get(k)
}

/// `d[k]`.
fn req<'a>(v: &'a Value, k: &str) -> Result<&'a Value> {
    v.get(k).ok_or_else(|| anyhow!("KeyError: {k}"))
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(pyval::truthy)
}

/// `d.get(k) or {}`.
fn dict_or_empty(v: Option<&Value>) -> Value {
    match v {
        Some(x) if pyval::truthy(x) => x.clone(),
        _ => Value::Object(Map::new()),
    }
}

fn s(v: &Value) -> String {
    pyval::py_str(v)
}

fn list(v: Option<&Value>) -> &[Value] {
    match v {
        Some(Value::Array(a)) => a,
        _ => &[],
    }
}

fn field(name: impl Into<Value>, value: impl Into<Value>, inline: bool) -> Value {
    json!({"name": name.into(), "value": value.into(), "inline": inline})
}

fn footer(text: impl Into<Value>) -> Value {
    json!({"text": text.into()})
}

fn obj(pairs: Vec<(&str, Value)>) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert(k.to_owned(), v);
    }
    Value::Object(m)
}

fn push(v: &mut Value, k: &str, x: Value) {
    if let Value::Object(m) = v {
        m.insert(k.to_owned(), x);
    }
}

// --- /status -----------------------------------------------------------------------------

pub fn status_color(st: &str) -> i64 {
    match st {
        "online" => 0x46D39A,
        "maintenance" | "down" => 0xF0556A,
        _ => 0x8A93A3,
    }
}

pub fn dot(st: &str) -> &'static str {
    match st {
        "online" => "\u{1F7E2}",
        "maintenance" | "down" => "\u{1F534}",
        _ => "⚪",
    }
}

pub fn overall_label(st: &str) -> &'static str {
    match st {
        "online" => "All systems operational",
        "maintenance" | "down" => "Major outage",
        _ => "Status unknown",
    }
}

fn env_label(st: &str) -> &'static str {
    match st {
        "online" => "Online",
        "maintenance" | "down" => "Down",
        _ => "Unknown",
    }
}

pub const REGIONS: [(&str, &str); 3] = [("eu", "🇪🇺 EU Live"), ("us", "🇺🇸 US Live"), ("pts", "🧪 PTS")];

fn status_str(v: Option<&Value>, default: &'static str) -> String {
    match v {
        None => default.into(),
        Some(Value::String(s)) => s.clone(),
        Some(_) => String::new(),
    }
}

/// `overall_text` with the "Partial outage" rule.
pub fn overall_text(l: Lang, overall: &str, envs: &Value) -> String {
    let live_up = ["eu", "us"]
        .iter()
        .any(|k| dict_or_empty(get(envs, k)).get("status").and_then(Value::as_str) == Some("online"));
    if overall == "down" && live_up { l.t("Partial outage") } else { l.t(overall_label(overall)) }
}

pub fn status_embed(l: Lang, snap: &Value) -> Result<Value> {
    let overall = status_str(get(snap, "overall"), "unknown");
    let envs = dict_or_empty(get(snap, "environments"));
    let mut fields = Vec::new();
    for (k, label) in REGIONS {
        let env = dict_or_empty(get(&envs, k));
        let st = status_str(get(&env, "status"), "unknown");
        let mut value = format!("{} {}", dot(&st), l.t(env_label(&st)));
        let game = dict_or_empty(get(&env, "game"));
        let latency = get(&game, "latency_ms");
        if st == "online" && is_num(latency) {
            value.push_str(&format!(" · {} ms", round_num(latency.unwrap_or(&Value::Null))?));
        }
        fields.push(field(label, value, true));
    }
    let auth_value = match get(snap, "auth") {
        None | Some(Value::Null) => format!("{} {}", dot("unknown"), l.t("Checking…")),
        Some(a) => {
            if !a.is_object() {
                return Err(anyhow!("AttributeError: auth.get"));
            }
            if truthy(get(a, "online")) {
                format!("{} {}", dot("online"), l.t("Reachable"))
            } else {
                format!("{} {}", dot("down"), l.t("Unreachable"))
            }
        }
    };
    fields.push(field(l.t("Account auth"), auth_value, false));
    let overall_text = overall_text(l, &overall, &envs);
    let mut embed = json!({
        "title": l.t("Trove server status"),
        "url": format!("{SITE}/status"),
        "color": status_color(&overall),
        "description": format!("{}  **{overall_text}**", dot(&overall)),
        "fields": fields,
        "footer": footer("trove.aallyn.net/status"),
    });
    if let Some(checked) = get(snap, "checked_at").filter(|v| pyval::truthy(v)) {
        let f = py_float(checked).ok_or_else(|| anyhow!("fromtimestamp of {checked}"))?;
        let at = crate::dt::from_timestamp(f).ok_or_else(|| anyhow!("timestamp out of range"))?;
        push(&mut embed, "timestamp", Value::String(crate::dt::isoformat(&at)));
    }
    Ok(embed)
}

// --- /activity ---------------------------------------------------------------------------

pub fn activity_label(period: &str) -> Option<&'static str> {
    match period {
        "7d" => Some("Last 7 days"),
        "1m" => Some("Last month"),
        _ => None,
    }
}

pub fn activity_embed(l: Lang, asset: &str, period: &str, live: &Value, series: &Value) -> Result<Value> {
    let period = if activity_label(period).is_some() { period } else { "7d" };
    let label = l.t(activity_label(period).unwrap_or("Last 7 days"));
    let mut fields = vec![
        field(l.t("Last 24h"), num(get(live, "estimate_24h")), true),
        field(l.t("Last 7d"), num(get(live, "estimate_7d")), true),
    ];
    let peak = dict_or_empty(get(series, "peak"));
    if is_num(get(&peak, "active")) {
        let when = if truthy(get(&peak, "t")) { format!(" ({})", ts(req(&peak, "t")?, "d")?) } else { String::new() };
        fields.push(field(
            l.f("Peak · {label}", &[("label", &label)]),
            format!("{}{when}", thousands(round_num(req(&peak, "active")?)?)),
            true,
        ));
    }
    if is_num(get(series, "average")) {
        fields.push(field(l.t("Average"), thousands(round_num(req(series, "average")?)?), true));
    }
    let cb = [get(live, "computed_at"), get(series, "window_end")]
        .into_iter()
        .flatten()
        .find(|v| pyval::truthy(v))
        .cloned()
        .unwrap_or(json!(0));
    Ok(json!({
        "title": l.f("Player activity — {label}", &[("label", &label)]),
        "url": format!("{SITE}/activity?period={period}"),
        "color": 0x4CC9F0,
        "description": l.t("Active players per day and per week, estimated from the leaderboard captures."),
        "fields": fields,
        "image": {"url": format!("{asset}/activity/og.png?period={period}&v={}", s(&cb))},
        "footer": footer("trove.aallyn.net/activity · Trove server time (UTC−11)"),
    }))
}

// --- /chaos ------------------------------------------------------------------------------

pub fn chaos_desc(l: Lang, c: &Value) -> String {
    let item = dict_or_empty(get(c, "item"));
    match get(&item, "name").filter(|v| pyval::truthy(v)) {
        Some(name) => l.f("This week's featured item: **{name}**", &[("name", &s(name))]),
        None => l.t("No item has been relayed for this week's chest yet."),
    }
}

pub fn chaos_embed(l: Lang, c: &Value) -> Result<Value> {
    let desc = chaos_desc(l, c);
    let ends = req(c, "ends_at")?;
    let starts = req(c, "starts_at")?;
    Ok(json!({
        "title": l.t("🎁 Chaos Chest"),
        "color": 0xB794F6,
        "description": desc,
        "fields": [
            field(l.t("Rotates"), format!("{}\n{}", ts(ends, "R")?, ts(ends, "f")?), true),
            field(l.t("This week"), format!("{} → {}", ts(starts, "d")?, ts(ends, "d")?), true),
        ],
        "footer": footer(l.t("Trove Chaos Chest · rotates weekly (Tue 11:00 UTC)")),
    }))
}

// --- /servertime -------------------------------------------------------------------------

/// `trove_now().strftime("%A, %d %b %Y · %H:%M")`.
pub fn trove_clock(now_us: i64) -> String {
    let at = chrono::DateTime::from_timestamp_micros(now_us).unwrap_or_default() - chrono::Duration::hours(11);
    at.format("%A, %d %b %Y · %H:%M").to_string()
}

pub fn servertime_embed(l: Lang, st: &Value, clock: &str) -> Result<Value> {
    let now = req(st, "now_unix")?;
    let daily = req(st, "daily_reset_at")?;
    let weekly = req(st, "weekly_reset_at")?;
    Ok(json!({
        "title": l.t("🕙 Trove server time"),
        "color": 0x5EC6FF,
        "description": l.f("Trove runs on **UTC−11**. Right now it's **{server}** in server time.", &[("server", clock)]),
        "fields": [
            field(l.t("Your local time"), format!("{}\n{}", ts(now, "F")?, ts(now, "R")?), false),
            field(l.t("Daily reset"), format!("{}\n{}", ts(daily, "R")?, ts(daily, "f")?), true),
            field(l.t("Weekly reset"), format!("{}\n{}", ts(weekly, "R")?, ts(weekly, "f")?), true),
        ],
        "footer": footer(l.t("Daily reset 11:00 UTC · weekly Monday 11:00 UTC")),
    }))
}

// --- /bonuses ----------------------------------------------------------------------------

/// `_bullets`.
pub fn bullets(items: Option<&Value>) -> String {
    let lines: Vec<String> = match items {
        Some(Value::Array(a)) => a.iter().map(|b| format!("• {}", s(b))).collect(),
        Some(Value::String(st)) => st.chars().map(|c| format!("• {c}")).collect(),
        _ => Vec::new(),
    };
    if lines.is_empty() { "—".into() } else { lines.join("\n") }
}

fn join_strs(v: &Value, sep: &str) -> Result<String> {
    match v {
        Value::Array(a) => a
            .iter()
            .map(|x| x.as_str().map(str::to_owned).ok_or_else(|| anyhow!("TypeError: join of non-str")))
            .collect::<Result<Vec<_>>>()
            .map(|v| v.join(sep)),
        Value::String(st) => Ok(st.chars().map(String::from).collect::<Vec<_>>().join(sep)),
        _ => Err(anyhow!("TypeError: can only join an iterable")),
    }
}

fn or_default(v: Option<&Value>, default: &str) -> String {
    v.map_or_else(|| default.to_owned(), s)
}

pub fn bonuses_embed(l: Lang, daily: &Value, weekly: &Value) -> Result<Value> {
    let d = dict_or_empty(get(&dict_or_empty(Some(daily)), "current"));
    let w = dict_or_empty(get(&dict_or_empty(Some(weekly)), "current"));
    let mut daily_val = bullets(get(&d, "normal_buffs"));
    if truthy(get(&d, "premium_buffs")) {
        let buffs = join_strs(req(&d, "premium_buffs")?, ", ")?;
        daily_val.push('\n');
        daily_val.push_str(&l.f("*Patron:* {buffs}", &[("buffs", &buffs)]));
    }
    let mut color: i128 = 0xFFB86B;
    if let Some(c) = get(&d, "color").filter(|v| pyval::truthy(v))
        && let Some(st) = c.as_str()
        && let Some(i) = kiwi_core::feeds::py::int(st, 16)
    {
        color = i;
    }
    let mut embed = json!({
        "title": l.t("✨ Trove bonuses"),
        "color": i64::try_from(color).unwrap_or(i64::MAX),
        "fields": [
            field(
                l.f("{emoji} Daily — {name} ({weekday})", &[
                    ("emoji", &or_default(get(&d, "emoji"), "📅")),
                    ("name", &or_default(get(&d, "name"), "?")),
                    ("weekday", &or_default(get(&d, "weekday"), "")),
                ]),
                daily_val,
                false,
            ),
            field(
                l.f("{emoji} Weekly — {name}", &[
                    ("emoji", &or_default(get(&w, "emoji"), "🗓️")),
                    ("name", &or_default(get(&w, "name"), "?")),
                ]),
                bullets(get(&w, "buffs")),
                false,
            ),
        ],
        "footer": footer(l.t("Daily resets 11:00 UTC · weekly Monday")),
    });
    if let Some(icon) = get(&d, "icon").filter(|v| pyval::truthy(v)) {
        push(&mut embed, "thumbnail", json!({"url": icon}));
    }
    Ok(embed)
}

// --- /hourly_challenge -------------------------------------------------------------------

pub fn challenge_embed(l: Lang, cur: &Value) -> Result<Value> {
    let cur_val = if truthy(get(cur, "name")) {
        let mut v = format!("**{}**", s(req(cur, "name")?));
        if truthy(get(cur, "type")) {
            let t = req(cur, "type")?.as_str().ok_or_else(|| anyhow!("AttributeError: title"))?;
            v.push_str(&format!(" · {}", title(t)));
        }
        v.push('\n');
        if truthy(get(cur, "active")) {
            v.push_str(&l.f("Ends {when}", &[("when", &ts(req(cur, "ends_at")?, "R")?)]));
        } else {
            v.push_str(&l.f("Next window {when}", &[("when", &ts(req(cur, "starts_at")?, "R")?)]));
        }
        v
    } else {
        format!(
            "{}\n{}",
            l.t("No challenge captured for this window."),
            l.f(
                "Window {start} → {end}",
                &[("start", &ts(req(cur, "starts_at")?, "t")?), ("end", &ts(req(cur, "ends_at")?, "t")?)]
            )
        )
    };
    Ok(json!({
        "title": l.t("⚔️ Hourly challenge"),
        "color": 0xFF9800,
        "description": l.t("Trove's hourly challenge - 20 minutes at the top of each hour."),
        "fields": [field(l.t("Current"), cur_val, false)],
        "footer": footer(l.t("Times shown in your local zone")),
    }))
}

// --- biome rotations ---------------------------------------------------------------------

/// `_biomes_str`.
pub fn biomes_str(rot: &Value) -> String {
    let lines: Vec<String> = list(get(rot, "biomes"))
        .iter()
        .map(|b| {
            let name = get(b, "final_name").filter(|v| pyval::truthy(v)).or_else(|| get(b, "name"));
            format!("• {}", name.map_or_else(|| "None".to_owned(), s))
        })
        .collect();
    if lines.is_empty() { "—".into() } else { lines.join("\n") }
}

fn next_slot(r: &Value) -> Option<Value> {
    match get(r, "upcoming") {
        Some(Value::Array(a)) if !a.is_empty() => Some(a[0].clone()),
        _ => None,
    }
}

fn rotation_embed(l: Lang, r: &Value, labels: (&str, &str, &str, &str, &str, &str), color: i64) -> Result<Value> {
    let (cur_label, next_label, next_style, title_, desc, foot) = labels;
    let cur = dict_or_empty(get(r, "current"));
    let mut fields = vec![
        field(l.t(cur_label), biomes_str(&cur), true),
        field(l.t("Rotates"), ts(get(&cur, "ends_at").unwrap_or(&json!(0)), "R")?, true),
    ];
    if let Some(nxt) = next_slot(r).filter(pyval::truthy) {
        fields.push(field(
            l.f(next_label, &[("when", &ts(req(&nxt, "starts_at")?, next_style)?)]),
            biomes_str(&nxt),
            false,
        ));
    }
    Ok(json!({
        "title": l.t(title_),
        "color": color,
        "description": l.t(desc),
        "fields": fields,
        "footer": footer(l.t(foot)),
    }))
}

pub fn longshade_embed(l: Lang, r: &Value) -> Result<Value> {
    rotation_embed(
        l,
        r,
        (
            "Current biomes",
            "Up next ({when})",
            "t",
            "🌑 Depth 15 biomes",
            "Current Depth-15 adventure biome rotation (long shade).",
            "Rotates every 3 hours · Trove server time (UTC−11)",
        ),
        0x9B8AFB,
    )
}

pub fn wild_mana_embed(l: Lang, r: &Value) -> Result<Value> {
    rotation_embed(
        l,
        r,
        (
            "This week's biomes",
            "Next week ({when})",
            "d",
            "🌿 Wild Mana",
            "Weekly Wild Mana biome rotation.",
            "Rotates weekly · Trove server time (UTC−11)",
        ),
        0x46D39A,
    )
}

// --- /giveaways --------------------------------------------------------------------------

/// `_unix(dt)` of a giveaway view's ISO datetime.
fn unix_of(v: &Value) -> Result<i64> {
    let st = v.as_str().ok_or_else(|| anyhow!("not a datetime: {v}"))?;
    Ok(crate::dt::parse_utc(st).ok_or_else(|| anyhow!("bad datetime {st}"))?.timestamp())
}

fn gw_section(l: Lang, items: &[Value], fmt: impl Fn(&Value) -> Result<String>, empty: String) -> Result<String> {
    if items.is_empty() {
        return Ok(empty);
    }
    let parts = items.iter().take(5).map(fmt).collect::<Result<Vec<_>>>()?;
    let mut out = parts.join("\n\n");
    if items.len() > 5 {
        out.push('\n');
        out.push_str(&l.f("…and {n} more", &[("n", &(items.len() - 5).to_string())]));
    }
    Ok(out)
}

fn gw_head(g: &Value) -> String {
    format!(
        "**{}** — {}\n",
        get(g, "title").map_or_else(String::new, s),
        get(g, "prize_name").map_or_else(String::new, s)
    )
}

pub fn giveaways_embed(l: Lang, ongoing: &[Value], upcoming: &[Value], ended: &[Value]) -> Result<Value> {
    let open = |g: &Value| -> Result<String> {
        Ok(gw_head(g)
            + &l.f(
                "Ends {when} · {n} entered",
                &[
                    ("when", &ts_i(unix_of(req(g, "ends_at")?)?, "R")),
                    ("n", &get(g, "entry_count").map_or_else(|| "0".into(), s)),
                ],
            ))
    };
    let soon = |g: &Value| -> Result<String> {
        Ok(gw_head(g) + &l.f("Starts {when}", &[("when", &ts_i(unix_of(req(g, "starts_at")?)?, "R"))]))
    };
    let done = |g: &Value| -> Result<String> {
        let won = match get(g, "winner_username").filter(|v| pyval::truthy(v)) {
            Some(w) => format!(" · {}", l.f("won by {name}", &[("name", &s(w))])),
            None => String::new(),
        };
        Ok(gw_head(g) + &l.f("Ended {when}", &[("when", &ts_i(unix_of(req(g, "ends_at")?)?, "R"))]) + &won)
    };
    Ok(json!({
        "title": l.t("🎉 Giveaways"),
        "url": format!("{SITE}/giveaways"),
        "color": 0xFF5C80,
        "fields": [
            field(l.f("🟢 Open now ({n})", &[("n", &ongoing.len().to_string())]),
                  gw_section(l, ongoing, open, l.t("None right now."))?, false),
            field(l.f("🔜 Upcoming ({n})", &[("n", &upcoming.len().to_string())]),
                  gw_section(l, upcoming, soon, l.t("Nothing scheduled."))?, false),
            field(l.f("🏁 Ended · last 7 days ({n})", &[("n", &ended.len().to_string())]),
                  gw_section(l, ended, done, l.t("None in the last 7 days."))?, false),
        ],
        "footer": footer("trove.aallyn.net/giveaways"),
    }))
}

// --- merchants ---------------------------------------------------------------------------

fn schedule_tail(m: &Value) -> Vec<Value> {
    list(get(m, "schedule")).iter().skip(1).take(4).cloned().collect()
}

pub fn corruxion_embed(l: Lang, c: &Value) -> Result<Value> {
    let ends = req(c, "ends_at")?;
    let desc = if pyval::truthy(req(c, "active")?) {
        format!(
            "{}\n{} · {}",
            l.t("**Corruxion is here!**"),
            l.f("Leaves {when}", &[("when", &ts(ends, "R")?)]),
            ts(ends, "f")?
        )
    } else {
        let starts = req(c, "starts_at")?;
        format!(
            "{}\n{} · {}\n{}",
            l.t("Corruxion is away."),
            l.f("Arrives {when}", &[("when", &ts(starts, "R")?)]),
            ts(starts, "f")?,
            l.f("Stays until {when}", &[("when", &ts(ends, "f")?)])
        )
    };
    let mut fields = Vec::new();
    let up = schedule_tail(c);
    if !up.is_empty() {
        let lines = up
            .iter()
            .map(|w| Ok(format!("{} → {}", ts(req(w, "starts_at")?, "d")?, ts(req(w, "ends_at")?, "d")?)))
            .collect::<Result<Vec<_>>>()?;
        fields.push(field(l.t("Upcoming visits"), lines.join("\n"), false));
    }
    Ok(json!({
        "title": l.t("🐲 Corruxion"),
        "color": 0x9B5DE5,
        "description": desc,
        "fields": fields,
        "footer": footer(l.t("Corruxion merchant · 14-day cycle, 3-day visits")),
    }))
}

pub fn fluxion_embed(l: Lang, f: &Value) -> Result<Value> {
    let desc = if pyval::truthy(req(f, "active")?) {
        let ends = req(f, "ends_at")?;
        format!(
            "{}\n{} · {}",
            l.f("**Fluxion is here** — currently in the **{state}** stage.", &[("state", &s(req(f, "state")?))]),
            l.f("Leaves {when}", &[("when", &ts(ends, "R")?)]),
            ts(ends, "f")?
        )
    } else {
        let first = match get(f, "schedule") {
            Some(Value::Array(a)) if !a.is_empty() => a[0].clone(),
            Some(v) if pyval::truthy(v) => return Err(anyhow!("schedule[0]")),
            _ => json!({}),
        };
        let stage = get(&first, "state").map_or_else(|| "?".into(), s);
        let starts = req(f, "starts_at")?;
        format!(
            "{}\n{} · {}\n{}",
            l.t("Fluxion is away."),
            l.f("Next up: the **{stage}** stage, {when}", &[("stage", &stage), ("when", &ts(starts, "R")?)]),
            ts(starts, "f")?,
            l.f("Runs until {when}", &[("when", &ts(req(f, "ends_at")?, "f")?)])
        )
    };
    let mut fields = Vec::new();
    let up = schedule_tail(f);
    if !up.is_empty() {
        let lines = up
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
        fields.push(field(l.t("Upcoming stages"), lines.join("\n"), false));
    }
    Ok(json!({
        "title": l.t("💧 Fluxion"),
        "color": 0x4CC9F0,
        "description": desc,
        "fields": fields,
        "footer": footer(l.t("Fluxion merchant · alternates voting / selling")),
    }))
}

// --- luxion ------------------------------------------------------------------------------

fn luxion_rotations(schedule: &[Value], now: i64) -> Result<Vec<String>> {
    let mut out = Vec::new();
    for w in schedule {
        let ends = match get(w, "ends_at").filter(|v| pyval::truthy(v)) {
            Some(v) => int_of(v)?,
            None => 0,
        };
        if ends > i128::from(now) {
            let st = req(w, "starts_at")?;
            out.push(format!("{} - {}  ·  {}", ts(st, "F")?, ts(req(w, "ends_at")?, "F")?, ts(st, "R")?));
        }
    }
    out.truncate(7);
    Ok(out)
}

pub fn luxion_embed(l: Lang, asset: &str, lux: &Value, now: i64) -> Result<Value> {
    let active = truthy(get(lux, "active"));
    let schedule = list(get(lux, "schedule"));
    let mut desc;
    if active {
        if let Some(win) = get(lux, "current_window").filter(|v| pyval::truthy(v)) {
            let e = req(win, "ends_at")?;
            desc = format!(
                "{}\n{} · {}",
                l.t("**A rotation is open!**"),
                l.f("Closes {when}", &[("when", &ts(e, "R")?)]),
                ts(e, "f")?
            );
        } else {
            let mut opened = false;
            for w in schedule {
                if int_of(req(w, "starts_at")?)? <= i128::from(now) {
                    opened = true;
                    break;
                }
            }
            desc = if opened { l.t("**The Trials of Luxion are live**") } else { l.t("**Trials of Luxion started**") };
            if let Some(nxt) = get(lux, "next_window").filter(|v| pyval::truthy(v)) {
                let st = req(nxt, "starts_at")?;
                let when = ts(st, "R")?;
                let line = if opened {
                    l.f("Next rotation {when}", &[("when", &when)])
                } else {
                    l.f("First rotation {when}", &[("when", &when)])
                };
                desc.push_str(&format!("\n{line} · {}", ts(st, "f")?));
            }
        }
        desc.push('\n');
        desc.push_str(&l.f("Ends {when}", &[("when", &ts(req(lux, "ends_at")?, "R")?)]));
    } else {
        desc = l.t("The Trials of Luxion are away — they return roughly every 4 weeks (dev-set, unpredictable).");
    }
    let mut fields = Vec::new();
    let rotations = luxion_rotations(schedule, now)?;
    if !rotations.is_empty() {
        fields.push(field(l.t("Next rotations"), rotations.join("\n"), false));
    }
    let mut embed = json!({
        "title": l.t("🐉 Trials of Luxion"),
        "color": 0xF5C542,
        "description": desc,
        "fields": fields,
        "footer": footer(l.t("Trials of Luxion · a 3-hour rotation every 27 hours")),
    });
    if active {
        let v = get(lux, "starts_at").filter(|v| pyval::truthy(v)).map_or_else(|| "0".into(), s);
        push(&mut embed, "image", json!({"url": format!("{asset}/announce.png?kind=luxion&v={v}")}));
    }
    Ok(embed)
}

pub fn luxion_caption(l: Lang, lux: &Value, now: i64) -> Result<Value> {
    if !truthy(get(lux, "active")) {
        return Ok(json!({}));
    }
    let mut desc = match get(lux, "current_window").filter(|v| pyval::truthy(v)) {
        Some(win) => l.f("**A rotation is open** — closes {when}", &[("when", &ts(req(win, "ends_at")?, "R")?)]),
        None => l.t("**Trials of Luxion started**"),
    };
    let rotations = luxion_rotations(list(get(lux, "schedule")), now)?;
    if !rotations.is_empty() {
        desc.push_str(&format!("\n\n{}\n{}", l.t("Next rotations:"), rotations.join("\n")));
    }
    Ok(json!({"description": desc}))
}

// --- /stampy -----------------------------------------------------------------------------

pub fn stampy_embed(l: Lang, st: &Value, now: i64) -> Result<Value> {
    let Some(cur) = get(st, "current").filter(|v| pyval::truthy(v)) else {
        return Ok(json!({
            "title": l.t("🐘 Stampy"),
            "color": 0xFFB86B,
            "description": l.t("No upcoming Stampy event scheduled."),
            "footer": footer(l.t("Stampy event · weekly, 48-hour window")),
        }));
    };
    let biome = match get(cur, "biomes") {
        Some(Value::Array(a)) if !a.is_empty() => a[0].clone(),
        Some(v) if pyval::truthy(v) => return Err(anyhow!("biomes[0]")),
        _ => json!({}),
    };
    let bname = [get(&biome, "final_name"), get(&biome, "name")]
        .into_iter()
        .flatten()
        .find(|v| pyval::truthy(v))
        .cloned()
        .unwrap_or_else(|| json!("?"));
    let starts = req(cur, "starts_at")?;
    let ends = req(cur, "ends_at")?;
    let state = if int_of(starts)? <= i128::from(now) && i128::from(now) < int_of(ends)? {
        l.f("**Active now** — ends {when}", &[("when", &ts(ends, "R")?)])
    } else {
        format!("{} · {}", l.f("Starts {when}", &[("when", &ts(starts, "R")?)]), ts(starts, "f")?)
    };
    Ok(json!({
        "title": l.t("🐘 Stampy"),
        "color": 0xFFB86B,
        "description": state,
        "fields": [
            field(l.t("Biome"), bname, true),
            field(l.t("Window"), format!("{} → {}", ts(starts, "d")?, ts(ends, "d")?), true),
        ],
        "footer": footer(l.t("Stampy event · 48-hour window")),
    }))
}

// --- /trove_news -------------------------------------------------------------------------

pub fn trove_news_embed(l: Lang, items: &[crate::feeds::store::News]) -> Value {
    let Some(top) = items.first() else {
        return json!({
            "title": l.t("📰 Trove news"),
            "color": 0x5EC6FF,
            "description": l.t("No news cached yet — check back shortly."),
        });
    };
    let fields: Vec<Value> = items
        .iter()
        .skip(1)
        .take(4)
        .map(|n| {
            let when = n.published_at.map_or_else(String::new, |p| format!(" · {}", ts_i(p.timestamp(), "R")));
            field(prefix(&n.title, 250), format!("[{}]({}){when}", l.t("Read more"), n.url), false)
        })
        .collect();
    let mut embed = json!({
        "title": prefix(&format!("📰 {}", top.title), 256),
        "url": top.url,
        "color": 0x5EC6FF,
        "description": prefix(&top.summary, 600),
        "fields": fields,
        "footer": footer(l.t("Latest from trovegame.com")),
    });
    if let Some(img) = top.image.as_deref().filter(|i| !i.is_empty()) {
        push(&mut embed, "image", json!({"url": img}));
    }
    if let Some(p) = top.published_at {
        push(&mut embed, "timestamp", Value::String(crate::dt::isoformat(&p)));
    }
    embed
}

// --- /download, /web, /change_log ---------------------------------------------------------

/// One platform's newest release: `(tag_name, html_url, first matched asset url)`.
pub type PlatformRelease = Option<(Value, Value, Option<Value>)>;

fn platform_label(p: &str) -> String {
    match p {
        "windows" => "🪟 Windows".into(),
        "linux" => "🐧 Linux".into(),
        "android" => "🤖 Android".into(),
        other => title(other),
    }
}

pub fn download_embed(l: Lang, latest: &[(&str, PlatformRelease)]) -> Value {
    let mut fields = Vec::new();
    let mut page: Option<Value> = None;
    for (plat, entry) in latest {
        let label = platform_label(plat);
        let Some((tag, html, asset)) = entry else {
            fields.push(field(label, "—", true));
            continue;
        };
        if !page.as_ref().is_some_and(pyval::truthy) {
            page = Some(html.clone());
        }
        let url = asset.clone().unwrap_or_else(|| html.clone());
        fields.push(field(label, format!("[{}]({})", s(tag), s(&url)), true));
    }
    let url =
        page.filter(pyval::truthy).unwrap_or_else(|| json!("https://github.com/AallynReed/BetterTroveTools/releases"));
    json!({
        "title": l.t("⬇️ Download Better Trove Tools"),
        "url": url,
        "color": 0x4CC9F0,
        "description": l.t("Latest desktop build per platform. Prefer the browser? Use the web app: **https://btt.aallyn.net**"),
        "fields": fields,
        "footer": footer("github.com/AallynReed/BetterTroveTools/releases"),
    })
}

pub fn web_embed(l: Lang) -> Value {
    let desc = format!(
        "{}\n\n**→ https://btt.aallyn.net**\n\n{}",
        l.t("Use Better Trove Tools right in your browser — no install needed."),
        l.t("Gem builds, star chart, calculators, codexes and more. For the full suite (mod manager, file explorer), grab the desktop app with `/download`.")
    );
    json!({
        "title": l.t("🌐 Better Trove Tools — Web App"),
        "url": "https://btt.aallyn.net",
        "color": 0x4CC9F0,
        "description": desc,
        "footer": footer("btt.aallyn.net"),
    })
}

/// `cl` is the stored changelog (`groups`, `rate_limited`) or `None`.
pub fn changelog_embed(l: Lang, cl: Option<&Value>) -> Result<Value> {
    let groups = cl.map(|c| list(get(c, "groups"))).unwrap_or(&[]);
    let Some(cl) = cl.filter(|_| !groups.is_empty()) else {
        return Ok(json!({
            "title": l.t("📝 BTT changelog"),
            "color": 0x8A93A3,
            "description": l.t("No changelog cached yet."),
        }));
    };
    let mut fields = Vec::new();
    for g in groups {
        let commits = list(get(g, "commits"));
        if commits.is_empty() {
            continue;
        }
        let mut lines = Vec::new();
        for c in commits.iter().take(6) {
            let msg = get(c, "message").filter(|v| pyval::truthy(v)).map_or_else(String::new, s);
            let first = first_line(&msg).ok_or_else(|| anyhow!("IndexError: list index out of range"))?;
            lines.push(format!("• {}", prefix(first, 100)));
        }
        if commits.len() > 6 {
            lines.push(l.f("…and {n} more", &[("n", &(commits.len() - 6).to_string())]));
        }
        let name = get(g, "version").filter(|v| pyval::truthy(v)).cloned().unwrap_or_else(|| json!(l.t("Unreleased")));
        fields.push(field(name, lines.join("\n"), false));
        if fields.len() >= 4 {
            break;
        }
    }
    let mut embed = json!({
        "title": l.t("📝 Better Trove Tools — changelog"),
        "url": "https://github.com/AallynReed/BetterTroveTools/releases",
        "color": 0xB794F6,
        "fields": fields,
        "footer": footer("Recent changes · github.com/AallynReed/BetterTroveTools"),
    });
    if truthy(get(cl, "rate_limited")) {
        push(
            &mut embed,
            "description",
            json!(l.t("⚠️ GitHub rate-limited the last refresh — this may be slightly behind.")),
        );
    }
    Ok(embed)
}

// --- live board, /ping, game_update -------------------------------------------------------

pub fn live_board_embed(l: Lang, asset: &str, overall: &str, minute: i64, now_iso: &str) -> Value {
    let suffix = if l.code() != "en" { format!("&lang={}", l.code()) } else { String::new() };
    let color = match overall {
        "online" | "maintenance" | "down" | "unknown" => status_color(overall),
        _ => 0x46D39A,
    };
    json!({
        "title": l.t("🥝 Trove Now"),
        "url": SITE,
        "color": color,
        "image": {"url": format!("{asset}/board.png?v={minute}{suffix}")},
        "footer": footer(l.t("Live · updates every minute · trove.aallyn.net")),
        "timestamp": now_iso,
    })
}

/// A store's ping: `Ok(ms)`, `Err(())` unreachable; Redis `None` = not configured.
pub fn ping_embed(l: Lang, mongo: Result<f64, ()>, redis: Option<Result<f64, ()>>) -> Value {
    let up = |ms: f64| format!("🟢 {ms:.0} ms");
    let down = || format!("🔴 {}", l.t("unreachable"));
    let fields = vec![
        field("MongoDB", mongo.map_or_else(|_| down(), up), true),
        field(
            "Redis",
            match redis {
                None => format!("⚪ {}", l.t("not configured")),
                Some(r) => r.map_or_else(|_| down(), up),
            },
            true,
        ),
    ];
    json!({
        "title": l.t("🏓 Pong"),
        "color": 0x46D39A,
        "description": l.t("Kiwi API is online."),
        "fields": fields,
        "footer": footer("api.aallyn.net/health"),
    })
}

/// `v` is the newest complete `live-us` update version, as JSON.
pub fn game_update_embed(l: Lang, asset: &str, v: Option<&Value>) -> Value {
    let Some(v) = v else {
        return json!({
            "title": l.t("🧩 Trove update"),
            "color": 0x5EC6FF,
            "url": format!("{SITE}/updates"),
            "description": l.t("No update has been archived yet."),
            "footer": footer("trove.aallyn.net/updates"),
        });
    };
    let tag = get(v, "version_tag").map_or_else(|| "None".into(), s);
    let ordinal = get(v, "ordinal").map_or_else(|| "None".into(), s);
    json!({
        "title": l.f("🧩 New Trove update — {version}", &[("version", &tag)]),
        "color": 0x5EC6FF,
        "url": format!("{SITE}/updates"),
        "description": l.t("A new build is live on the US servers. See what changed:"),
        "fields": [
            field(l.t("Added"), num(get(v, "files_added")), true),
            field(l.t("Modified"), num(get(v, "files_modified")), true),
            field(l.t("Removed"), num(get(v, "files_removed")), true),
        ],
        "image": {"url": format!("{asset}/announce.png?kind=game_update&v={ordinal}")},
        "footer": footer(l.t("Browse the changed files at trove.aallyn.net/updates")),
    })
}

// --- /price, market watch -----------------------------------------------------------------

pub fn price_embed(l: Lang, name: &str, summary: Option<&Value>) -> Result<Value> {
    let Some(sm) = summary else {
        return Ok(json!({
            "title": l.f("💰 Market — {name}", &[("name", name)]),
            "color": 0xF0A500,
            "url": format!("{SITE}/market"),
            "description": l.f("No active listings found for **{name}**. Check the spelling, or browse the market.", &[("name", name)]),
            "footer": footer("trove.aallyn.net/market"),
        }));
    };
    let canonical = get(sm, "name").filter(|v| pyval::truthy(v)).map_or_else(|| name.to_owned(), s);
    let ea = l.t("flux ea.");
    let min = flux(Some(req(sm, "min_each")?))?;
    Ok(json!({
        "title": l.f("💰 Market — {name}", &[("name", &canonical)]),
        "color": 0xF0A500,
        "url": format!("{SITE}/market"),
        "description": l.f("Cheapest **{price}** flux each, across **{count}** active listing(s).", &[
            ("price", &min),
            ("count", &s(req(sm, "count")?)),
        ]),
        "fields": [
            field(l.t("Cheapest"), format!("{min} {ea}"), true),
            field(l.t("Median"), format!("{} {ea}", flux(Some(req(sm, "median_each")?))?), true),
            field(l.t("Average"), format!("{} {ea}", flux(Some(req(sm, "avg_each")?))?), true),
            field(l.t("Highest"), format!("{} {ea}", flux(Some(req(sm, "max_each")?))?), true),
            field(l.t("Listings"), num(Some(req(sm, "count")?)), true),
            field(l.t("In stock"), num(Some(req(sm, "total_stack")?)), true),
        ],
        "footer": footer(l.t("Trove marketplace · recently-seen player listings")),
    }))
}

pub fn market_watch_embed(l: Lang, name: &str, summary: &Value, max_price_each: Option<f64>) -> Result<Value> {
    let cheapest = flux(get(summary, "min_each"))?;
    let ea = l.t("flux ea.");
    let target = match max_price_each {
        Some(p) => l.f("dropped to {p} flux or less", &[("p", &flux(Some(&json!(p)))?)]),
        None => l.t("is now listed"),
    };
    Ok(json!({
        "title": l.f("🔔 Market watch — {name}", &[("name", name)]),
        "color": 0xF0A500,
        "url": format!("{SITE}/market"),
        "description": l.f("**{name}** {target}: cheapest **{price}** flux each.", &[
            ("name", name), ("target", &target), ("price", &cheapest),
        ]),
        "fields": [
            field(l.t("Cheapest"), format!("{cheapest} {ea}"), true),
            field(l.t("Listings"), num(get(summary, "count")), true),
        ],
        "footer": footer("trove.aallyn.net/market"),
    }))
}

// --- /trove_uptime -----------------------------------------------------------------------

/// `_pct`.
pub fn pct(v: Option<&Value>) -> String {
    match v {
        Some(Value::Bool(b)) => format!("{:.2}%", f64::from(u8::from(*b)) * 100.0),
        Some(Value::Number(n)) => match n.as_i64() {
            Some(i) if !n.is_f64() => format!("{:.2}%", (i128::from(i) * 100) as f64),
            _ => format!("{:.2}%", n.as_f64().unwrap_or(0.0) * 100.0),
        },
        _ => "—".into(),
    }
}

/// `hists` is `get_history(env, days)` for each region in order.
pub fn uptime_embed(l: Lang, hists: &[Value], days: i64) -> Value {
    let fields: Vec<Value> = REGIONS
        .iter()
        .zip(hists)
        .map(|((_, label), h)| {
            let mut line = pct(get(h, "uptime"));
            let outages = list(get(h, "outages"));
            if !outages.is_empty() {
                line.push_str(&l.f(" · {n} outage(s)", &[("n", &outages.len().to_string())]));
            }
            field(*label, line, true)
        })
        .collect();
    json!({
        "title": l.f("📈 Trove uptime — last {days} days", &[("days", &days.to_string())]),
        "url": format!("{SITE}/status"),
        "color": 0x46D39A,
        "description": l.t("Region uptime measured by Kiwi's status prober."),
        "fields": fields,
        "footer": footer("trove.aallyn.net/status · Trove server time (UTC−11)"),
    })
}

// --- /gem --------------------------------------------------------------------------------

pub fn gem_error_embed(l: Lang, reason: &str) -> Value {
    json!({
        "title": l.t("💎 Gem evaluator"),
        "color": 0xF0556A,
        "description": l.f("Couldn't evaluate that gem: {reason}", &[("reason", reason)]),
        "footer": footer("trove.aallyn.net"),
    })
}

/// `r` is `evaluate_gem(...)["result"]` as JSON.
pub fn gem_embed(l: Lang, tier_name: &str, type_name: &str, level: i64, r: &Value) -> Result<Value> {
    let f1 =
        |v: &Value| -> Result<String> { Ok(format!("{:.1}", v.as_f64().ok_or_else(|| anyhow!("not a float: {v}"))?)) };
    let quality = f1(req(r, "quality_percent")?)?;
    let pr = thousands(int_of(req(r, "calculated_power_rank")?)?);
    let mut fields = Vec::new();
    for st in list(get(r, "stats")) {
        let flag = if truthy(get(st, "is_within_range")) { "" } else { " ⚠️" };
        let entered = req(st, "entered_value")?.as_f64().ok_or_else(|| anyhow!("entered_value"))?;
        fields.push(field(
            l.t(&s(req(st, "display_name")?)),
            format!("{} · {}%{flag}", fmt_g(entered), f1(req(st, "quality_percent")?)?),
            true,
        ));
    }
    let cost = dict_or_empty(get(r, "headline_cost"));
    let precise = get(&cost, "precise").map_or_else(|| "0".into(), s);
    let rough = get(&cost, "rough").map_or_else(|| "0".into(), s);
    if truthy(get(r, "has_issues")) {
        let notes: Vec<String> = list(get(r, "issues")).iter().take(3).map(|i| format!("• {}", l.t(&s(i)))).collect();
        fields.push(field(l.t("⚠️ Notes"), notes.join("\n"), false));
    }
    Ok(json!({
        "title": l.f("💎 {tier} {type} Gem — Level {level}", &[
            ("tier", &l.t(tier_name)), ("type", &l.t(type_name)), ("level", &level.to_string()),
        ]),
        "url": format!("{SITE}/"),
        "color": 0x9B5DE5,
        "description": l.f("**{quality}%** quality · est. Power Rank **{pr}**", &[("quality", &quality), ("pr", &pr)]),
        "fields": fields,
        "footer": footer(l.f("Cost to 100%: {precise} Precise + {rough} Rough focus", &[("precise", &precise), ("rough", &rough)])),
    }))
}

/// `obj` for callers that assemble embeds field by field.
pub fn object(pairs: Vec<(&str, Value)>) -> Value {
    obj(pairs)
}
