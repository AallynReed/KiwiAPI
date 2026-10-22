//! The leaderboards warmer (`detection.py`: `_warmup_loop`, `_warm_all`,
//! `_persisted_pass_age`, cold tiering) and the master routes that exist to
//! drive it: `POST /v1/leaderboards/warm`, `/delete-anchor` and
//! `/reingest-backlog` (`backlog.reingest`). Both move with the loop: they are
//! served here only under `RUST_OWNS=leaderboards_warmer`, since until then
//! only Python's warmer can act on them.

pub mod pass;
pub mod reingest;
pub mod routes;
pub mod store;

#[cfg(test)]
mod tests;

use std::future::Future;

use axum::{Router, routing::post};

use crate::{jobs, state::AppState};

pub const WARMER: &str = crate::cheaters::WARMER;

/// The routes, registered only while this server owns the warmer.
pub fn router() -> Router<AppState> {
    if jobs::owns(WARMER) { routes() } else { Router::new() }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/v1/leaderboards/warm", post(routes::warm))
        .route("/v1/leaderboards/delete-anchor", post(routes::delete_anchor))
        .route("/v1/leaderboards/reingest-backlog", post(routes::reingest_backlog))
}

/// Starts the loop when this server owns it.
pub fn start(state: &AppState) {
    jobs::spawn(state, WARMER, pass::run);
}

/// `detection.trigger_warmer()`: wake this server's warmer. While Python owns
/// it there is nothing here to wake.
pub fn trigger() {
    if jobs::owns(WARMER) {
        crate::cheaters::store::warmer_wake().notify_one();
    }
}

/// The in-process memos `service.reset_all` drops with the data.
pub fn reset_memos() {
    crate::cheaters::reset();
    crate::activity::player::reset_caches();
    crate::activity::class::reset_caches();
    crate::routes::lb_names::renames::reset_guard();
    crate::routes::lb_names::duplicates::reset_guard();
}

/// Drives `make()`'s future on a blocking-pool thread, so the CPU a pass or a
/// replay spends (decode, detection, JSON) never stalls the request workers.
pub async fn off_executor<F, Fut, T>(make: F) -> anyhow::Result<T>
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let handle = tokio::runtime::Handle::current();
    if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::CurrentThread {
        return Ok(make().await);
    }
    tokio::task::spawn_blocking(move || handle.block_on(make()))
        .await
        .map_err(|e| anyhow::anyhow!("background job died: {e}"))
}

/// Awaits `fut`, turning a panic into an error like an uncaught Python exception.
pub async fn guarded<T>(fut: impl Future<Output = anyhow::Result<T>>) -> anyhow::Result<T> {
    use futures::FutureExt;
    match std::panic::AssertUnwindSafe(fut).catch_unwind().await {
        Ok(r) => r,
        Err(p) => {
            let msg = p
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| p.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            Err(anyhow::anyhow!("panicked: {msg}"))
        }
    }
}
