//! Route registry. Each ported feature area adds one module here and one
//! `.merge(...)` line in `ported()`. Anything not matched falls through to the
//! Python app. Port a path with every method Python serves on it: a method
//! missing here is a 405, not a trip to Python.

pub mod leaf;
pub mod rotations;
pub mod stats;

use std::sync::Arc;
use std::{net::SocketAddr, sync::Arc};

use axum::{
    Router,
    extract::{ConnectInfo, Request, State},
    middleware::{Next, from_fn_with_state},
    response::Response,
};

use crate::{middleware, proxy::proxy, state::AppState};

/// Routes answered in Rust.
fn ported() -> Router<AppState> {
    Router::new()
        .merge(leaf::router())
        .merge(rotations::router())
        .merge(stats::router())
}

/// axum matches the raw path, Starlette the decoded one, so a `%2F` (a `/`
/// once decoded, which can change the route or trigger a slash redirect)
/// always goes to Python.
async fn encoded_slash(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let raw = req.uri().path();
    if (raw.contains("%2F") || raw.contains("%2f"))
        && let Some(&ConnectInfo(peer)) = req.extensions().get::<ConnectInfo<SocketAddr>>()
    {
        return proxy(State(state), ConnectInfo(peer), req).await;
    }
    next.run(req).await
    Router::new().merge(crate::gems::router())
}

pub fn app(state: AppState) -> Router {
    // Kill switch: send everything to Python without redeploying.
    let proxy_all = std::env::var("KIWI_PROXY_ALL").is_ok_and(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"));
    let routes = if proxy_all { Router::new() } else { ported() };
    let mw = Arc::new(middleware::Mw::from_state(&state));
    let inner = middleware::wrap(routes, mw, Some(state.clone()))
        .fallback(proxy)
        .layer(from_fn_with_state(state.clone(), encoded_slash))
        .with_state(state);
    middleware::outer(inner)
}
