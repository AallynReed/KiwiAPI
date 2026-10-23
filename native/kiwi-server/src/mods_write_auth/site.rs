//! The site router's `_resolve_feature_flags` for `/site/mods/*` and
//! `/site/modpacks/*`: every site flag is read, then the Mods Hub switch
//! decides (a plain framework 404 while it is off).

use axum::{
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
};

use crate::{error, features, state::AppState};

pub struct SiteGate;

impl FromRequestParts<AppState> for SiteGate {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        let mut mods_on = true;
        for flag in crate::activity::SITE_FLAGS {
            let on = features::is_enabled(state, flag).await.map_err(|e| {
                tracing::error!("site feature flag {flag}: {e:#}");
                error::ApiError::internal().into_response()
            })?;
            if flag == features::MODS_HUB_FLAG {
                mods_on = on;
            }
        }
        let path = parts.uri.path();
        if !mods_on && (path.starts_with("/site/mods/") || path.starts_with("/site/modpacks/")) {
            return Err(error::http_exception(StatusCode::NOT_FOUND, None, &parts.method, path, &parts.headers));
        }
        Ok(SiteGate)
    }
}
