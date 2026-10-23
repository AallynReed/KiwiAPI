//! `app/trove/render/bp_cache.py`: decoded payloads cached as gzipped blobs in
//! the mods content store, indexed by `blueprint_cache` rows, and served with
//! their blob sha as the `ETag`. Python reads and writes the same rows and
//! blobs, so keys, row fields and the blob layout are kept exactly.

use std::{
    future::Future,
    io::Write,
    path::{Path, PathBuf},
    time::Instant,
};

use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::pack;
use crate::{
    dt,
    error::{ApiError, ErrorCode},
    pyjson,
    state::AppState,
};

pub const PACK_VERSION: &str = "v3";
pub const ASSEMBLY_VERSION: &str = "a22";
const COLLECTION: &str = "blueprint_cache";

/// `_norm(path)`.
fn norm(path: &str) -> String {
    let p = kiwi_core::pystr::strip(path).replace('\\', "/");
    p.trim_start_matches('/').to_lowercase()
}

pub fn key_for_file(sha: &str, path: &str) -> String {
    format!("{PACK_VERSION}:file:{sha}:{}", norm(path))
}

pub fn key_for_tmod(sha: &str, path: &str) -> String {
    format!("{PACK_VERSION}:tmod:{sha}:{}", norm(path))
}

pub fn key_for_assembly(rig_sig: &str, ident: &str) -> String {
    format!("{PACK_VERSION}:{ASSEMBLY_VERSION}:asm:{rig_sig}:{}", norm(ident))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fmt {
    Json,
    Bin,
}

impl Fmt {
    pub fn media_type(self) -> &'static str {
        match self {
            Fmt::Json => "application/json",
            Fmt::Bin => "application/x-kiwi-voxel",
        }
    }

    /// The wire body before compression: compact ASCII JSON, or `KVX1`.
    pub fn encode(self, payload: &Map<String, Value>) -> Vec<u8> {
        match self {
            Fmt::Json => pyjson::dumps_compact_ascii(&Value::Object(payload.clone())).into_bytes(),
            Fmt::Bin => pack::kvx_encode(payload),
        }
    }
}

/// `Cached`.
#[derive(Clone, Debug)]
pub struct Cached {
    pub etag: String,
    pub gz: Vec<u8>,
    pub count: i64,
    pub hit: bool,
    pub build_ms: f64,
    pub stored: bool,
    pub media_type: &'static str,
}

impl Cached {
    pub fn state(&self) -> &'static str {
        if self.hit {
            "hit"
        } else if self.stored {
            "miss"
        } else {
            "off"
        }
    }
}

/// Why a builder produced nothing: an `APIError` (remembered when it is a
/// 413/422) or any other raise, answered as is and never cached.
#[derive(Debug)]
pub enum BuildError {
    Api(ApiError),
    Other(Response),
}

impl From<ApiError> for BuildError {
    fn from(e: ApiError) -> Self {
        BuildError::Api(e)
    }
}

impl IntoResponse for BuildError {
    fn into_response(self) -> Response {
        match self {
            BuildError::Api(e) => e.into_response(),
            BuildError::Other(r) => r,
        }
    }
}

/// `ContentStore(root).path_for(sha)`.
pub fn blob_path(root: &str, sha: &str) -> PathBuf {
    let shard: String = sha.chars().take(2).collect();
    Path::new(root).join("objects").join(shard).join(sha)
}

/// `ContentStore.get`: `Ok(None)` when there is no such blob.
pub fn store_get(root: &str, sha: &str) -> std::io::Result<Option<Vec<u8>>> {
    let p = blob_path(root, sha);
    if !p.is_file() {
        return Ok(None);
    }
    std::fs::read(p).map(Some)
}

