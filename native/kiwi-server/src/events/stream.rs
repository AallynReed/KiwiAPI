//! `GET /v1/events/stream` (`app/events/router.py`): the retry hint, the
//! snapshot, then live frames with a `: ping` after each quiet heartbeat.

use std::{future::Future, io, pin::Pin, time::Duration};

use axum::{
    body::Body,
    extract::State,
    http::{HeaderValue, StatusCode, header},
    response::Response,
};
use bytes::Bytes;
use futures::Stream;
use serde_json::Value;

use super::bus::{self, Subscription};
use crate::{
    auth::{Public, scopes},
    error::{ApiError, ErrorCode},
    feeds::pyval::py_str,
    pyjson,
    state::AppState,
};

pub const RETRY: &[u8] = b"retry: 5000\n\n";
pub const PING: &[u8] = b": ping\n\n";
/// The uvicorn workers one process stands in for (`API_WORKERS`, 8 in prod):
/// `events_max_connections` is a per-worker cap.
const PY_WORKERS: i64 = 8;

/// `_sse(payload)`; `None` where Python raises (not a dict, or no `type`).
pub fn frame(payload: &Value) -> Option<String> {
    let kind = payload.as_object()?.get("type")?;
    Some(format!("event: {}\ndata: {}\n\n", py_str(kind), pyjson::dumps_spaced(payload)))
}

fn capacity(state: &AppState) -> usize {
    let workers = std::env::var("API_WORKERS").ok().and_then(|v| v.trim().parse::<i64>().ok()).unwrap_or(PY_WORKERS);
    (state.settings().events_max_connections.saturating_mul(workers.max(1))).max(0) as usize
}

pub fn at_capacity() -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::ServiceUnavailable,
        "The event stream is at capacity; retry shortly.",
    )
}

/// `max(5, settings.events_heartbeat_seconds)`.
pub fn heartbeat(seconds: i64) -> Duration {
    Duration::from_secs(seconds.max(5) as u64)
}

pub async fn stream(
    State(state): State<AppState>,
    _ctx: Public<{ scopes::EVENTS_READ }>,
) -> Result<Response, ApiError> {
    if bus::connection_count() >= capacity(&state) {
        return Err(at_capacity());
    }
    let heartbeat = heartbeat(state.settings().events_heartbeat_seconds);
    let snapshot = async move { bus::current_snapshot(&state).await };
    Ok(response(body(snapshot, bus::subscribe, heartbeat)))
}

/// Starlette's `StreamingResponse(media_type="text/event-stream", headers=...)`.
pub fn response(stream: impl Stream<Item = io::Result<Bytes>> + Send + 'static) -> Response {
    let mut res = Response::new(Body::from_stream(stream));
    let h = res.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    h.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
    h.insert("x-accel-buffering", HeaderValue::from_static("no"));
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream; charset=utf-8"));
    res
}

type Snapshot = Pin<Box<dyn Future<Output = Vec<Value>> + Send>>;

enum Phase<S> {
    Retry(Snapshot, S),
    Snapshot(Snapshot, S),
    Subscribe(S),
    Live(Subscription),
    Done,
}

/// The body: `subscribe` registers the queue once the snapshot is out, as
/// Python's `async with bus.subscribe()` does after yielding it. A live event
/// Python cannot frame ends the stream with an error, like its exception.
pub fn body<F, S>(snapshot: F, subscribe: S, heartbeat: Duration) -> impl Stream<Item = io::Result<Bytes>> + Send
where
    F: Future<Output = Vec<Value>> + Send + 'static,
    S: FnOnce() -> Subscription + Send + 'static,
{
    let start = Phase::Retry(Box::pin(snapshot), subscribe);
    futures::stream::unfold(start, move |phase| async move {
        match phase {
            Phase::Retry(snap, sub) => Some((Ok(Bytes::from_static(RETRY)), Phase::Snapshot(snap, sub))),
            Phase::Snapshot(snap, sub) => {
                let text: String = snap.await.iter().filter_map(frame).collect();
                if text.is_empty() {
                    live(sub(), heartbeat).await
                } else {
                    Some((Ok(Bytes::from(text)), Phase::Subscribe(sub)))
                }
            }
            Phase::Subscribe(sub) => live(sub(), heartbeat).await,
            Phase::Live(sub) => live(sub, heartbeat).await,
            Phase::Done => None,
        }
    })
}

async fn live<S>(mut sub: Subscription, heartbeat: Duration) -> Option<(io::Result<Bytes>, Phase<S>)> {
    match tokio::time::timeout(heartbeat, sub.recv()).await {
        Err(_) => Some((Ok(Bytes::from_static(PING)), Phase::Live(sub))),
        Ok(Some(ev)) => match frame(&ev) {
            Some(f) => Some((Ok(Bytes::from(f)), Phase::Live(sub))),
            None => Some((Err(io::Error::other("event without a type")), Phase::Done)),
        },
        Ok(None) => None,
    }
}
