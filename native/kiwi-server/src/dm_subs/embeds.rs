//! `app/dm_subs/embeds.py`: the DM bodies. An `Err` is the exception Python
//! raises on a payload of the wrong shape.

use serde_json::{Map, Value, json};

use crate::{
    dt,
    outbound::py::{capitalize, commas, fixed, get, get_or, py_float, py_int, py_str, truthy, type_name},
};

const FOOTER: &str = "Kiwi DM alerts \u{b7} manage at trove.aallyn.net/dashboard";

fn color(kind: &str) -> i64 {
    match kind {
        "challenge" => 0x9B59B6,
        "corruxion" => 0xF1C40F,
        "fluxion" => 0xF39C12,
        "market_watch" => 0x2ECC71,
        _ => 0x5EC6FF,
    }
}

/// `classify_challenge`.
pub fn classify_challenge(name: &Value) -> anyhow::Result<Option<&'static str>> {
    let n = match name {
        Value::Null => return Ok(None),
        Value::String(s) => kiwi_core::pystr::strip(s),
        other => anyhow::bail!("AttributeError: '{}' object has no attribute 'strip'", type_name(other)),
    };
    Ok(match n {
        "" => None,
        "Collection Challenge" => Some("collection"),
        "RAMPAGE ALERT!" => Some("rampage"),
        "Racing Challenge" => Some("racing"),
        "Target Challenge" => Some("target"),
        _ => Some("dungeon"),
    })
}

fn int_of(v: &Value) -> anyhow::Result<i128> {
    py_int(v).ok_or_else(|| anyhow::anyhow!("int() failed on {}", type_name(v)))
}

/// `discord_ts(unix, "R") if unix else ""`.
fn rel(unix: &Value) -> anyhow::Result<String> {
    if !truthy(unix) {
        return Ok(String::new());
    }
    Ok(format!("<t:{}:R>", int_of(unix)?))
}

/// `unix_to_iso`: `datetime.fromtimestamp(int(unix), UTC).isoformat()`.
fn unix_to_iso(unix: &Value) -> anyhow::Result<Option<String>> {
    if !truthy(unix) {
        return Ok(None);
    }
    let Some(i) = py_int(unix) else { return Ok(None) };
    if i64::try_from(i).is_err() {
        anyhow::bail!("OverflowError: timestamp out of range for platform time_t");
    }
    if !(-62_135_596_800..=253_402_300_799).contains(&i) {
        return Ok(None);
    }
    Ok(dt::from_timestamp(i as f64).map(|d| dt::isoformat(&d)))
}

fn embed(kind: &str, title: String, description: String, ts: Option<&Value>) -> anyhow::Result<Value> {
    let mut e = Map::new();
    e.insert("title".into(), title.into());
    e.insert("description".into(), description.into());
    e.insert("color".into(), color(kind).into());
    e.insert("footer".into(), json!({"text": FOOTER}));
    if let Some(iso) = match ts {
        Some(v) => unix_to_iso(v)?,
        None => None,
    } {
        e.insert("timestamp".into(), iso.into());
    }
    Ok(json!({"embeds": [Value::Object(e)]}))
}

/// `_flux`.
pub fn flux(v: Option<&Value>) -> anyhow::Result<String> {
    let Some(n) = v.and_then(py_float) else {
        return Ok("?".into());
    };
    let trim = |s: String| s.trim_end_matches('0').trim_end_matches('.').to_owned();
    if n >= 1_000_000.0 {
        return Ok(format!("{}M", trim(fixed(n / 1_000_000.0, 2))));
    }
    if n >= 1_000.0 {
        return Ok(format!("{}k", trim(fixed(n / 1_000.0, 1))));
    }
    anyhow::ensure!(n.is_finite(), "round() of a non-finite float");
    Ok(commas(n.round_ties_even() as i128))
}

/// `build`: the DM body for an event, `None` when the type has none.
pub fn build(event_type: &str, data: &Value) -> anyhow::Result<Option<Value>> {
    let empty = Value::Object(Map::new());
    let data = if truthy(data) { data } else { &empty };
    let s = |v: &str| Value::String(v.to_owned());
    Ok(Some(match event_type {
        "challenge" => {
            let name = get_or(data, "name", &s("Challenge"))?.clone();
            let ctype = classify_challenge(&name)?;
            let ends = match get(data, "window_ends_at")? {
                Some(v) if truthy(v) => v.clone(),
                _ => get(data, "ends_at")?.cloned().unwrap_or(Value::Null),
            };
            let label = match ctype {
                Some("collection") => "Collection",
                Some("rampage") => "Rampage",
                Some("racing") => "Racing",
                Some("target") => "Target",
                Some("dungeon") => "Dungeon",
                _ => "Challenge",
            };
            let mut desc = format!("**{}**", py_str(&name));
            if truthy(&ends) {
                desc.push_str(&format!("\nEnds {}", rel(&ends)?));
            }
            embed("challenge", format!("\u{2694}\u{fe0f} {label} challenge is live"), desc, Some(&ends))?
        }
        "corruxion" | "fluxion" => {
            let ends = get(data, "ends_at")?.cloned().unwrap_or(Value::Null);
            let mut desc = "The merchant is in town.".to_owned();
            if truthy(&ends) {
                desc.push_str(&format!("\nLeaves {}", rel(&ends)?));
            }
            embed(event_type, format!("\u{1f6d2} {} has arrived", capitalize(event_type)), desc, Some(&ends))?
        }
        "game_update" => {
            let tag = match get(data, "version_tag")? {
                Some(v) if truthy(v) => v.clone(),
                _ => match get(data, "tag")? {
                    Some(v) if truthy(v) => v.clone(),
                    _ => get_or(data, "version", &s(""))?.clone(),
                },
            };
            let branch = py_str(get_or(data, "branch", &s("live"))?);
            let mut desc = format!("A new Trove build shipped on **{branch}**.");
            if truthy(&tag) {
                desc.push_str(&format!("\nVersion `{}`", py_str(&tag)));
            }
            desc.push_str("\nBrowse the changed files at trove.aallyn.net/updates");
            embed("game_update", "\u{1f9e9} New Trove update".to_owned(), desc, None)?
        }
        "market_watch" => {
            let name = py_str(get_or(data, "name", &s("Item"))?);
            let thr = get(data, "max_price_each")?.filter(|v| !v.is_null());
            let median = get(data, "median_each")?.filter(|v| !v.is_null());
            let mut desc = format!("**{}** is listed at **{}** flux each.", name, flux(get(data, "price_each")?)?);
            if thr.is_some() {
                desc.push_str(&format!("\nYour alert: at or below {}.", flux(thr)?));
            }
            if median.is_some() {
                desc.push_str(&format!("\nCurrent median: {}.", flux(median)?));
            }
            desc.push_str("\nSee live listings at trove.aallyn.net/market");
            embed("market_watch", format!("\u{1f4c9} {} hit your price", name), desc, None)?
        }
        _ => return Ok(None),
    }))
}

pub fn test_body() -> Value {
    json!({"embeds": [{
        "title": "\u{1f95d} DM alerts connected",
        "description": "This is a test message. You'll get alerts like this when your subscribed events fire. Manage them from your Dashboard.",
        "color": 0x5EC6FF,
        "footer": {"text": FOOTER},
    }]})
}