/// `ContentStore.put`: `(sha, created)`, written atomically.
pub fn store_put(root: &str, data: &[u8]) -> std::io::Result<(String, bool)> {
    let sha = hex::encode(Sha256::digest(data));
    let dest = blob_path(root, &sha);
    if dest.is_file() {
        return Ok((sha, false));
    }
    let dir = dest.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mut name = [0u8; 6];
    getrandom::fill(&mut name).map_err(std::io::Error::other)?;
    let tmp = dir.join(format!("tmp{}.part", hex::encode(name)));
    let result = (|| {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        f.write_all(data)?;
        f.flush()?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, &dest)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map(|()| (sha, true))
}

/// pymongo's int: Int32 when it fits.
fn py_int(v: i64) -> Bson {
    i32::try_from(v).map(Bson::Int32).unwrap_or(Bson::Int64(v))
}

fn int_of(b: Option<&Bson>) -> Option<i64> {
    match b? {
        Bson::Int32(v) => Some(i64::from(*v)),
        Bson::Int64(v) => Some(*v),
        Bson::Double(v) if v.fract() == 0.0 => Some(*v as i64),
        _ => None,
    }
}

fn str_of(b: Option<&Bson>) -> Option<String> {
    match b? {
        Bson::String(s) => Some(s.clone()),
        _ => None,
    }
}

/// A `BlueprintCacheEntry` row.
fn row(
    key: &str,
    blob_sha: Option<&str>,
    byte_len: i64,
    count: i64,
    size: Option<&Value>,
    err: Option<&ApiError>,
) -> Document {
    let size = size
        .and_then(Value::as_array)
        .map(|a| Bson::Array(a.iter().map(|v| py_int(v.as_i64().unwrap_or(0))).collect()));
    doc! {
        "_id": ObjectId::new(),
        "key": key,
        "blob_sha": blob_sha.map(str::to_owned),
        "byte_len": py_int(byte_len),
        "voxel_count": py_int(count),
        "size": size.unwrap_or(Bson::Null),
        "err_status": err.map(|e| py_int(i64::from(e.status.as_u16()))).unwrap_or(Bson::Null),
        "err_code": err.map(|e| e.code.to_owned()),
        "err_msg": err.map(|e| e.message.clone()),
        "created_at": dt::to_bson(dt::now()),
    }
}

fn code_str(code: Option<String>) -> &'static str {
    let code = code.filter(|c| !c.is_empty()).unwrap_or_else(|| "bad_request".into());
    ErrorCode::ALL.iter().map(|c| c.as_str()).find(|c| *c == code).unwrap_or_else(|| Box::leak(code.into_boxed_str()))
}

async fn lookup(state: &AppState, key: &str) -> (Option<Document>, bool) {
    let Some(db) = state.inner.db.mongo.as_ref() else { return (None, false) };
    match db.collection::<Document>(COLLECTION).find_one(doc! {"key": key}).await {
        Ok(d) => (d, true),
        Err(e) => {
            tracing::warn!("bp_cache: index unavailable, decoding uncached: {e}");
            (None, false)
        }
    }
}

async fn remember(state: &AppState, d: Document) {
    let Some(db) = state.inner.db.mongo.as_ref() else { return };
    if let Err(e) = db.collection::<Document>(COLLECTION).insert_one(d).await {
        let duplicate = matches!(&*e.kind, mongodb::error::ErrorKind::Write(mongodb::error::WriteFailure::WriteError(w)) if w.code == 11000);
        if !duplicate {
            tracing::warn!("bp_cache: could not record a row: {e}");
        }
    }
}

async fn forget(state: &AppState, id: Option<&Bson>) {
    let (Some(db), Some(id)) = (state.inner.db.mongo.as_ref(), id) else { return };
    if let Err(e) = db.collection::<Document>(COLLECTION).delete_one(doc! {"_id": id.clone()}).await {
        tracing::warn!("bp_cache: could not drop a stale row: {e}");
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    tokio::task::spawn_blocking(f).await.ok()
}

fn encode_gz(payload: Map<String, Value>, fmt: Fmt) -> Option<(Vec<u8>, Map<String, Value>)> {
    let body = fmt.encode(&payload);
    let mtime = chrono::Utc::now().timestamp() as u32;
    Some((pack::gzip(&body, 6, mtime)?, payload))
}

fn count_of(payload: &Map<String, Value>) -> i64 {
    payload.get("count").and_then(Value::as_i64).unwrap_or(0)
}

/// `get_or_build(key, build, fmt)`.
pub async fn get_or_build<F, Fut>(state: &AppState, key: &str, fmt: Fmt, build: F) -> Result<Cached, BuildError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Map<String, Value>, BuildError>>,
{
    let root = state.inner.settings.mods_store_dir.clone();
    let key = if fmt == Fmt::Json { key.to_owned() } else { format!("{key}:bin") };
    let (entry, usable) = lookup(state, &key).await;
    if let Some(entry) = &entry {
        if let Some(status) = int_of(entry.get("err_status")).filter(|s| *s != 0) {
            let status = StatusCode::from_u16(status as u16).unwrap_or(StatusCode::UNPROCESSABLE_ENTITY);
            let message = str_of(entry.get("err_msg"))
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| "This blueprint can't be previewed.".into());
            return Err(ApiError::new(status, code_str(str_of(entry.get("err_code"))), message).into());
        }
        if let Some(sha) = str_of(entry.get("blob_sha")).filter(|s| !s.is_empty()) {
            let (r, s) = (root.clone(), sha.clone());
            match blocking(move || store_get(&r, &s)).await {
                Some(Ok(Some(gz))) => {
                    let count = int_of(entry.get("voxel_count")).unwrap_or(0);
                    return Ok(Cached {
                        etag: sha,
                        gz,
                        count,
                        hit: true,
                        build_ms: 0.0,
                        stored: true,
                        media_type: fmt.media_type(),
                    });
                }
                Some(Err(e)) => tracing::warn!("bp_cache: could not read blob {sha}: {e}"),
                _ => {}
            }
            tracing::info!("bp_cache: blob {sha} missing for {key}, rebuilding");
            forget(state, entry.get("_id")).await;
        }
    }

    let started = Instant::now();
    let payload = match build().await {
        Ok(p) => p,
        Err(BuildError::Api(e)) => {
            if usable && matches!(e.status.as_u16(), 413 | 422) {
                remember(state, row(&key, None, 0, 0, None, Some(&e))).await;
            }
            return Err(BuildError::Api(e));
        }
        Err(other) => return Err(other),
    };
    let (gz, payload) = blocking(move || encode_gz(payload, fmt)).await.flatten().ok_or_else(internal)?;
    let count = count_of(&payload);
    let ms = started.elapsed().as_secs_f64() * 1000.0;
    let sha = if usable {
        let (r, g) = (root, gz.clone());
        match blocking(move || store_put(&r, &g)).await {
            Some(Ok((sha, _))) => Some(sha),
            Some(Err(e)) => {
                tracing::warn!("bp_cache: could not write a {}-byte payload: {e}", gz.len());
                None
            }
            None => None,
        }
    } else {
        None
    };
    let Some(sha) = sha else {
        return Ok(Cached {
            etag: String::new(),
            gz,
            count,
            hit: false,
            build_ms: ms,
            stored: false,
            media_type: fmt.media_type(),
        });
    };
    remember(state, row(&key, Some(&sha), gz.len() as i64, count, payload.get("size"), None)).await;
    Ok(Cached { etag: sha, gz, count, hit: false, build_ms: ms, stored: true, media_type: fmt.media_type() })
}

