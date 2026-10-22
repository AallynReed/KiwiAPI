//! Route registry. Each ported feature area adds one module here and one
//! `.merge(...)` line in `ported()`. Anything not matched falls through to the
//! Python app.

use axum::Router;

use crate::{proxy::proxy, state::AppState};

/// Routes answered in Rust.
fn ported() -> Router<AppState> {
    Router::new()
}

pub fn app(state: AppState) -> Router {
    ported().fallback(proxy).with_state(state)
}
