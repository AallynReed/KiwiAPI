//! The market scope: `/v1/market` reads and reset (`app/trove/router.py`),
//! the `/site/market/*` proxies and analytics and `/site/tomes`
//! (`app/site/router.py`), and the master interest-list and category admin
//! (`app/admin/router.py`), and the scraper's `POST /v1/market/insert`.

pub mod admin;
pub mod insert;
pub mod pg;
pub mod reads;
pub mod store;
pub mod tomes;

#[cfg(test)]
mod insert_tests;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod tests;

use std::marker::PhantomData;

use axum::{
    Router,
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
    routing::{delete, get, post},
};

use crate::{
    error::{self, ApiError},
    features::{self, Feature},
    middleware::pyurl::scope_path,
    state::AppState,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/market/listings", get(reads::v1_listings))
        .route("/v1/market/items", get(reads::v1_items))
        .route("/v1/market/items/{name}/summary", get(reads::v1_summary))
        .route(insert::PATH, post(insert::insert))
        .route("/v1/market/reset", post(reads::v1_reset))
        .route("/site/market/items", get(reads::site_items))
        .route("/site/market/item-images", get(reads::site_item_images))
        .route("/site/market/listings", get(reads::site_listings))
        .route("/site/market/items/{name}/summary", get(reads::site_summary))
        .route("/site/market/items/{name}/history", get(reads::site_history))
        .route("/site/market/analytics/timeline", get(reads::site_timeline))
        .route("/site/market/analytics/deals", get(reads::site_deals))
        .route("/site/market/analytics/movers", get(reads::site_movers))
        .route("/site/market/analytics/overview", get(reads::site_overview))
        .route("/site/market/analytics/signals", get(reads::site_signals))
        .route("/site/market/analytics/liquidity", get(reads::site_liquidity))
        .route("/site/market/analytics/volume", get(reads::site_volume))
        .route("/site/tomes", get(tomes::site_tomes))
        .route(
            "/admin/market/interest-items",
            get(admin::list_interest)
                .post(admin::add_interest)
                .put(admin::replace_interest)
                .fallback(|| async { error::method_not_allowed("GET") }),
        )
        .route("/admin/market/interest-items/{name}", delete(admin::remove_interest))
        .route(
            "/admin/market/categories",
            get(admin::list_categories)
                .post(admin::create_category)
                .fallback(|| async { error::method_not_allowed("GET") }),
        )
        .route(
            "/admin/market/categories/{category_id}",
            axum::routing::patch(admin::update_category)
                .delete(admin::delete_category)
                .fallback(|| async { error::method_not_allowed("PATCH") }),
        )
        // Starlette tries `/{category_id}` first, so only PUT reaches the reorder.
        .route(
            "/admin/market/categories/order",
            axum::routing::put(admin::reorder_categories)
                .patch(admin::update_category)
                .delete(admin::delete_category)
                .fallback(|| async { error::method_not_allowed("PATCH") }),
        )
}

/// The `/site` router's feature gate (`_resolve_feature_flags`): a plain
/// framework 404 while the feature is off, before anything else runs.
pub struct SiteFeature<F>(PhantomData<fn() -> F>);

impl<F: Feature> FromRequestParts<AppState> for SiteFeature<F> {
    type Rejection = ApiError;

    async fn from_request_parts(_parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        match features::is_enabled(state, F::FLAG).await {
            Ok(true) => Ok(SiteFeature(PhantomData)),
            Ok(false) => Err(ApiError::http(StatusCode::NOT_FOUND, None)),
            Err(e) => {
                tracing::error!("site feature flag {}: {e:#}", F::FLAG);
                Err(ApiError::internal())
            }
        }
    }
}

/// The `{name}` segment as Starlette sees it (decoded) between `prefix` and `suffix`.
pub fn path_segment(parts: &Parts, prefix: &str, suffix: &str) -> String {
    let path = scope_path(&parts.uri);
    path.strip_prefix(prefix).and_then(|p| p.strip_suffix(suffix)).unwrap_or_default().to_owned()
}

/// Decoded `{name}` of a request, as an extractor.
pub struct Segment<const P: usize>(pub String);

/// `(prefix, suffix)` of each `{name}` route.
const SEGMENTS: [(&str, &str); 5] = [
    ("/v1/market/items/", "/summary"),
    ("/site/market/items/", "/summary"),
    ("/site/market/items/", "/history"),
    ("/admin/market/interest-items/", ""),
    ("/admin/market/categories/", ""),
];

impl<S: Send + Sync, const P: usize> FromRequestParts<S> for Segment<P> {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        let (prefix, suffix) = SEGMENTS[P];
        Ok(Segment(path_segment(parts, prefix, suffix)))
    }
}
