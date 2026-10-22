//! Server-rendered social cards (`app/site/og_image.py` and its routes in
//! `app/site/router.py`), drawn pixel-exact to Pillow 13 through kiwi-pil.
//!
//! Only English renders here: a `lang` that resolves to another language goes
//! to Python (`defers`), as does every card while DejaVu is not on disk.

pub mod activity;
pub mod pymath;
pub mod status;
#[cfg(test)]
mod tests;
pub mod text;

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
};

use axum::{
    Router,
    body::Body,
    extract::{FromRequestParts, State},
    http::{HeaderValue, StatusCode, Uri, header, request::Parts},
    response::{IntoResponse, Response},
    routing::get,
};
use bytes::Bytes;

use crate::{
    error::{self, ApiError},
    features,
    state::AppState,
    validation::{FromParams, Params, Query},
};

pub const W: usize = 1200;
pub const H: usize = 630;

/// `i18n.LANGS`.
const SUPPORTED: [&str; 10] = ["en", "fr", "de", "pt-PT", "es", "ru", "ja", "ko", "zh-CN", "th"];

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/activity/og.png", get(activity_og))
        .route("/status/og.png", get(status::og))
        .route("/embed/status.svg", get(status::badge))
}

/// `normalize_lang`: a supported code or "en".
pub fn normalize_lang(lang: &str) -> &str {
    if SUPPORTED.contains(&lang) { lang } else { "en" }
}

/// The last `name` in the query string, decoded like Starlette's.
fn query_last(uri: &Uri, name: &str) -> Option<String> {
    let q = uri.query()?;
    form_urlencoded::parse(q.as_bytes()).filter(|(k, _)| k == name).map(|(_, v)| v.into_owned()).last()
}

/// For `routes::defer_to_python`: cards Rust can't draw like Python would.
pub fn defers(matched: &str, uri: &Uri) -> bool {
    match matched {
        "/activity/og.png" => text::fonts().is_none(),
        "/status/og.png" => {
            text::fonts().is_none() || normalize_lang(&query_last(uri, "lang").unwrap_or_else(|| "en".into())) != "en"
        }
        _ => false,
    }
}

/// `_resolve_feature_flags` for these paths: every site flag is read, then a
/// disabled feature's card is the framework 404.
pub struct SiteGate;

impl FromRequestParts<AppState> for SiteGate {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        let (mut player, mut status) = (true, true);
        for flag in crate::activity::SITE_FLAGS {
            let on = features::is_enabled(state, flag).await.map_err(|e| {
                tracing::error!("site feature flag {flag}: {e:#}");
                ApiError::internal().into_response()
            })?;
            if flag == features::PLAYER_ACTIVITY_FLAG {
                player = on;
            } else if flag == features::SERVER_STATUS_FLAG {
                status = on;
            }
        }
        let path = parts.uri.path();
        let blocked = (!player && (path == "/activity" || path.starts_with("/activity/")))
            || (!status && (path == "/status" || path.starts_with("/status/")));
        if blocked {
            return Err(error::http_exception(StatusCode::NOT_FOUND, None, &parts.method, path, &parts.headers));
        }
        Ok(SiteGate)
    }
}

/// `og_image._CACHE`: renders shared for a TTL, capped at 32 entries.
static CACHE: LazyLock<Mutex<HashMap<String, (f64, Bytes)>>> = LazyLock::new(Default::default);

pub fn cache_get(key: &str, ttl: f64) -> Option<Bytes> {
    let cache = CACHE.lock().ok()?;
    let (at, png) = cache.get(key)?;
    (crate::activity::unix_now_f64() - at < ttl).then(|| png.clone())
}

pub fn cache_put(key: &str, png: Bytes) {
    let Ok(mut cache) = CACHE.lock() else { return };
    cache.insert(key.to_owned(), (crate::activity::unix_now_f64(), png));
    if cache.len() > 32
        && let Some(oldest) = cache.iter().min_by(|a, b| a.1.0.total_cmp(&b.1.0)).map(|(k, _)| k.clone())
    {
        cache.remove(&oldest);
    }
}

/// The PNG response, or the favicon redirect a failed render answers.
pub fn png_reply(png: Option<Bytes>, cache_control: &'static str) -> Response {
    match png {
        Some(png) => {
            let mut res = Response::new(Body::from(png));
            let h = res.headers_mut();
            h.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
            h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache_control));
            res
        }
        None => (StatusCode::FOUND, [(header::LOCATION, HeaderValue::from_static("/static/assets/favicon.png"))])
            .into_response(),
    }
}

/// Encodes `img.convert("RGB").save(out, "PNG", optimize=True)`.
pub fn encode(img: &kiwi_pil::Rgba) -> Bytes {
    Bytes::from(kiwi_pil::png::encode_rgb(&img.to_rgb(), img.width, img.height))
}

pub struct PeriodQuery {
    pub period: String,
}

impl FromParams for PeriodQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        PeriodQuery { period: p.query("period").str().default("7d".into()) }
    }
}

/// `render_activity_og`'s period: lower-cased, unknown values are "7d".
pub fn activity_period(period: &str) -> &'static str {
    let p = if period.is_empty() { "7d".to_owned() } else { period.to_lowercase() };
    if p == "1m" { "1m" } else { "7d" }
}

/// `GET /activity/og.png`.
async fn activity_og(_: SiteGate, State(state): State<AppState>, Query(q): Query<PeriodQuery>) -> Response {
    let period = activity_period(&q.period);
    let key = format!("activity:{period}:en");
    if let Some(png) = cache_get(&key, 600.0) {
        return png_reply(Some(png), "public, max-age=600");
    }
    let png = render_activity(&state, period).await;
    if let Some(png) = &png {
        cache_put(&key, png.clone());
    }
    png_reply(png, "public, max-age=600")
}

async fn render_activity(state: &AppState, period: &'static str) -> Option<Bytes> {
    let series = crate::activity::player::series(state, period)
        .await
        .map_err(|e| tracing::error!("activity OG image render failed: {e:#}"))
        .ok()?;
    let live = crate::activity::player::estimate_active_players(state, false)
        .await
        .map_err(|e| tracing::error!("activity OG image render failed: {e:#}"))
        .ok()?;
    let inputs = activity::Inputs::from_payloads(&series, &live);
    let fonts = text::fonts()?;
    tokio::task::spawn_blocking(move || {
        let lib = kiwi_pil::Library::new().map_err(kiwi_pil::ImageError::from)?;
        activity::draw(&lib, fonts, &inputs, period).map(|img| encode(&img))
    })
    .await
    .ok()?
    .map_err(|e| tracing::error!("activity OG image render failed: {e}"))
    .ok()
}
