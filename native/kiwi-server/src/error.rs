//! The error envelope every route returns, mirroring `app/core/errors.py`:
//! `{"error":{"code":..,"message":..,"details":..,"request_id":..}}` in that
//! key order, compact, `ensure_ascii=False` ([`crate::pyjson::dumps`]).
//!
//! - [`ApiError`] is Python's `APIError`. For `APIError(headers=...)` return
//!   `(headers, error)`; axum merges them. It stays under clippy's
//!   `result_large_err` size so `Result<_, ApiError>` needs no boxing.
//! - [`ApiError::http`] is a raised `HTTPException`: the code comes from the
//!   status ([`code_for_status`]) and the message is the detail, or the status
//!   phrase when there is none.
//! - [`http_exception`] adds the request-dependent part: a 404 on a browser
//!   page navigation is the themed HTML page. [`not_found`] /
//!   [`method_not_allowed`] are the framework 404 and 405.
//! - 422 validation bodies are built in [`crate::validation`].

use axum::{
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{json, Value};

use crate::{pyjson, request_id};

/// Stable, machine-readable error slugs (`ErrorCode` in Python).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    BadRequest,
    ValidationError,
    NotAuthenticated,
    InvalidCredentials,
    CaptchaFailed,
    Forbidden,
    NotFound,
    NotPublic,
    Conflict,
    EmailTaken,
    EmailUnverified,
    DisposableEmail,
    PasswordBreached,
    AccountLocked,
    InsufficientScope,
    IpNotAllowed,
    RateLimited,
    MethodNotAllowed,
    ServiceUnavailable,
    InternalError,
}

impl ErrorCode {
    pub const ALL: [ErrorCode; 20] = [
        ErrorCode::BadRequest,
        ErrorCode::ValidationError,
        ErrorCode::NotAuthenticated,
        ErrorCode::InvalidCredentials,
        ErrorCode::CaptchaFailed,
        ErrorCode::Forbidden,
        ErrorCode::NotFound,
        ErrorCode::NotPublic,
        ErrorCode::Conflict,
        ErrorCode::EmailTaken,
        ErrorCode::EmailUnverified,
        ErrorCode::DisposableEmail,
        ErrorCode::PasswordBreached,
        ErrorCode::AccountLocked,
        ErrorCode::InsufficientScope,
        ErrorCode::IpNotAllowed,
        ErrorCode::RateLimited,
        ErrorCode::MethodNotAllowed,
        ErrorCode::ServiceUnavailable,
        ErrorCode::InternalError,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            ErrorCode::BadRequest => "bad_request",
            ErrorCode::ValidationError => "validation_error",
            ErrorCode::NotAuthenticated => "not_authenticated",
            ErrorCode::InvalidCredentials => "invalid_credentials",
            ErrorCode::CaptchaFailed => "captcha_failed",
            ErrorCode::Forbidden => "forbidden",
            ErrorCode::NotFound => "not_found",
            ErrorCode::NotPublic => "not_public",
            ErrorCode::Conflict => "conflict",
            ErrorCode::EmailTaken => "email_taken",
            ErrorCode::EmailUnverified => "email_unverified",
            ErrorCode::DisposableEmail => "disposable_email",
            ErrorCode::PasswordBreached => "password_breached",
            ErrorCode::AccountLocked => "account_locked",
            ErrorCode::InsufficientScope => "insufficient_scope",
            ErrorCode::IpNotAllowed => "ip_not_allowed",
            ErrorCode::RateLimited => "rate_limited",
            ErrorCode::MethodNotAllowed => "method_not_allowed",
            ErrorCode::ServiceUnavailable => "service_unavailable",
            ErrorCode::InternalError => "internal_error",
        }
    }
}

impl From<ErrorCode> for &'static str {
    fn from(code: ErrorCode) -> Self {
        code.as_str()
    }
}

/// The code a framework `HTTPException` with this status gets
/// (`_STATUS_TO_CODE`, else `internal_error`).
pub fn code_for_status(status: StatusCode) -> ErrorCode {
    match status.as_u16() {
        400 => ErrorCode::BadRequest,
        401 => ErrorCode::NotAuthenticated,
        403 => ErrorCode::Forbidden,
        404 => ErrorCode::NotFound,
        405 => ErrorCode::MethodNotAllowed,
        409 => ErrorCode::Conflict,
        422 => ErrorCode::ValidationError,
        429 => ErrorCode::RateLimited,
        503 => ErrorCode::ServiceUnavailable,
        _ => ErrorCode::InternalError,
    }
}

