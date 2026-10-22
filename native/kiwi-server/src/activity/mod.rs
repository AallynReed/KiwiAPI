//! Player and class activity (`app/trove/leaderboards/activity.py`,
//! `class_activity.py`), their `/v1/activity`, `/v1/class-activity` routes and
//! the `/site/leaderboards/{activity,class-activity}` proxies.
//!
//! For the leaderboards warmer: `player::estimate_active_players(state, true)`
//! and `class::estimate_class_activity(state)` compute and persist the latest
//! capture pair; `player::backfill_history_chunked` /
//! `class::backfill_class_history_chunked` replay a range.

pub mod cache;
pub mod class;
pub mod models;
pub mod player;
mod routes;
pub mod store;
#[cfg(test)]
mod tests;

use axum::{
    Router,
    routing::{get, post},
};

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/activity/current", get(routes::v1_current))
        .route("/v1/activity/history", get(routes::v1_history))
        .route("/v1/activity/series", get(routes::v1_series))
        .route("/v1/activity/backfill", post(routes::v1_backfill))
        .route("/v1/class-activity/current", get(routes::v1_class_current))
        .route("/v1/class-activity/series", get(routes::v1_class_series))
        .route("/v1/class-activity/backfill", post(routes::v1_class_backfill))
        .route("/site/leaderboards/activity", get(routes::site_current))
        .route("/site/leaderboards/activity/history", get(routes::site_history))
        .route("/site/leaderboards/activity/series", get(routes::site_series))
        .route("/site/leaderboards/class-activity/current", get(routes::site_class_current))
        .route("/site/leaderboards/class-activity/series", get(routes::site_class_series))
}

pub fn unix_now_f64() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// `int(time.time())`.
pub fn unix_now() -> i64 {
    unix_now_f64() as i64
}
