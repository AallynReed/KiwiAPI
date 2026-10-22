//! Small leaf routes: `/health`, `/config`, `/api-info` and FastAPI's docs
//! pages, the supporters list and its admin CRUD, design-feedback votes and
//! the content-moderation report endpoint.

mod body;
pub mod design_feedback;
mod docs_html;
pub mod meta;
pub mod moderation;
pub mod supporters;

#[cfg(test)]
mod tests;

use axum::Router;

use crate::{error::ApiError, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .merge(meta::router())
        .merge(supporters::router())
        .merge(design_feedback::router())
        .merge(moderation::router())
}

/// What an unexpected store error becomes: Python's unhandled-exception 500.
fn internal(err: impl std::fmt::Display) -> ApiError {
    tracing::error!("leaf route: {err}");
    ApiError::internal()
}