/// The envelope value, with the current request id.
pub fn envelope(code: &str, message: &str, details: Option<&Value>) -> Value {
    json!({
        "error": {
            "code": code,
            "message": message,
            "details": details.cloned().unwrap_or(Value::Null),
            "request_id": request_id::current(),
        }
    })
}

/// `build_error_body(...)` serialized as Python sends it.
pub fn envelope_bytes(code: &str, message: &str, details: Option<&Value>) -> String {
    pyjson::dumps(&envelope(code, message, details))
}

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    /// Machine code, e.g. `"not_found"`, `"rate_limited"`.
    pub code: &'static str,
    pub message: String,
    pub details: Option<Value>,
}

impl ApiError {
    pub fn new(status: StatusCode, code: impl Into<&'static str>, message: impl Into<String>) -> Self {
        ApiError { status, code: code.into(), message: message.into(), details: None }
    }

    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    /// A raised `HTTPException(status_code, detail)`.
    pub fn http(status: StatusCode, detail: Option<&str>) -> Self {
        let message = match detail {
            Some(d) => d.to_owned(),
            None => status_phrase(status.as_u16()).unwrap_or("").to_owned(),
        };
        ApiError::new(status, code_for_status(status), message)
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        ApiError::new(StatusCode::BAD_REQUEST, ErrorCode::BadRequest, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        ApiError::new(StatusCode::NOT_FOUND, ErrorCode::NotFound, message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        ApiError::new(StatusCode::FORBIDDEN, ErrorCode::Forbidden, message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        ApiError::new(StatusCode::CONFLICT, ErrorCode::Conflict, message)
    }

    /// An unhandled error.
    pub fn internal() -> Self {
        ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, ErrorCode::InternalError, "Internal server error")
    }

    /// `raise_from_value_error`: "already exists" is a 409 conflict, anything
    /// else a 400.
    pub fn from_value_error(message: impl Into<String>) -> Self {
        let message = message.into();
        if message.contains("already exists") {
            ApiError::conflict(message)
        } else {
            ApiError::bad_request(message)
        }
    }

    /// The envelope body exactly as Python serializes it.
    pub fn body(&self) -> String {
        envelope_bytes(self.code, &self.message, self.details.as_ref())
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}: {}", self.status.as_u16(), self.code, self.message)
    }
}

impl std::error::Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, [(header::CONTENT_TYPE, HeaderValue::from_static("application/json"))], self.body())
            .into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

/// Paths that stay JSON on a 404 even for a browser (`_API_PREFIXES`).
const API_PREFIXES: [&str; 11] = [
    "/v1/",
    "/v1",
    "/admin",
    "/site/",
    "/git/",
    "/secret-scanning",
    "/openapi",
    "/.well-known",
    "/health",
    "/config",
    "/metrics",
];

/// `_wants_html_page`: a GET/HEAD page navigation (Accept contains
/// `text/html`) outside the API prefixes. `accept` is the first Accept header.
pub fn wants_html_page(method: &Method, path: &str, accept: Option<&str>) -> bool {
    (method == Method::GET || method == Method::HEAD)
        && !API_PREFIXES.iter().any(|p| path.starts_with(p))
        && accept.is_some_and(|a| a.contains("text/html"))
}

/// A raised `HTTPException` as `_http_exception_handler` answers it: a 404
/// (any detail) on a page navigation gets the themed HTML page, everything
/// else the envelope. An `ApiError` 404 never gets the HTML page.
pub fn http_exception(
    status: StatusCode,
    detail: Option<&str>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
) -> Response {
    let accept = headers.get(header::ACCEPT).map(|v| request_id::latin1(v.as_bytes()));
    if status == StatusCode::NOT_FOUND && wants_html_page(method, path, accept.as_deref()) {
        return (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"))],
            NOT_FOUND_HTML,
        )
            .into_response();
    }
    ApiError::http(status, detail).into_response()
}

/// The framework 404 for an unmatched route.
pub fn not_found(method: &Method, path: &str, headers: &HeaderMap) -> Response {
    http_exception(StatusCode::NOT_FOUND, None, method, path, headers)
}

/// The framework 405: `Method Not Allowed` plus `Allow`, which Starlette
/// fills with the methods of the FIRST route whose path matched (comma-space
/// joined), not every method the path supports.
pub fn method_not_allowed(allow: &str) -> Response {
    let mut resp = ApiError::http(StatusCode::METHOD_NOT_ALLOWED, None).into_response();
    if let Ok(v) = HeaderValue::from_str(allow) {
        resp.headers_mut().insert(header::ALLOW, v);
    }
    resp
}

