//! Route registry. Each ported feature area adds one module here and one
//! `.merge(...)` line in `ported()`. What no ported route serves is answered
//! the way Starlette would ([`crate::starlette`]), or by the Python app until
//! `KIWI_NATIVE_FALLBACK` is on. Port a path with every method Python serves
//! on it: a method missing here is a 405, not a trip to Python.

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
    http::Method,
    middleware::{Next, from_fn_with_state},
    response::Response,
};

use crate::{
    middleware,
    proxy::proxy,
    starlette::{self, Table},
    state::AppState,
};

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
        .merge(crate::tools::router())
        .merge(crate::ocr::router())
        .merge(crate::mods_catalog::router())
        .merge(crate::mod_releases::router())
        .merge(crate::mods_git::router())
        .merge(crate::mods_creators::router())
        .merge(crate::mods_issues::router())
        .merge(crate::mods_moderation::router())
        .merge(crate::modpacks::router())
        .merge(crate::custom_art::router())
        .merge(crate::mod_stats::router())
        .merge(crate::workshop::router())
        .merge(crate::rigs::router())
        .merge(crate::site_live::router())
        .merge(crate::creature::router())
        .merge(crate::openapi::router())
        .merge(crate::discord::router())
}

/// A body route can own only some bodies (portal emails needing
/// Unicode/IDNA), a render route only what it can draw: the rest go to Python
/// after routing but before the ported stack has touched the request.
async fn defer_to_python(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let matched = req.extensions().get::<MatchedPath>().cloned();
    let mut defer = false;
    if let Some(m) = &matched {
        defer = crate::blueprints::defers(&state, m.as_str(), req.uri()).await
            || crate::mod_releases::defers(&state, m.as_str(), req.uri()).await;
    }
    if !defer
        && let Some(field) = matched.as_ref().and_then(|m| crate::portal::defer::email_field(m.as_str(), req.method()))
    {
        (req, defer) = crate::portal::defer::inspect(req, field).await;
    }
    if !defer && matched.as_ref().is_some_and(|m| crate::custom_art::intake::inspects(m.as_str(), req.method())) {
        (req, defer) = crate::custom_art::intake::inspect(&state, req).await;
    }
    if !defer && let Some(kind) = matched.as_ref().and_then(|m| crate::images::defer_kind(m.as_str(), req.method())) {
        (req, defer) = crate::images::inspect(&state, req, kind).await;
    }
    if defer && let Some(&ConnectInfo(peer)) = req.extensions().get::<ConnectInfo<SocketAddr>>() {
        return proxy(State(state), ConnectInfo(peer), req).await;
    }
    next.run(req).await
}

/// Proxy mode only: what Starlette would route elsewhere (an int segment that
/// isn't one, a method another route owns, ...) and the event stream while
/// Python feeds it. OPTIONS stays here: the CORS layers answer it before any
/// routing, in both stacks.
async fn proxy_defers(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let defer = req.extensions().get::<MatchedPath>().is_some_and(|m| {
        crate::events::defers(m.as_str())
            || (req.method() != Method::OPTIONS && starlette::defers(Table::api(), m.as_str(), req.method(), req.uri()))
    });
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

/// The API app; `KIWI_NATIVE_FALLBACK` picks [`app_with`]'s mode.
pub fn app(state: AppState) -> Router {
    app_with(state, starlette::native_enabled())
}

/// `native`: every request is answered here, Starlette's way where no ported
/// route serves it. Otherwise those go to Python (`PY_UPSTREAM`).
pub fn app_with(state: AppState, native: bool) -> Router {
    // Kill switch: send everything to Python without redeploying.
    let proxy_all = std::env::var("KIWI_PROXY_ALL").is_ok_and(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"));
    let mw = Arc::new(middleware::Mw::from_state(&state));
    let table = Table::api();
    if native && !proxy_all {
        let routes = ported()
            .method_not_allowed_fallback(middleware::errors::method_not_allowed)
            .fallback(move |req: Request| starlette::fallback(table, req))
            .layer(from_fn_with_state(table, starlette::gate));
        let inner = middleware::wrap(routes, mw, Some(state.clone()))
            .layer(from_fn_with_state(state.clone(), defer_to_python))
            .with_state(state);
        return middleware::outer(inner).layer(from_fn_with_state(table, starlette::rewrite));
    }
    let routes = if proxy_all { Router::new() } else { ported() };
    let inner = middleware::wrap(routes, mw, Some(state.clone()))
        .layer(from_fn_with_state(state.clone(), defer_to_python))
        .layer(from_fn_with_state(state.clone(), proxy_defers))
        .fallback(proxy)
        .layer(from_fn_with_state(state.clone(), encoded_slash))
        .with_state(state);
    middleware::outer(inner)
}
