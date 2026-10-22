//! Auth from `app/core/{security,dependencies,scopes,ip_hash,lockout}.py`, `app/site_auth` and `app/tokens/usage.py`.
//! Every artifact is interchangeable with Python's; `testdata/golden.json` pins that.

pub mod client;
pub mod extract;
pub mod jwt;
pub mod lockout;
pub mod models;
pub mod passwords;
pub mod pyutil;
pub mod scopes;
pub mod sessions;
pub mod settings;
pub mod site;
pub mod token_use;
pub mod tokens;

#[cfg(test)]
mod extract_tests;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod tests;

use axum::{
    http::{HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};

use crate::error::ApiError;

pub use extract::{
    AccessContext, AuthContext, CurrentSiteUser, CurrentUser, IngestAuth, MasterIngest, OptionalSiteUser, Public,
    PublicScope, PublicScopeSpec, RequireScope, Superuser, TokenAuth, TokenContext, VerifiedSiteUser,
};
pub use settings::{auth_settings, AuthSettings};

/// An error envelope plus the headers Python's `APIError(headers=...)` carries
/// (`WWW-Authenticate`, `Retry-After`, `X-RateLimit-*`).
#[derive(Debug)]
pub struct Rejection {
    pub error: Box<ApiError>,
    pub headers: Vec<(HeaderName, HeaderValue)>,
}

impl Rejection {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Rejection { error: Box::new(ApiError::new(status, code, message)), headers: Vec::new() }
    }

    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.error.details = Some(details);
        self
    }

    pub fn with_header(mut self, name: &'static str, value: impl AsRef<str>) -> Self {
        if let Ok(v) = HeaderValue::from_str(value.as_ref()) {
            self.headers.push((HeaderName::from_static(name), v));
        }
        self
    }

    /// 401 `not_authenticated` with `WWW-Authenticate: Bearer`.
    pub fn not_authenticated(message: &str) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "not_authenticated", message).with_header("www-authenticate", "Bearer")
    }

    pub fn forbidden(code: &'static str, message: &str) -> Self {
        Self::new(StatusCode::FORBIDDEN, code, message)
    }

    /// What an uncaught exception becomes in Python.
    pub fn internal(err: impl std::fmt::Display) -> Self {
        tracing::error!("auth: {err}");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", "Internal server error")
    }

    pub fn status(&self) -> StatusCode {
        self.error.status
    }
}

impl From<ApiError> for Rejection {
    fn from(error: ApiError) -> Self {
        Rejection { error: Box::new(error), headers: Vec::new() }
    }
}

impl IntoResponse for Rejection {
    fn into_response(self) -> Response {
        let mut resp = (*self.error).into_response();
        for (name, value) in self.headers {
            resp.headers_mut().insert(name, value);
        }
        resp
    }
}
