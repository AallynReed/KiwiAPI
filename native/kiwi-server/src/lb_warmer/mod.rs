//! The leaderboards warmer (`detection.py`: `_warmup_loop`, `_warm_all`,
//! `_persisted_pass_age`, cold tiering) and the master routes that exist to
//! drive it: `POST /v1/leaderboards/warm`, `/delete-anchor` and
//! `/reingest-backlog` (`backlog.reingest`).

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

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/leaderboards/warm", post(routes::warm))
        .route("/v1/leaderboards/delete-anchor", post(routes::delete_anchor))
        .route("/v1/leaderboards/reingest-backlog", post(routes::reingest_backlog))
}

pub fn start(state: &AppState) {
    jobs::spawn(state, WARMER, pass::run);
}

/// `detection.trigger_warmer()`: wake the warmer.
pub fn trigger() {
    crate::cheaters::store::warmer_wake().notify_one();
}

/// The in-process memos `service.reset_all` drops with the data.
pub fn reset_memos() {
    crate::cheaters::reset();
    crate::activity::player::reset_caches();
    crate::activity::class::reset_caches();
    crate::routes::lb_names::renames::reset_guard();
    crate::routes::lb_names::duplicates::reset_guard();
    crate::lbread::store::reset_charts();
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
