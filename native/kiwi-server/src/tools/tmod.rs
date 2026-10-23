//! `app/trove/tmod.py` `read_tmod` / `build_tmod` behind `/v1/mods/read` and
//! `/v1/mods/build`, as production runs them (the native header, contents and
//! packer, with Python's own error text where the native reader gives up).

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use kiwi_core::{mod_categories, tmod};
use serde_json::{Map, Value, json};

use super::pybytes::{b64decode_strict, utf8_error};
use crate::{
    auth::{extract::require_scope, scopes::MODS_READ},
    error::ApiError,
    pyjson::{self, LoadsError, PydanticJson},
    request_id,
    state::AppState,
    validation::{
        BodyInput, ErrorItem, Fail, Field, FromBody, Input, LocItem, Params, ValidationError, Validator,
        is_json_content_type, read_body, validate,
    },
};

pub const KIWI_MOD_LOADER: &str = "KiwiAPI";

/// `_parse_header(data, header_size, prop_count, False)` re-run after both
/// readings failed: the exception it raises (`None` if it returns), as
/// `str(exc)`.
fn strict_parse_error(d: &[u8], header_size: u64, prop_count: u16) -> Option<String> {
    let len = d.len() as u64;
    let leb = |pos: &mut u64| -> Result<u64, String> {
        let (mut out, mut shift) = (0u128, 0u32);
        loop {
            let b = *d.get(*pos as usize).filter(|_| *pos < len).ok_or("index out of range")?;
            *pos += 1;
            out |= ((b & 0x7F) as u128) << shift;
            if b & 0x80 == 0 {
                return Ok((out & 0xFFFF_FFFF) as u64);
            }
            shift += 7;
            if shift >= 64 {
                return Err("varint too long".into());
            }
        }
    };
    let string = |pos: &mut u64, n: u64| -> Result<(), String> {
        let (a, b) = ((*pos).min(len) as usize, pos.saturating_add(n).min(len) as usize);
        if let Some(e) = utf8_error(&d[a..b.max(a)]) {
            return Err(e);
        }
        *pos = pos.saturating_add(n);
        Ok(())
    };
    let run = || -> Result<u64, String> {
        let mut pos = 12u64;
        for _ in 0..prop_count {
            let n = leb(&mut pos)?;
            string(&mut pos, n)?;
            let n = leb(&mut pos)?;
            string(&mut pos, n)?;
        }
        while pos < header_size {
            let n = d[pos as usize] as u64;
            pos += 1;
            string(&mut pos, n)?;
            for _ in 0..4 {
                leb(&mut pos)?;
            }
        }
        Ok(pos)
    };
    run().err()
}

/// `read_tmod(data, metadata_only)` as the `TmodReadResponse` body; `Err` is
/// the `TmodError` text (a 400).
pub fn read(data: &[u8], metadata_only: bool) -> Result<Value, String> {
    if data.len() < 12 {
        return Err("file too small to be a .tmod".into());
    }
    let header_size = u64::from_le_bytes(data[0..8].try_into().unwrap_or_default());
    let version = u16::from_le_bytes([data[8], data[9]]);
    let prop_count = u16::from_le_bytes([data[10], data[11]]);
    if !(12..=data.len() as u64).contains(&header_size) {
        return Err("declared header size is outside the file".into());
    }
    let hs = header_size as usize;
    let Some(h) = tmod::header(data, hs, prop_count) else {
        return Err(match strict_parse_error(data, header_size, prop_count) {
            Some(e) => format!("malformed .tmod file: {e}"),
            None => "the file table does not end where the declared header size says".into(),
        });
    };
    let contents = (!metadata_only).then(|| tmod::contents(&data[hs..], &h.entries));
    let mut properties = Map::new();
    for (k, v) in h.properties {
        properties.insert(k, Value::String(v));
    }
    let files: Vec<Value> = h
        .entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            json!({
                "path": e.path, "index": e.index, "offset": e.offset, "size": e.size,
                "checksum": e.checksum,
                "content_base64": contents.as_ref().map(|c| Value::String(c[i].clone())).unwrap_or(Value::Null),
            })
        })
        .collect();
    Ok(json!({
        "version": version,
        "header_size": header_size,
        "properties": properties,
        "file_count": files.len(),
        "files": files,
        "metadata_only": metadata_only,
        "header_repaired": h.repaired,
    }))
}

