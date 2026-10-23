//! `POST /v1/ocr/character` (`app/trove/router.py` `ocr_character`, `app/trove/ocr`):
//! RapidOCR's PP-OCRv4 models on onnxruntime (the same shared library and
//! ONNX files the Python image ships), with the pipeline in `kiwi_core::ocr`.

pub mod engine;
#[cfg(test)]
mod tests;

use std::sync::OnceLock;

use axum::{
    Router,
    extract::{Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use kiwi_core::{
    form::{self, FormError, FormValue},
    ocr::{
        cv::Img,
        decode::{self, DecodeError},
        pil,
        rapid::{self, Line},
        text::{self, Stat, Vocab},
    },
    pystr,
};
use serde_json::{Map, Value, json};

use crate::{
    auth::{extract::require_scope, scopes::OCR_READ},
    error::ApiError,
    pyjson::PydanticJson,
    state::AppState,
    validation::{ErrorItem, LocItem, ValidationError},
};

pub fn router() -> Router<AppState> {
    Router::new().route("/v1/ocr/character", post(route))
}

const ALLOWED: [&str; 5] = ["image/png", "image/jpeg", "image/webp", "image/gif", "image/bmp"];

/// `vocabulary._load()`: the repo copy of `character_stats.json`, once.
pub fn vocab() -> Option<&'static Vocab> {
    static V: OnceLock<Option<Vocab>> = OnceLock::new();
    V.get_or_init(|| {
        #[cfg(test)]
        let raw = include_str!("../../../../app/trove/gamedata/character_stats.json").to_owned();
        #[cfg(not(test))]
        let raw =
            std::fs::read_to_string(std::path::Path::new(crate::gamedata::BASELINE_DIR).join("character_stats.json"))
                .ok()?;
        Vocab::from_json(&serde_json::from_str(&raw).ok()?)
    })
    .as_ref()
}

/// Why `_process` failed, as the route maps it.
#[derive(Debug, PartialEq)]
pub enum OcrFailure {
    Unavailable,
    BadImage(String),
    Internal(String),
}

/// `engine._run`: lines with text, sorted by their box's top then left.
fn run(inf: &dyn rapid::Infer, img: &Img) -> Result<Vec<Line>, OcrFailure> {
    let mut lines = rapid::run(inf, img).map_err(|e| OcrFailure::Internal(e.0))?;
    lines.retain(|l| !l.text.is_empty() && !pystr::strip(&l.text).is_empty());
    let key = |l: &Line| {
        let ys = l.boxes.iter().map(|p| p.1 as f64).fold(f64::INFINITY, f64::min);
        let xs = l.boxes.iter().map(|p| p.0 as f64).fold(f64::INFINITY, f64::min);
        (ys, xs)
    };
    lines.sort_by(|a, b| key(a).partial_cmp(&key(b)).unwrap_or(std::cmp::Ordering::Equal));
    Ok(lines)
}

/// `engine.ocr_row`: re-OCR a 4x LANCZOS strip right of a label box.
fn ocr_row(inf: &dyn rapid::Infer, img: &Img, b: &rapid::Quad) -> Result<Vec<String>, OcrFailure> {
    let (h, w) = (img.h as i64, img.w as i64);
    let xs: Vec<f64> = b.iter().map(|p| p.0 as f64).collect();
    let ys: Vec<f64> = b.iter().map(|p| p.1 as f64).collect();
    let min = |v: &[f64]| v.iter().copied().fold(f64::INFINITY, f64::min);
    let max = |v: &[f64]| v.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let y0 = 0.max(min(&ys) as i64 - 2);
    let y1 = h.min(max(&ys) as i64 + 2);
    let x0 = 0.max((min(&xs) - 0.05 * w as f64) as i64);
    let x1 = w.min((max(&xs) + 0.30 * w as f64) as i64);
    if y1 <= y0 || x1 <= x0 {
        return Ok(Vec::new());
    }
    let strip = img.crop(x0 as usize, y0 as usize, x1 as usize, y1 as usize);
    let up = pil::resize_lanczos(&strip, ((x1 - x0) * 4).max(1) as usize, ((y1 - y0) * 4).max(1) as usize);
    Ok(run(inf, &up)?.into_iter().map(|l| l.text).collect())
}

/// `engine._decode`.
pub fn decode_upload(data: &[u8]) -> Result<Img, OcrFailure> {
    match decode::decode_rgb(data) {
        Ok(img) => Ok(img),
        Err(DecodeError::Unreadable(msg)) => Err(OcrFailure::BadImage(format!("unreadable image: {msg}"))),
        Err(DecodeError::Bomb) => Err(OcrFailure::Internal("DecompressionBombError".into())),
    }
}

/// `service._process`: the `CharacterStatsOcr` body.
pub fn process(inf: &dyn rapid::Infer, vocab: &Vocab, data: &[u8]) -> Result<Value, OcrFailure> {
    process_image(inf, vocab, &decode_upload(data)?)
}

/// `_process` after the decode (which runs before the engine is touched).
pub fn process_image(inf: &dyn rapid::Infer, vocab: &Vocab, img: &Img) -> Result<Value, OcrFailure> {
    let elems = run(inf, img)?;
    let lines: Vec<String> = elems.iter().map(|l| l.text.clone()).collect();
    let mut stats = text::extract(vocab, &lines);
    let mut missing: Vec<&str> =
        vocab.defs.iter().map(|d| d.key.as_str()).filter(|k| !stats.iter().any(|(s, _)| s == k)).collect();
    for el in &elems {
        if missing.is_empty() {
            break;
        }
        let Some((key, _)) = vocab.match_label(&el.text) else { continue };
        if !missing.contains(&key.as_str()) {
            continue;
        }
        let sd = vocab.stat_def(&key);
        for txt in ocr_row(inf, img, &el.boxes)? {
            let Some((value, pct)) = text::parse_number(&txt) else { continue };
            if sd.is_some_and(|d| d.percent != pct) {
                continue;
            }
            if sd.is_some_and(|d| !(d.min <= value && value <= d.max)) {
                continue;
            }
            let is_int = sd.is_some_and(|d| !d.percent);
            stats.push((
                key.clone(),
                Stat {
                    value: if is_int { value.round_ties_even() } else { value },
                    is_int,
                    unit: if sd.is_some_and(|d| d.percent) { "percent" } else { "count" },
                    raw: pystr::strip(&txt).to_owned(),
                    confidence: 0.7,
                    in_range: true,
                    type_match: true,
                    derived: false,
                },
            ));
            missing.retain(|k| *k != key);
            break;
        }
    }
    if stats.iter().any(|(_, s)| s.is_int && !s.value.is_finite()) {
        return Err(OcrFailure::Internal("OverflowError".into()));
    }
    let mut m = Map::new();
    for (k, s) in &stats {
        m.insert(k.clone(), text::stat_json(s));
    }
    Ok(json!({"stats": m, "matched": stats.len(), "total_known": vocab.defs.len(), "lines": lines}))
}

fn upload_field(items: Vec<(String, FormValue)>) -> Result<form::UploadFile, ValidationError> {
    let item = |kind: &'static str, msg: &str, input: Value, ctx: Option<Map<String, Value>>| ErrorItem {
        kind: kind.into(),
        loc: vec![LocItem::from("body"), LocItem::from("file")],
        msg: msg.into(),
        input,
        ctx,
    };
    match items.into_iter().rev().find(|(k, _)| k == "file").map(|(_, v)| v) {
        Some(FormValue::File(f)) => Ok(f),
        Some(FormValue::Text(t)) if !t.is_empty() => {
            let mut ctx = Map::new();
            ctx.insert("error".into(), Value::Object(Map::new()));
            Err(ValidationError(vec![item(
                "value_error",
                "Value error, Expected UploadFile, received: <class 'str'>",
                Value::String(t),
                Some(ctx),
            )]))
        }
        _ => Err(ValidationError(vec![item("missing", "Field required", Value::Null, None)])),
    }
}

