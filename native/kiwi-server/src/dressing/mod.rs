//! The Dressing Room's read API (`app/trove/dressing`). Only the palette is
//! answered here: the catalogue, outfit resolution, still and model need the
//! codex locale map, the rig map, the baked rigs, the assembler, the voxel
//! rasterizer and the payload cache, which stay in Python for now.

use axum::{
    Router,
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::json;

use crate::{
    auth::{Public, scopes},
    error::method_not_allowed,
    features::{DressingRoom, RequireFeature},
    pyjson::PydanticJson,
    state::AppState,
};

/// `palette.HAIR`, as Trove's picker lays it out.
pub const HAIR: [&str; 18] = [
    "#7a4e24", "#e01b0c", "#d6197a", "#3fd016", "#2a46b8", "#1e2224", "#b5762a", "#f08010", "#f221c7", "#2fd9a8",
    "#7a18e0", "#8a9096", "#c2b24a", "#f2e06a", "#f9a8d8", "#3fe0f0", "#6e8cf0", "#ffffff",
];

/// `palette.EYE`.
pub const EYE: [&str; 18] = [
    "#7a1f1f", "#6b4a1f", "#1f5a33", "#1f5a9e", "#4a2a7a", "#1e2224", "#e01b0c", "#f08010", "#2fcf4a", "#5a93e8",
    "#8560c8", "#8a9096", "#f221c7", "#f2e020", "#3fe0a8", "#3fd8f0", "#a020f0", "#ffffff",
];

pub const COLUMNS: i64 = 6;

pub fn router() -> Router<AppState> {
    Router::new().route("/v1/dressing/palette", get(palette).fallback(|| async { method_not_allowed("GET") }))
}

pub fn palette_body() -> serde_json::Value {
    json!({"columns": COLUMNS, "hair": HAIR, "eyes": EYE})
}

async fn palette(_: RequireFeature<DressingRoom>, Public(ctx): Public<{ scopes::CODEXES_READ }>) -> Response {
    (ctx.rate_limit, PydanticJson(palette_body())).into_response()
}

#[cfg(test)]
mod tests {
    #[test]
    fn palette_matches_python() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/embed/golden.json");
        let g: serde_json::Value = serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap();
        assert_eq!(crate::pyjson::pydantic(&super::palette_body()), g["palette"].as_str().unwrap());
    }
}
