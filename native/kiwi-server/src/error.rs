//! The error envelope every route returns, mirroring `app/core/errors.py`:
//! `{"error": {"code", "message", "details", "request_id"}}`.
//! (Foundation wave: the full ErrorCode list, status mapping, 422 validation
//! details and request-id plumbing land here.)

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    /// Machine code, e.g. `"not_found"`, `"rate_limited"`.
    pub code: &'static str,
    pub message: String,
    pub details: Option<serde_json::Value>,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        ApiError { status, code, message: message.into(), details: None }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({
            "error": {"code": self.code, "message": self.message, "details": self.details, "request_id": null}
        });
        (self.status, [(axum::http::header::CONTENT_TYPE, "application/json")], body.to_string()).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
