//! Error envelopes the middleware builds itself (`build_error_body` +
//! Starlette `JSONResponse`), and the 405 a ported path answers for a method
//! it doesn't route.

use axum::{
    body::Body,
    http::{HeaderValue, StatusCode, header},
    response::Response,
};
use serde_json::json;

use super::request_id;

pub fn envelope_with(rid: &str, status: u16, code: &str, message: &str) -> Response {
    let body = json!({"error": {"code": code, "message": message, "details": null, "request_id": rid}});
    let text = crate::pyjson::dumps(&body);
    let mut res = Response::new(Body::from(text));
    *res.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    res
}

/// The envelope with the current request's id.
pub fn envelope(status: u16, code: &str, message: &str) -> Response {
    envelope_with(&request_id::current(), status, code, message)
}

/// Marks a response as the router's "method not allowed" answer.
#[derive(Clone, Copy, Debug)]
pub struct MethodNotAllowed;

pub async fn method_not_allowed() -> Response {
    let mut res = envelope(405, "method_not_allowed", "Method Not Allowed");
    res.extensions_mut().insert(MethodNotAllowed);
    res
}
