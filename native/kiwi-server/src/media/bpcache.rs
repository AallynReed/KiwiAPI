//! The JSON half of `app/trove/render/bp_cache.py`: `blueprint_cache` index
//! rows pointing at gzipped JSON bodies in the mods content store, keyed by
//! content hash, failures (413/422) remembered too. Rows and blobs are
//! written exactly as Python writes them, so either side reads the other's.

use std::{
    collections::HashSet,
    future::Future,
    sync::{Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use axum::http::StatusCode;
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use serde_json::Value;

use super::cas::ContentStore;
use crate::{
    auth::pyutil::bson_int,
    dt,
    error::{ApiError, ErrorCode},
    pyjson,
    state::AppState,
};

pub const COLLECTION: &str = "blueprint_cache";
pub const SWF_VERSION: &str = "s3";
pub const SCRIPT_VERSION: &str = "c1";
pub const BANK_VERSION: &str = "b1";

pub fn key_for_swf(sha: &str) -> String {
    format!("{SWF_VERSION}:swf:{sha}")
}

pub fn key_for_swf_scripts(sha: &str) -> String {
    format!("{SCRIPT_VERSION}:swfsrc:{sha}")
}

pub fn key_for_bank(sha: &str, sidecar_sha: Option<&str>) -> String {
    format!("{BANK_VERSION}:bnk:{sha}:{}", sidecar_sha.unwrap_or("-"))
}

/// An `APIError` code read back from a row, as the `&'static str` the
/// envelope takes.
fn static_code(code: &str) -> &'static str {
    if let Some(c) = ErrorCode::ALL.iter().find(|c| c.as_str() == code) {
        return c.as_str();
    }
    static EXTRA: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut set = EXTRA.get_or_init(Mutex::default).lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = set.get(code) {
        return c;
    }
    let leaked: &'static str = Box::leak(code.to_owned().into_boxed_str());
    set.insert(leaked);
    leaked
}

struct Entry {
    id: Bson,
    blob_sha: Option<String>,
    err: Option<(i64, Option<String>, Option<String>)>,
}

fn opt_str(d: &Document, k: &str) -> Result<Option<String>, ()> {
    match d.get(k) {
        None | Some(Bson::Null) => Ok(None),
        Some(Bson::String(s)) => Ok(Some(s.clone())),
        _ => Err(()),
    }
}

fn opt_int(d: &Document, k: &str) -> Result<Option<i64>, ()> {
    match d.get(k) {
        None | Some(Bson::Null) => Ok(None),
        Some(Bson::Int32(v)) => Ok(Some(i64::from(*v))),
        Some(Bson::Int64(v)) => Ok(Some(*v)),
        Some(Bson::Double(f)) if f.fract() == 0.0 && f.is_finite() => Ok(Some(*f as i64)),
        _ => Err(()),
    }
}

/// Beanie's load of a `BlueprintCacheEntry`; Err where its validation raises.
fn parse_entry(d: &Document) -> Result<Entry, ()> {
    if !matches!(d.get("key"), Some(Bson::String(_))) {
        return Err(());
    }
    let err = opt_int(d, "err_status")?;
    let code = opt_str(d, "err_code")?;
    let msg = opt_str(d, "err_msg")?;
    opt_int(d, "byte_len")?;
    opt_int(d, "voxel_count")?;
    Ok(Entry {
        id: d.get("_id").cloned().unwrap_or(Bson::Null),
        blob_sha: opt_str(d, "blob_sha")?,
        err: err.filter(|s| *s != 0).map(|s| (s, code, msg)),
    })
}

fn store(state: &AppState) -> ContentStore {
    ContentStore::new(&state.settings().mods_store_dir)
}

async fn lookup(state: &AppState, key: &str) -> (Option<Entry>, bool) {
    let Some(db) = state.inner.db.mongo.as_ref() else {
        tracing::warn!("bp_cache: index unavailable, decoding uncached");
        return (None, false);
    };
    match db.collection::<Document>(COLLECTION).find_one(doc! {"key": key}).await {
        Ok(None) => (None, true),
        Ok(Some(d)) => match parse_entry(&d) {
            Ok(e) => (Some(e), true),
            Err(()) => {
                tracing::warn!("bp_cache: index unavailable, decoding uncached");
                (None, false)
            }
        },
        Err(e) => {
            tracing::warn!("bp_cache: index unavailable, decoding uncached: {e}");
            (None, false)
        }
    }
}

async fn remember(state: &AppState, row: Document) {
    let Some(db) = state.inner.db.mongo.as_ref() else { return };
    if let Err(e) = db.collection::<Document>(COLLECTION).insert_one(row).await
        && !e.to_string().contains("E11000")
    {
        tracing::warn!("bp_cache: could not record: {e}");
    }
}

pub fn row(key: &str, blob_sha: Option<&str>, byte_len: usize, count: i64, err: Option<&ApiError>) -> Document {
    let opt = |v: Option<&str>| v.map_or(Bson::Null, |s| Bson::String(s.to_owned()));
    doc! {
        "_id": ObjectId::new(),
        "key": key,
        "blob_sha": opt(blob_sha),
        "byte_len": bson_int(byte_len as i64),
        "voxel_count": bson_int(count),
        "size": Bson::Null,
        "err_status": err.map_or(Bson::Null, |e| bson_int(i64::from(e.status.as_u16()))),
        "err_code": opt(err.map(|e| e.code)),
        "err_msg": opt(err.map(|e| e.message.as_str())),
        "created_at": Bson::DateTime(dt::to_bson(dt::now())),
    }
}

/// `gzip.compress(json.dumps(payload, separators=(",", ":")).encode(), 6)`.
pub fn encode(payload: &Value) -> Vec<u8> {
    let body = pyjson::dumps_compact_ascii(payload);
    let mtime = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as u32);
    gzip(body.as_bytes(), mtime)
}