/// `build_uncached(build, fmt)`.
pub async fn build_uncached<Fut>(fmt: Fmt, build: Fut) -> Result<Cached, BuildError>
where
    Fut: Future<Output = Result<Map<String, Value>, BuildError>>,
{
    let started = Instant::now();
    let payload = build.await?;
    let (gz, payload) = blocking(move || encode_gz(payload, fmt)).await.flatten().ok_or_else(internal)?;
    Ok(Cached {
        etag: String::new(),
        gz,
        count: count_of(&payload),
        hit: false,
        build_ms: started.elapsed().as_secs_f64() * 1000.0,
        stored: false,
        media_type: fmt.media_type(),
    })
}

fn internal() -> BuildError {
    BuildError::Api(ApiError::internal())
}

/// `etag_matches(header, etag)`.
pub fn etag_matches(header: Option<&str>, etag: &str) -> bool {
    header.is_some_and(|h| {
        h.split(',').any(|t| {
            let t = kiwi_core::pystr::strip(t);
            t.strip_prefix("W/").unwrap_or(t) == etag
        })
    })
}

fn latin1(v: &HeaderValue) -> String {
    v.as_bytes().iter().map(|&b| char::from(b)).collect()
}

/// `respond(request, cached, max_age=)`.
pub fn respond(headers: &HeaderMap, cached: &Cached, max_age: u32) -> Response {
    let mut out = HeaderMap::new();
    let set = |out: &mut HeaderMap, k: &'static str, v: String| {
        if let Ok(v) = HeaderValue::from_str(&v) {
            out.insert(k, v);
        }
    };
    set(&mut out, "cache-control", format!("public, max-age={max_age}"));
    set(&mut out, "vary", "Accept-Encoding".into());
    set(&mut out, "x-kiwi-cache", cached.state().into());
    let timing = if cached.hit {
        format!("bpcache;desc=\"{}\"", cached.state())
    } else {
        format!("bpcache;desc=\"{}\";dur={:.0}", cached.state(), cached.build_ms)
    };
    set(&mut out, "server-timing", timing);
    if !cached.etag.is_empty() {
        let etag = format!("\"{}\"", cached.etag);
        set(&mut out, "etag", etag.clone());
        let inm = headers.get(header::IF_NONE_MATCH).map(latin1);
        if etag_matches(inm.as_deref(), &etag) {
            let mut r = Response::new(Body::empty());
            *r.status_mut() = StatusCode::NOT_MODIFIED;
            *r.headers_mut() = out;
            return r;
        }
    }
    let accepts =
        headers.get(header::ACCEPT_ENCODING).map(|v| latin1(v).to_lowercase().contains("gzip")).unwrap_or(false);
    let body = if accepts {
        set(&mut out, "content-encoding", "gzip".into());
        cached.gz.clone()
    } else {
        pack::gunzip(&cached.gz).unwrap_or_default()
    };
    set(&mut out, "content-length", body.len().to_string());
    set(&mut out, "content-type", cached.media_type.into());
    let mut r = Response::new(Body::from(body));
    *r.headers_mut() = out;
    r
}
