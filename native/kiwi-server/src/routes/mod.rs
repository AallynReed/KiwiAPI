//! Route registry. Each ported feature area adds one module here and one
//! `.merge(...)` line in `ported()`. Anything not matched falls through to the
//! Python app. Port a path with every method Python serves on it: a method
//! missing here is a 405, not a trip to Python.

pub mod btt;
pub mod lb_names;
pub mod lb_players;
pub mod leaf;
pub mod misc;
pub mod rotations;
pub mod stats;
pub mod store;
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
        .merge(lb_players::router())
        .merge(crate::activity::router())
        .merge(lb_names::router())
        .merge(crate::lb_ingest::router())
        .merge(crate::cheaters::router())
        .merge(crate::lb_warmer::router())
        .merge(crate::market::router())
        .merge(store::router())
        .merge(crate::events::router())
        .merge(crate::portal::router())
        .merge(crate::site_auth::router())
        .merge(crate::admin::router())
        .merge(crate::giveaways::router())
        .merge(crate::webhooks::router())
        .merge(crate::dm_subs::router())
        .merge(crate::drops::router())
        .merge(crate::internal::router())
        .merge(crate::og::router())
        .merge(crate::archiver::router())
        .merge(crate::updates::router())
        .merge(crate::decode::routes::router())
        .merge(crate::codexes::router())
        .merge(crate::media::router())
        .merge(crate::blueprints::router())
        .merge(crate::images::router())
        .merge(crate::embed::router())
        .merge(crate::dressing::router())
}

/// A param route can own only some values of its segment (Starlette's
/// `{uuid:int}` next to literals Python still serves), and a body route some
/// bodies (portal emails needing Unicode/IDNA): the rest go to Python after
/// routing but before the ported stack has touched the request.
async fn defer_to_python(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let matched = req.extensions().get::<MatchedPath>().cloned();
    let mut defer = matched.as_ref().is_some_and(|m| {
        let m = m.as_str();
        crate::lbread::defers(m, req.method(), req.uri().path())
            || crate::events::defers(m)
            || crate::og::defers(m, req.uri())
            || crate::decode::routes::defers(m, req.method(), req.uri())
            || crate::codexes::defers(m, req.uri())
    });
    if !defer && let Some(m) = &matched {
        defer = crate::blueprints::defers(&state, m.as_str(), req.uri()).await;
    }
    if !defer
        && let Some(field) = matched.as_ref().and_then(|m| crate::portal::defer::email_field(m.as_str(), req.method()))
    {
        (req, defer) = crate::portal::defer::inspect(req, field).await;
    }
    if !defer && let Some(kind) = matched.as_ref().and_then(|m| crate::images::defer_kind(m.as_str(), req.method())) {
        (req, defer) = crate::images::inspect(&state, req, kind).await;
    }
    if defer && let Some(&ConnectInfo(peer)) = req.extensions().get::<ConnectInfo<SocketAddr>>() {
        return proxy(State(state), ConnectInfo(peer), req).await;
    }
    next.run(req).await
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
