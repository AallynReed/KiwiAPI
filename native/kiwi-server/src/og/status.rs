//! `og_image._draw_status` (`GET /status/og.png`) and the embeddable status
//! badge (`GET /embed/status.svg`).

use axum::{
    extract::State,
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
};
use kiwi_pil::{Draw, ImageError, Library, Rgba};
use serde_json::{Map, Value};

use super::{
    H, SiteGate, W,
    activity::{BG, MUTE, TEXT, rgba},
    cache_get, cache_put, draw_blocking, png_reply,
    text::{Fonts, Text},
};
use crate::{error::ApiError, routes::trove_status, state::AppState};

const UPTIME_DAYS: i64 = 30;

/// `_ST_COLOR` with the "unknown" default.
fn st_color(st: &str) -> [u8; 3] {
    match st {
        "online" => [93, 208, 120],
        "down" | "maintenance" => [248, 81, 73],
        _ => [154, 164, 178],
    }
}

fn st_label(st: &str) -> &'static str {
    match st {
        "online" => "Online",
        "down" | "maintenance" => "Down",
        _ => "Unknown",
    }
}

fn st_overall(st: &str) -> &'static str {
    match st {
        "online" => "All systems online",
        "down" | "maintenance" => "Servers down",
        _ => "Status unknown",
    }
}

/// A dict lookup key as Python sees it: unhashable values raise.
fn key_of(v: Option<&Value>, default: &str) -> Result<String, ImageError> {
    match v {
        None => Ok(default.to_owned()),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(Value::Array(_) | Value::Object(_)) => Err(ImageError::Value("unhashable type")),
        Some(_) => Ok(String::new()),
    }
}

/// `x or {}` then `.get(...)`: falsy is empty, a non-dict raises.
fn dict_or_empty(v: Option<&Value>) -> Result<Map<String, Value>, ImageError> {
    match v {
        Some(Value::Object(m)) => Ok(m.clone()),
        None | Some(Value::Null) => Ok(Map::new()),
        Some(Value::Bool(false)) => Ok(Map::new()),
        Some(Value::String(s)) if s.is_empty() => Ok(Map::new()),
        Some(Value::Array(a)) if a.is_empty() => Ok(Map::new()),
        Some(Value::Number(n)) if n.as_f64() == Some(0.0) => Ok(Map::new()),
        _ => Err(ImageError::Value("'object' has no attribute 'get'")),
    }
}

/// Python's `f"{x:.2f}"`.
pub fn fixed2(x: f64) -> String {
    format!("{x:.2}")
}

/// The footer's "Checked …" text from `checked_at` (falsy: "Live status").
fn checked_text(ca: Option<&Value>, now: i64) -> Result<String, ImageError> {
    let ca = match ca {
        None | Some(Value::Null) | Some(Value::Bool(false)) => return Ok("Live status".into()),
        Some(Value::Bool(true)) => 1,
        Some(Value::Number(n)) => match n.as_i64() {
            Some(0) => return Ok("Live status".into()),
            Some(i) => i,
            None => {
                let f = n.as_f64().unwrap_or(0.0);
                if f == 0.0 {
                    return Ok("Live status".into());
                }
                f.trunc() as i64
            }
        },
        Some(Value::String(s)) if s.is_empty() => return Ok("Live status".into()),
        Some(Value::String(s)) => {
            s.trim().parse::<i64>().map_err(|_| ImageError::Value("invalid literal for int()"))?
        }
        Some(_) => return Err(ImageError::Value("int() argument must be a number")),
    };
    let ago = (now - ca).max(0);
    let rel = if ago < 60 {
        format!("{ago}s ago")
    } else if ago < 3600 {
        format!("{}m ago", ago / 60)
    } else {
        format!("{}h ago", ago / 3600)
    };
    Ok(format!("Checked {rel}"))
}

