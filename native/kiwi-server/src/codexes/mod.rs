//! The codexes: the `/v1/codexes/*` read API (`app/trove/router.py`), the
//! `/site/codexes/*` proxies (`app/site/router.py`, renders excluded), the
//! recipe cost calculator, and the indexer that builds the Postgres tables
//! from the update archive with its two `/admin/codexes/*` routes.
//! `/v1/codexes/render` and `/site/codexes/render` stay with the renderer.

pub mod admin;
pub mod archive;
pub mod crafting;
pub mod indexer;
pub mod pg;
pub mod reads;
pub mod store;

#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tree_tests;

use axum::{
    Router,
    extract::FromRequestParts,
    http::request::Parts,
    routing::{get, post},
};
use kiwi_core::codex::types::ALL_TYPES;

use crate::{
    auth::{AccessContext, Rejection, extract::public_scope, scopes},
    error::ApiError,
    middleware::pyurl::scope_path,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/codexes/types", get(reads::v1_types))
        .route("/v1/codexes/search", get(reads::v1_search))
        .route("/v1/codexes/links", get(reads::v1_links))
        .route("/v1/codexes/stats", get(reads::v1_stats))
        .route("/v1/codexes/requirements", get(reads::v1_requirements))
        .route("/v1/codexes/upgrades", get(reads::v1_upgrades))
        .route("/v1/codexes/upgrades/{system_key}", get(reads::v1_upgrade_system))
        .route("/v1/codexes/{codex_type}", get(reads::v1_entries))
        .route("/v1/codexes/{codex_type}/categories", get(reads::v1_categories))
        .route("/v1/codexes/{codex_type}/stat-keys", get(reads::v1_stat_keys))
        .route("/v1/codexes/{codex_type}/abilities", get(reads::v1_abilities))
        .route("/v1/codexes/{codex_type}/entry", get(reads::v1_entry))
        .route("/site/codexes/types", get(reads::site_types))
        .route("/site/codexes/categories", get(reads::site_categories))
        .route("/site/codexes/stat-keys", get(reads::site_stat_keys))
        .route("/site/codexes/abilities", get(reads::site_abilities))
        .route("/site/codexes/search", get(reads::site_search))
        .route("/site/codexes/entry", get(reads::site_entry))
        .route("/site/codexes/related", get(reads::site_related))
        .route("/site/codexes/upgrades", get(reads::site_upgrades))
        .route("/site/codexes/crafting", get(reads::site_crafting))
        .route("/admin/codexes/rebuild", post(admin::rebuild))
        .route("/admin/codexes/status", get(admin::status))
}

pub const BRANCHES: [&str; 2] = ["live-us", "pts"];
pub const DEFAULT_BRANCH: &str = "live-us";

pub fn branch_message(branch: &str) -> String {
    format!("Unknown branch '{branch}' (known: {})", BRANCHES.join(", "))
}

pub fn type_message(t: &str) -> String {
    format!("Unknown codex type '{t}' (known: {})", ALL_TYPES.join(", "))
}

pub fn is_type(t: &str) -> bool {
    ALL_TYPES.contains(&t)
}

/// The decoded path segment at `index` (`/v1/codexes/{x}` is index 3).
pub fn segment(parts: &Parts, index: usize) -> String {
    scope_path(&parts.uri).split('/').nth(index).unwrap_or_default().to_owned()
}

/// `public_scope("codexes:read", rate_multiplier=settings.codexes_rate_limit_multiplier)`.
pub struct CodexScope(pub AccessContext);

impl FromRequestParts<AppState> for CodexScope {
    type Rejection = Rejection;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Rejection> {
        let m = state.settings().codexes_rate_limit_multiplier;
        Ok(CodexScope(public_scope(state, parts, scopes::CODEXES_READ, m, None).await?))
    }
}

/// `ensure_codex_indexed(branch, store, summary)`: the update archiver's
/// post-sync hook (`changed` / `ordinal` from its sync summary).
pub async fn after_sync(
    state: &AppState,
    branch: &str,
    changed: bool,
    ordinal: Option<i64>,
) -> anyhow::Result<indexer::Counts> {
    let a = admin::archive(state, branch);
    let local = state.settings().trove_local_game_dir.clone();
    indexer::ensure_indexed(state.inner.db.pg.as_ref(), &a, branch, changed, ordinal, &local).await
}

pub(crate) fn internal(e: impl std::fmt::Display) -> ApiError {
    tracing::error!("codexes: {e}");
    ApiError::internal()
}
