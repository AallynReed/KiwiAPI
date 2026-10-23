//! `og_image._draw_board` (the live "Trove Now" board) and the per-kind
//! announcement banners (`_announcement_content`, `_draw_announcement`).
//! Inputs are the service dicts as JSON, read with Python's dict semantics.

use kiwi_pil::{Draw, ImageError, Ink, Library, Rgba};
use serde_json::{Map, Value};

use super::{
    W,
    activity::{BG, MUTE, TEXT, rgba},
    i18n::{Arg, Locale, Tr},
    text::Fonts,
};
use crate::features::py_truthy;

pub const ANN_W: usize = 1200;
pub const ANN_H: usize = 420;
const GREEN: [u8; 3] = [93, 208, 120];
const GREY: [u8; 3] = [90, 96, 106];
const CARDS: [(&str, [u8; 3]); 6] = [
    ("HOURLY CHALLENGE", [255, 152, 0]),
    ("CHAOS CHEST", [183, 148, 246]),
    ("DEPTH-15 BIOMES", [155, 138, 251]),
    ("RESETS", [94, 198, 255]),
    ("CORRUXION", [155, 93, 229]),
    ("FLUXION", [76, 201, 240]),
];

fn bad(msg: &'static str) -> ImageError {
    ImageError::Value(msg)
}

/// `d.get(key)` on a dict (anything else raises).
fn get<'v>(d: &'v Value, key: &str) -> Result<Option<&'v Value>, ImageError> {
    match d {
        Value::Object(m) => Ok(m.get(key).filter(|v| !v.is_null())),
        _ => Err(bad("'object' has no attribute 'get'")),
    }
}

/// `x or {}`.
fn or_empty(v: Option<&Value>) -> Value {
    match v {
        Some(v) if py_truthy(v) => v.clone(),
        _ => Value::Object(Map::new()),
    }
}

fn truthy(v: Option<&Value>) -> bool {
    v.is_some_and(py_truthy)
}

/// `int(x)`.
fn int_of(v: &Value) -> Result<i64, ImageError> {
    match v {
        Value::Bool(b) => Ok(*b as i64),
        Value::Number(n) => n.as_i64().or_else(|| n.as_f64().map(|f| f.trunc() as i64)).ok_or(bad("int()")),
        Value::String(s) => s.trim().parse().map_err(|_| bad("invalid literal for int()")),
        _ => Err(bad("int() argument must be a string or a number")),
    }
}

/// `str(x)` for the values a service dict carries.
fn str_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Null => "None".into(),
        other => other.to_string(),
    }
}

/// A text value to draw (Pillow wants a str).
fn text_of(v: &Value) -> Result<String, ImageError> {
    match v {
        Value::String(s) => Ok(s.clone()),
        _ => Err(bad("expected string")),
    }
}

/// `_rel`: "in 2h 14m" / "now" / "—".
pub fn rel(tr: &Tr, ts: Option<&Value>, now: i64) -> Result<String, ImageError> {
    let Some(ts) = ts.filter(|v| py_truthy(v)) else { return Ok("—".into()) };
    let delta = int_of(ts)? - now;
    if delta <= 0 {
        return Ok(tr.t("now"));
    }
    let (days, rem) = (delta / 86_400, delta % 86_400);
    let (hours, rem) = (rem / 3600, rem % 3600);
    let mins = rem / 60;
    if days > 0 {
        tr.tf("in {d}d {h}h", &[("d", Arg::Int(days)), ("h", Arg::Int(hours))])
    } else if hours > 0 {
        tr.tf("in {h}h {m}m", &[("h", Arg::Int(hours)), ("m", Arg::Int(mins))])
    } else {
        tr.tf("in {m}m", &[("m", Arg::Int(mins))])
    }
}

/// `_rel_coarse` over `countdown_bucket`.
pub fn rel_coarse(tr: &Tr, ts: Option<&Value>, now: i64) -> Result<String, ImageError> {
    let Some(ts) = ts.filter(|v| py_truthy(v)) else { return Ok("—".into()) };
    let rem = int_of(ts)? - now;
    if rem <= 0 {
        Ok(tr.t("now"))
    } else if rem < 3600 {
        tr.tf("in {m}m", &[("m", Arg::Int(rem / 60))])
    } else if rem < 86_400 {
        tr.tf("in {h}h", &[("h", Arg::Int(rem / 3600))])
    } else {
        tr.tf("in {d}d", &[("d", Arg::Int(rem / 86_400))])
    }
}

