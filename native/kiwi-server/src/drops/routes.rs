//! The handlers of `app/drops/router.py` (uploader, tokenless) and
//! `app/drops/admin.py` (superuser), in FastAPI's order: the form is read
//! first, then the feature gate, then field validation, then the handler.

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, Uri, header, request::Parts},
    response::{IntoResponse, Response},
};
use kiwi_core::form::FormError;
use mongodb::bson::oid::ObjectId;

use super::{
    DropUpload, FileDrop, drop_dir, public_view,
    service::{self, DropCreate, DropUpdate, internal},
    upload::{self, Spool, WriteError},
    upload_view,
};
use crate::{
    auth::{Superuser, client::client_ip},
    dt,
    error::{ApiError, ErrorCode},
    features::{self, FileDrops},
    middleware::pyurl::scope_path,
    pyjson::{PydanticJson, no_content},
    ratelimit::check_rate_limit,
    runtime_config,
    state::AppState,
    validation::{DepsThenBody, ValidationError},
};

const MIB: i64 = 1024 * 1024;

/// One segment of the decoded path (`%2F` never reaches the Rust routes).
fn segment(uri: &Uri, i: usize) -> String {
    scope_path(uri).split('/').nth(i).unwrap_or_default().to_owned()
}

fn body_error(msg: &str) -> Response {
    ApiError::http(StatusCode::BAD_REQUEST, Some(msg)).into_response()
}

/// `_throttle`: per IP per link, then the whole link at four times that.
async fn throttle(state: &AppState, parts: &Parts, slug: &str) -> Result<(), Response> {
    let (max, window) = runtime_config::get_rate_limit(state, "file_drop_rate_limit")
        .await
        .map_err(|e| internal(format!("{e:#}")).into_response())?;
    let ip = client_ip(parts).unwrap_or_else(|| "unknown".into());
    check_rate_limit(state, &format!("drop:{slug}:{ip}"), max, window).await.map_err(IntoResponse::into_response)?;
    check_rate_limit(state, &format!("drop:{slug}"), max * 4, window).await.map_err(IntoResponse::into_response)?;
    Ok(())
}

/// `GET /site/drops/{slug}`.
pub async fn meta(State(state): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    let slug = segment(&parts.uri, 3);
    if let Err(e) = features::check::<FileDrops>(&state).await {
        return e.into_response();
    }
    if let Err(r) = throttle(&state, &parts, &slug).await {
        return r;
    }
    match service::by_slug(&state, &slug).await {
        Ok(drop) => ([(header::CACHE_CONTROL, HeaderValue::from_static("no-store"))], PydanticJson(public_view(&drop)))
            .into_response(),
        Err(e) => e.into_response(),
    }
}

/// The spooled, parsed form with every part named in `need` readable.
async fn read_form(
    parts: &Parts,
    body: axum::body::Body,
    need: &'static [&'static str],
) -> Result<(Arc<Spool>, upload::Parsed), Response> {
    let ct = parts.headers.get(header::CONTENT_TYPE).map(|v| v.as_bytes().to_vec());
    let (kind, _) = kiwi_core::form::parse_options_header(ct.as_deref());
    if kind != b"multipart/form-data" && kind != b"application/x-www-form-urlencoded" {
        return Ok((Arc::new(Spool::empty()), (Vec::new(), Vec::new())));
    }
    let spool = Arc::new(Spool::read(body).await.map_err(|_| body_error("There was an error parsing the body"))?);
    match upload::parse(spool.clone(), ct, need).await {
        Ok(parsed) => Ok((spool, parsed)),
        Err(FormError::Http(msg)) => Err(body_error(&msg)),
    }
}

