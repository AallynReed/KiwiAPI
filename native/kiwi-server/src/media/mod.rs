//! Game media out of the updates archive: SWF artwork and ActionScript
//! (`app/trove/swf`), Wwise banks and Sound Studio (`app/trove/audio`), and
//! their `/site` routes in `app/site/router.py`.

pub mod audio;
pub mod bpcache;
pub mod cas;
pub mod pil;
pub mod swf;
pub mod zipfile;

#[cfg(test)]
mod tests;

use axum::{
    Router,
    extract::FromRequestParts,
    http::{HeaderValue, StatusCode, request::Parts},
    response::{IntoResponse, Response},
};

use crate::{error::ApiError, features, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new().merge(swf::router()).merge(audio::router())
}

/// `cdn.BRANCHES`.
pub const BRANCHES: [&str; 2] = ["live-us", "pts"];

/// `_site_check_branch`.
pub fn check_branch(branch: &str) -> Result<(), ApiError> {
    if BRANCHES.contains(&branch) {
        return Ok(());
    }
    let detail = format!("Unknown branch '{branch}' (known: {})", BRANCHES.join(", "));
    Err(ApiError::http(StatusCode::NOT_FOUND, Some(&detail)))
}

/// The site router's `_resolve_feature_flags` dependency for the paths served
/// here: every site flag resolved (a lookup failure is a 500), then the bare
/// 404 when this path's feature is off.
pub struct SiteGate;

impl FromRequestParts<AppState> for SiteGate {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        let (mut updates, mut studio) = (true, true);
        for flag in crate::activity::SITE_FLAGS {
            let on = features::is_enabled(state, flag).await.map_err(|e| {
                tracing::error!("site feature flags: {e:#}");
                ApiError::internal().into_response()
            })?;
            match flag {
                features::UPDATES_FLAG => updates = on,
                features::SOUND_STUDIO_FLAG => studio = on,
                _ => {}
            }
        }
        let p = parts.uri.path();
        if (!updates && p.starts_with("/site/updates/")) || (!studio && p.starts_with("/site/sound-studio")) {
            return Err(ApiError::http(StatusCode::NOT_FOUND, None).into_response());
        }
        Ok(SiteGate)
    }
}

/// `updates_read.get_file_meta(branch, path)["content_sha256"]`.
pub async fn file_sha(state: &AppState, branch: &str, path: &str) -> Result<Option<String>, ApiError> {
    #[cfg(test)]
    if let Some(archive) = tests::ARCHIVE.get() {
        return Ok(archive.get(&format!("{branch}:{path}")).cloned());
    }
    let db = state.inner.db.mongo.as_ref().ok_or_else(ApiError::internal)?;
    crate::lbread::routes::file_meta_sha(db, branch, path).await.map_err(|e| {
        tracing::error!("update file meta {branch}:{path}: {e:#}");
        ApiError::internal()
    })
}

pub fn update_store(state: &AppState) -> cas::ContentStore {
    cas::ContentStore::new(&state.settings().trove_update_store_dir)
}

pub fn mods_store(state: &AppState) -> cas::ContentStore {
    cas::ContentStore::new(&state.settings().mods_store_dir)
}

/// `ContentStore.get` off the async runtime; an I/O error is Python's 500.
pub async fn read_blob(store: cas::ContentStore, sha: String) -> Result<Option<Vec<u8>>, ApiError> {
    tokio::task::spawn_blocking(move || store.get(&sha)).await.map_err(|_| ApiError::internal())?.map_err(|e| {
        tracing::error!("store read: {e}");
        ApiError::internal()
    })
}

/// A header value Starlette encodes as latin-1; anything outside it is the
/// `UnicodeEncodeError` 500.
pub fn latin1_header(s: &str) -> Result<HeaderValue, ApiError> {
    let bytes: Option<Vec<u8>> = s.chars().map(|c| u8::try_from(u32::from(c)).ok()).collect();
    bytes.and_then(|b| HeaderValue::from_bytes(&b).ok()).ok_or_else(ApiError::internal)
}

/// `path.rsplit("/", 1)[-1].rsplit(".", 1)[0] or default`.
pub fn file_stem<'a>(path: &'a str, default: &'a str) -> &'a str {
    let base = path.rsplit('/').next().unwrap_or(path);
    let stem = match base.rfind('.') {
        Some(i) => &base[..i],
        None => base,
    };
    if stem.is_empty() { default } else { stem }
}

/// `"".join(c if c.isalnum() or c in keep else "_" for c in s)`.
pub fn sanitize(s: &str, keep: &str) -> String {
    s.chars().map(|c| if kiwi_core::pystr::is_alnum(c) || keep.contains(c) { c } else { '_' }).collect()
}