/// CPython's `gzip.compress(data, 6, mtime=mtime)`.
pub fn gzip(data: &[u8], mtime: u32) -> Vec<u8> {
    kiwi_zlib::gzip_compress(data, 6, mtime).expect("deflate")
}

/// `Cached.payload()`: the JSON body of a stored blob.
fn decode(gz: &[u8]) -> Result<Value, ApiError> {
    let raw = kiwi_core::zlib::gunzip(gz).map_err(|e| {
        tracing::error!("bp_cache: unreadable blob: {e}");
        ApiError::internal()
    })?;
    pyjson::loads(&raw).map_err(|e| {
        tracing::error!("bp_cache: blob is not JSON: {e:?}");
        ApiError::internal()
    })
}

async fn read_blob(state: &AppState, sha: String) -> Option<Vec<u8>> {
    let s = store(state);
    match tokio::task::spawn_blocking(move || s.get(&sha)).await {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            tracing::warn!("bp_cache: could not read blob: {e}");
            None
        }
        Err(_) => None,
    }
}

/// `get_or_build(key, build)` followed by `cached.payload()`. `build` errors
/// with status 413 or 422 are remembered and replayed on later hits.
pub async fn get_or_build<F, Fut>(state: &AppState, key: &str, build: F) -> Result<Value, ApiError>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Value, ApiError>>,
{
    let (entry, usable) = lookup(state, key).await;
    if let Some(entry) = entry {
        if let Some((status, code, msg)) = entry.err {
            let status = u16::try_from(status)
                .ok()
                .and_then(|s| StatusCode::from_u16(s).ok())
                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
            let code = static_code(code.as_deref().filter(|c| !c.is_empty()).unwrap_or("bad_request"));
            let msg = msg.filter(|m| !m.is_empty()).unwrap_or_else(|| "This blueprint can't be previewed.".into());
            return Err(ApiError::new(status, code, msg));
        }
        if let Some(sha) = entry.blob_sha.filter(|s| !s.is_empty()) {
            if let Some(gz) = read_blob(state, sha.clone()).await {
                return decode(&gz);
            }
            tracing::info!("bp_cache: blob {sha} missing for {key}, rebuilding");
            if let Some(db) = state.inner.db.mongo.as_ref()
                && let Err(e) = db.collection::<Document>(COLLECTION).delete_one(doc! {"_id": entry.id}).await
            {
                tracing::warn!("bp_cache: could not drop stale row {key}: {e}");
            }
        }
    }

    let payload = match build().await {
        Ok(p) => p,
        Err(e) => {
            if usable && matches!(e.status.as_u16(), 413 | 422) {
                remember(state, row(key, None, 0, 0, Some(&e))).await;
            }
            return Err(e);
        }
    };
    if usable {
        let count = payload.get("count").and_then(Value::as_i64).unwrap_or(0);
        let gz = {
            let p = payload.clone();
            tokio::task::spawn_blocking(move || encode(&p)).await.map_err(|_| ApiError::internal())?
        };
        let s = store(state);
        let len = gz.len();
        match tokio::task::spawn_blocking(move || s.put(&gz)).await {
            Ok(Ok((sha, _))) => remember(state, row(key, Some(&sha), len, count, None)).await,
            Ok(Err(e)) => tracing::warn!("bp_cache: could not write a {len}-byte payload: {e}"),
            Err(_) => {}
        }
    }
    Ok(payload)
}
