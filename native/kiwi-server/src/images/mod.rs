//! Image Studio (`app/images`): the Dashboard CRUD under `/v1/images` (feature
//! gate, then the site session), the upload, the live preview and the public
//! `/site/images/{id}.png` render Discord fetches.
//!
//! A design whose image blobs are not PNGs Rust decodes like Pillow (JPEG,
//! WebP, GIF, 16-bit greyscale) renders in Python, as does any render while
//! DejaVu is not on disk (`inspect`).

pub mod contexts;
pub mod model;
pub mod render;
pub mod service;

#[cfg(test)]
mod tests;

use axum::{
    Router,
    body::Body,
    extract::{FromRequestParts, Request, State},
    http::{HeaderValue, Method, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use kiwi_core::form::{self, FormError, FormValue};
use serde_json::{Value, json};

use crate::{
    auth::CurrentSiteUser,
    error::{ApiError, method_not_allowed},
    features::{self, ImageStudio, RequireFeature},
    pyjson::{PydanticJson, no_content},
    state::AppState,
    validation::{self, DepsThenBody, Fail, Params},
};
use model::DesignBody;

const PREFIX: &str = "/v1/images/";
const PNG_ROUTE: &str = "/site/images/{file}";

pub fn router() -> Router<AppState> {
    let get_only = || async { method_not_allowed("GET") };
    let post_only = || async { method_not_allowed("POST") };
    Router::new()
        .route("/v1/images/bindings", get(bindings).put(update_design).delete(delete_design).fallback(get_only))
        .route("/v1/images", get(list_designs).post(create_design).fallback(get_only))
        .route(
            "/v1/images/upload",
            post(upload).get(get_design).put(update_design).delete(delete_design).fallback(post_only),
        )
        .route(
            "/v1/images/preview",
            post(preview).get(get_design).put(update_design).delete(delete_design).fallback(post_only),
        )
        .route("/v1/images/{design_id}", get(get_design).put(update_design).delete(delete_design).fallback(get_only))
        .route(PNG_ROUTE, get(render_png).fallback(get_only))
}

/// `{design_id}` as Starlette decodes it.
pub struct DesignId(pub String);

impl<S: Send + Sync> FromRequestParts<S> for DesignId {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        let path = crate::middleware::pyurl::scope_path(&parts.uri);
        Ok(DesignId(path.strip_prefix(PREFIX).unwrap_or_default().to_owned()))
    }
}

fn reply(r: crate::error::ApiResult<Value>) -> Response {
    match r {
        Ok(v) => PydanticJson(v).into_response(),
        Err(e) => e.into_response(),
    }
}

fn png(bytes: bytes::Bytes, cache_control: &'static str) -> Response {
    let mut res = Response::new(Body::from(bytes));
    res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(cache_control));
    res
}

async fn bindings(_: RequireFeature<ImageStudio>, _: CurrentSiteUser) -> PydanticJson {
    PydanticJson(json!({"bindings": contexts::bindings()}))
}

async fn list_designs(
    State(state): State<AppState>,
    _: RequireFeature<ImageStudio>,
    CurrentSiteUser(user): CurrentSiteUser,
) -> Response {
    reply(service::list(&state, user.id).await)
}

