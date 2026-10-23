//! FastAPI's form handling for the multipart tool routes: the body is read
//! before any dependency runs, fields are validated after them, in signature
//! order, every error collected.

use axum::{
    body::Body,
    http::{StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
};
use kiwi_core::form::{FormValue, UploadFile};
use serde_json::Value;

use crate::{
    error::ApiError,
    pyjson::{self, LoadsError},
    routes::misc::feedback::str_field,
    validation::{ErrorItem, Fail, LocItem, ValidationError},
};

pub type Items = Vec<(String, FormValue)>;

/// An early reply from a sync helper (boxed: a `Response` is large).
pub type Rej = Box<Response>;

pub trait Reply {
    fn reply(self) -> Response;
}

impl Reply for Response {
    fn reply(self) -> Response {
        self
    }
}

impl Reply for Rej {
    fn reply(self) -> Response {
        *self
    }
}

/// `await request.form()` for a route with form fields; other bodies are no form.
pub async fn read(parts: &Parts, body: Body) -> Result<Items, Response> {
    let ct = parts.headers.get(header::CONTENT_TYPE).map(|v| v.as_bytes().to_vec());
    let (kind, _) = kiwi_core::form::parse_options_header(ct.as_deref());
    if kind != b"multipart/form-data" && kind != b"application/x-www-form-urlencoded" {
        return Ok(Vec::new());
    }
    let body = axum::body::to_bytes(body, usize::MAX).await.map_err(|_| {
        ApiError::http(StatusCode::BAD_REQUEST, Some("There was an error parsing the body")).into_response()
    })?;
    tokio::task::spawn_blocking(move || kiwi_core::form::parse_form(ct.as_deref(), &body))
        .await
        .map_err(|_| ApiError::internal().into_response())?
        .map_err(|kiwi_core::form::FormError::Http(m)| {
            ApiError::http(StatusCode::BAD_REQUEST, Some(&m)).into_response()
        })
}

fn loc(path: &[LocItem]) -> Vec<LocItem> {
    let mut l = vec![LocItem::from("body")];
    l.extend_from_slice(path);
    l
}

fn missing(name: &str) -> ErrorItem {
    ErrorItem {
        kind: "missing".into(),
        loc: loc(&[name.into()]),
        msg: "Field required".into(),
        input: Value::Null,
        ctx: None,
    }
}

fn not_upload(at: Vec<LocItem>, text: &str) -> ErrorItem {
    let fail = Fail::value_error("Expected UploadFile, received: <class 'str'>");
    ErrorItem { kind: fail.kind, loc: at, msg: fail.msg, input: Value::String(text.into()), ctx: fail.ctx }
}

fn last<'a>(items: &'a Items, name: &str) -> Option<&'a FormValue> {
    items.iter().rev().find(|(k, _)| k == name).map(|(_, v)| v)
}

/// `UploadFile = File(...)`.
pub fn file(items: &Items, name: &str, errors: &mut Vec<ErrorItem>) -> Option<UploadFile> {
    match last(items, name) {
        Some(FormValue::File(f)) => Some(f.clone()),
        Some(FormValue::Text(t)) if !t.is_empty() => {
            errors.push(not_upload(loc(&[name.into()]), t));
            None
        }
        _ => {
            errors.push(missing(name));
            None
        }
    }
}

/// `UploadFile | None = File(default=None)`.
pub fn file_opt(items: &Items, name: &str, errors: &mut Vec<ErrorItem>) -> Option<UploadFile> {
    match last(items, name) {
        Some(FormValue::File(f)) => Some(f.clone()),
        Some(FormValue::Text(t)) if !t.is_empty() => {
            errors.push(not_upload(loc(&[name.into()]), t));
            None
        }
        _ => None,
    }
}

/// `list[UploadFile] = File(default=[])`.
pub fn files(items: &Items, name: &str, errors: &mut Vec<ErrorItem>) -> Vec<UploadFile> {
    let mut out = Vec::new();
    for (i, v) in items.iter().filter(|(k, _)| k == name).map(|(_, v)| v).enumerate() {
        match v {
            FormValue::File(f) => out.push(f.clone()),
            FormValue::Text(t) => errors.push(not_upload(loc(&[name.into(), LocItem::Index(i)]), t)),
        }
    }
    out
}

/// `str = Form(default=...)`.
pub fn text(items: &Items, name: &str, default: &str, errors: &mut Vec<ErrorItem>) -> String {
    str_field(items, name, None, None, errors).unwrap_or_else(|| default.to_owned())
}

/// `str = Form(..., min_length=, max_length=)`.
pub fn text_required(
    items: &Items,
    name: &str,
    min: Option<usize>,
    max: Option<usize>,
    errors: &mut Vec<ErrorItem>,
) -> Option<String> {
    let before = errors.len();
    let v = str_field(items, name, min, max, errors);
    if v.is_none() && errors.len() == before {
        errors.push(missing(name));
    }
    v
}

pub fn check(errors: Vec<ErrorItem>) -> Result<(), Rej> {
    if errors.is_empty() { Ok(()) } else { Err(Box::new(ValidationError(errors).into_response())) }
}

/// `json.loads(text)`: `Ok(None)` is the `ValueError` a route turns into its
/// 400; NaN/Infinity (Python takes them, `Value` can't hold them) are a 500.
pub fn loads(s: &str) -> Result<Option<Value>, Rej> {
    if s.starts_with('\u{feff}') {
        return Ok(None);
    }
    match pyjson::loads(s.as_bytes()) {
        Ok(v) => Ok(Some(v)),
        Err(LoadsError::NonFinite) => Err(Box::new(ApiError::internal().into_response())),
        Err(_) => Ok(None),
    }
}
