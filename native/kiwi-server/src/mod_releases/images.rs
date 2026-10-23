//! `get_image(sha, width)` behind `/v1/mods/hub/image/{sha}` and
//! `/site/mods/image/{sha}`: the stored image, or a width variant already
//! recorded in `mod_images.variants`. A width that still has to be rendered
//! goes to Python (see `defers`).

use axum::{
    extract::State,
    http::{HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use mongodb::bson::{Bson, Document, doc};

use super::{ModsAsset, SiteMods, access, read::blob};
use crate::{
    error::ApiError,
    features::{ModsHub, RequireFeature},
    state::AppState,
    validation::{FromParams, Params, Path, parse_qsl, str_as_int},
};

pub const COLLECTION: &str = "mod_images";
pub const THUMB_WIDTHS: [i64; 4] = [200, 400, 708, 1416];
const IMMUTABLE: &str = "public, max-age=31536000, immutable";

pub struct ImgQ {
    sha: String,
    w: Option<i64>,
}

impl FromParams for ImgQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        ImgQ { sha: p.path("sha").str().required(), w: p.query("w").int().optional() }
    }
}

fn bad_width() -> String {
    format!("w must be one of {}", THUMB_WIDTHS.map(|w| w.to_string()).join(", "))
}

struct Asset {
    content_type: String,
    variants: Vec<(String, String)>,
}

fn parse_asset(d: &Document) -> Result<Asset, ApiError> {
    let content_type = match d.get("content_type") {
        Some(Bson::String(s)) => s.clone(),
        _ => return Err(access::internal("mod_images: invalid document")),
    };
    let variants = match d.get("variants") {
        None => Vec::new(),
        Some(Bson::Document(v)) => {
            let mut out = Vec::new();
            for (k, val) in v {
                match val {
                    Bson::String(s) => out.push((k.clone(), s.clone())),
                    _ => return Err(access::internal("mod_images: invalid variants")),
                }
            }
            out
        }
        Some(_) => return Err(access::internal("mod_images: invalid variants")),
    };
    Ok(Asset { content_type, variants })
}

enum Got {
    Image(Vec<u8>, String),
    Missing,
    Render,
}

async fn lookup(state: &AppState, sha: &str, width: Option<i64>) -> Result<Got, ApiError> {
    let Some(d) = access::find_one(state, COLLECTION, doc! {"sha": sha}).await? else { return Ok(Got::Missing) };
    let asset = parse_asset(&d)?;
    if let Some(w) = width
        && let Some((_, variant)) = asset.variants.iter().find(|(k, _)| *k == w.to_string())
        && let Some(cached) = blob(state, variant).await?
    {
        return Ok(Got::Image(cached, "image/webp".into()));
    }
    let Some(data) = blob(state, sha).await? else { return Ok(Got::Missing) };
    if width.is_none() {
        return Ok(Got::Image(data, asset.content_type));
    }
    Ok(Got::Render)
}

/// For `routes::defer_to_python`: the request would render a new variant.
pub async fn defers(state: &AppState, uri: &Uri) -> bool {
    let Some(raw) = uri.path().rsplit('/').next() else { return false };
    let sha = percent_encoding::percent_decode_str(raw).decode_utf8_lossy().into_owned();
    let pairs = uri.query().map(parse_qsl).unwrap_or_default();
    let Some((_, w)) = pairs.iter().rev().find(|(k, _)| k == "w") else { return false };
    let Ok(w) = str_as_int(w) else { return false };
    let Ok(w) = i64::try_from(w) else { return false };
    if !THUMB_WIDTHS.contains(&w) {
        return false;
    }
    matches!(lookup(state, &sha, Some(w)).await, Ok(Got::Render))
}

fn reply(got: Got) -> Response {
    match got {
        Got::Image(data, ct) => {
            let ct = if ct.starts_with("text/") && !ct.to_lowercase().contains("charset=") {
                format!("{ct}; charset=utf-8")
            } else {
                ct
            };
            let ct = HeaderValue::from_str(&ct).unwrap_or(HeaderValue::from_static("application/octet-stream"));
            ([(header::CONTENT_TYPE, ct), (header::CACHE_CONTROL, HeaderValue::from_static(IMMUTABLE))], data)
                .into_response()
        }
        Got::Missing | Got::Render => ApiError::internal().into_response(),
    }
}

pub async fn v1_image(
    State(state): State<AppState>,
    _: RequireFeature<ModsHub>,
    _: ModsAsset,
    Path(q): Path<ImgQ>,
) -> Response {
    if q.w.is_some_and(|w| !THUMB_WIDTHS.contains(&w)) {
        return ApiError::bad_request(bad_width()).into_response();
    }
    match lookup(&state, &q.sha, q.w).await {
        Ok(Got::Missing) => ApiError::not_found("Image not found").into_response(),
        Ok(got) => reply(got),
        Err(e) => e.into_response(),
    }
}

pub async fn site_image(State(state): State<AppState>, _: SiteMods, Path(q): Path<ImgQ>) -> Response {
    if q.w.is_some_and(|w| !THUMB_WIDTHS.contains(&w)) {
        return ApiError::http(StatusCode::BAD_REQUEST, Some(&bad_width())).into_response();
    }
    match lookup(&state, &q.sha, q.w).await {
        Ok(Got::Missing) => ApiError::http(StatusCode::NOT_FOUND, Some("Image not found")).into_response(),
        Ok(got) => reply(got),
        Err(e) => e.into_response(),
    }
}
