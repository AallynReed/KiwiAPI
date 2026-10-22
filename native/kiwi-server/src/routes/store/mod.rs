//! The store catalog: `store_router` of `app/trove/router.py` (reads behind
//! `store:read`, the master reset) and the `/site/store/*` proxies of
//! `app/site/router.py`. `POST /v1/store/insert` stays in Python because it
//! publishes to the events bus; the parser and ingest rules it needs are
//! ported in `kiwi_core::store`.

pub mod model;
pub mod service;

#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod tests;

use std::path::PathBuf;

use axum::{
    Router,
    extract::{FromRequestParts, State},
    http::{HeaderValue, StatusCode, Uri, header, request::Parts},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use mongodb::bson::doc;
use serde_json::{Value, json};

use self::{
    model::Flavour,
    service::{Fetch, Mongo, ProductsQuery},
};
use crate::{
    auth::{IngestAuth, MasterIngest, RequireScope, pyutil::bson_int, scopes},
    error::ApiError,
    features::{self, RequireFeature},
    middleware::pyurl::scope_path,
    pyjson::{PyJson, PydanticJson},
    ratelimit::RateLimitInfo,
    state::AppState,
    validation::{FromParams, Params, Query},
};

type StoreRead = RequireScope<{ scopes::STORE_READ }>;
type Gate = RequireFeature<features::Store>;

const SITE_CACHE: &str = "public, max-age=120";
const TEXTURE_CACHE: &str = "public, max-age=86400";
pub const V1_PRODUCT: &str = "/v1/store/products/";
pub const SITE_PRODUCT: &str = "/site/store/products/";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/store/products", get(v1_products))
        .route("/v1/store/categories", get(v1_categories))
        .route("/v1/store/timeline", get(v1_timeline))
        .route("/v1/store/products/{code}", get(v1_product))
        .route("/v1/store/reset", post(v1_reset))
        .route("/site/store/products", get(site_products))
        .route("/site/store/categories", get(site_categories))
        .route("/site/store/timeline", get(site_timeline))
        .route("/site/store/products/{code}", get(site_product))
        .route("/site/store/texture", get(site_texture))
}

/// The site router's `_resolve_feature_flags`: a disabled store is a bare 404.
pub struct SiteGate;

impl FromRequestParts<AppState> for SiteGate {
    type Rejection = ApiError;

    async fn from_request_parts(_parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        match features::is_enabled(state, features::STORE_FLAG).await {
            Ok(true) => Ok(SiteGate),
            Ok(false) => Err(ApiError::http(StatusCode::NOT_FOUND, None)),
            Err(e) => service::unhandled(format!("site feature flags: {e:#}")),
        }
    }
}

/// `limit` defaults to 100 on `/v1`, 500 on `/site`.
pub struct ProductsParams<const LIMIT: i64>(pub ProductsQuery);

impl<const LIMIT: i64> FromParams for ProductsParams<LIMIT> {
    fn from_params(p: &mut Params<'_>) -> Self {
        ProductsParams(ProductsQuery {
            category: p.query("category").int().optional(),
            kind: p.query("kind").str().optional(),
            currency: p.query("currency").str().optional(),
            q: p.query("q").str().optional(),
            active: p.query("active").bool().default(true),
            on_sale: p.query("on_sale").bool().default(false),
            limit: p.query("limit").int().ge(1).le(1000).default(LIMIT),
            offset: p.query("offset").int().ge(0).default(0),
        })
    }
}

pub struct TimelineParams {
    pub kind: Option<String>,
    pub limit: i64,
}

impl FromParams for TimelineParams {
    fn from_params(p: &mut Params<'_>) -> Self {
        TimelineParams {
            kind: p.query("kind").str().optional(),
            limit: p.query("limit").int().ge(1).le(4000).default(1000),
        }
    }
}

pub struct TextureParams(pub String);

impl FromParams for TextureParams {
    fn from_params(p: &mut Params<'_>) -> Self {
        TextureParams(p.query("path").str().required())
    }
}

fn mongo(state: &AppState) -> Mongo<'_> {
    Mongo { db: state.mongo(), blob_root: PathBuf::from(&state.settings().trove_update_store_dir) }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// The `{code}` segment as Starlette matched it (percent-decoded, lossy).
pub fn code_of(uri: &Uri, prefix: &str) -> String {
    scope_path(uri).strip_prefix(prefix).unwrap_or_default().to_owned()
}