type Rel = fn(&Tr, Option<&Value>, i64) -> Result<String, ImageError>;

/// `t(text, when=rel(ts))`.
fn when(tr: &Tr, text: &str, rel: Rel, ts: Option<&Value>, now: i64) -> Result<String, ImageError> {
    tr.tf(text, &[("when", Arg::Str(&rel(tr, ts, now)?))])
}

/// `_merchant_lines` (and the announcement's copy of it).
fn merchant_lines(tr: &Tr, m: &Value, now: i64, rel: Rel) -> Result<(String, String), ImageError> {
    if truthy(get(m, "active")?) {
        let mut head = tr.t("Here now");
        if let Some(stage) = get(m, "state")?.filter(|s| py_truthy(s))
            && *stage != Value::String("away".into())
        {
            head.push_str(" · ");
            head.push_str(&str_of(stage));
        }
        return Ok((head, when(tr, "Leaves {when}", rel, get(m, "ends_at")?, now)?));
    }
    Ok((tr.t("Away"), when(tr, "Arrives {when}", rel, get(m, "starts_at")?, now)?))
}

fn st_color(st: &Value) -> [u8; 3] {
    match st.as_str() {
        Some("online") => [93, 208, 120],
        Some("down" | "maintenance") => [248, 81, 73],
        _ => [154, 164, 178],
    }
}

fn st_label(st: &Value) -> &'static str {
    match st.as_str() {
        Some("online") => "Online",
        Some("down" | "maintenance") => "Down",
        _ => "Unknown",
    }
}

fn st_overall(st: &Value) -> &'static str {
    match st.as_str() {
        Some("online") => "All systems online",
        Some("down" | "maintenance") => "Servers down",
        _ => "Status unknown",
    }
}

/// A dict key: unhashable values raise.
fn hashable(v: Value) -> Result<Value, ImageError> {
    match v {
        Value::Array(_) | Value::Object(_) => Err(bad("unhashable type")),
        v => Ok(v),
    }
}

fn status_overall(status: Option<&Value>) -> Result<Value, ImageError> {
    let s = or_empty(status);
    hashable(get(&s, "overall")?.cloned().unwrap_or_else(|| "unknown".into()))
}

