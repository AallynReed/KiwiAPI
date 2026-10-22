//! Community feeds (`app/trove/{feeds,events,news}.py` and their routes):
//! news, Twitch / YouTube / Bilibili, the Bilibili thumbnail proxy and the
//! Trovesaurus calendar. The `feeds` and `trove_events` loops run here when
//! owned; the news loop publishes to the events bus and stays in Python.

pub mod events;
mod http;
pub mod image;
pub mod pyval;
pub mod relay;
pub mod routes;
pub mod store;

#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod tests;

use std::time::Duration;

use crate::{jobs, state::AppState};

pub use routes::router;

/// Register the `feeds` and `trove_events` loops (each only if owned).
pub fn spawn_loops(state: &AppState) {
    jobs::spawn_periodic(state, "feeds", relay::refresh_all, |s: AppState| async move {
        Duration::from_secs(s.settings().trove_feeds_refresh_seconds.max(0) as u64)
    });
    jobs::spawn_periodic(state, "trove_events", events::refresh_cycle, |s: AppState| async move {
        Duration::from_secs(s.settings().trove_events_refresh_seconds.max(0) as u64)
    });
}
