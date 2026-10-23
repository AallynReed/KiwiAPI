//! `app/trove/swf/service.py` and the three `/site/updates/{branch}/file/swf*`
//! routes: a movie's artwork extracted once into a cached manifest (images in
//! the mods content store), then listed, served one by one, or zipped.
//! [`decompile`] is the FFDec engine behind the code views.

pub mod decompile;
pub mod extract;

use axum::{
    Router,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::{Map, Value, json};

use super::{SiteGate, bpcache, cas::ContentStore, check_branch, file_sha, latin1_header, read_blob, sanitize};
use crate::{
    admin::Args,
    error::ApiError,
    pyfloat,
    pyjson::PyJson,
    state::AppState,
    validation::{FromParams, Params},
};

pub const MAX_ZIP_BYTES: usize = 256 * 1024 * 1024;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/site/updates/{branch}/file/swf", get(site_swf))
        .route("/site/updates/{branch}/file/swf/asset", get(site_swf_asset))
        .route("/site/updates/{branch}/file/swf/zip", get(site_swf_zip))
}

/// `_build_sync`: extract, store every image, return the manifest.
pub fn build_manifest(raw: &[u8], store: &ContentStore) -> Result<Value, extract::Fail> {
    let (header, images, inventory) = extract::extract_images(raw)?;
    let io = |e: std::io::Error| extract::Fail::Crash(e.to_string());
    let mut assets = Vec::with_capacity(images.len());
    for img in &images {
        let (sha, _) = store.put(&img.data).map_err(io)?;
        let thumb_sha = match &img.thumb {
            Some(t) => Value::String(store.put(t).map_err(io)?.0),
            None => Value::Null,
        };
        assets.push(json!({
            "id": img.char_id,
            "name": img.name,
            "source": img.source,
            "codec": img.codec,
            "width": img.width,
            "height": img.height,
            "mime": img.mime,
            "bytes": img.data.len(),
            "sha": sha,
            "thumb_sha": thumb_sha,
        }));
    }
    let inv: Map<String, Value> = inventory.into_iter().map(|(k, v)| (k.to_owned(), v.into())).collect();
    let count = assets.len();
    Ok(json!({
        "swf": {
            "version": header.version,
            "compression": header.compression,
            "width": header.width,
            "height": header.height,
            "frame_rate": pyfloat::round(header.frame_rate, 2),
            "frame_count": header.frame_count,
        },
        "inventory": inv,
        "assets": assets,
        "count": count,
    }))
}

/// `service.manifest(raw, sha)`: the cached manifest, extracting on a miss.
pub async fn manifest(state: &AppState, raw: Vec<u8>, sha: &str) -> Result<Value, ApiError> {
    let store = super::mods_store(state);
    let label = sha.chars().take(12).collect::<String>();
    bpcache::get_or_build(state, &bpcache::key_for_swf(sha), || async move {
        let built = tokio::task::spawn_blocking(move || build_manifest(&raw, &store)).await;
        match built {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(extract::Fail::Swf(e))) => {
                tracing::info!("swf: cannot read movie {label}: {e}");
                Err(ApiError::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "bad_request",
                    "This file could not be read as a Flash movie.",
                ))
            }
            Ok(Err(extract::Fail::Crash(e))) => {
                tracing::error!("swf: extraction of {label} failed: {e}");
                Err(ApiError::internal())
            }
            Err(_) => Err(ApiError::internal()),
        }
    })
    .await
}

pub struct FileQ {
    pub branch: String,
    pub path: String,
}

impl FromParams for FileQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        FileQ { branch: p.path("branch").str().required(), path: p.query("path").str().required() }
    }
}

pub struct AssetQ {
    branch: String,
    path: String,
    id: i64,
    thumb: bool,
}

impl FromParams for AssetQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        AssetQ {
            branch: p.path("branch").str().required(),
            path: p.query("path").str().required(),
            id: p.query("id").int().ge(0).le(65535).required(),
            thumb: p.query("thumb").bool().default(false),
        }
    }
}

/// `_swf_manifest(branch, path)`.
async fn swf_manifest(state: &AppState, branch: &str, path: &str) -> Result<(Value, String), ApiError> {
    check_branch(branch)?;
    if !path.to_lowercase().ends_with(".swf") {
        return Err(ApiError::http(StatusCode::BAD_REQUEST, Some("Not a .swf file")));
    }
    let Some(sha) = file_sha(state, branch, path).await? else {
        return Err(ApiError::http(StatusCode::NOT_FOUND, Some(&format!("No file '{path}'"))));
    };
    let Some(raw) = read_blob(super::update_store(state), sha.clone()).await? else {
        return Err(ApiError::http(StatusCode::NOT_FOUND, Some("Blob missing from the store")));
    };
    Ok((manifest(state, raw, &sha).await?, sha))
}

fn public_max_age(secs: u32) -> HeaderValue {
    HeaderValue::from_str(&format!("public, max-age={secs}")).unwrap_or(HeaderValue::from_static("no-store"))
}

async fn site_swf(_: SiteGate, State(state): State<AppState>, Args(q): Args<FileQ>) -> Result<Response, ApiError> {
    let (payload, sha) = swf_manifest(&state, &q.branch, &q.path).await?;
    let assets: Vec<Value> = payload
        .get("assets")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .map(|a| {
                    let mut m: Map<String, Value> = a
                        .as_object()
                        .map(|o| {
                            o.iter()
                                .filter(|(k, _)| *k != "sha" && *k != "thumb_sha")
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect()
                        })
                        .unwrap_or_default();
                    let thumb = a.get("thumb_sha").is_some_and(|t| !t.is_null());
                    m.insert("thumb".into(), thumb.into());
                    Value::Object(m)
                })
                .collect()
        })
        .unwrap_or_default();
    let count = assets.len();
    let body = json!({
        "branch": q.branch,
        "path": q.path,
        "content_sha256": sha,
        "swf": payload.get("swf").cloned().unwrap_or(Value::Null),
        "inventory": payload.get("inventory").cloned().unwrap_or(Value::Null),
        "assets": assets,
        "count": count,
    });
    Ok(([(header::CACHE_CONTROL, public_max_age(300))], PyJson(body)).into_response())
}

