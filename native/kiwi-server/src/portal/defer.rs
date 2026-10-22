//! Bodies whose `EmailStr` needs Unicode normalization, IDNA or a domain
//! literal (see `kiwi_core::email_validator`) are answered by Python: the
//! body is read once here, before the ported stack, and handed on intact.

use axum::{
    body::Body,
    extract::Request,
    http::{HeaderMap, Method, header},
};
use serde_json::Value;

use crate::{middleware::pyurl::latin1, pyjson, validation::is_json_content_type};

const MAX_INSPECT: u64 = 1 << 20;

/// The `EmailStr` field of a ported route's body.
pub fn email_field(matched: &str, method: &Method) -> Option<&'static str> {
    if method != Method::POST {
        return None;
    }
    match matched {
        "/auth/signup" | "/auth/login" | "/auth/forgot-password" | "/auth/resend-verification" => Some("email"),
        "/auth/change-email" => Some("new_email"),
        "/auth/delete-account" => Some("confirm_email"),
        _ => None,
    }
}

/// Whether this body's email is one only Python can judge (decoded exactly
/// as `validation::read_body` decodes it).
pub fn needs_python(headers: &HeaderMap, body: &[u8], field: &str) -> bool {
    let is_json = headers
        .get(header::CONTENT_TYPE)
        .filter(|v| !v.is_empty())
        .is_some_and(|v| is_json_content_type(&latin1(v.as_bytes())));
    if !is_json || body.is_empty() {
        return false;
    }
    match pyjson::loads(body) {
        Ok(Value::Object(m)) => {
            m.get(field).and_then(Value::as_str).is_some_and(kiwi_core::email_validator::needs_unicode)
        }
        _ => false,
    }
}

/// Reads the body of a request to an email route and says whether Python
/// must answer it; the request comes back with its body restored.
pub async fn inspect(req: Request, field: &str) -> (Request, bool) {
    let too_big = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .is_some_and(|n| n > MAX_INSPECT);
    if too_big {
        return (req, false);
    }
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap_or_default();
    let defer = needs_python(&parts.headers, &bytes, field);
    (Request::from_parts(parts, Body::from(bytes)), defer)
}
