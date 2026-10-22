//! Site accounts (`app/site_auth`, the website "Dashboard", not the Dev
//! Portal): Discord-only sign-in, session cookies, profile, Trove-name claims,
//! username-change requests and the GDPR export / self-delete.

pub mod account;
pub mod email;
#[rustfmt::skip]
mod email_table;
pub mod oauth;
pub mod routes;
pub mod store;
pub mod usernames;

#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod tests;

use axum::{
    Router,
    routing::{delete, get, post},
};

use crate::{error, state::AppState};

/// Starlette's 405 names only the first route declared on a path.
fn not_allowed(allow: &'static str) -> impl Fn() -> std::future::Ready<axum::response::Response> + Clone {
    move || std::future::ready(error::method_not_allowed(allow))
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/site-auth/refresh", post(routes::refresh))
        .route("/v1/site-auth/logout", post(routes::logout))
        .route("/v1/site-auth/logout-all", post(routes::logout_all))
        .route("/v1/site-auth/me", get(routes::me).patch(routes::update_profile).fallback(not_allowed("GET")))
        .route(
            "/v1/site-auth/me/claim-trove-name",
            post(routes::claim).delete(routes::unclaim).fallback(not_allowed("POST")),
        )
        .route(
            "/v1/site-auth/me/username-request",
            get(usernames::get_request)
                .post(usernames::post_request)
                .delete(usernames::delete_request)
                .fallback(not_allowed("GET")),
        )
        .route("/v1/site-auth/me/verify-trove-claim", post(routes::verify_claim))
        .route("/v1/site-auth/me/trove-stats", get(routes::trove_stats))
        .route("/v1/site-auth/sessions", get(routes::list_sessions))
        .route("/v1/site-auth/sessions/{session_id}", delete(routes::revoke_session))
        .route("/v1/site-auth/me/export", get(account::export))
        .route("/v1/site-auth/me/delete", post(account::delete))
        .route("/v1/site-auth/oauth/discord/start", get(oauth::start))
        .route("/v1/site-auth/oauth/discord/callback", get(oauth::callback))
        .route("/v1/site-auth/oauth/exchange", post(oauth::exchange))
}
