//! `/site/updates/{branch}/file/blueprint` (the viewer payload of an archived
//! blueprint, through the payload cache) and `/site/render/brdf-map.png`.

use std::sync::{LazyLock, Mutex};

use axum::{
    extract::{FromRequestParts, Request, State},
    http::{HeaderValue, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
};
use serde_json::Value;

use super::{
    SiteGate,
    cache::{self, BuildError, Fmt},
    dds,
};
use crate::{
    error::{self, ApiError, ErrorCode},
    lbread::routes::file_meta_sha,
    state::AppState,
    validation::{FromParams, Params, Path},
};

const BRANCHES: [&str; 2] = ["live-us", "pts"];

struct FileQuery {
    branch: String,
    path: String,
    fmt: String,
}

impl FromParams for FileQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        FileQuery {
            branch: p.path("branch").str().required(),
            path: p.query("path").str().required(),
            fmt: p.query("fmt").str().one_of_pattern(&["json", "bin"]).default("json".into()),
        }
    }
}

fn http(p: &Parts, status: StatusCode, detail: &str) -> Response {
    error::http_exception(status, Some(detail), &p.method, p.uri.path(), &p.headers)
}

/// `GET /site/updates/{branch}/file/blueprint`.
pub async fn site_file_blueprint(State(state): State<AppState>, req: Request) -> Response {
    let (mut parts, _) = req.into_parts();
    if let Err(r) = SiteGate::from_request_parts(&mut parts, &state).await {
        return r;
    }
    let q = match Path::<FileQuery>::from_request_parts(&mut parts, &state).await {
        Ok(Path(q)) => q,
        Err(r) => return r.into_response(),
    };
    if !BRANCHES.contains(&q.branch.as_str()) {
        let detail = format!("Unknown branch '{}' (known: {})", q.branch, BRANCHES.join(", "));
        return http(&parts, StatusCode::NOT_FOUND, &detail);
    }
    let sha = match file_meta_sha(state.mongo(), &q.branch, &q.path).await {
        Ok(Some(sha)) => sha,
        Ok(None) => return http(&parts, StatusCode::NOT_FOUND, &format!("No file '{}'", q.path)),
        Err(e) => {
            tracing::error!("file/blueprint meta: {e:#}");
            return ApiError::internal().into_response();
        }
    };
    let fmt = if q.fmt == "bin" { Fmt::Bin } else { Fmt::Json };
    let root = state.inner.settings.trove_update_store_dir.clone();
    let (path, blob, p2) = (q.path.clone(), sha.clone(), parts.clone());
    let build = || async move {
        let raw = tokio::task::spawn_blocking(move || cache::store_get(&root, &blob))
            .await
            .map_err(|_| BuildError::Api(ApiError::internal()))?
            .map_err(|e| {
                tracing::error!("file/blueprint blob: {e}");
                BuildError::Api(ApiError::internal())
            })?;
        let Some(raw) = raw else {
            return Err(BuildError::Other(http(&p2, StatusCode::NOT_FOUND, "Blob missing from the store")));
        };
        let packed = tokio::task::spawn_blocking(move || {
            kiwi_core::voxel::pack(&raw, &path, kiwi_core::voxel::RENDER_VOXEL_CAP)
        })
        .await
        .map_err(|_| BuildError::Api(ApiError::internal()))?;
        use kiwi_core::voxel::PackError;
        match packed {
            Ok(Value::Object(m)) => Ok(m),
            Ok(_) => Err(BuildError::Api(ApiError::internal())),
            Err(PackError::TooLarge(n)) => {
                tracing::info!("blueprint too large: {n} voxels");
                Err(ApiError::new(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    ErrorCode::BadRequest,
                    "This blueprint is too large to preview.",
                )
                .into())
            }
            Err(PackError::Empty | PackError::Invalid(_)) => Err(ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                ErrorCode::BadRequest,
                "This blueprint is empty or could not be decoded.",
            )
            .into()),
            Err(PackError::Unhandled(m)) => {
                tracing::error!("file/blueprint decode: {m}");
                Err(BuildError::Api(ApiError::internal()))
            }
        }
    };
    match cache::get_or_build(&state, &cache::key_for_file(&sha, &q.path), fmt, build).await {
        Ok(cached) => cache::respond(&parts.headers, &cached, 300),
        Err(e) => e.into_response(),
    }
}

const BRDF_PATH: &str = "textures/brdfmap.dds";
/// `_brdf_png`: `(blob sha, PNG)`.
type Memo = Option<(String, Vec<u8>)>;
static BRDF: LazyLock<Mutex<Memo>> = LazyLock::new(Default::default);

fn brdf_png(dds: &[u8]) -> Option<Vec<u8>> {
    let (w, h, rgb) = dds::to_rgb(dds)?;
    Some(kiwi_pil::png::encode_rgb(&rgb, w, h))
}

/// `GET /site/render/brdf-map.png`.
pub async fn brdf_map(State(state): State<AppState>, req: Request) -> Response {
    let (mut parts, _) = req.into_parts();
    if let Err(r) = SiteGate::from_request_parts(&mut parts, &state).await {
        return r;
    }
    let sha = match file_meta_sha(state.mongo(), "live-us", BRDF_PATH).await {
        Ok(sha) => sha.filter(|s| !s.is_empty()),
        Err(e) => {
            tracing::error!("brdf map meta: {e:#}");
            return ApiError::internal().into_response();
        }
    };
    let Some(sha) = sha else { return http(&parts, StatusCode::NOT_FOUND, "brdf map not in the archive") };
    let reply = |png: Vec<u8>| {
        let mut res = Response::new(axum::body::Body::from(png));
        let h = res.headers_mut();
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=604800"));
        h.insert(header::ETAG, HeaderValue::from_str(&format!("\"{sha}\"")).unwrap_or(HeaderValue::from_static("")));
        h.insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
        res
    };
    let memo = BRDF.lock().ok().and_then(|m| m.as_ref().filter(|(s, _)| *s == sha).map(|(_, p)| p.clone()));
    if let Some(png) = memo {
        return reply(png);
    }
    let blob = cache::blob_path(&state.inner.settings.trove_update_store_dir, &sha);
    if !blob.is_file() {
        return http(&parts, StatusCode::NOT_FOUND, "blob missing");
    }
    let converted = tokio::task::spawn_blocking(move || std::fs::read(blob).ok().and_then(|d| brdf_png(&d))).await;
    let Ok(Some(png)) = converted else {
        tracing::warn!("brdf map: could not convert {sha}");
        return http(&parts, StatusCode::NOT_FOUND, "brdf map unreadable");
    };
    if let Ok(mut m) = BRDF.lock() {
        *m = Some((sha.clone(), png.clone()));
    }
    reply(png)
}
