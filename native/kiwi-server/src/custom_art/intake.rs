//! `/site/custom-art` (`app/custom_art/router.py` + `service.submit`): the
//! form's settings and the tokenless, login-free request intake.

use axum::{
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use kiwi_core::{
    email_validator::{self, Email},
    form::{self, FormError, FormValue, UploadFile},
    pystr,
};
use mongodb::bson::{Document, oid::ObjectId};
use serde_json::{Value, json};

use super::{ArtPicture, ArtRequest, COLLECTION, WIDEST, check_shape, clean_name, lane_label, lane_of, slots};
use crate::{
    auth::{Rejection, client::client_ip},
    dt,
    error::{ApiError, ErrorCode},
    media::cas::ContentStore,
    portal::checks::{is_disposable, verify_captcha},
    pyjson::PydanticJson,
    ratelimit::check_rate_limit,
    routes::misc::feedback::{str_field, upload_json},
    runtime_config,
    state::AppState,
    validation::{ErrorItem, Fail, LocItem, ValidationError},
};

const KINDS: &[&str] = &["player", "club"];

pub async fn form_config(State(state): State<AppState>) -> Response {
    let s = state.settings();
    let body = json!({
        "captcha_provider": s.captcha_provider,
        "captcha_sitekey": s.captcha_sitekey,
        "max_bytes": s.custom_art_image_max_bytes,
        "widest": WIDEST,
    });
    ([(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))], PydanticJson(body)).into_response()
}

/// The form's parameters after FastAPI's validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtForm {
    pub kind: String,
    pub name: String,
    pub email: String,
    pub note: Option<String>,
    pub captcha_token: Option<String>,
    pub pfp: Option<UploadFile>,
    pub banner: Option<UploadFile>,
}

fn loc(name: &str) -> Vec<LocItem> {
    vec!["body".into(), name.into()]
}

fn item(name: &str, fail: Fail, input: Value) -> ErrorItem {
    ErrorItem { kind: fail.kind, loc: loc(name), msg: fail.msg, input, ctx: fail.ctx }
}

fn last<'a>(items: &'a [(String, FormValue)], name: &str) -> Option<&'a FormValue> {
    items.iter().rev().find(|(k, _)| k == name).map(|(_, v)| v)
}

fn present(items: &[(String, FormValue)], name: &str) -> bool {
    last(items, name).is_some_and(|v| !matches!(v, FormValue::Text(t) if t.is_empty()))
}

fn required(items: &[(String, FormValue)], name: &str, max: usize, errors: &mut Vec<ErrorItem>) -> String {
    if !present(items, name) {
        errors.push(item(name, Fail::missing(), Value::Null));
        return String::new();
    }
    str_field(items, name, None, Some(max), errors).unwrap_or_default()
}

fn upload(items: &[(String, FormValue)], name: &str, errors: &mut Vec<ErrorItem>) -> Option<UploadFile> {
    match last(items, name)? {
        FormValue::File(f) => Some(f.clone()),
        FormValue::Text(t) if t.is_empty() => None,
        FormValue::Text(t) => {
            errors.push(item(
                name,
                Fail::value_error("Expected UploadFile, received: <class 'str'>"),
                Value::String(t.clone()),
            ));
            None
        }
    }
}

/// FastAPI's extraction and validation of the seven form parameters.
pub fn validate_form(items: &[(String, FormValue)]) -> Result<ArtForm, Vec<ErrorItem>> {
    let mut errors = Vec::new();
    let kind = match last(items, "kind") {
        None => None,
        Some(FormValue::Text(t)) if t.is_empty() => None,
        Some(FormValue::Text(t)) if KINDS.contains(&t.as_str()) => Some(t.clone()),
        Some(FormValue::Text(t)) => {
            errors.push(item("kind", Fail::literal_error(KINDS), Value::String(t.clone())));
            Some(String::new())
        }
        Some(FormValue::File(f)) => {
            errors.push(item("kind", Fail::literal_error(KINDS), upload_json(f)));
            Some(String::new())
        }
    };
    if kind.is_none() {
        errors.push(item("kind", Fail::missing(), Value::Null));
    }
    let name = required(items, "name", 200, &mut errors);
    let email = required(items, "email", 254, &mut errors);
    let note = str_field(items, "note", None, Some(500), &mut errors);
    let captcha_token = str_field(items, "captcha_token", None, Some(4096), &mut errors);
    let pfp = upload(items, "pfp", &mut errors);
    let banner = upload(items, "banner", &mut errors);
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(ArtForm { kind: kind.unwrap_or_default(), name, email, note, captcha_token, pfp, banner })
}

/// `clean_email`.
pub fn clean_email(email: &str) -> Result<String, ApiError> {
    match email_validator::validate_direct(pystr::strip(email)) {
        Email::Valid(address) if is_disposable(&address) => Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::DisposableEmail,
            "Use an address you'll still have in a few days - the answer goes there.",
        )),
        Email::Valid(address) => Ok(address),
        Email::Invalid(_) => Err(super::validation("That email address doesn't look right.")),
    }
}

/// What `submit` keeps once everything correctable has been checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checked {
    pub name: String,
    pub email: String,
    /// Slot, bytes, content type, width, height.
    pub pictures: Vec<(String, Vec<u8>, &'static str, i64, i64)>,
}

