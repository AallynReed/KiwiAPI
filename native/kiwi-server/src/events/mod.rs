//! The live event bus (`app/events`): exactly-once publishing over Redis, the
//! event sources, the SSE stream and its loops (`event_listener`,
//! `event_watch`, `event_scheduler`).
//!
//! Producers in other areas call [`publish`], [`publish_type`] or the
//! `publish_<source>` shortcuts.

pub mod bus;
pub mod scheduler;
pub mod sources;
pub mod stream;

#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod tests;

use axum::{Router, routing::get};

pub use bus::{
    current_snapshot, publish, publish_challenge, publish_chaos, publish_luxion, publish_source, publish_type,
};
pub use sources::Source;

use crate::{jobs, state::AppState};

pub const STREAM_PATH: &str = "/v1/events/stream";
pub const LISTENER: &str = "event_listener";

pub fn router() -> Router<AppState> {
    Router::new().route(STREAM_PATH, get(stream::stream))
}

/// Whether this process feeds its own streams: always once nothing is proxied,
/// else when `RUST_OWNS=event_listener` switched Python's listener off.
pub fn listener_runs() -> bool {
    jobs::owns(LISTENER) || crate::starlette::native_enabled()
}

/// Proxy mode: the stream stays with Python until this process runs the listener.
pub fn defers(route: &str) -> bool {
    route == STREAM_PATH && !listener_runs()
}

pub fn start_jobs(state: &AppState) {
    if listener_runs() {
        tokio::spawn(bus::listen(state.clone()));
    }
    jobs::spawn(state, "event_watch", bus::watch);
    jobs::spawn(state, "event_scheduler", scheduler::run);
}