/// Why a build failed: a `TmodError` (400) or the `OverflowError` the pure
/// Python serializer raises past the format's 16-bit fields (500).
#[derive(Debug, PartialEq)]
pub enum BuildError {
    Bad(String),
    Overflow,
}

/// `build_tmod(version, properties, files, mod_loader, lowercase_paths)`.
pub fn build(
    version: i64,
    properties: &[(String, String)],
    files: &[(String, Vec<u8>)],
    mod_loader: &str,
    lowercase_paths: bool,
) -> Result<Vec<u8>, BuildError> {
    if files.is_empty() {
        return Err(BuildError::Bad("a .tmod needs at least one file".into()));
    }
    let mut props: Vec<(String, String)> = Vec::with_capacity(properties.len() + 2);
    for (k, v) in properties {
        match props.iter_mut().find(|(pk, _)| pk == k) {
            Some(slot) => slot.1 = v.clone(),
            None => props.push((k.clone(), v.clone())),
        }
    }
    let set = |props: &mut Vec<(String, String)>, k: &str, v: String| match props.iter_mut().find(|(pk, _)| pk == k) {
        Some(slot) => slot.1 = v,
        None => props.push((k.to_owned(), v)),
    };
    set(&mut props, "modLoader", mod_loader.to_owned());
    let tags = props.iter().find(|(k, _)| k == "tags").map(|(_, v)| v.clone());
    if let Some(tags) = tags.filter(|t| !t.is_empty())
        && !props.iter().any(|(k, _)| k == "flags")
    {
        let flags = mod_categories::flags_from_tags(tags.split(','));
        if flags != 0 {
            set(&mut props, "flags", flags.to_string());
        }
    }
    let mut packed: Vec<(String, &[u8])> = Vec::with_capacity(files.len());
    for (raw, content) in files {
        let mut path = raw.replace('\\', "/").trim_start_matches('/').to_owned();
        if lowercase_paths && !path.to_lowercase().ends_with(".cfg") {
            path = path.to_lowercase();
        }
        if path.len() > 255 {
            return Err(BuildError::Bad(format!("internal path too long for the .tmod format (>255 bytes): {path}")));
        }
        packed.push((path, content));
    }
    let version = u16::try_from(version).map_err(|_| BuildError::Overflow)?;
    if props.len() > 0xFFFF {
        return Err(BuildError::Overflow);
    }
    let props: Vec<(&str, &str)> = props.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let files: Vec<(&str, &[u8])> = packed.iter().map(|(p, c)| (p.as_str(), *c)).collect();
    tmod::pack(version, &props, &files).ok_or(BuildError::Overflow)
}

/// `_safe_filename` of `mods_router`.
pub fn safe_filename(name: &str) -> String {
    let cleaned: String =
        name.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '_' | '-')).collect();
    let cleaned = cleaned.trim_matches(' ');
    let cleaned = if cleaned.is_empty() { "mod" } else { cleaned };
    cleaned.chars().take(80).collect()
}

// --- routes ----------------------------------------------------------------

/// The `bytes = Body(...)` parameter as FastAPI read it.
pub enum RawBody {
    Missing,
    Bytes(Vec<u8>),
    Json(Value),
}

pub async fn read_raw_body(headers: &axum::http::HeaderMap, body: Body) -> Result<RawBody, ApiError> {
    let bytes = axum::body::to_bytes(body, usize::MAX)
        .await
        .map_err(|_| ApiError::http(StatusCode::BAD_REQUEST, Some("There was an error parsing the body")))?;
    if bytes.is_empty() {
        return Ok(RawBody::Missing);
    }
    let is_json = match headers.get(header::CONTENT_TYPE) {
        Some(ct) if !ct.is_empty() => is_json_content_type(&request_id::latin1(ct.as_bytes())),
        _ => false,
    };
    if !is_json {
        return Ok(RawBody::Bytes(bytes.to_vec()));
    }
    match pyjson::loads(&bytes) {
        Ok(Value::Null) => Ok(RawBody::Missing),
        Ok(v) => Ok(RawBody::Json(v)),
        Err(LoadsError::Syntax { msg, pos }) => Err(ValidationError(vec![ErrorItem::json_invalid(pos, msg)]).into()),
        Err(LoadsError::Decode) => {
            Err(ApiError::http(StatusCode::BAD_REQUEST, Some("There was an error parsing the body")))
        }
        Err(LoadsError::NonFinite) => Err(ApiError::internal()),
    }
}