/// `submit` up to the captcha: name, address, files, sizes and shapes.
pub fn check(
    kind: &str,
    name: &str,
    email: &str,
    files: Vec<(&str, Vec<u8>)>,
    limit: i64,
) -> Result<Checked, ApiError> {
    let name = clean_name(name)?;
    let email = clean_email(email)?;
    let files: Vec<(&str, Vec<u8>)> = files.into_iter().filter(|(_, d)| !d.is_empty()).collect();
    if files.is_empty() {
        return Err(super::validation(if kind == "player" {
            "Add a profile picture."
        } else {
            "Add a club picture, a club banner, or both."
        }));
    }
    if files.iter().any(|(slot, _)| !slots(kind).contains(slot)) {
        return Err(super::validation("A player request carries a profile picture only."));
    }
    let mut pictures = Vec::new();
    for (slot, data) in files {
        let label = lane_of(kind, slot).map(lane_label).unwrap_or_default();
        if data.len() as i64 > limit {
            return Err(ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                ErrorCode::BadRequest,
                format!("The {label} is over the {} MB limit.", limit.div_euclid(1024 * 1024)),
            ));
        }
        let Some((content_type, w, h)) = super::measure::measure(&data) else {
            return Err(ApiError::bad_request(format!("The {label} has to be a PNG, JPEG, WebP or GIF picture.")));
        };
        check_shape(kind, slot, i64::from(w), i64::from(h))?;
        pictures.push((slot.to_owned(), data, content_type, i64::from(w), i64::from(h)));
    }
    Ok(Checked { name, email, pictures })
}

/// The document `submit` inserts.
pub fn new_request(kind: &str, checked: &Checked, shas: &[String], note: Option<&str>) -> ArtRequest {
    let note = note.map(|n| pystr::prefix(pystr::strip(n), 500)).filter(|n| !n.is_empty()).map(str::to_owned);
    let pictures = checked
        .pictures
        .iter()
        .zip(shas)
        .map(|((slot, _, ct, w, h), sha)| {
            (slot.clone(), ArtPicture { sha: sha.clone(), content_type: (*ct).to_owned(), width: *w, height: *h })
        })
        .collect();
    ArtRequest {
        id: ObjectId::new(),
        kind: kind.to_owned(),
        name: checked.name.clone(),
        email: checked.email.clone(),
        note,
        pictures,
        status: "pending".into(),
        reason: None,
        batch: None,
        log: None,
        versions: Vec::new(),
        steam_pending: false,
        created_at: dt::now(),
        decided_at: None,
        released_at: None,
    }
}

async fn submit(state: &AppState, f: &ArtForm, ip: Option<&str>) -> Result<(), Rejection> {
    let limit = state.settings().custom_art_image_max_bytes;
    let read =
        |u: &Option<UploadFile>| u.as_ref().map(|u| u.data[..u.data.len().min((limit + 1).max(0) as usize)].to_vec());
    let mut files = Vec::new();
    for (slot, u) in [("pfp", &f.pfp), ("banner", &f.banner)] {
        if let Some(data) = read(u) {
            files.push((slot, data));
        }
    }
    let kind = f.kind.clone();
    let (name, email) = (f.name.clone(), f.email.clone());
    let checked = tokio::task::spawn_blocking(move || check(&kind, &name, &email, files, limit))
        .await
        .map_err(Rejection::internal)??;
    if !verify_captcha(state.settings(), f.captcha_token.as_deref(), ip).await? {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            ErrorCode::CaptchaFailed,
            "The captcha check didn't pass. Try it again.",
        )
        .into());
    }
    let store = ContentStore::new(&state.settings().mods_store_dir);
    let blobs: Vec<Vec<u8>> = checked.pictures.iter().map(|p| p.1.clone()).collect();
    let shas = tokio::task::spawn_blocking(move || {
        blobs.iter().map(|b| store.put(b).map(|(sha, _)| sha)).collect::<std::io::Result<Vec<String>>>()
    })
    .await
    .map_err(Rejection::internal)?
    .map_err(Rejection::internal)?;
    let request = new_request(&f.kind, &checked, &shas, f.note.as_deref());
    state.mongo().collection::<Document>(COLLECTION).insert_one(request.to_doc()).await.map_err(Rejection::internal)?;
    let slots: Vec<&str> = request.pictures.iter().map(|(s, _)| s.as_str()).collect();
    tracing::info!(
        "custom art request {}: {} {} ({})",
        request.id.to_hex(),
        request.kind,
        pystr::repr(&request.name),
        slots.join(", ")
    );
    Ok(())
}

fn parse_error(msg: &str) -> Response {
    ApiError::http(StatusCode::BAD_REQUEST, Some(msg)).into_response()
}

pub async fn submit_request(State(state): State<AppState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, usize::MAX).await else {
        return parse_error("There was an error parsing the body");
    };
    let items = match form::parse_form(parts.headers.get(header::CONTENT_TYPE).map(|v| v.as_bytes()), &bytes) {
        Ok(items) => items,
        Err(FormError::Http(msg)) => return parse_error(&msg),
    };
    let f = match validate_form(&items) {
        Ok(f) => f,
        Err(errors) => return ValidationError(errors).into_response(),
    };
    drop(items);
    let ip = client_ip(&parts);
    let (max, window) = match runtime_config::get_rate_limit(&state, "art_request_rate_limit").await {
        Ok(v) => v,
        Err(e) => return Rejection::internal(e).into_response(),
    };
    if let Err(r) =
        check_rate_limit(&state, &format!("custom-art:{}", ip.as_deref().unwrap_or("unknown")), max, window).await
    {
        return r.into_response();
    }
    match submit(&state, &f, ip.as_deref()).await {
        Ok(()) => (StatusCode::ACCEPTED, PydanticJson(json!({"status": "received"}))).into_response(),
        Err(r) => r.into_response(),
    }
}
