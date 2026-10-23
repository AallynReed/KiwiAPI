//! Mods Hub catalog: projects, profiles, stars, collaborators, remixes,
//! claims and popularity (`app/trove/mods_hub`), the public `/v1/mods` API
//! over them, and the `/site/mods/*` catalog proxies. Releases, git, issues,
//! images and moderation are other components; their paths stay in Python.

pub mod announce;
pub mod bodies;
pub mod dto;
pub mod git;
pub mod model;
pub mod routes;
pub mod service;
pub mod stats;
pub mod write_auth;
pub mod writes;

#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod tests;

use axum::{
    Router,
    http::Uri,
    routing::{delete, get, patch, post},
};

use crate::{error, middleware::pyurl::scope_path, state::AppState};

const PROJECT: &str = "/v1/mods/hub/projects/{handle}/{slug}";

pub fn router() -> Router<AppState> {
    use routes::*;
    let na = |allow: &'static str| move || async move { error::method_not_allowed(allow) };
    let sub = |s: &str| format!("{PROJECT}/{s}");
    Router::new()
        .route("/v1/mods/hub/projects", get(hub_list).post(create_project).fallback(na("GET")))
        .route("/v1/mods/hub/tags", get(hub_tags).fallback(na("GET")))
        .route("/v1/mods/hub/profile/{handle}", get(hub_profile).fallback(na("GET")))
        .route(PROJECT, get(hub_project).patch(update_project).delete(delete_project).fallback(na("GET")))
        .route(&sub("releases"), get(hub_releases).post(crate::mod_releases::write::create).fallback(na("GET")))
        .route(&sub("forks"), get(hub_forks).fallback(na("GET")))
        .route(&sub("fork"), post(fork_project).fallback(na("POST")))
        .route(&sub("remixes"), get(list_remixes).fallback(na("GET")))
        .route(&sub("remixes/{remix_id}/status"), post(set_remix_status).fallback(na("POST")))
        .route(&sub("remix-claim"), post(remix_claim).fallback(na("POST")))
        .route(&sub("banner"), post(upload_banner).delete(delete_banner).fallback(na("POST")))
        .route(&sub("previews"), post(upload_previews).fallback(na("POST")))
        .route(&sub("previews/{sha}"), delete(delete_preview).fallback(na("DELETE")))
        .route(&sub("star"), post(star).delete(unstar).fallback(na("POST")))
        .route(&sub("collaborators"), post(add_collaborator).fallback(na("POST")))
        .route(&sub("collaborators/{user_id}"), delete(remove_collaborator).fallback(na("DELETE")))
        .route(&sub("claim"), post(claim).fallback(na("POST")))
        .route("/v1/mods/hub/me/projects", get(me_projects).fallback(na("GET")))
        .route("/v1/mods/hub/me/projects/{handle}/{slug}", get(me_project).fallback(na("GET")))
        .route("/v1/mods/hub/me/projects/{handle}/{slug}/releases", get(me_releases).fallback(na("GET")))
        .route("/v1/mods/hub/me/starred", get(me_starred).fallback(na("GET")))
        .route("/v1/mods/hub/me/stats", get(me_stats).fallback(na("GET")))
        .route("/v1/mods/hub/me/profile", patch(update_profile).fallback(na("PATCH")))
        .route("/v1/mods/hub/me/profile/avatar", post(upload_avatar).fallback(na("POST")))
        .route("/v1/mods/hub/me/profile/banner", post(upload_profile_banner).fallback(na("POST")))
        .route("/v1/mods", get(public_list).fallback(na("GET")))
        .route("/v1/mods/popular", get(public_popular).fallback(na("GET")))
        .route("/v1/mods/lookup", post(lookup).fallback(na("POST")))
        .route("/v1/mods/categories", get(categories).fallback(na("GET")))
        .route("/v1/mods/{handle}/{slug}", get(public_detail).fallback(na("GET")))
        .route("/site/mods/projects", get(site_projects).fallback(na("GET")))
        .route("/site/mods/tags", get(site_tags).fallback(na("GET")))
        .route("/site/mods/profile/{handle}", get(site_profile).fallback(na("GET")))
        .route("/site/mods/modders", get(site_modders).fallback(na("GET")))
        .route("/site/mods/me/projects", get(site_my_projects).fallback(na("GET")))
        .route("/site/mods/projects/{handle}/{slug}", get(site_project).fallback(na("GET")))
        .route("/site/mods/projects/{handle}/{slug}/releases", get(site_releases).fallback(na("GET")))
        .route("/site/mods/projects/{handle}/{slug}/forks", get(site_forks).fallback(na("GET")))
}

/// For `routes::defer_to_python`: the `POST` on the releases path belongs to
/// the release component, and `/v1/mods/hub/creator-links` (caught here by
/// `/v1/mods/{handle}/{slug}`) to the creator-link one.
pub fn defers(matched: &str, uri: &Uri) -> bool {
    match matched {
        "/v1/mods/{handle}/{slug}" => scope_path(uri) == "/v1/mods/hub/creator-links",
        _ => false,
    }
}