pub async fn read_route(State(state): State<AppState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let raw = match read_raw_body(&parts.headers, body).await {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    let ctx = match require_scope(&state, &parts, MODS_READ).await {
        Ok(ctx) => ctx,
        Err(r) => return r.into_response(),
    };
    match read_reply(raw, parts.uri.query(), &parts.headers).await {
        Ok(body) => (ctx.rate_limit, PydanticJson(body)).into_response(),
        Err(e) => e.into_response(),
    }
}

/// Everything after the scope dependency: the query and body validation,
/// then `read_tmod`.
pub async fn read_reply(raw: RawBody, query: Option<&str>, headers: &axum::http::HeaderMap) -> Result<Value, ApiError> {
    let mut p = Params::new(Vec::new(), query, headers);
    let metadata_only = p.query("metadata_only").bool().default(false);
    let mut v: Validator = p.v;
    let data = match raw {
        RawBody::Bytes(b) => b,
        RawBody::Json(Value::String(s)) => s.into_bytes(),
        RawBody::Missing => {
            Field::new(&mut v, vec![LocItem::from("body")], Input::Missing(None)).fail(Fail::missing());
            Vec::new()
        }
        RawBody::Json(other) => {
            Field::new(&mut v, vec![LocItem::from("body")], Input::Json(&other))
                .fail(Fail::new("bytes_type", "Input should be a valid bytes"));
            Vec::new()
        }
    };
    v.finish(())?;
    if data.is_empty() {
        return Err(ApiError::bad_request("Empty body - POST the raw .tmod file bytes."));
    }
    match tokio::task::spawn_blocking(move || read(&data, metadata_only)).await {
        Ok(Ok(body)) => Ok(body),
        Ok(Err(msg)) => Err(ApiError::bad_request(msg)),
        Err(_) => Err(ApiError::internal()),
    }
}

/// `TmodBuildRequest`.
#[derive(Debug, Default)]
pub struct BuildReq {
    pub version: i64,
    pub properties: Vec<(String, String)>,
    pub files: Vec<(String, String)>,
}

impl FromBody for BuildReq {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            let version = m.field("version").int().saturating().default(1);
            let properties = m.field("properties").dict(|f| f.str().required()).default(Vec::new());
            let files = m
                .field("files")
                .list(|f| {
                    f.model(|fm| (fm.field("path").str().required(), fm.field("content_base64").str().required()))
                        .required()
                })
                .required();
            Some(BuildReq { version, properties, files })
        })
        .required()
    }
}

pub async fn build_route(State(state): State<AppState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let body = match read_body(&parts.headers, body).await {
        Ok(b) => b,
        Err(e) => return e.into_response(),
    };
    if let Err(r) = require_scope(&state, &parts, MODS_READ).await {
        return r.into_response();
    }
    build_reply(&body).await
}

/// Everything after the scope dependency.
pub async fn build_reply(body: &BodyInput) -> Response {
    let req: BuildReq = match validate(body) {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    let mut files = Vec::with_capacity(req.files.len());
    for (path, b64) in &req.files {
        match b64decode_strict(b64) {
            Ok(data) => files.push((path.clone(), data)),
            Err(e) => {
                return ApiError::bad_request(format!("Invalid base64 content for '{path}': {e}")).into_response();
            }
        }
    }
    let title = req.properties.iter().rev().find(|(k, _)| k == "title").map_or("mod", |(_, v)| v.as_str());
    let filename = format!("{}.tmod", safe_filename(title));
    let (version, properties) = (req.version, req.properties.clone());
    let built = tokio::task::spawn_blocking(move || build(version, &properties, &files, KIWI_MOD_LOADER, true)).await;
    match built {
        Ok(Ok(data)) => {
            let disposition = format!("attachment; filename=\"{filename}\"");
            let mut resp = (StatusCode::OK, data).into_response();
            let h = resp.headers_mut();
            h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
            if let Ok(v) = HeaderValue::from_str(&disposition) {
                h.insert(header::CONTENT_DISPOSITION, v);
            }
            resp
        }
        Ok(Err(BuildError::Bad(msg))) => ApiError::bad_request(msg).into_response(),
        Ok(Err(BuildError::Overflow)) | Err(_) => ApiError::internal().into_response(),
    }
}