/// The board as RGBA pixels. `data` holds the service dicts under
/// challenge/chaos/biomes/corruxion/fluxion/server/status.
pub fn draw_board(
    lib: &Library,
    fonts: &Fonts,
    loc: &Locale,
    data: &Value,
    now: i64,
    hhmm: &str,
) -> Result<Rgba, ImageError> {
    let mut img = Rgba::new(W, super::H, rgba(BG, 255));
    let mut d = Draw::new(&mut img);
    let mut tx = loc.text(lib, fonts);
    let tr = &loc.tr;
    let m = 56.0;
    let overall = status_overall(data.get("status"))?;
    let oc = st_color(&overall);

    d.rectangle([0.0, 0.0, W as f64, 6.0], Some(rgba(oc, 255)), None, 1)?;
    d.ellipse([m, 54.0, m + 16.0, 70.0], Some(rgba(oc, 255)), None, 1)?;
    tx.fit_text(
        &mut d,
        (m + 30.0, 50.0),
        &tr.t("TROVE NOW · LIVE"),
        1200 - 56 - (56 + 30),
        24,
        rgba(MUTE, 255),
        true,
        false,
        None,
    )?;
    let headline = tr.t(st_overall(&overall));
    tx.fit_text(&mut d, (m, 84.0), &headline, 1200 - 2 * 56, 60, rgba(oc, 255), true, false, Some(38))?;

    let cur = data.get("challenge").ok_or(bad("challenge"))?;
    let challenge_on = truthy(get(cur, "active")?) && truthy(get(cur, "name")?);
    let (ch_val, ch_sub) = if challenge_on {
        let name = match cur {
            Value::Object(o) => o.get("name").ok_or(bad("name"))?,
            _ => return Err(bad("challenge")),
        };
        (text_of(name)?, when(tr, "Ends {when}", rel, get(cur, "ends_at")?, now)?)
    } else if truthy(get(cur, "active")?) {
        (tr.t("No Challenge"), tr.t("Starting…"))
    } else {
        let interval = if truthy(get(cur, "is_friday_window")?) { 1800 } else { 3600 };
        let starts = match get(cur, "starts_at")?.filter(|v| py_truthy(v)) {
            Some(v) => int_of(v)?,
            None => 0,
        };
        let nxt = starts + interval;
        let sub = if nxt != 0 { when(tr, "Next {when}", rel, Some(&Value::from(nxt)), now)? } else { String::new() };
        (tr.t("No Challenge"), sub)
    };

    let chaos = data.get("chaos").ok_or(bad("chaos"))?;
    let item = get(&or_empty(get(chaos, "item")?), "name")?.cloned();
    let (chaos_val, chaos_sub) = match item.as_ref().filter(|v| py_truthy(v)) {
        Some(name) => (text_of(name)?, when(tr, "Rotates {when}", rel, get(chaos, "ends_at")?, now)?),
        None => ("—".to_owned(), tr.t("Not relayed yet")),
    };

    let biomes = data.get("biomes").ok_or(bad("biomes"))?;
    let mut bnames = Vec::new();
    if let Some(Value::Array(list)) = get(biomes, "biomes")?.filter(|v| py_truthy(v)) {
        for b in list {
            let n = match get(b, "final_name")?.filter(|v| py_truthy(v)) {
                Some(v) => Some(v.clone()),
                None => get(b, "name")?.cloned(),
            };
            if let Some(n) = n.filter(py_truthy) {
                bnames.push(text_of(&n)?);
            }
        }
    }
    let biome_rotate = when(tr, "rotates {when}", rel, get(biomes, "ends_at")?, now)?;

    let s = data.get("server").ok_or(bad("server"))?;
    let need = |k: &str| match s {
        Value::Object(o) => o.get(k).ok_or(bad("KeyError")),
        _ => Err(bad("server")),
    };
    let reset_val = when(tr, "Daily {when}", rel, Some(need("daily_reset_at")?), now)?;
    let reset_sub = when(tr, "Weekly {when}", rel, Some(need("weekly_reset_at")?), now)?;
    let (cx_val, cx_sub) = merchant_lines(tr, data.get("corruxion").ok_or(bad("corruxion"))?, now, rel)?;
    let (fx_val, fx_sub) = merchant_lines(tr, data.get("fluxion").ok_or(bad("fluxion"))?, now, rel)?;
    let values = [
        Some((ch_val, ch_sub)),
        Some((chaos_val, chaos_sub)),
        None,
        Some((reset_val, reset_sub)),
        Some((cx_val, cx_sub)),
        Some((fx_val, fx_sub)),
    ];

    let gap = 20i64;
    let cw = (1200 - 2 * 56 - gap) / 2;
    let (ch_h, y0) = (118i64, 172i64);
    for (i, (title, accent)) in CARDS.iter().enumerate() {
        let muted = i == 0 && !challenge_on;
        let accent = if muted { GREY } else { *accent };
        let x = 56 + (i as i64 % 2) * (cw + gap);
        let y = y0 + (i as i64 / 2) * (ch_h + gap);
        let (xf, yf) = (x as f64, y as f64);
        d.rounded_rectangle(
            [xf, yf, xf + cw as f64, yf + ch_h as f64],
            18.0,
            Some([22, 27, 34, 255]),
            Some(rgba(accent, 90)),
            2,
        )?;
        d.rounded_rectangle([xf, yf, xf + 6.0, yf + ch_h as f64], 3.0, Some(rgba(accent, 255)), None, 1)?;
        let mut title_max = cw - 52;
        match &values[i] {
            None => {
                let rotate_w = tx.fit_text(
                    &mut d,
                    ((x + cw - 26) as f64, yf + 16.0),
                    &biome_rotate,
                    (cw as f64 * 0.5) as i64,
                    20,
                    rgba(MUTE, 255),
                    false,
                    true,
                    None,
                )?;
                title_max = cw - 52 - rotate_w - 14;
                let names: Vec<String> =
                    if bnames.is_empty() { vec!["—".into()] } else { bnames.iter().take(3).cloned().collect() };
                for (j, name) in names.iter().enumerate() {
                    let yy = yf + 44.0 + j as f64 * 23.0;
                    tx.fit_text(&mut d, (xf + 26.0, yy), name, cw - 52, 22, rgba(TEXT, 255), true, false, None)?;
                }
            }
            Some((val, sub)) => {
                let fill = rgba(if muted { MUTE } else { TEXT }, 255);
                tx.fit_text(&mut d, (xf + 26.0, yf + 44.0), val, cw - 52, 33, fill, true, false, None)?;
                tx.fit_text(&mut d, (xf + 26.0, yf + 84.0), sub, cw - 52, 23, rgba(MUTE, 255), false, false, None)?;
            }
        }
        let title = tr.t(title);
        tx.fit_text(&mut d, (xf + 26.0, yf + 16.0), &title, 90.max(title_max), 20, rgba(MUTE, 255), true, false, None)?;
    }

    let h = super::H as f64;
    tx.draw(&mut d, (m, h - 46.0), "trove.aallyn.net", 23, false, rgba(MUTE, 255))?;
    let lw = tx.w("trove.aallyn.net", 23, false)?;
    let fr = tr.tf("Updated {time} UTC", &[("time", Arg::Str(hhmm))])?;
    tx.fit_text(
        &mut d,
        (W as f64 - m, h - 46.0),
        &fr,
        (1200 - 56) - (56 + lw + 24),
        23,
        rgba(MUTE, 210),
        false,
        true,
        None,
    )?;
    Ok(img)
}

