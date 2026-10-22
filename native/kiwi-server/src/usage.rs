//! `app/usage`: one `usage_events` row per request an API-token extractor identified, buffered.
//! Wire with `route_layer(from_fn_with_state(state, usage::record))`; call `usage::shutdown()` on exit.

use std::{
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

use axum::{
    extract::{MatchedPath, Request, State},
    middleware::Next,
    response::Response,
};
use mongodb::{
    Collection,
    bson::{DateTime, oid::ObjectId},
};
use serde::Serialize;
use tokio::sync::Notify;

use crate::{auth::pyutil::round2, state::AppState};

pub const USAGE_COLLECTION: &str = "usage_events";
const FLUSH_INTERVAL: Duration = Duration::from_secs(3);
const FLUSH_AT: usize = 200;
const MAX_BUFFER: usize = 10_000;

/// `UsageEvent`, fields in model order (`_id` first, as pymongo inserts it).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct UsageEvent {
    #[serde(rename = "_id")]
    pub id: ObjectId,
    pub user_id: ObjectId,
    pub token_id: ObjectId,
    pub method: String,
    pub route: String,
    pub path: String,
    pub status_code: i32,
    pub duration_ms: f64,
    pub created_at: DateTime,
}

/// Placed in request extensions by [`record`]; token extractors fill it with
/// `(user_id, token_id)` (Python's `request.state.usage_*`).
#[derive(Clone, Default)]
pub struct UsageSlot(Arc<Mutex<Option<(ObjectId, ObjectId)>>>);

impl UsageSlot {
    pub fn set(&self, user_id: ObjectId, token_id: ObjectId) {
        *self.0.lock().expect("usage slot") = Some((user_id, token_id));
    }

    pub fn get(&self) -> Option<(ObjectId, ObjectId)> {
        *self.0.lock().expect("usage slot")
    }
}

/// FastAPI templates with a typed converter axum can't spell.
const TYPED_TEMPLATES: &[&str] = &[
    "/v1/leaderboards/{uuid:int}",
    "/v1/leaderboards/{uuid:int}/entries",
    "/v1/leaderboards/{uuid:int}/history",
    "/v1/leaderboards/{uuid:int}/health",
];

/// Axum route template -> FastAPI's (`{*rest}` -> `{rest:path}`, typed
/// converters restored), so per-route stats stay continuous across the cutover.
pub fn fastapi_template(axum_path: &str) -> String {
    let out = untyped_template(axum_path);
    match TYPED_TEMPLATES.iter().find(|t| t.replace(":int}", "}") == out) {
        Some(t) => (*t).to_owned(),
        None => out,
    }
}

fn untyped_template(axum_path: &str) -> String {
    let mut out = String::with_capacity(axum_path.len() + 8);
    let mut rest = axum_path;
    while let Some(start) = rest.find("{*") {
        out.push_str(&rest[..start]);
        let tail = &rest[start + 2..];
        let end = tail.find('}').unwrap_or(tail.len());
        out.push('{');
        out.push_str(&tail[..end]);
        out.push_str(":path}");
        rest = tail.get(end + 1..).unwrap_or("");
    }
    out.push_str(rest);
    out
}

/// Starlette's `request.url.path` of a raw path: percent-decoded (UTF-8, lossy).
pub fn starlette_path(raw: &str) -> String {
    crate::middleware::pyurl::request_url_path(&percent_encoding::percent_decode_str(raw).decode_utf8_lossy())
}

struct Recorder {
    buffer: Mutex<Vec<UsageEvent>>,
    wake: Notify,
    collection: Option<Collection<UsageEvent>>,
}

static RECORDER: OnceLock<Arc<Recorder>> = OnceLock::new();

fn recorder(state: &AppState) -> &'static Arc<Recorder> {
    RECORDER.get_or_init(|| {
        let rec = Arc::new(Recorder {
            buffer: Mutex::new(Vec::new()),
            wake: Notify::new(),
            collection: state.inner.db.mongo.as_ref().map(|db| db.collection(USAGE_COLLECTION)),
        });
        let looped = rec.clone();
        tokio::spawn(async move {
            loop {
                let _ = tokio::time::timeout(FLUSH_INTERVAL, looped.wake.notified()).await;
                looped.flush().await;
            }
        });
        rec
    })
}

impl Recorder {
    fn push(&self, event: UsageEvent) {
        let mut buf = self.buffer.lock().expect("usage buffer");
        buf.push(event);
        if buf.len() > MAX_BUFFER {
            let dropped = buf.len() - MAX_BUFFER;
            buf.drain(..dropped);
            tracing::warn!("usage buffer full - dropped {dropped} event(s)");
        }
        if buf.len() >= FLUSH_AT {
            self.wake.notify_one();
        }
    }

    async fn flush(&self) {
        let batch = std::mem::take(&mut *self.buffer.lock().expect("usage buffer"));
        if batch.is_empty() {
            return;
        }
        let Some(coll) = &self.collection else {
            tracing::warn!("usage: no Mongo configured, dropped {} event(s)", batch.len());
            return;
        };
        let n = batch.len();
        if let Err(err) = coll.insert_many(batch).await {
            tracing::error!("Failed to flush {n} usage event(s): {err}");
        }
    }
}

/// Queue one event (never blocks the request).
pub fn enqueue(state: &AppState, event: UsageEvent) {
    recorder(state).push(event);
}

/// Drain whatever is buffered (call on shutdown).
pub async fn shutdown() {
    if let Some(rec) = RECORDER.get() {
        rec.flush().await;
    }
}

/// The usage middleware.
pub async fn record(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let start = Instant::now();
    let slot = UsageSlot::default();
    req.extensions_mut().insert(slot.clone());
    let method = req.method().as_str().to_owned();
    let raw_path = req.uri().path().to_owned();
    let route = req.extensions().get::<MatchedPath>().map(|m| fastapi_template(m.as_str()));

    let response = next.run(req).await;

    if let Some((user_id, token_id)) =
        slot.get().filter(|_| response.extensions().get::<crate::error::Unhandled>().is_none())
    {
        let path = starlette_path(&raw_path);
        enqueue(
            &state,
            UsageEvent {
                id: ObjectId::new(),
                user_id,
                token_id,
                method,
                route: route.unwrap_or_else(|| path.clone()),
                path,
                status_code: i32::from(response.status().as_u16()),
                duration_ms: round2(start.elapsed().as_secs_f64() * 1000.0),
                created_at: DateTime::now(),
            },
        );
    }
    response
}
