//! Server-rendered social cards (`app/site/og_image.py` and its routes in
//! `app/site/router.py`), drawn pixel-exact to Pillow 13 through kiwi-pil.
//!
//! Only English renders here: a `lang` that resolves to another language goes
//! to Python (`defers`), as does every card while DejaVu is not on disk.

pub mod activity;
pub mod board;
pub mod data;
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
        .route("/board.png", get(board_png))
        .route("/announce.png", get(announce_png))
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
    let other_lang = || normalize_lang(&query_last(uri, "lang").unwrap_or_else(|| "en".into())) != "en";
    match matched {
        "/activity/og.png" => text::fonts().is_none(),
        "/status/og.png" | "/board.png" => text::fonts().is_none() || other_lang(),
        "/announce.png" => {
            text::fonts().is_none() || other_lang() || query_last(uri, "kind").as_deref() == Some("game_update")
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
    draw_blocking("activity OG image", move |lib, fonts| activity::draw(lib, fonts, &inputs, period)).await
}

/// Draws off the async runtime and encodes the PNG; `None` is a failed render.
pub async fn draw_blocking<F>(what: &'static str, f: F) -> Option<Bytes>
where
    F: FnOnce(&kiwi_pil::Library, &'static text::Fonts) -> Result<kiwi_pil::Rgba, kiwi_pil::ImageError>
        + Send
        + 'static,
{
    let fonts = text::fonts()?;
    tokio::task::spawn_blocking(move || {
        let lib = kiwi_pil::Library::new().map_err(kiwi_pil::ImageError::from)?;
        f(&lib, fonts).map(|img| encode(&img))
    })
    .await
    .ok()?
    .map_err(|e| tracing::error!("{what} render failed: {e}"))
    .ok()
}

/// The cached base64 PNG (`redis.get`), if any; Redis trouble is a miss.
async fn redis_png(state: &AppState, key: &str) -> Option<Bytes> {
    use base64::Engine;
    let mut conn = state.inner.db.redis.clone()?;
    let cached: Option<String> = match redis::cmd("GET").arg(key).query_async(&mut conn).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("og image: Redis read failed: {e}");
            return None;
        }
    };
    let cached = cached.filter(|c| !c.is_empty())?;
    match base64::engine::general_purpose::STANDARD.decode(cached.trim()) {
        Ok(png) => Some(Bytes::from(png)),
        Err(e) => {
            tracing::warn!("og image: Redis read failed: {e}");
            None
        }
    }
}

/// `redis.set(key, base64(png), ex=120)`, best-effort.
async fn redis_put_png(state: &AppState, key: &str, png: &[u8]) {
    use base64::Engine;
    let Some(mut conn) = state.inner.db.redis.clone() else { return };
    let b64 = base64::engine::general_purpose::STANDARD.encode(png);
    let res: redis::RedisResult<()> =
        redis::cmd("SET").arg(key).arg(b64).arg("EX").arg(120).query_async(&mut conn).await;
    if let Err(e) = res {
        tracing::warn!("og image: Redis write failed: {e}");
    }
}

/// `int(time.time())`, the minute bucket and the "Updated HH:MM" stamp.
fn clock() -> (i64, i64, String) {
    let now = crate::activity::unix_now_f64();
    let hhmm = chrono::DateTime::<chrono::Utc>::from_timestamp(now.floor() as i64, 0)
        .unwrap_or_default()
        .format("%H:%M")
        .to_string();
    (now.trunc() as i64, (now / 60.0).floor() as i64, hhmm)
}

fn now_us() -> i64 {
    crate::dt::now().timestamp_micros()
}

/// `GET /board.png` (English; `v` is only the bot's cache-buster).
async fn board_png(_: SiteGate, State(state): State<AppState>) -> Response {
    let (now, minute, hhmm) = clock();
    let key = format!("board:img:{minute}:en");
    if let Some(png) = redis_png(&state, &key).await {
        return png_reply(Some(png), "public, max-age=60");
    }
    let data = match data::board(&state, now_us()).await {
        Ok(d) => d,
        Err(e) => {
            tracing::error!("board image render failed: {e:#}");
            return png_reply(None, "public, max-age=60");
        }
    };
    let png = draw_blocking("board image", move |lib, fonts| board::draw_board(lib, fonts, &data, now, &hhmm)).await;
    if let Some(png) = &png {
        redis_put_png(&state, &key, png).await;
    }
    png_reply(png, "public, max-age=60")
}

pub struct AnnounceQuery {
    pub kind: String,
}

impl FromParams for AnnounceQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        AnnounceQuery { kind: p.query("kind").str().required() }
    }
}

/// `GET /announce.png?kind=` (English; `game_update` stays in Python).
async fn announce_png(_: SiteGate, State(state): State<AppState>, Query(q): Query<AnnounceQuery>) -> Response {
    let (now, minute, hhmm) = clock();
    let key = format!("announce:img:{}:{minute}:en", q.kind);
    if let Some(png) = redis_png(&state, &key).await {
        return png_reply(Some(png), "public, max-age=60");
    }
    let kind = q.kind;
    let content = match data::announcement(&state, &kind, now_us()).await {
        Ok(d) => board::announcement_content(&kind, &d, now).map_err(|e| anyhow::anyhow!("{e}")),
        Err(e) => Err(e),
    };
    let (title, accent, lines) = match content {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("announcement image render failed for {kind}: {e:#}");
            return png_reply(None, "public, max-age=60");
        }
    };
    let png = draw_blocking("announcement image", move |lib, fonts| {
        board::draw_announcement(lib, fonts, &title, accent, &lines, &hhmm)
    })
    .await;
    if let Some(png) = &png {
        redis_put_png(&state, &key, png).await;
    }
    png_reply(png, "public, max-age=60")
}
