//! Route registry. Each ported feature area adds one module here and one
//! `.merge(...)` line in `ported()`. Anything not matched falls through to the
//! Python app. Port a path with every method Python serves on it: a method
//! missing here is a 405, not a trip to Python.

pub mod btt;
pub mod lb_players;
pub mod leaf;
pub mod misc;
pub mod rotations;
pub mod stats;
pub mod trove_status;

use std::{net::SocketAddr, sync::Arc};

use axum::{
    Router,
    extract::{ConnectInfo, MatchedPath, Request, State},
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
        .merge(crate::gems::router())
        .merge(misc::router())
        .merge(trove_status::router())
        .merge(btt::router())
        .merge(crate::feeds::router())
        .merge(crate::lbread::router())
}

/// A param route can own only some values of its segment (Starlette's
/// `{uuid:int}` next to literals Python still serves): the rest go to Python
/// after routing but before the ported stack has touched the request.
async fn defer_to_python(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let defer = req
        .extensions()
        .get::<MatchedPath>()
        .is_some_and(|m| crate::lbread::defers(m.as_str(), req.method(), req.uri().path()));
    if defer && let Some(&ConnectInfo(peer)) = req.extensions().get::<ConnectInfo<SocketAddr>>() {
        return proxy(State(state), ConnectInfo(peer), req).await;
    }
    next.run(req).await
        .merge(lb_players::router())
        .merge(crate::activity::router())
}

/// axum matches the raw path, Starlette the decoded one, so a `%2F` (a `/`
/// once decoded, which can change the route or trigger a slash redirect)
/// always goes to Python. So does an empty segment (`//`): axum binds it to a
/// `{param}`, Starlette's `[^/]+` never does.
async fn encoded_slash(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let raw = req.uri().path();
    if (raw.contains("%2F") || raw.contains("%2f") || raw.contains("//"))
        && let Some(&ConnectInfo(peer)) = req.extensions().get::<ConnectInfo<SocketAddr>>()
    {
        return proxy(State(state), ConnectInfo(peer), req).await;
    }
    next.run(req).await
}

pub fn app(state: AppState) -> Router {
    // Kill switch: send everything to Python without redeploying.
    let proxy_all = std::env::var("KIWI_PROXY_ALL").is_ok_and(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"));
    let routes = if proxy_all { Router::new() } else { ported() };
    let mw = Arc::new(middleware::Mw::from_state(&state));
    let inner = middleware::wrap(routes, mw, Some(state.clone()))
        .layer(from_fn_with_state(state.clone(), defer_to_python))
        .fallback(proxy)
        .layer(from_fn_with_state(state.clone(), encoded_slash))
        .with_state(state);
    middleware::outer(inner)
}