/// `_ANN_TITLE`.
fn ann_title(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "hourly_challenge" => "Hourly Challenge",
        "chaos_chest" => "Chaos Chest",
        "daily_bonuses" => "Daily Bonus",
        "longshade" => "Depth-15 Biomes",
        "wild_mana" => "Wild Mana",
        "stampy" => "Stampy Event",
        "corruxion" => "Corruxion Merchant",
        "fluxion" => "Fluxion Merchant",
        "luxion" => "Trials of Luxion",
        "server_status" => "Trove Server Status",
        "game_update" => "Trove Update",
        "challenge_collection" => "Collection Challenge",
        "challenge_rampage" => "Rampage Alert",
        "challenge_racing" => "Racing Challenge",
        "challenge_target" => "Target Challenge",
        "challenge_dungeon" => "Dungeon Challenge",
        _ => return None,
    })
}

/// `_ANN_ACCENT`, GREEN for anything else.
fn ann_accent(kind: &str) -> [u8; 3] {
    match kind {
        "hourly_challenge" | "challenge_dungeon" => [255, 152, 0],
        "chaos_chest" => [183, 148, 246],
        "daily_bonuses" | "stampy" => [255, 184, 107],
        "longshade" => [155, 138, 251],
        "wild_mana" => [70, 211, 154],
        "corruxion" => [155, 93, 229],
        "fluxion" => [76, 201, 240],
        "luxion" => [245, 197, 66],
        "game_update" => [94, 198, 255],
        "challenge_collection" => [255, 193, 7],
        "challenge_rampage" => [244, 67, 54],
        "challenge_racing" => [33, 150, 243],
        "challenge_target" => [156, 39, 176],
        _ => GREEN,
    }
}

/// Python's `str.title()`.
pub fn py_title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_cased = false;
    for c in s.chars() {
        let cased = c.is_lowercase() || c.is_uppercase();
        if cased {
            if prev_cased {
                out.extend(c.to_lowercase());
            } else {
                out.extend(c.to_uppercase());
            }
        } else {
            out.push(c);
        }
        prev_cased = cased;
    }
    out
}

/// What `_announcement_content(kind)` reads, by the name of its source.
pub fn sources(kind: &str) -> &'static [&'static str] {
    match kind {
        k if k == "hourly_challenge" || k.starts_with("challenge_") => &["challenge"],
        "chaos_chest" => &["chaos"],
        "daily_bonuses" => &["daily", "server"],
        "longshade" => &["biomes"],
        "wild_mana" => &["wild_mana"],
        "stampy" => &["stampy"],
        "corruxion" => &["corruxion"],
        "fluxion" => &["fluxion"],
        "luxion" => &["luxion"],
        "server_status" => &["status"],
        "game_update" => &["game_update"],
        _ => &[],
    }
}

fn names_of(cur: &Value) -> Result<Vec<String>, ImageError> {
    let mut out = Vec::new();
    if let Some(Value::Array(list)) = get(cur, "biomes")?.filter(|v| py_truthy(v)) {
        for b in list {
            let n = match get(b, "final_name")?.filter(|v| py_truthy(v)) {
                Some(v) => Some(v.clone()),
                None => get(b, "name")?.cloned(),
            };
            if let Some(n) = n.filter(py_truthy) {
                out.push(text_of(&n)?);
            }
        }
    }
    Ok(out)
}