/// `POST /site/drops/{slug}/verify`.
pub async fn verify(State(state): State<AppState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let slug = segment(&parts.uri, 3);
    let (_spool, (items, _)) = match read_form(&parts, body, &["pin"]).await {
        Ok(f) => f,
        Err(r) => return r,
    };
    if let Err(e) = features::check::<FileDrops>(&state).await {
        return e.into_response();
    }
    let mut errors = Vec::new();
    let pin = upload::required_str(&items, "pin", &mut errors);
    if !errors.is_empty() {
        return ValidationError(errors).into_response();
    }
    if let Err(r) = throttle(&state, &parts, &slug).await {
        return r;
    }
    let drop = match service::by_slug(&state, &slug).await {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = service::check_pin(&drop, &pin).await {
        return e.into_response();
    }
    PydanticJson(public_view(&drop)).into_response()
}

/// `POST /site/drops/{slug}/upload`.
pub async fn upload(State(state): State<AppState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let slug = segment(&parts.uri, 3);
    let (spool, (items, spans)) = match read_form(&parts, body, &["pin", "note"]).await {
        Ok(f) => f,
        Err(r) => return r,
    };
    if let Err(e) = features::check::<FileDrops>(&state).await {
        return e.into_response();
    }
    let mut errors = Vec::new();
    let pin = upload::required_str(&items, "pin", &mut errors);
    let file = upload::required_file(&items, &spans, "file", &mut errors);
    let note = upload::optional_str(&items, "note", &mut errors);
    let Some((file, span)) = file.filter(|_| errors.is_empty()) else {
        return ValidationError(errors).into_response();
    };
    if let Err(r) = throttle(&state, &parts, &slug).await {
        return r;
    }
    let drop = match service::by_slug(&state, &slug).await {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    if let Err(e) = service::check_pin(&drop, &pin).await {
        return e.into_response();
    }
    receive(&state, &drop, &file, span, note.as_deref(), spool).await.unwrap_or_else(IntoResponse::into_response)
}

/// `receive`: claim a slot, stream the file to disk, record it. A failure
/// after the claim hands the slot back.
async fn receive(
    state: &AppState,
    drop: &FileDrop,
    file: &kiwi_core::form::UploadFile,
    span: std::ops::Range<usize>,
    note: Option<&str>,
    spool: Arc<Spool>,
) -> Result<Response, ApiError> {
    let filename = kiwi_core::drops::safe_filename(Some(&file.filename));
    if !service::claim_slot(state, drop).await? {
        return Err(ApiError::conflict("This link was just used up. Ask for a new one."));
    }
    let mut tag = [0u8; 8];
    getrandom::fill(&mut tag).map_err(internal)?;
    let stored_name = format!("{}-{filename}", hex::encode(tag));
    let dest = drop_dir(state.settings(), &drop.slug).join(&stored_name);
    let (dest2, limit) = (dest.clone(), drop.max_file_bytes);
    let written = tokio::task::spawn_blocking(move || upload::write_stream(&spool.bytes()[span], &dest2, limit)).await;
    let (size, sha256) = match written {
        Ok(Ok(w)) => w,
        Ok(Err(WriteError::OverLimit)) => {
            let mb = drop.max_file_bytes.div_euclid(MIB);
            let msg = format!("That file is bigger than this link's {mb} MB limit.");
            return undo(state, drop, &dest, ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, ErrorCode::BadRequest, msg))
                .await;
        }
        Ok(Err(WriteError::Io(err))) => {
            tracing::error!("file drop write failed (slug={}): {err}", drop.slug);
            let msg = "The file couldn't be saved. Try again in a moment.";
            return undo(
                state,
                drop,
                &dest,
                ApiError::new(StatusCode::SERVICE_UNAVAILABLE, ErrorCode::ServiceUnavailable, msg),
            )
            .await;
        }
        Err(join) => return Err(internal(join)),
    };
    if size == 0 {
        return undo(state, drop, &dest, ApiError::bad_request("That file is empty.")).await;
    }
    let note = note.map(kiwi_core::pystr::strip).map(|n| kiwi_core::pystr::prefix(n, 500)).filter(|n| !n.is_empty());
    let row = DropUpload {
        id: ObjectId::new(),
        drop_id: drop.id,
        filename: filename.clone(),
        stored_name,
        size: size as i64,
        content_type: file.content_type().filter(|c| !c.is_empty()),
        sha256,
        note: note.map(str::to_owned),
        uploaded_at: dt::now(),
    };
    service::insert_upload(state, &row).await?;
    tracing::info!("file drop received {filename} ({size} bytes) on {}", drop.slug);
    Ok(PydanticJson(upload_view(&row)).into_response())
}

/// Drop the partial file and hand the claimed slot back.
async fn undo(state: &AppState, drop: &FileDrop, dest: &std::path::Path, e: ApiError) -> Result<Response, ApiError> {
    let _ = tokio::fs::remove_file(dest).await;
    service::release_slot(state, drop).await?;
    Err(e)
}

/// `GET /admin/drops`.
pub async fn list(State(state): State<AppState>, _admin: Superuser) -> Response {
    service::list(&state).await.map(PydanticJson).into_response()
}

/// `POST /admin/drops`.
pub async fn create(
    State(state): State<AppState>,
    DepsThenBody(_admin, req): DepsThenBody<Superuser, DropCreate>,
) -> Response {
    match service::create(&state, req).await {
        Ok(v) => (StatusCode::CREATED, PydanticJson(v)).into_response(),
        Err(e) => e.into_response(),
    }
}

/// `PATCH /admin/drops/{drop_id}`.
pub async fn update(
    State(state): State<AppState>,
    uri: Uri,
    DepsThenBody(_admin, req): DepsThenBody<Superuser, DropUpdate>,
) -> Response {
    service::update(&state, &segment(&uri, 3), req).await.map(PydanticJson).into_response()
}

/// `DELETE /admin/drops/{drop_id}`.
pub async fn remove(State(state): State<AppState>, uri: Uri, _admin: Superuser) -> Response {
    service::delete(&state, &segment(&uri, 3)).await.map(|()| no_content()).into_response()
}

/// `GET /admin/drops/uploads/{upload_id}/download`.
pub async fn download(State(state): State<AppState>, uri: Uri, headers: HeaderMap, _admin: Superuser) -> Response {
    let res = async {
        let (row, path) = service::upload_path(&state, &segment(&uri, 4)).await?;
        let media = row.content_type.as_deref().filter(|c| !c.is_empty()).unwrap_or("application/octet-stream");
        super::download::file_response(&path, media, &row.filename, &headers).await
    };
    res.await.into_response()
}

/// `DELETE /admin/drops/uploads/{upload_id}`.
pub async fn remove_upload(State(state): State<AppState>, uri: Uri, _admin: Superuser) -> Response {
    service::delete_upload(&state, &segment(&uri, 4)).await.map(|()| no_content()).into_response()
}
