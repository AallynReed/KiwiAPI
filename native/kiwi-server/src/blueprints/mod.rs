//! Blueprints and the voxel renderer: the stateless `/site/blueprint-editor/*`
//! tools (`app/trove/blueprint`), the codex thumbnails (`/v1/codexes/render`,
//! `/site/codexes/render`), the viewer payload of an archived blueprint behind
//! the shared payload cache (`app/trove/render/bp_cache.py`) and the BRDF atlas.
//!
//! The model projects (`/site/blueprint-editor/{model,game-model,model-save}`)
//! are in `crate::rigs::model`. Left in Python: an assembled-creature render
//! (`?prefab=`) that isn't in the Redis cache yet.

pub mod cache;
pub mod dds;
pub mod editor;
pub mod files;
pub mod pack;
pub mod render;

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

use crate::{error, features, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/site/blueprint-editor/inspect", post(editor::inspect))
        .route("/site/blueprint-editor/flatten", post(editor::flatten))
        .route("/site/blueprint-editor/transform", post(editor::transform))
        .route("/site/blueprint-editor/export-qb", post(editor::export_qb))
        .route("/site/blueprint-editor/import-qb", post(editor::import_qb))
        .route("/site/blueprint-editor/check", post(editor::check))
        .route("/site/blueprint-editor/save", post(editor::save))
        .route("/v1/codexes/render", get(render::v1_render))
        .route("/site/codexes/render", get(render::site_render))
        .route("/site/updates/{branch}/file/blueprint", get(files::site_file_blueprint))
        .route("/site/render/brdf-map.png", get(files::brdf_map))
}

/// `_resolve_feature_flags` for these paths: every site flag is read, then a
/// disabled feature's path is the framework 404.
pub struct SiteGate;

impl FromRequestParts<AppState> for SiteGate {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        let mut on = std::collections::HashMap::new();
        for flag in crate::activity::SITE_FLAGS {
            let v = features::is_enabled(state, flag).await.map_err(|e| {
                tracing::error!("site feature flag {flag}: {e:#}");
                error::ApiError::internal().into_response()
            })?;
            on.insert(flag, v);
        }
        let off = |flag: &str| on.get(flag) == Some(&false);
        let path = parts.uri.path();
        let blocked = (off(features::BLUEPRINT_EDITOR_FLAG) && path.starts_with("/site/blueprint-editor"))
            || (off(features::CODEXES_FLAG) && path.starts_with("/site/codexes/"))
            || (off(features::UPDATES_FLAG) && path.starts_with("/site/updates/"));
        if blocked {
            return Err(error::http_exception(StatusCode::NOT_FOUND, None, &parts.method, path, &parts.headers));
        }
        Ok(SiteGate)
    }
}
