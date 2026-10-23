//! Background loops. `RUST_OWNS` (a comma list shared with Python's
//! `app/core/rust_owns.py`) names the loops this server runs; Python skips
//! exactly those, so a loop never runs twice. Register a loop in [`start`].

use std::{collections::HashSet, future::Future, sync::OnceLock, time::Duration};

use crate::state::AppState;

/// Every loop name either side knows (keep in step with `rust_owns.LOOPS`).
pub const LOOPS: [&str; 20] = [
    "usage",
    "email_outbox",
    "news",
    "feeds",
    "trove_events",
    "mod_stats",
    "steam_workshop",
    "chaos",
    "status_prober",
    "delves",
    "btt_releases",
    "update_archiver",
    "leaderboards_warmer",
    "giveaways",
    "event_listener",
    "event_watch",
    "event_scheduler",
    "webhooks",
    "dm_subs",
    "maintenance",
];

fn owned() -> &'static HashSet<String> {
    static OWNED: OnceLock<HashSet<String>> = OnceLock::new();
    OWNED.get_or_init(|| parse(&std::env::var("RUST_OWNS").unwrap_or_default()))
}

fn parse(raw: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    for name in raw.split(',').map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()) {
        if LOOPS.contains(&name.as_str()) {
            out.insert(name);
        } else {
            tracing::warn!("RUST_OWNS: unknown loop {name:?} ignored");
        }
    }
    out
}

/// Whether this server runs the loop `name` (and Python does not).
pub fn owns(name: &str) -> bool {
    owned().contains(name)
}

/// `PeriodicRefresher`: run `refresh` forever, sleeping `delay` (evaluated
/// after every cycle, so runtime-config changes apply) between cycles. A
/// failing cycle is logged and retried. Does nothing unless [`owns`] `name`.
pub fn spawn_periodic<R, RF, D, DF>(state: &AppState, name: &'static str, refresh: R, delay: D)
where
    R: Fn(AppState) -> RF + Send + Sync + 'static,
    RF: Future<Output = anyhow::Result<()>> + Send,
    D: Fn(AppState) -> DF + Send + Sync + 'static,
    DF: Future<Output = Duration> + Send,
{
    if !owns(name) {
        return;
    }
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

/// Spawn a loop with its own schedule. Does nothing unless [`owns`] `name`.
pub fn spawn<F, Fut>(state: &AppState, name: &'static str, run: F)
where
    F: FnOnce(AppState) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    if owns(name) {
        tokio::spawn(run(state.clone()));
    }
}

/// Start every loop this server owns. Called once from `main`.
pub fn start(state: &AppState) {
    for name in owned() {
        tracing::info!("running loop {name}");
    }
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_like_python() {
        let got = parse(" News, feeds,,nope ,STATUS_PROBER");
        let mut v: Vec<_> = got.into_iter().collect();
        v.sort();
        assert_eq!(v, ["feeds", "news", "status_prober"]);
    }
}
