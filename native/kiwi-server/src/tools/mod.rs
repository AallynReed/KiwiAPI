//! Stateless file tools: `POST /v1/mods/read` and `/v1/mods/build`
//! (`app/trove/router.py` `mods_router`) and `POST /site/unlock-debug`
//! (`app/site/router.py`). Nothing is stored.

pub mod pybytes;
pub mod tmod;
pub mod unlock_debug;

#[cfg(test)]
mod tests;

use axum::{Router, routing::post};

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/mods/read", post(tmod::read_route))
        .route("/v1/mods/build", post(tmod::build_route))
        .route("/site/unlock-debug", post(unlock_debug::route))
}
