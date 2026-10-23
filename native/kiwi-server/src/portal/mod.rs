//! The Dev Portal: accounts and sessions (`app/auth`), API token management
//! (`app/tokens`) and the GitHub secret-scanning partner webhook
//! (`app/scanning`).

pub mod account;
pub mod activity;
pub mod checks;
pub mod emails;
pub mod gdpr;
pub mod oauth;
pub mod req;
pub mod scanning;
pub mod sessions;
pub mod store;
pub mod tokens;

#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod tests;

use axum::{
    Router,
    routing::{delete, get, patch, post},
};

use crate::{error, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/signup", post(account::signup))
        .route("/auth/login", post(account::login))
        .route(
            "/auth/me",
            get(account::me).patch(account::update_profile).fallback(|| async { error::method_not_allowed("GET") }),
        )
        .route("/auth/verify-email", get(account::verify_email))
        .route("/auth/resend-verification", post(account::resend_verification))
        .route("/auth/change-password", post(account::change_password))
        .route("/auth/change-email", post(account::change_email))
        .route("/auth/verify-email-change", get(account::verify_email_change))
        .route("/auth/forgot-password", post(account::forgot_password))
        .route(
            "/auth/reset-password",
            post(account::reset_password)
                .get(account::reset_password_form)
                .fallback(|| async { error::method_not_allowed("POST") }),
        )
        .route("/auth/refresh", post(sessions::refresh))
        .route("/auth/logout", post(sessions::logout))
        .route("/auth/logout-all", post(sessions::logout_all))
        .route("/auth/sessions", get(sessions::list))
        .route("/auth/sessions/{session_id}", delete(sessions::revoke))
        .route("/auth/me/export", get(gdpr::export))
        .route("/auth/delete-account", post(gdpr::delete_account))
        .route("/auth/oauth/github/start", get(oauth::start))
        .route("/auth/oauth/github/callback", get(oauth::callback))
        .route("/auth/oauth/exchange", post(oauth::exchange))
        .route(
            "/tokens",
            post(tokens::create).get(tokens::list).fallback(|| async { error::method_not_allowed("POST") }),
        )
        // Starlette tries `/tokens/{token_id}` after the literal, so PATCH lands there.
        .route(
            "/tokens/activity",
            get(tokens::my_activity).patch(tokens::edit).fallback(|| async { error::method_not_allowed("GET") }),
        )
        .route("/tokens/{token_id}/activity", get(tokens::token_activity))
        .route("/tokens/{token_id}", patch(tokens::edit))
        .route("/tokens/{token_id}/rotate", post(tokens::rotate))
        .route("/tokens/{token_id}/revoke", post(tokens::revoke))
        .route("/secret-scanning/github", post(scanning::github))
}