/// Everything after the scope dependency: field validation, the type and
/// emptiness checks, then the OCR.
pub async fn reply(items: Vec<(String, FormValue)>) -> Result<Value, ApiError> {
    let file = upload_field(items)?;
    let ctype = file.content_type().unwrap_or_default().to_lowercase();
    if !ctype.is_empty() && !ALLOWED.contains(&ctype.as_str()) {
        return Err(ApiError::bad_request(format!(
            "Unsupported image type '{ctype}'. Allowed: PNG, JPEG, WebP, GIF, BMP."
        )));
    }
    if file.data.is_empty() {
        return Err(ApiError::bad_request("Empty upload - attach a screenshot in the 'file' field."));
    }
    let result = tokio::task::spawn_blocking(move || -> Result<Value, OcrFailure> {
        let img = decode_upload(&file.data)?;
        drop(file);
        let inf = engine::get().ok_or(OcrFailure::Unavailable)?;
        let vocab = vocab().ok_or_else(|| OcrFailure::Internal("character_stats.json".into()))?;
        process_image(inf, vocab, &img)
    })
    .await
    .map_err(|_| ApiError::internal())?;
    match result {
        Ok(v) => Ok(v),
        Err(OcrFailure::Unavailable) => Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "service_unavailable",
            "OCR engine is not available on this server.",
        )),
        Err(OcrFailure::BadImage(msg)) => Err(ApiError::bad_request(msg)),
        Err(OcrFailure::Internal(e)) => {
            tracing::error!("ocr: {e}");
            Err(ApiError::internal())
        }
    }
}

pub async fn route(State(state): State<AppState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, usize::MAX).await else {
        return ApiError::http(StatusCode::BAD_REQUEST, Some("There was an error parsing the body")).into_response();
    };
    let items = match form::parse_form(parts.headers.get(header::CONTENT_TYPE).map(|v| v.as_bytes()), &bytes) {
        Ok(items) => items,
        Err(FormError::Http(msg)) => return ApiError::http(StatusCode::BAD_REQUEST, Some(&msg)).into_response(),
    };
    drop(bytes);
    let ctx = match require_scope(&state, &parts, OCR_READ).await {
        Ok(ctx) => ctx,
        Err(r) => return r.into_response(),
    };
    match reply(items).await {
        Ok(v) => (ctx.rate_limit, PydanticJson(v)).into_response(),
        Err(e) => e.into_response(),
    }
}
