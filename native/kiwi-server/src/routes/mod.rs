//! Route registry. Each ported feature area adds one module here and one
//! `.merge(...)` line in `ported()`. Anything not matched falls through to the
//! Python app. Port a path with every method Python serves on it: a method
//! missing here is a 405, not a trip to Python.

use std::sync::Arc;

use axum::Router;

use crate::{middleware, proxy::proxy, state::AppState};

/// Routes answered in Rust.
fn ported() -> Router<AppState> {
    Router::new()
}

pub fn app(state: AppState) -> Router {
    // Kill switch: send everything to Python without redeploying.
    let proxy_all = std::env::var("KIWI_PROXY_ALL").is_ok_and(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"));
    let routes = if proxy_all { Router::new() } else { ported() };
    let mw = Arc::new(middleware::Mw::from_state(&state));
    let inner = middleware::wrap(routes, mw).fallback(proxy).with_state(state);
    middleware::outer(inner)
}