/// The card as RGBA pixels. `uptimes` are eu, us, pts.
pub fn draw(
    lib: &Library,
    fonts: &Fonts,
    payload: &Value,
    uptimes: [Option<f64>; 3],
    now: i64,
) -> Result<Rgba, ImageError> {
    let mut img = Rgba::new(W, H, rgba(BG, 255));
    let mut d = Draw::new(&mut img);
    let mut tx = Text::new(lib, fonts);
    let m = 64.0;
    let overall = key_of(payload.get("overall"), "unknown")?;
    let oc = st_color(&overall);

    d.rectangle([0.0, 0.0, W as f64, 6.0], Some(rgba(oc, 255)), None, 1)?;
    let cy = 82.0;
    d.ellipse([m, cy - 8.0, m + 16.0, cy + 8.0], Some(rgba(oc, 255)), None, 1)?;
    tx.fit_text(
        &mut d,
        (m + 28.0, cy - 16.0),
        "TROVE PC SERVER STATUS",
        1200 - 64 - (64 + 28),
        26,
        rgba(MUTE, 255),
        true,
        false,
        None,
    )?;

    let environments = dict_or_empty(payload.get("environments"))?;
    let mut partial = false;
    if overall == "down" {
        for k in ["eu", "us"] {
            if dict_or_empty(environments.get(k))?.get("status") == Some(&Value::String("online".into())) {
                partial = true;
                break;
            }
        }
    }
    let overall_text = if partial { "Partial outage" } else { st_overall(&overall) };
    tx.fit_text(&mut d, (m, 118.0), overall_text, 1200 - 2 * 64, 86, rgba(oc, 255), true, false, Some(46))?;

    let (gap, y0, ch) = (28, 330, 170);
    let cw = (1200 - 2 * 64 - 2 * gap) / 3;
    for (i, (label, key)) in [("EU", "eu"), ("US", "us"), ("PTS", "pts")].into_iter().enumerate() {
        let x0 = 64 + i as i64 * (cw + gap);
        let st = key_of(dict_or_empty(environments.get(key))?.get("status"), "unknown")?;
        let sc = st_color(&st);
        let (xf, yf) = (x0 as f64, y0 as f64);
        d.rounded_rectangle(
            [xf, yf, xf + cw as f64, yf + ch as f64],
            20.0,
            Some([22, 27, 34, 255]),
            Some(rgba(sc, 130)),
            2,
        )?;
        d.ellipse([xf + 30.0, yf + 32.0, xf + 52.0, yf + 54.0], Some(rgba(sc, 255)), None, 1)?;
        tx.draw(&mut d, (xf + 66.0, yf + 22.0), label, 40, true, rgba(TEXT, 255))?;
        tx.fit_text(&mut d, (xf + 30.0, yf + 80.0), st_label(&st), cw - 60, 30, rgba(sc, 255), false, false, None)?;
        let up_txt = match uptimes[i] {
            Some(u) => format!("{}% · {UPTIME_DAYS}d uptime", fixed2(u * 100.0)),
            None => format!("uptime n/a · {UPTIME_DAYS}d"),
        };
        tx.fit_text(&mut d, (xf + 30.0, yf + 124.0), &up_txt, cw - 60, 23, rgba(MUTE, 255), false, false, None)?;
    }

    let site = "trove.aallyn.net/status";
    tx.draw(&mut d, (m, H as f64 - 54.0), site, 24, false, rgba(MUTE, 255))?;
    let lw = tx.w(site, 24, false)?;
    let fr = checked_text(payload.get("checked_at"), now)?;
    tx.fit_text(
        &mut d,
        (W as f64 - m, H as f64 - 54.0),
        &fr,
        (1200 - 64) - (64 + lw + 24),
        24,
        rgba(MUTE, 210),
        false,
        true,
        None,
    )?;
    Ok(img)
}

/// `(await get_history(env, 30)).get("uptime")`, `None` on any failure.
async fn uptime(state: &AppState, env: &'static str) -> Option<f64> {
    let rows = trove_status::load_segments(state, env).await.ok()?;
    kiwi_core::status::history(env, UPTIME_DAYS, trove_status::unix_now(), &rows).uptime
}

/// `GET /status/og.png` (English; other languages stay in Python).
pub async fn og(_: SiteGate, State(state): State<AppState>) -> Response {
    let key = "status:en";
    if let Some(png) = cache_get(key, 45.0) {
        return png_reply(Some(png), "public, max-age=60");
    }
    let payload = super::data::status(&state).await;
    let uptimes = [uptime(&state, "eu").await, uptime(&state, "us").await, uptime(&state, "pts").await];
    let now = trove_status::unix_now();
    let png = draw_blocking("status OG image", move |lib, fonts| draw(lib, fonts, &payload, uptimes, now)).await;
    if let Some(png) = &png {
        cache_put(key, png.clone());
    }
    png_reply(png, "public, max-age=60")
}

/// `_badge_svg(label, value, color)`.
pub fn badge_svg(label: &str, value: &str, color: &str) -> String {
    let w = |s: &str| (s.chars().count() as f64 * 6.5) as i64 + 12;
    let (lw, vw) = (w(label), w(value));
    let total = lw + vw;
    let (lx, vx) = (lw * 5, (lw * 2 + vw) * 5);
    format!(
        concat!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{total}" height="20" role="img" aria-label="{label}: {value}">"#,
            r##"<linearGradient id="s" x2="0" y2="100%"><stop offset="0" stop-color="#bbb" stop-opacity=".1"/>"##,
            r#"<stop offset="1" stop-opacity=".1"/></linearGradient>"#,
            r##"<clipPath id="r"><rect width="{total}" height="20" rx="3" fill="#fff"/></clipPath>"##,
            r##"<g clip-path="url(#r)"><rect width="{lw}" height="20" fill="#2b3038"/>"##,
            r#"<rect x="{lw}" width="{vw}" height="20" fill="{color}"/>"#,
            r##"<rect width="{total}" height="20" fill="url(#s)"/></g>"##,
            r##"<g fill="#fff" text-anchor="middle" font-family="Verdana,Geneva,DejaVu Sans,sans-serif" font-size="110">"##,
            r#"<text x="{lx}" y="140" transform="scale(.1)" textLength="{lt}">{label}</text>"#,
            r#"<text x="{vx}" y="140" transform="scale(.1)" textLength="{vt}">{value}</text>"#,
            "</g></svg>"
        ),
        total = total,
        label = label,
        value = value,
        lw = lw,
        vw = vw,
        color = color,
        lx = lx,
        vx = vx,
        lt = (lw - 12) * 10,
        vt = (vw - 12) * 10,
    )
}

/// `GET /embed/status.svg`.
pub async fn badge(_: SiteGate, State(state): State<AppState>) -> Result<Response, ApiError> {
    let snap = super::data::status(&state).await;
    let overall = match snap.get("overall") {
        None => "unknown",
        Some(Value::String(s)) => s.as_str(),
        Some(Value::Array(_) | Value::Object(_)) => return Err(ApiError::internal()),
        Some(_) => "unknown",
    };
    let (value, color) = match overall {
        "online" => ("online", "#3fb950"),
        "down" => ("offline", "#f85149"),
        _ => ("unknown", "#9aa4b2"),
    };
    Ok((
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("image/svg+xml")),
            (header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=60")),
            (header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*")),
        ],
        badge_svg("trove", value, color),
    )
        .into_response())
}
