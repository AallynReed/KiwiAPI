//! `app/events/scheduler.py` (`event_scheduler`): one task per time-driven
//! source that publishes, then sleeps until just past its next boundary.

use std::time::Duration;

use super::{
    bus,
    sources::{self, SOURCES, Source},
};
use crate::{routes::rotations::gamedata::Gamedata, state::AppState};

const MAX_SLEEP: i64 = 3600;
const MIN_SLEEP: i64 = 5;

/// Seconds until the next wake: 2s past the boundary, clamped to [5, 3600];
/// no boundary (or 0) waits the hour.
pub fn delay(next_at: Option<i64>, now: i64) -> i64 {
    let delay = match next_at {
        Some(n) if n != 0 => n - now + 2,
        _ => MAX_SLEEP,
    };
    delay.clamp(MIN_SLEEP, MAX_SLEEP)
}

pub async fn run(state: AppState) {
    let scheduled: Vec<Source> = SOURCES.into_iter().filter(|s| s.scheduled()).collect();
    tracing::info!("event scheduler started ({} source(s))", scheduled.len());
    for source in scheduled {
        tokio::spawn(run_source(state.clone(), source));
    }
}

async fn run_source(state: AppState, source: Source) {
    loop {
        if let Err(e) = bus::publish_source(&state, source).await {
            tracing::warn!("scheduler: publish {} failed: {e:#}", source.kind());
        }
        let now = sources::now_us();
        let wait = delay(source.next_at(Gamedata::get(), now), kiwi_core::rotations::unix(sources::now_us()));
        tokio::time::sleep(Duration::from_secs(wait as u64)).await;
    }
}
