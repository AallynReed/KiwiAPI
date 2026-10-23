//! Mods Hub releases and the content-addressed images (`app/trove/mods_hub`
//! service release sections, `store.py`, `vfx.py`): cutting, repacking,
//! publishing and deleting releases, every artifact view under
//! `/v1/mods/hub/releases/*` and `/site/mods/releases/*`, the `mod_release`
//! event, and `/v1/mods/hub/image/{sha}` + `/site/mods/image/{sha}`.
//!
//! Left in Python: `.../assembled` (the creature assembler), rendering a
//! thumbnail width nobody has asked for yet (the Pillow WebP encode; a
//! recorded width is served here), and cutting a release from a commit, whose
//! path shares its GET with the catalog (`write::create` is ready for it).

pub mod access;
pub mod artifact;
pub mod event;
pub mod git;
pub mod images;
pub mod model;
pub mod read;
pub mod rig;
pub mod vfx;
pub mod write;
pub mod writeauth;

#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod tests;

use axum::{
    Router,
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
    routing::{get, post},
};

use crate::{
    auth::{AccessContext, Rejection, extract::public_scope, scopes},
    error::{self, ApiError},
    features,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    let get_only = || async { error::method_not_allowed("GET") };
    let post_only = || async { error::method_not_allowed("POST") };
    Router::new()
        .route(
            "/v1/mods/hub/releases/{release_id}",
            get(read::v1_release).patch(write::update).delete(write::delete).fallback(get_only),
        )
        .route("/v1/mods/hub/releases/{release_id}/download", get(read::v1_download).fallback(get_only))
        .route("/v1/mods/hub/releases/{release_id}/blueprints", get(read::v1_blueprints).fallback(get_only))
        .route("/v1/mods/hub/releases/{release_id}/blueprint", get(read::v1_blueprint).fallback(get_only))
        .route("/v1/mods/hub/releases/{release_id}/files", get(read::v1_files).fallback(get_only))
        .route("/v1/mods/hub/releases/{release_id}/file", get(read::v1_file).fallback(get_only))
        .route("/v1/mods/hub/releases/{release_id}/swfs", get(read::v1_swfs).fallback(get_only))
        .route("/v1/mods/hub/releases/{release_id}/swf/scripts", get(read::v1_swf_scripts).fallback(get_only))
        .route("/v1/mods/hub/releases/{release_id}/inspect", get(read::v1_inspect).fallback(get_only))
        .route("/v1/mods/hub/releases/{release_id}/cfgs", get(read::v1_cfgs).fallback(get_only))
        .route("/v1/mods/hub/releases/{release_id}/cfg", get(read::v1_cfg).fallback(get_only))
        .route("/v1/mods/hub/releases/{release_id}/config", post(write::attach_config).fallback(post_only))
        .route("/v1/mods/hub/projects/{handle}/{slug}/releases/upload", post(write::upload).fallback(post_only))
        .route("/v1/mods/hub/image/{sha}", get(images::v1_image).fallback(get_only))
        .route("/site/mods/image/{sha}", get(images::site_image).fallback(get_only))
        .route("/site/mods/releases/{release_id}/download", get(read::site_download).fallback(get_only))
        .route("/site/mods/releases/{release_id}/blueprints", get(read::site_blueprints).fallback(get_only))
        .route("/site/mods/releases/{release_id}/files", get(read::site_files).fallback(get_only))
        .route("/site/mods/releases/{release_id}/file", get(read::site_file).fallback(get_only))
        .route("/site/mods/releases/{release_id}/inspect", get(read::site_inspect).fallback(get_only))
        .route("/site/mods/releases/{release_id}/swfs", get(read::site_swfs).fallback(get_only))
        .route("/site/mods/releases/{release_id}/swf/scripts", get(read::site_swf_scripts).fallback(get_only))
        .route("/site/mods/releases/{release_id}/cfgs", get(read::site_cfgs).fallback(get_only))
        .route("/site/mods/releases/{release_id}/cfg", get(read::site_cfg).fallback(get_only))
        .route("/site/mods/releases/{release_id}/blueprint", get(read::site_blueprint).fallback(get_only))
        .route("/site/mods/releases/{release_id}/vfx", get(read::site_vfx).fallback(get_only))
        .route("/site/mods/releases/{release_id}/vfx/manifest", get(read::site_vfx_manifest).fallback(get_only))
        .route("/site/mods/releases/{release_id}/vfx/asset", get(read::site_vfx_asset).fallback(get_only))
        .route("/site/mods/releases/{release_id}/audio", get(read::site_audio).fallback(get_only))
        .route("/site/mods/releases/{release_id}/audio/bank", get(read::site_audio_bank).fallback(get_only))
        .route("/site/mods/releases/{release_id}/audio/sound", get(read::site_audio_sound).fallback(get_only))
}

/// For `routes::defer_to_python`: an image width that still has to be rendered.
pub async fn defers(state: &AppState, matched: &str, uri: &axum::http::Uri) -> bool {
    matches!(matched, "/v1/mods/hub/image/{sha}" | "/site/mods/image/{sha}") && images::defers(state, uri).await
}

/// `public_scope("mods:read", rate_multiplier=settings.mods_rate_limit_multiplier, bucket="mods:browse")`.
pub struct ModsPub(pub AccessContext);

impl FromRequestParts<AppState> for ModsPub {
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Rejection> {
        let m = state.settings().mods_rate_limit_multiplier;
        Ok(ModsPub(public_scope(state, parts, scopes::MODS_READ, m, Some("mods:browse")).await?))
    }
}

/// The `mods:asset` bucket.
pub struct ModsAsset(pub AccessContext);

impl FromRequestParts<AppState> for ModsAsset {
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Rejection> {
        let m = state.settings().mods_asset_rate_limit_multiplier;
        Ok(ModsAsset(public_scope(state, parts, scopes::MODS_READ, m, Some("mods:asset")).await?))
    }
}

/// The site router's `_resolve_feature_flags` for `/site/mods/*`.
pub struct SiteMods;

impl FromRequestParts<AppState> for SiteMods {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        let mut mods = true;
        for flag in crate::activity::SITE_FLAGS {
            let on = features::is_enabled(state, flag).await.map_err(|e| {
                tracing::error!("site feature flags: {e:#}");
                ApiError::internal().into_response()
            })?;
            if flag == features::MODS_HUB_FLAG {
                mods = on;
            }
        }
        if !mods {
            return Err(error::http_exception(
                StatusCode::NOT_FOUND,
                None,
                &parts.method,
                parts.uri.path(),
                &parts.headers,
            ));
        }
        Ok(SiteMods)
    }
}
