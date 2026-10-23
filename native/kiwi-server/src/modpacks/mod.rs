//! Modpacks (`app/trove/modpacks`): user-curated bundles of Mods Hub mod
//! references, built into a `.tpack`, `.zip` or configs zip on the fly. The
//! hub surface and the documented catalog ride the Mods Hub gate; the
//! `/site/modpacks/*` proxies sit on the same service.

pub mod artifact;
pub mod body;
pub mod hub;
pub mod model;
pub mod routes;
pub mod service;
pub mod text;

#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod tests;

use axum::{
    Router,
    routing::{delete, get, patch, post, put},
};

use crate::{error::method_not_allowed, state::AppState};

const HUB: &str = "/v1/modpacks/hub/projects/{handle}/{slug}";

pub fn router() -> Router<AppState> {
    use routes as r;
    let allow = |m: &'static str| move || async move { method_not_allowed(m) };
    let hub = |rest: &str| format!("{HUB}{rest}");
    Router::new()
        .route("/v1/modpacks/hub/projects", get(r::hub_list).post(r::hub_create).fallback(allow("GET")))
        .route(HUB, get(r::hub_get).patch(r::hub_update).delete(r::hub_delete).fallback(allow("GET")))
        .route(&hub("/download"), get(r::hub_download))
        .route("/v1/modpacks/hub/me/projects", get(r::hub_mine))
        .route(&hub("/banner"), post(r::hub_banner))
        .route(&hub("/variants"), post(r::hub_add_variant))
        .route(
            &hub("/variants/{name}"),
            patch(r::hub_update_variant).delete(r::hub_delete_variant).fallback(allow("PATCH")),
        )
        .route(&hub("/variants/{name}/entries"), put(r::hub_set_entries))
        .route(&hub("/variants/{name}/upload"), post(r::hub_upload_entry))
        .route(&hub("/collaborators"), post(r::hub_add_collaborator))
        .route(&hub("/collaborators/{user_id}"), delete(r::hub_remove_collaborator))
        .route(&hub("/star"), post(r::hub_star).delete(r::hub_unstar).fallback(allow("POST")))
        .route("/v1/modpacks", get(r::public_list))
        .route("/v1/modpacks/for-mod/{handle}/{slug}", get(r::public_for_mod))
        .route("/v1/modpacks/{handle}/{slug}", get(r::public_get))
        .route("/v1/modpacks/{handle}/{slug}/download", get(r::public_download))
        .route("/site/modpacks/projects", get(r::site_list))
        .route("/site/modpacks/me/projects", get(r::site_mine))
        .route("/site/modpacks/projects/{handle}/{slug}", get(r::site_get))
        .route("/site/modpacks/for-mod/{handle}/{slug}", get(r::site_for_mod))
        .route("/site/modpacks/projects/{handle}/{slug}/download", get(r::site_download))
}