/// `http.HTTPStatus(code).phrase` as of Python 3.13 (RFC 9110 names).
pub fn status_phrase(code: u16) -> Option<&'static str> {
    Some(match code {
        100 => "Continue",
        101 => "Switching Protocols",
        102 => "Processing",
        103 => "Early Hints",
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        203 => "Non-Authoritative Information",
        204 => "No Content",
        205 => "Reset Content",
        206 => "Partial Content",
        207 => "Multi-Status",
        208 => "Already Reported",
        226 => "IM Used",
        300 => "Multiple Choices",
        301 => "Moved Permanently",
        302 => "Found",
        303 => "See Other",
        304 => "Not Modified",
        305 => "Use Proxy",
        307 => "Temporary Redirect",
        308 => "Permanent Redirect",
        400 => "Bad Request",
        401 => "Unauthorized",
        402 => "Payment Required",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        406 => "Not Acceptable",
        407 => "Proxy Authentication Required",
        408 => "Request Timeout",
        409 => "Conflict",
        410 => "Gone",
        411 => "Length Required",
        412 => "Precondition Failed",
        413 => "Content Too Large",
        414 => "URI Too Long",
        415 => "Unsupported Media Type",
        416 => "Range Not Satisfiable",
        417 => "Expectation Failed",
        418 => "I'm a Teapot",
        421 => "Misdirected Request",
        422 => "Unprocessable Content",
        423 => "Locked",
        424 => "Failed Dependency",
        425 => "Too Early",
        426 => "Upgrade Required",
        428 => "Precondition Required",
        429 => "Too Many Requests",
        431 => "Request Header Fields Too Large",
        451 => "Unavailable For Legal Reasons",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        505 => "HTTP Version Not Supported",
        506 => "Variant Also Negotiates",
        507 => "Insufficient Storage",
        508 => "Loop Detected",
        510 => "Not Extended",
        511 => "Network Authentication Required",
        _ => return None,
    })
}

/// `_NOT_FOUND_HTML`, byte for byte.
pub const NOT_FOUND_HTML: &str = r##"<!DOCTYPE html>
<html lang="en" dir="ltr"><head>
<meta charset="UTF-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Page not found · Better Trove Tools</title>
<meta name="robots" content="noindex">
<meta name="theme-color" content="#0a0e14"><meta name="color-scheme" content="dark">
<link rel="icon" href="data:image/png;base64,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" type="image/png">
<style>
  :root{--bg:#0a0e14;--card:#11161f;--line:#222c3a;--text:#e6ebf2;--soft:#9aa6b8;--mute:#8a94a6;--blue:#569cff}
  *{box-sizing:border-box}
  body{margin:0;min-height:100vh;display:grid;place-items:center;padding:32px;
    background:radial-gradient(1200px 600px at 50% -10%,rgba(86,156,255,.10),transparent),var(--bg);
    color:var(--text);font-family:'Inter',system-ui,sans-serif}
  .nf{max-width:520px;text-align:center}
  .nf-code{font:700 clamp(5rem,18vw,9rem)/1 'Space Grotesk','Inter',sans-serif;letter-spacing:-.04em;
    background:linear-gradient(135deg,#569cff,#a06bff 60%,#ff5e7e);-webkit-background-clip:text;
    background-clip:text;color:transparent;margin:0}
  .nf-title{font:700 1.5rem 'Space Grotesk','Inter',sans-serif;margin:6px 0 10px}
  .nf-text{color:var(--soft);line-height:1.6;margin:0 0 26px}
  .nf-actions{display:flex;gap:12px;justify-content:center;flex-wrap:wrap}
  .nf-btn{display:inline-flex;align-items:center;gap:8px;text-decoration:none;border-radius:10px;
    padding:11px 20px;font-weight:600;font-size:.95rem;border:1px solid var(--line);
    color:var(--text);background:var(--card);transition:border-color .15s,transform .12s}
  .nf-btn:hover{border-color:var(--blue);transform:translateY(-1px)}
  .nf-btn-primary{background:linear-gradient(135deg,#569cff,#4a7fe0);border-color:transparent;color:#04121f}
  .nf-mark{color:var(--mute);font-size:.82rem;margin-top:30px;letter-spacing:.04em;text-transform:uppercase}
</style></head>
<body><main class="nf">
  <p class="nf-code">404</p>
  <h1 class="nf-title">This page wandered off</h1>
  <p class="nf-text">The page you're looking for doesn't exist, moved, or never did. Let's get you back to solid ground.</p>
  <div class="nf-actions">
    <a class="nf-btn nf-btn-primary" href="/">&larr; Back home</a>
    <a class="nf-btn" href="/mods">Browse the Mods Hub</a>
  </div>
  <p class="nf-mark">Better Trove Tools</p>
</main></body></html>"##;
