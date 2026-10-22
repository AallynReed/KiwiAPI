//! The api's `/internal/*` routes the website container calls with the shared
//! `X-Internal-Key`: page-view ingest (`app/pageviews/ingest.py`) and the
//! regional maintenance list (`app/geo_maintenance/router.py`). Without
//! `INTERNAL_KEY` both answer the plain 404 envelope.

pub mod pageviews;

#[cfg(test)]
mod tests;

use axum::{
    Router,
    extract::State,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde_json::{Value, json};

use crate::{
    error::ApiError,
    pyjson::PyJson,
    runtime_config,
    state::AppState,
    validation::{FromParams, Params, Query},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/internal/pageviews", post(pageviews::ingest))
        .route("/internal/geo-maintenance", get(geo_maintenance))
}

/// `internal_key_ok`: never true without a key; `hmac.compare_digest` on two
/// `str`s raises (a 500) when either holds non-ASCII text.
pub fn internal_key_ok(key: &str, supplied: &str) -> Result<bool, ApiError> {
    if key.is_empty() {
        return Ok(false);
    }
    if !key.is_ascii() || !supplied.is_ascii() {
        return Err(ApiError::internal());
    }
    let (a, b) = (key.as_bytes(), supplied.as_bytes());
    Ok(a.len() == b.len() && a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0)
}

pub fn not_found() -> ApiError {
    ApiError::not_found("Not found.")
}

/// `x_internal_key: str = Header(default="")`.
pub struct InternalKey(pub String);

impl FromParams for InternalKey {
    fn from_params(p: &mut Params<'_>) -> Self {
        InternalKey(p.header("x-internal-key").str().default(String::new()))
    }
}

/// `parse_countries`: unique upper-cased codes, sorted.
pub fn parse_countries(raw: &str) -> Vec<String> {
    let mut out: Vec<String> =
        raw.split(',').map(kiwi_core::pystr::strip).filter(|c| !c.is_empty()).map(str::to_uppercase).collect();
    out.sort();
    out.dedup();
    out
}

async fn geo_maintenance(State(state): State<AppState>, Query(InternalKey(supplied)): Query<InternalKey>) -> Response {
    match internal_key_ok(&state.settings().internal_key, &supplied) {
        Ok(true) => {}
        Ok(false) => return not_found().into_response(),
        Err(e) => return e.into_response(),
    }
    let raw = match runtime_config::get_setting(&state, "geo_maintenance_countries").await {
        Ok(v) => v,
        Err(err) => {
            tracing::error!("geo maintenance setting: {err:#}");
            return ApiError::internal().into_response();
        }
    };
    let Value::String(raw) = raw else {
        tracing::error!("geo_maintenance_countries is not a string");
        return ApiError::internal().into_response();
    };
    PyJson(json!({"countries": parse_countries(&raw)})).into_response()
}