pub fn v1_json(rate_limit: Option<RateLimitInfo>, body: Value) -> Response {
    (rate_limit, PydanticJson(body)).into_response()
}

pub fn site_json(body: Value) -> Response {
    ([(header::CACHE_CONTROL, HeaderValue::from_static(SITE_CACHE))], PyJson(body)).into_response()
}

async fn v1_products(
    State(state): State<AppState>,
    _: Gate,
    RequireScope(ctx): StoreRead,
    Query(ProductsParams(q)): Query<ProductsParams<100>>,
) -> Response {
    v1_json(Some(ctx.rate_limit), service::products_page(&mongo(&state), &q, Flavour::V1).await)
}

async fn v1_categories(State(state): State<AppState>, _: Gate, RequireScope(ctx): StoreRead) -> Response {
    v1_json(Some(ctx.rate_limit), service::categories(&mongo(&state)).await)
}

async fn v1_timeline(
    State(state): State<AppState>,
    _: Gate,
    RequireScope(ctx): StoreRead,
    Query(q): Query<TimelineParams>,
) -> Response {
    v1_json(Some(ctx.rate_limit), service::timeline(&mongo(&state), q.kind.as_deref(), q.limit, now()).await)
}

pub async fn product_response<F: Fetch>(
    f: &F,
    code: &str,
    now: i64,
    flavour: Flavour,
    rate_limit: Option<RateLimitInfo>,
) -> Response {
    match (service::product_detail(f, code, now, flavour).await, flavour) {
        (Some(v), Flavour::V1) => v1_json(rate_limit, v),
        (Some(v), Flavour::Site) => site_json(v),
        (None, Flavour::V1) => ApiError::not_found(format!("No store product with code '{code}'")).into_response(),
        (None, Flavour::Site) => ApiError::http(StatusCode::NOT_FOUND, Some("No such store product")).into_response(),
    }
}

async fn v1_product(State(state): State<AppState>, _: Gate, RequireScope(ctx): StoreRead, uri: Uri) -> Response {
    product_response(&mongo(&state), &code_of(&uri, V1_PRODUCT), now(), Flavour::V1, Some(ctx.rate_limit)).await
}

pub async fn reset_response<F: Fetch>(f: &F, auth: &IngestAuth, now: chrono::DateTime<chrono::Utc>) -> Response {
    let removed = service::reset(f).await as i64;
    let row = service::ingest_log_doc("/v1/store/reset", auth, doc! {"removed": bson_int(removed)}, now);
    service::record_ingest(f, row).await;
    v1_json(auth.rate_limit, json!({"reset": true, "removed": removed}))
}

async fn v1_reset(State(state): State<AppState>, _: Gate, MasterIngest(auth): MasterIngest) -> Response {
    reset_response(&mongo(&state), &auth, crate::dt::now()).await
}

async fn site_products(
    State(state): State<AppState>,
    _: SiteGate,
    Query(ProductsParams(q)): Query<ProductsParams<500>>,
) -> Response {
    site_json(service::products_page(&mongo(&state), &q, Flavour::Site).await)
}

async fn site_categories(State(state): State<AppState>, _: SiteGate) -> Response {
    site_json(service::categories(&mongo(&state)).await)
}

async fn site_timeline(State(state): State<AppState>, _: SiteGate, Query(q): Query<TimelineParams>) -> Response {
    site_json(service::timeline(&mongo(&state), q.kind.as_deref(), q.limit, now()).await)
}

async fn site_product(State(state): State<AppState>, _: SiteGate, uri: Uri) -> Response {
    product_response(&mongo(&state), &code_of(&uri, SITE_PRODUCT), now(), Flavour::Site, None).await
}

pub async fn texture_response<F: Fetch>(f: &F, path: &str) -> Response {
    let Some(sha) = service::texture_sha(f, path, service::TEXTURE_BRANCH).await else {
        return ApiError::http(StatusCode::NOT_FOUND, Some("texture not found")).into_response();
    };
    let Some(data) = f.blob(&sha).await else {
        return ApiError::http(StatusCode::NOT_FOUND, Some("blob missing")).into_response();
    };
    (
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(service::texture_media_type(path))),
            (header::CACHE_CONTROL, HeaderValue::from_static(TEXTURE_CACHE)),
        ],
        data,
    )
        .into_response()
}

async fn site_texture(
    State(state): State<AppState>,
    _: SiteGate,
    Query(TextureParams(path)): Query<TextureParams>,
) -> Response {
    texture_response(&mongo(&state), &path).await
}
