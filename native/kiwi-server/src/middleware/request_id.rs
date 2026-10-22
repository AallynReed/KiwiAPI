//! `request_context` of `app/core/observability.py`: every request gets an id
//! (the incoming `X-Request-ID` verbatim, else `req_` + 16 hex), bound for the
//! error envelope and logs, written back on the response; a panic becomes the
//! standard 500 envelope.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use axum::{extract::Request, http::HeaderValue, middleware::Next, response::Response};
use futures::FutureExt;

use super::errors::envelope_with;
use super::pyurl::latin1;

pub use crate::request_id::current;

pub async fn request_context(req: Request, next: Next) -> Response {
    let header = req.headers().get("x-request-id").filter(|v| !v.is_empty()).cloned();
    let (value, rid) = match header {
        Some(v) => {
            let text = latin1(v.as_bytes());
            (v, text)
        }
        None => {
            let id = crate::request_id::generate();
            (HeaderValue::from_str(&id).expect("ascii id"), id)
        }
    };
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let rid: Arc<str> = rid.into();
    let run = AssertUnwindSafe(next.run(req)).catch_unwind();
    let mut res = match crate::request_id::scope(rid.to_string(), run).await {
        Ok(res) => res,
        Err(_) => {
            tracing::error!(rid = %rid, "Unhandled error on {method} {path}");
            envelope_with(&rid, 500, "internal_error", "Internal server error")
        }
    };
    res.headers_mut().insert("x-request-id", value);
    res
}
