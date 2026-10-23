//! Background loops, all started once from `main` ([`start`]).

use std::{future::Future, time::Duration};

use crate::state::AppState;

/// `PeriodicRefresher`: run `refresh` forever, sleeping `delay` (evaluated
/// after every cycle, so runtime-config changes apply) between cycles. A
/// failing cycle is logged and retried.
pub fn spawn_periodic<R, RF, D, DF>(state: &AppState, name: &'static str, refresh: R, delay: D)
where
    R: Fn(AppState) -> RF + Send + Sync + 'static,
    RF: Future<Output = anyhow::Result<()>> + Send,
    D: Fn(AppState) -> DF + Send + Sync + 'static,
    DF: Future<Output = Duration> + Send,
{
    tracing::info!("running loop {name}");
    let state = state.clone();
    tokio::spawn(async move {
        loop {
            if let Err(err) = refresh(state.clone()).await {
                tracing::warn!("{name} failed: {err:#}");
            }
            tokio::time::sleep(delay(state.clone()).await).await;
        }
    });
}

/// Spawn a loop with its own schedule.
pub fn spawn<F, Fut>(state: &AppState, name: &'static str, run: F)
where
    F: FnOnce(AppState) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    tracing::info!("running loop {name}");
    tokio::spawn(run(state.clone()));
}

/// Start every loop. Called once from `main`.
pub fn start(state: &AppState) {
    crate::routes::rotations::start_jobs(state);
    crate::routes::btt::start_jobs(state);
    crate::feeds::spawn_loops(state);
    crate::lb_warmer::start(state);
    crate::events::start_jobs(state);
    crate::routes::trove_status::prober::start(state);
    crate::giveaways::spawn_worker(state, |st| async move {
        crate::events::publish_type(&st, "giveaways").await.map(|_| ())
    });
    crate::webhooks::start_jobs(state);
    crate::dm_subs::start_jobs(state);
    crate::email::worker::start(state);
    crate::maintenance::start(state);
    crate::archiver::set_decode_hook(|st, branch| {
        Box::pin(async move { crate::decode::runner::ensure_built(&st, &branch).await })
    });
    crate::archiver::set_codex_hook(|st, _store, summary| {
        Box::pin(async move {
            crate::codexes::after_sync(&st, &summary.branch, summary.changed, summary.ordinal).await.map(|_| ())
        })
    });
    crate::archiver::start(state);
    crate::mod_stats::start(state);
}
