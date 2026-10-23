//! Request ids, mirroring `app/core/context.py` + the `request_context`
//! middleware in `app/core/observability.py`.
//!
//! The id is the incoming `X-Request-ID` taken verbatim (first header, not
//! validated, latin-1 decoded like Starlette) or, when absent or empty,
//! `req_` + 16 random hex chars. It is bound for the request (error envelopes
//! read it through [`current`]) and always written back as the response's
//! `X-Request-ID`. Outside a request [`current`] is `"-"`.

use std::{
    collections::hash_map::RandomState,
    future::Future,
    hash::{BuildHasher, Hasher},
    panic::AssertUnwindSafe,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{
    extract::Request,
    http::{HeaderMap, HeaderName, HeaderValue},
    middleware::Next,
    response::{IntoResponse, Response},
};
use futures::FutureExt;
use tracing::Instrument;

use crate::error::ApiError;

pub const HEADER: HeaderName = HeaderName::from_static("x-request-id");

tokio::task_local! {
    static REQUEST_ID: String;
}

/// Marks the 500 the request-id layer writes itself (a plain response in
/// Python, not one streamed through `BaseHTTPMiddleware`).
#[derive(Clone, Copy, Debug)]
pub struct SelfWritten;

/// The current request's id, or `"-"` outside a request.
pub fn current() -> String {
    REQUEST_ID.try_with(Clone::clone).unwrap_or_else(|_| "-".to_owned())
}

/// Runs `fut` with `rid` as the current request id.
pub async fn scope<F: Future>(rid: String, fut: F) -> F::Output {
    REQUEST_ID.scope(rid, fut).await
}

/// `"req_" + secrets.token_hex(8)`.
pub fn generate() -> String {
    let mut b = [0u8; 8];
    if getrandom::fill(&mut b).is_err() {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let mut h = RandomState::new().build_hasher();
        h.write_u64(SEQ.fetch_add(1, Ordering::Relaxed));
        h.write_u128(SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        b = h.finish().to_le_bytes();
    }
    format!("req_{}", hex::encode(b))
}

/// Header bytes as Starlette exposes them (latin-1).
pub fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

fn to_latin1(s: &str) -> Vec<u8> {
    s.chars().map(|c| c as u32 as u8).collect()
}

/// The id Python would use for a request with these headers, and whether it
/// came from the client.
pub fn from_headers(headers: &HeaderMap) -> (String, bool) {
    match headers.get(&HEADER).map(|v| v.as_bytes()) {
        Some(b) if !b.is_empty() => (latin1(b), true),
        _ => (generate(), false),
    }
}

/// The request-id layer (`axum::middleware::from_fn(request_id::middleware)`).
///
/// A generated id is also set on the request, so inner layers reuse it
/// instead of minting their own. Log lines inside carry it as the span field
/// `rid`. A panic below becomes the 500 envelope, as Python's error boundary
/// turns an unhandled exception into one.
pub async fn middleware(mut req: Request, next: Next) -> Response {
    let (rid, incoming) = from_headers(req.headers());
    if !incoming && let Ok(v) = HeaderValue::from_str(&rid) {
        req.headers_mut().insert(HEADER, v);
    }
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let span = tracing::info_span!("request", rid = %rid);
    let run = async move {
        match AssertUnwindSafe(next.run(req)).catch_unwind().await {
            Ok(resp) => resp,
            Err(_) => {
                tracing::error!("Unhandled error on {method} {path}");
                ApiError::internal().into_response()
            }
        }
    };
    let mut resp = scope(rid.clone(), run.instrument(span)).await;
    if let Ok(v) = HeaderValue::from_bytes(&to_latin1(&rid)) {
        resp.headers_mut().insert(HEADER, v);
    }
    resp
}