async fn create_design(
    State(state): State<AppState>,
    DepsThenBody((_, CurrentSiteUser(user)), body): DepsThenBody<
        (RequireFeature<ImageStudio>, CurrentSiteUser),
        DesignBody,
    >,
) -> Response {
    match service::create(&state, user.id, &body, crate::dt::now()).await {
        Ok(v) => (StatusCode::CREATED, PydanticJson(v)).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn get_design(
    State(state): State<AppState>,
    DesignId(id): DesignId,
    _: RequireFeature<ImageStudio>,
    CurrentSiteUser(user): CurrentSiteUser,
) -> Response {
    reply(service::get(&state, user.id, &id).await)
}

async fn update_design(
    State(state): State<AppState>,
    DesignId(id): DesignId,
    DepsThenBody((_, CurrentSiteUser(user)), body): DepsThenBody<
        (RequireFeature<ImageStudio>, CurrentSiteUser),
        DesignBody,
    >,
) -> Response {
    reply(service::update(&state, user.id, &id, &body, crate::dt::now()).await)
}

async fn delete_design(
    State(state): State<AppState>,
    DesignId(id): DesignId,
    _: RequireFeature<ImageStudio>,
    CurrentSiteUser(user): CurrentSiteUser,
) -> Response {
    match service::delete(&state, user.id, &id).await {
        Ok(()) => no_content(),
        Err(e) => e.into_response(),
    }
}

fn parse_error(msg: &str) -> Response {
    ApiError::http(StatusCode::BAD_REQUEST, Some(msg)).into_response()
}

/// `file: UploadFile = File(...)`: the form is read before any dependency.
pub fn upload_file(items: Vec<(String, FormValue)>) -> Result<Vec<u8>, validation::ValidationError> {
    let mut v = validation::Validator::default();
    let loc = || vec!["body".into(), "file".into()];
    let file = match items.into_iter().rev().find(|(k, _)| k == "file").map(|(_, v)| v) {
        Some(FormValue::File(f)) => f.data,
        Some(FormValue::Text(t)) if !t.is_empty() => {
            v.push(loc(), Value::String(t), Fail::value_error("Expected UploadFile, received: <class 'str'>"));
            Vec::new()
        }
        _ => {
            v.push(loc(), Value::Null, Fail::missing());
            Vec::new()
        }
    };
    v.finish(file)
}

async fn upload(State(state): State<AppState>, req: Request) -> Response {
    let (mut parts, body) = req.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, usize::MAX).await else {
        return parse_error("There was an error parsing the body");
    };
    let items = match form::parse_form(parts.headers.get(header::CONTENT_TYPE).map(|v| v.as_bytes()), &bytes) {
        Ok(items) => items,
        Err(FormError::Http(msg)) => return parse_error(&msg),
    };
    drop(bytes);
    if let Err(e) = features::check::<ImageStudio>(&state).await {
        return e.into_response();
    }
    if let Err(r) = CurrentSiteUser::from_request_parts(&mut parts, &state).await {
        return r.into_response();
    }
    let data = match upload_file(items) {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    match service::upload(&state, &data).await {
        Ok(v) => (StatusCode::CREATED, PydanticJson(v)).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn preview(State(state): State<AppState>, req: Request) -> Response {
    let (mut parts, body) = req.into_parts();
    let body = match validation::read_body(&parts.headers, body).await {
        Ok(b) => b,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = features::check::<ImageStudio>(&state).await {
        return e.into_response();
    }
    if let Err(r) = CurrentSiteUser::from_request_parts(&mut parts, &state).await {
        return r.into_response();
    }
    let mut p = Params::new(Vec::new(), parts.uri.query(), &parts.headers);
    let kind = p.query("kind").str().optional();
    let mut v = p.v;
    let value = <DesignBody as validation::FromBody>::from_body(validation::Field::new(
        &mut v,
        vec!["body".into()],
        body.input(),
    ));
    let design = match v.finish(value) {
        Ok(Some(d)) => d,
        Ok(None) => return ApiError::internal().into_response(),
        Err(e) => return e.into_response(),
    };
    match service::preview(&state, design, kind.as_deref()).await {
        Ok(bytes) => png(bytes, "no-store"),
        Err(e) => e.into_response(),
    }
}

/// `/site/images/{design_id}.png` as Starlette splits it.
fn png_design_id(uri: &axum::http::Uri) -> Option<String> {
    let path = crate::middleware::pyurl::scope_path(uri);
    let file = path.strip_prefix("/site/images/")?;
    let id = file.strip_suffix(".png")?;
    (!id.is_empty() && !file.contains('/')).then(|| id.to_owned())
}

async fn render_png(State(state): State<AppState>, _: RequireFeature<ImageStudio>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    let mut p = Params::new(Vec::new(), parts.uri.query(), &parts.headers);
    let kind = p.query("kind").str().optional();
    let _v = p.query("v").str().optional();
    if let Err(e) = p.v.finish(()) {
        return e.into_response();
    }
    let Some(id) = png_design_id(&parts.uri) else {
        return ApiError::not_found("Image not found.").into_response();
    };
    match service::render_public(&state, &id, kind.as_deref()).await {
        Ok(Some(bytes)) => png(bytes, "public, max-age=60"),
        Ok(None) => ApiError::not_found("Image not found.").into_response(),
        Err(e) => e.into_response(),
    }
}

/// For `routes::defer_to_python`: the two render routes, when Python must draw.
pub fn defer_kind(matched: &str, method: &Method) -> Option<&'static str> {
    match matched {
        PNG_ROUTE => Some("png"),
        "/v1/images/preview" if method == Method::POST => Some("preview"),
        _ => None,
    }
}

async fn blobs_renderable(state: &AppState, shas: &[String]) -> bool {
    for sha in shas {
        if !service::blob_renderable(state, sha).await {
            return false;
        }
    }
    true
}

/// Whether Python must answer this render; the request comes back intact.
pub async fn inspect(state: &AppState, req: Request, kind: &str) -> (Request, bool) {
    if kind == "png" {
        let Some(id) = png_design_id(req.uri()) else { return (req, true) };
        if !matches!(*req.method(), Method::GET | Method::HEAD) {
            return (req, false);
        }
        if crate::og::text::fonts().is_none() {
            return (req, true);
        }
        let defer = match service::design_blobs(state, &id).await {
            Some(shas) => !blobs_renderable(state, &shas).await,
            None => false,
        };
        return (req, defer);
    }
    const MAX_INSPECT: usize = 8 << 20;
    if crate::og::text::fonts().is_none() {
        return (req, true);
    }
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap_or_default();
    let mut defer = false;
    if bytes.len() <= MAX_INSPECT
        && let Ok(v @ Value::Object(_)) = crate::pyjson::loads(&bytes)
    {
        defer = !blobs_renderable(state, &service::design_body_blobs(&v)).await;
    }
    (Request::from_parts(parts, Body::from(bytes)), defer)
}