/// `_announcement_content(kind)` over `data` (the sources it names): the
/// title, accent and lines (headline first). `game_update` is the newest
/// archived live-us version as `{version_tag, files_added, files_modified,
/// files_removed}`, or null.
pub fn announcement_content(
    tr: &Tr,
    kind: &str,
    data: &Value,
    now: i64,
) -> Result<(String, [u8; 3], Vec<String>), ImageError> {
    let mut title = tr.t(&match ann_title(kind) {
        Some(t) => t.to_owned(),
        None => py_title(&kind.replace('_', " ")),
    });
    let mut accent = ann_accent(kind);
    let src = |k: &str| data.get(k).ok_or(bad("missing source"));
    let mut lines: Vec<String> = Vec::new();
    if kind == "hourly_challenge" || kind.starts_with("challenge_") {
        let c = src("challenge")?;
        let sub = if truthy(get(c, "active")?) {
            when(tr, "Ends {when}", rel_coarse, get(c, "ends_at")?, now)?
        } else {
            when(tr, "Next window {when}", rel_coarse, get(c, "starts_at")?, now)?
        };
        lines.push(match get(c, "name")?.filter(|v| py_truthy(v)) {
            Some(n) => text_of(n)?,
            None => tr.t("No capture yet"),
        });
        if let Some(t) = get(c, "type")?.filter(|v| py_truthy(v)) {
            lines.push(tr.tf("{type} challenge", &[("type", Arg::Str(&py_title(&text_of(t)?)))])?);
        }
        lines.push(sub);
    } else if kind == "chaos_chest" {
        let c = src("chaos")?;
        match get(&or_empty(get(c, "item")?), "name")?.filter(|v| py_truthy(v)) {
            Some(item) => {
                lines.push(text_of(item)?);
                lines.push(when(tr, "Rotates {when}", rel_coarse, get(c, "ends_at")?, now)?);
            }
            None => lines.push(tr.t("Not relayed yet")),
        }
    } else if kind == "daily_bonuses" {
        let d = or_empty(get(&or_empty(Some(src("daily")?)), "current")?);
        lines.push(match get(&d, "name")?.filter(|v| py_truthy(v)) {
            Some(n) => text_of(n)?,
            None => tr.t("Daily bonus"),
        });
        if let Some(Value::Array(b)) = get(&d, "normal_buffs")?.filter(|v| py_truthy(v)) {
            for x in b.iter().take(2) {
                lines.push(text_of(x)?);
            }
        }
        let server = src("server")?;
        let reset = server.get("daily_reset_at").ok_or(bad("KeyError"))?;
        lines.push(when(tr, "Resets {when}", rel_coarse, Some(reset), now)?);
    } else if kind == "longshade" || kind == "wild_mana" {
        let cur = or_empty(get(src(if kind == "longshade" { "biomes" } else { "wild_mana" })?, "current")?);
        let names = names_of(&cur)?;
        let rotate = when(tr, "Rotates {when}", rel_coarse, get(&cur, "ends_at")?, now)?;
        let shown = if names.is_empty() { vec!["—".to_owned()] } else { names };
        if kind == "longshade" {
            title = tr.t("Long Shade Rotation");
            lines.push(tr.t("D15 Biomes"));
        }
        lines.extend(shown);
        lines.push(rotate);
    } else if kind == "stampy" {
        let cur = or_empty(get(src("stampy")?, "current")?);
        let first = match get(&cur, "biomes")?.filter(|v| py_truthy(v)) {
            Some(Value::Array(list)) => list.first().cloned().unwrap_or(Value::Object(Map::new())),
            Some(_) => return Err(bad("biomes")),
            None => Value::Object(Map::new()),
        };
        let name = match get(&first, "final_name")?.filter(|v| py_truthy(v)) {
            Some(v) => Some(v.clone()),
            None => get(&first, "name")?.filter(|v| py_truthy(v)).cloned(),
        };
        lines.push(match name {
            Some(n) => text_of(&n)?,
            None => tr.t("Stampy event"),
        });
        lines.push(when(tr, "Ends {when}", rel_coarse, get(&cur, "ends_at")?, now)?);
    } else if kind == "corruxion" || kind == "fluxion" {
        let (a, b) = merchant_lines(tr, src(kind)?, now, rel_coarse)?;
        lines.push(a);
        lines.push(b);
    } else if kind == "luxion" {
        let m = src("luxion")?;
        if truthy(get(m, "active")?) {
            let win = get(m, "current_window")?.filter(|v| py_truthy(v));
            let nxt = get(m, "next_window")?.filter(|v| py_truthy(v));
            if let Some(win) = win {
                lines.push(tr.t("Rotation open"));
                lines.push(when(tr, "Closes {when}", rel_coarse, get(win, "ends_at")?, now)?);
            } else if let Some(nxt) = nxt {
                lines.push(tr.t("Trials started"));
                lines.push(when(tr, "Next rotation {when}", rel_coarse, get(nxt, "starts_at")?, now)?);
            } else {
                lines.push(tr.t("Trials started"));
                lines.push(when(tr, "Ends {when}", rel_coarse, get(m, "ends_at")?, now)?);
            }
        } else {
            lines.push(tr.t("Away"));
            lines.push(tr.t("Returns every ~4 weeks"));
        }
    } else if kind == "server_status" {
        let s = or_empty(Some(src("status")?));
        let overall = hashable(get(&s, "overall")?.cloned().unwrap_or_else(|| "unknown".into()))?;
        accent = st_color(&overall);
        let envs = or_empty(get(&s, "environments")?);
        lines.push(tr.t(st_overall(&overall)));
        for (lbl, k) in [("EU", "eu"), ("US", "us"), ("PTS", "pts")] {
            let env = or_empty(get(&envs, k)?);
            let st = hashable(get(&env, "status")?.cloned().unwrap_or_else(|| "unknown".into()))?;
            lines.push(format!("{lbl}: {}", tr.t(st_label(&st))));
        }
    } else if kind == "game_update" {
        match src("game_update")? {
            Value::Null => lines.push(tr.t("No update archived yet")),
            v => {
                let n = |k: &str| v.get(k).and_then(Value::as_i64).ok_or(bad("version"));
                lines.push(text_of(v.get("version_tag").ok_or(bad("version"))?)?);
                let args = [
                    ("a", Arg::Int(n("files_added")?)),
                    ("m", Arg::Int(n("files_modified")?)),
                    ("r", Arg::Int(n("files_removed")?)),
                ];
                lines.push(tr.tf("{a} added, {m} modified, {r} removed", &args)?);
            }
        }
    }
    Ok((title, accent, lines))
}

