//! `POST /site/unlock-debug`: NOP the jump guarding Trove's debug console.

use std::sync::LazyLock;

use axum::{
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use kiwi_core::form::{self, FormError, FormValue};
use regex::bytes::Regex;
use serde_json::{Map, Value};

use crate::{
    auth::client::client_ip,
    error::{self, ApiError},
    features,
    ratelimit::{check_rate_limit, get_rate_limit},
    state::AppState,
    validation::{ErrorItem, LocItem, ValidationError},
};

static FIND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?-u)\x7C\x39\x68\xE0\x02\x00\x00").expect("static pattern"));
const REPL: &[u8] = b"\x90\x90\x68\xE0\x02\x00\x00";

/// The patched executable, or the 400 message.
pub fn patch(data: &[u8]) -> Result<Vec<u8>, &'static str> {
    if data.is_empty() {
        return Err("That file is empty.");
    }
    if !data.starts_with(b"MZ") {
        return Err("That doesn't look like a Windows executable.");
    }
    if !FIND.is_match(data) {
        return Err("This build of Trove.exe doesn't contain the sequence this patch edits - \
                    it may already be patched, or the game may have changed.");
    }
    Ok(FIND.replace_all(data, REPL).into_owned())
}

/// The site router's `_resolve_feature_flags` for this path.
async fn site_gate(state: &AppState, parts: &axum::http::request::Parts) -> Result<(), Response> {
    let mut on = true;
    for flag in crate::activity::SITE_FLAGS {
        let v = features::is_enabled(state, flag).await.map_err(|e| {
            tracing::error!("site feature flag {flag}: {e:#}");
            ApiError::internal().into_response()
        })?;
        if flag == features::UNLOCK_DEBUG_FLAG {
            on = v;
        }
    }
    if !on {
        return Err(error::http_exception(
            StatusCode::NOT_FOUND,
            None,
            &parts.method,
            parts.uri.path(),
            &parts.headers,
        ));
    }
    Ok(())
}

pub async fn route(State(state): State<AppState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, usize::MAX).await else {
        return ApiError::http(StatusCode::BAD_REQUEST, Some("There was an error parsing the body")).into_response();
    };
    let items = match form::parse_form(parts.headers.get(header::CONTENT_TYPE).map(|v| v.as_bytes()), &bytes) {
        Ok(items) => items,
        Err(FormError::Http(msg)) => return ApiError::http(StatusCode::BAD_REQUEST, Some(&msg)).into_response(),
    };
    drop(bytes);
    if let Err(resp) = site_gate(&state, &parts).await {
        return resp;
    }
    let (max, window) = get_rate_limit(&state, "unlock_debug_rate_limit").await;
    let ip = client_ip(&parts).unwrap_or_else(|| "unknown".into());
    if let Err(rej) = check_rate_limit(&state, &format!("unlockdebug:{ip}"), max, window).await {
        return rej.into_response();
    }
    form_reply(items).await
}

/// The `trove_exe` field's validation and the patch.
pub async fn form_reply(items: Vec<(String, FormValue)>) -> Response {
    let file = match items.into_iter().rev().find(|(k, _)| k == "trove_exe").map(|(_, v)| v) {
        Some(FormValue::File(f)) => Some(f),
        Some(FormValue::Text(t)) if !t.is_empty() => {
            let mut ctx = Map::new();
            ctx.insert("error".into(), Value::Object(Map::new()));
            return ValidationError(vec![ErrorItem {
                kind: "value_error".into(),
                loc: vec![LocItem::from("body"), LocItem::from("trove_exe")],
                msg: "Value error, Expected UploadFile, received: <class 'str'>".into(),
                input: Value::String(t),
                ctx: Some(ctx),
            }])
            .into_response();
        }
        _ => None,
    };
    let Some(file) = file.filter(|f| !f.filename.is_empty()) else {
        return ApiError::bad_request("No file was uploaded.").into_response();
    };
    let patched = match tokio::task::spawn_blocking(move || patch(&file.data)).await {
        Ok(Ok(p)) => p,
        Ok(Err(msg)) => return ApiError::bad_request(msg).into_response(),
        Err(_) => return ApiError::internal().into_response(),
    };
    let mut resp = (StatusCode::OK, patched).into_response();
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    h.insert(header::CONTENT_DISPOSITION, HeaderValue::from_static("attachment; filename=\"Trove.exe\""));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}
