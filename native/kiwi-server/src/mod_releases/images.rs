//! `get_image(sha, width)` behind `/v1/mods/hub/image/{sha}` and
//! `/site/mods/image/{sha}`: the stored image, or a WebP width variant
//! (`store.render_thumbnail`), rendered once and recorded in
//! `mod_images.variants`.

use axum::{
    extract::State,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use mongodb::bson::{Bson, Document, doc};

use super::{ModsAsset, SiteMods, access, read::blob};
use crate::{
    error::ApiError,
    features::{ModsHub, RequireFeature},
    images::pil::{self, Format},
    media::{self, pil as mpil},
    state::AppState,
    validation::{FromParams, Params, Path},
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
    let Some(w) = width else { return Ok(Got::Image(data, asset.content_type)) };
    let (src, width) = (data.clone(), w as usize);
    let thumb = tokio::task::spawn_blocking(move || render_thumbnail(&src, width))
        .await
        .map_err(access::internal)?
        .map_err(|()| access::internal("render_thumbnail raised"))?;
    let Some(thumb) = thumb else { return Ok(Got::Image(data, asset.content_type)) };
    let store = media::mods_store(state);
    let stored = thumb.clone();
    let (thumb_sha, _) = tokio::task::spawn_blocking(move || store.put(&stored))
        .await
        .map_err(access::internal)?
        .map_err(access::internal)?;
    let id = d.get("_id").cloned().unwrap_or(Bson::Null);
    let db = state.inner.db.mongo.as_ref().ok_or_else(|| access::internal("MONGO_URI not configured"))?;
    db.collection::<Document>(COLLECTION)
        .update_one(doc! {"_id": id}, doc! {"$set": {format!("variants.{w}"): thumb_sha}})
        .await
        .map_err(access::internal)?;
    Ok(Got::Image(thumb, "image/webp".into()))
}

/// `store.render_thumbnail(data, width)`: a WebP no wider than `width`, or
/// `None` to serve the original; `Err` where Python raises.
pub fn render_thumbnail(data: &[u8], width: usize) -> Result<Option<Vec<u8>>, ()> {
    let Some(img) = pil::open(data) else { return Ok(None) };
    let resize = img.width > width;
    if pil::n_frames(data, img.format)? > 1 || (!resize && img.format == Format::Webp) {
        return Ok(None);
    }
    let (w, h) = if resize {
        let h = (img.height as f64 * width as f64 / img.width as f64).round_ties_even() as usize;
        (width, h.max(1))
    } else {
        (img.width, img.height)
    };
    let pixels = if !resize {
        img.rgba
    } else if img.has_alpha {
        resize_straight_alpha(&img.rgba, img.width, img.height, w, h)
    } else {
        let mut rgb = img.rgba;
        for px in rgb.chunks_exact_mut(4) {
            px[3] = 255;
        }
        mpil::resample::lanczos(&rgb, img.width, img.height, w, h, [0.0, 0.0, img.width as f32, img.height as f32])
    };
    let Some(out) = pil::webp_encode(&pixels, w, h, img.has_alpha, 82.0, 4) else { return Ok(None) };
    Ok((resize || out.len() < data.len() * 3 / 4).then_some(out))
}

/// `_resize_straight_alpha`: numpy's float32 premultiply, Pillow's own RGBA
/// LANCZOS (which premultiplies again), then the float32 un-premultiply.
fn resize_straight_alpha(rgba: &[u8], iw: usize, ih: usize, w: usize, h: usize) -> Vec<u8> {
    let mut arr = rgba.to_vec();
    for px in arr.chunks_exact_mut(4) {
        let q = f32::from(px[3]) / 255.0;
        for c in &mut px[..3] {
            *c = (f32::from(*c) * q) as u8;
        }
    }
    mpil::premultiply(&mut arr);
    let mut small = mpil::resample::lanczos(&arr, iw, ih, w, h, [0.0, 0.0, iw as f32, ih as f32]);
    mpil::unpremultiply(&mut small);
    for px in small.chunks_exact_mut(4) {
        let a = f32::from(px[3]);
        for c in &mut px[..3] {
            *c = if a > 0.0 { ((f32::from(*c) * 255.0) / a).clamp(0.0, 255.0) as u8 } else { 0 };
        }
    }
    small
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
        Got::Missing => ApiError::internal().into_response(),
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
