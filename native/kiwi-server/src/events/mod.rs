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

/// The stream stays with Python until this process runs the listener that
/// feeds it (`RUST_OWNS=event_listener` switches Python's off).
pub fn defers(route: &str) -> bool {
    route == STREAM_PATH && !jobs::owns(LISTENER)
}

pub fn start_jobs(state: &AppState) {
    jobs::spawn(state, LISTENER, bus::listen);
    jobs::spawn(state, "event_watch", bus::watch);
    jobs::spawn(state, "event_scheduler", scheduler::run);
}
