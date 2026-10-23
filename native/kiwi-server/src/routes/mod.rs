//! Route registry. Each feature area adds one module and one `.merge(...)`
//! line in `router()`. What no route serves is answered the way Starlette
//! would ([`crate::starlette`]).

pub mod btt;
pub mod lb_names;
pub mod lb_players;
pub mod leaf;
pub mod misc;
pub mod rotations;
pub mod stats;
pub mod store;
pub mod trove_status;

use std::sync::Arc;

use axum::{Router, extract::Request, middleware::from_fn_with_state};

use crate::{
    middleware,
    starlette::{self, Table},
    state::AppState,
};

/// Every API route.
fn router() -> Router<AppState> {
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

/// The API app. Paths no route serves, and matches axum makes that Starlette
/// would not, are answered the way Starlette answers them.
pub fn app(state: AppState) -> Router {
    let mw = Arc::new(middleware::Mw::from_state(&state));
    let table = Table::api();
    let routes = router()
        .method_not_allowed_fallback(middleware::errors::method_not_allowed)
        .fallback(move |req: Request| starlette::fallback(table, req))
        .layer(from_fn_with_state(table, starlette::gate));
    let inner = middleware::wrap(routes, mw, Some(state.clone())).with_state(state);
    middleware::outer(inner).layer(from_fn_with_state(table, starlette::rewrite))
}