/// Python truthiness of a JSON value.
pub fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// `a["id"] == id` for a manifest row.
fn id_matches(a: &Value, id: i64) -> Result<bool, ApiError> {
    match a.get("id") {
        Some(Value::Bool(b)) => Ok(i64::from(*b) == id),
        Some(v) => Ok(v.as_i64() == Some(id) || v.as_f64() == Some(id as f64)),
        None => Err(ApiError::internal()),
    }
}

fn str_field(a: &Value, k: &str) -> Result<String, ApiError> {
    a.get(k).and_then(Value::as_str).map(str::to_owned).ok_or_else(ApiError::internal)
}

async fn site_swf_asset(
    _: SiteGate,
    State(state): State<AppState>,
    headers: HeaderMap,
    Args(q): Args<AssetQ>,
) -> Result<Response, ApiError> {
    let (payload, _) = swf_manifest(&state, &q.branch, &q.path).await?;
    let mut found = None;
    for a in payload.get("assets").and_then(Value::as_array).into_iter().flatten() {
        if id_matches(a, q.id)? {
            found = Some(a);
            break;
        }
    }
    let Some(asset) = found else {
        return Err(ApiError::http(StatusCode::NOT_FOUND, Some(&format!("No asset {} in '{}'", q.id, q.path))));
    };
    let use_thumb = q.thumb && truthy(asset.get("thumb_sha"));
    let blob_sha = str_field(asset, if use_thumb { "thumb_sha" } else { "sha" })?;
    let etag = latin1_header(&format!("\"{blob_sha}\""))?;
    if headers.get(header::IF_NONE_MATCH).is_some_and(|v| v.as_bytes() == etag.as_bytes()) {
        return Ok((StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response());
    }
    let Some(data) = read_blob(super::mods_store(&state), blob_sha).await? else {
        return Err(ApiError::http(StatusCode::NOT_FOUND, Some("Asset blob missing from the store")));
    };
    let media = if use_thumb { "image/png".to_owned() } else { str_field(asset, "mime")? };
    Ok((
        [
            (header::CONTENT_TYPE, latin1_header(&media)?),
            (header::ETAG, etag),
            (header::CACHE_CONTROL, public_max_age(86400)),
        ],
        data,
    )
        .into_response())
}

/// `_zip_name`.
fn zip_name(asset: &Value, taken: &mut std::collections::HashSet<String>) -> Result<String, ApiError> {
    let ext = match asset.get("mime").and_then(Value::as_str) {
        Some("image/jpeg") => "jpg",
        Some("image/gif") => "gif",
        _ => "png",
    };
    let id = asset.get("id").and_then(Value::as_i64).ok_or_else(ApiError::internal)?;
    let name = asset.get("name").and_then(Value::as_str).filter(|s| !s.is_empty());
    let stem = name.map_or_else(|| format!("asset_{id}"), str::to_owned);
    let stem = sanitize(&stem, "._- ");
    let stem = kiwi_core::pystr::strip(&stem);
    let stem = if stem.is_empty() { "asset" } else { stem };
    let base = format!("{id:04}_{stem}");
    let mut out = format!("{base}.{ext}");
    let mut n = 2;
    while taken.contains(&out) {
        out = format!("{base}_{n}.{ext}");
        n += 1;
    }
    taken.insert(out.clone());
    Ok(out)
}

/// `build_zip`.
pub fn build_zip(assets: &[Value], store: &ContentStore) -> Result<Vec<u8>, ApiError> {
    let mut zip = super::zipfile::ZipWriter::new();
    let mut taken = std::collections::HashSet::new();
    let mut total = 0usize;
    for asset in assets {
        let sha = str_field(asset, "sha")?;
        let Some(data) = store.get(&sha).map_err(|_| ApiError::internal())? else { continue };
        total += data.len();
        if total > MAX_ZIP_BYTES {
            return Err(ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "bad_request",
                "These assets are too large to download as one archive.",
            ));
        }
        let name = zip_name(asset, &mut taken)?;
        zip.writestr(&name, &data);
    }
    Ok(zip.finish())
}

async fn site_swf_zip(_: SiteGate, State(state): State<AppState>, Args(q): Args<FileQ>) -> Result<Response, ApiError> {
    let (payload, sha) = swf_manifest(&state, &q.branch, &q.path).await?;
    let assets = payload.get("assets").and_then(Value::as_array).cloned().unwrap_or_default();
    if assets.is_empty() {
        return Err(ApiError::http(StatusCode::NOT_FOUND, Some("This movie has no extractable images")));
    }
    let store = super::mods_store(&state);
    let blob =
        tokio::task::spawn_blocking(move || build_zip(&assets, &store)).await.map_err(|_| ApiError::internal())??;
    let stem = super::file_stem(&q.path, "assets");
    Ok((
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("application/zip")),
            (header::CONTENT_DISPOSITION, latin1_header(&format!("attachment; filename=\"{stem}-assets.zip\""))?),
            (header::ETAG, latin1_header(&format!("\"{sha}-zip\""))?),
            (header::CACHE_CONTROL, public_max_age(3600)),
        ],
        blob,
    )
        .into_response())
}
