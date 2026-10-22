//! Leaderboard dump ingest and maintenance: `POST /v1/leaderboards/insert`
//! (the capture bot), `/reset`, `/reingest-status`, and the `/admin`
//! leaderboards maintenance routes. Re-ingest, delete-anchor and warm stay in
//! Python: they run Python-only compute (activity backfills, the warmer).
//!
//! The pieces the warmer port needs are public: [`store`] (snapshot write,
//! player-aggregate jobs, cold tier) and [`signals`] (cache, events, wake-up).

pub mod admin;
pub mod backlog;
pub mod ingest;
pub mod routes;
pub mod signals;
pub mod store;

#[cfg(test)]
mod tests;

use axum::{
    Router,
    routing::{get, patch, post},
};

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/leaderboards/insert", post(routes::insert))
        .route("/v1/leaderboards/reset", post(routes::reset))
        .route("/v1/leaderboards/reingest-status", get(routes::reingest_status))
        .route("/admin/leaderboards/boards", get(admin::list_boards))
        .route("/admin/leaderboards/boards/{uuid}", patch(admin::patch_board))
        .route("/admin/leaderboards/rebuild-player-agg", post(admin::start_rebuild))
        .route("/admin/leaderboards/rebuild-player-agg/status", get(admin::rebuild_status))
        .route("/admin/leaderboards/backfill-last-rise", post(admin::start_last_rise))
        .route("/admin/leaderboards/backfill-last-rise/status", get(admin::last_rise_status))
        .route("/admin/leaderboards/tier-cold", post(admin::start_cold))
        .route("/admin/leaderboards/tier-cold/status", get(admin::cold_status))
        .route("/admin/ingest/log", get(admin::ingest_log))
}