/// `_draw_announcement(title, accent, lines)`.
pub fn draw_announcement(
    lib: &Library,
    fonts: &Fonts,
    loc: &Locale,
    title: &str,
    accent: [u8; 3],
    lines: &[String],
    hhmm: &str,
) -> Result<Rgba, ImageError> {
    let mut img = Rgba::new(ANN_W, ANN_H, rgba(BG, 255));
    let mut d = Draw::new(&mut img);
    let mut tx = loc.text(lib, fonts);
    let m = 64.0;
    let acc: Ink = rgba(accent, 255);
    d.rectangle([0.0, 0.0, ANN_W as f64, 6.0], Some(acc), None, 1)?;
    d.ellipse([m, 54.0, m + 18.0, 72.0], Some(acc), None, 1)?;
    tx.fit_text(
        &mut d,
        (m + 30.0, 50.0),
        &title.to_uppercase(),
        1200 - (64 + 30) - 64,
        26,
        rgba(MUTE, 255),
        true,
        false,
        None,
    )?;
    let headline = lines.first().map(String::as_str).unwrap_or("—");
    tx.fit_text(&mut d, (m, 90.0), headline, 1200 - 2 * 64, 64, acc, true, false, None)?;
    let mut y = 188.0;
    for ln in lines.iter().skip(1).take(4) {
        tx.fit_text(&mut d, (m, y), ln, 1200 - 2 * 64, 32, rgba(TEXT, 255), false, false, None)?;
        y += 44.0;
    }
    let h = ANN_H as f64;
    tx.draw(&mut d, (m, h - 46.0), "trove.aallyn.net", 23, false, rgba(MUTE, 255))?;
    let lw = tx.w("trove.aallyn.net", 23, false)?;
    let fr = loc.tr.tf("Updated {time} UTC", &[("time", Arg::Str(hhmm))])?;
    tx.fit_text(
        &mut d,
        (ANN_W as f64 - m, h - 46.0),
        &fr,
        (1200 - 64) - (64 + lw + 24),
        23,
        rgba(MUTE, 210),
        false,
        true,
        None,
    )?;
    Ok(img)
}
