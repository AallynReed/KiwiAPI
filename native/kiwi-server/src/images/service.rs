//! `app/images/service.py`: CRUD, the upload, the blob reads and the cached
//! render.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{LazyLock, Mutex},
};

use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use mongodb::{
    Collection,
    bson::{Document, doc, oid::ObjectId},
};
use serde_json::{Value, json};

use super::{
    contexts,
    model::{Background, COLLECTION, Design, DesignBody, Layer, MAX_DESIGNS_PER_USER},
    render::{self, Ctx, Scene},
};
use crate::{
    error::{ApiError, ApiResult},
    state::AppState,
};

fn coll(state: &AppState) -> Collection<Document> {
    state.mongo().collection(COLLECTION)
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    tracing::error!("image studio: {e}");
    ApiError::internal()
}

fn not_found() -> ApiError {
    ApiError::not_found("Image design not found.")
}

/// `ContentStore(settings.mods_store_dir).path_for(sha)`; only names that stay
/// inside the store.
pub fn blob_path(root: &str, sha: &str) -> Option<PathBuf> {
    if sha.chars().count() < 2 || !sha.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    Some(std::path::Path::new(root).join("objects").join(&sha[..2]).join(sha))
}

async fn get_blob(root: &str, sha: &str) -> Option<Vec<u8>> {
    tokio::fs::read(blob_path(root, sha)?).await.ok()
}

/// `ContentStore.put`: atomic, idempotent; `(sha, created)`.
pub async fn put_blob(root: &str, data: &[u8]) -> std::io::Result<(String, bool)> {
    use sha2::{Digest, Sha256};
    let sha = hex::encode(Sha256::digest(data));
    let dest = blob_path(root, &sha).ok_or_else(|| std::io::Error::other("bad sha"))?;
    if tokio::fs::metadata(&dest).await.is_ok_and(|m| m.is_file()) {
        return Ok((sha, false));
    }
    let dir = dest.parent().map(PathBuf::from).unwrap_or_default();
    let data = data.to_vec();
    let dest2 = dest.clone();
    tokio::task::spawn_blocking(move || -> std::io::Result<()> {
        use std::io::Write;
        std::fs::create_dir_all(&dir)?;
        let mut tmp = tempfile_in(&dir)?;
        tmp.1.write_all(&data)?;
        tmp.1.flush()?;
        tmp.1.sync_all()?;
        drop(tmp.1);
        std::fs::rename(&tmp.0, &dest2).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp.0);
        })
    })
    .await
    .map_err(std::io::Error::other)??;
    Ok((sha, true))
}

fn tempfile_in(dir: &std::path::Path) -> std::io::Result<(PathBuf, std::fs::File)> {
    for _ in 0..16 {
        let mut b = [0u8; 6];
        getrandom::fill(&mut b).map_err(std::io::Error::other)?;
        let p = dir.join(format!("tmp{}.part", hex::encode(b)));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&p) {
            Ok(f) => return Ok((p, f)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("no temp name"))
}

/// `store.sniff_image`: `(content_type, width, height)`.
pub fn sniff_image(data: &[u8]) -> Option<(&'static str, Option<u32>, Option<u32>)> {
    if data.len() < 24 {
        return None;
    }
    if &data[..8] == b"\x89PNG\r\n\x1a\n" {
        let w = u32::from_be_bytes([data[16], data[17], data[18], data[19]]);
        let h = u32::from_be_bytes([data[20], data[21], data[22], data[23]]);
        return Some(("image/png", Some(w), Some(h)));
    }
    if &data[..6] == b"GIF87a" || &data[..6] == b"GIF89a" {
        let w = u16::from_le_bytes([data[6], data[7]]) as u32;
        let h = u16::from_le_bytes([data[8], data[9]]) as u32;
        return Some(("image/gif", Some(w), Some(h)));
    }
    if &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        let (w, h) = webp_dims(data);
        return Some(("image/webp", w, h));
    }
    if &data[..2] == b"\xff\xd8" {
        let (w, h) = jpeg_dims(data);
        return Some(("image/jpeg", w, h));
    }
    None
}

fn webp_dims(d: &[u8]) -> (Option<u32>, Option<u32>) {
    let at = |i: usize| d.get(i).copied().map(u32::from);
    let le16 = |i: usize| Some(at(i)? | at(i + 1)? << 8);
    match &d[12..16] {
        b"VP8 " => match (le16(26), le16(28)) {
            (Some(w), Some(h)) => (Some(w & 0x3FFF), Some(h & 0x3FFF)),
            _ => (None, None),
        },
        b"VP8L" => match (at(21), at(22), at(23), at(24)) {
            (Some(a), Some(b), Some(c), Some(e)) => {
                let bits = a | b << 8 | c << 16 | e << 24;
                (Some((bits & 0x3FFF) + 1), Some(((bits >> 14) & 0x3FFF) + 1))
            }
            _ => (None, None),
        },
        b"VP8X" => {
            let w = (|| Some((at(24)? | at(25)? << 8 | at(26)? << 16) + 1))();
            let h = (|| Some((at(27)? | at(28)? << 8 | at(29)? << 16) + 1))();
            match (w, h) {
                (Some(w), Some(h)) => (Some(w), Some(h)),
                _ => (None, None),
            }
        }
        _ => (None, None),
    }
}

fn jpeg_dims(d: &[u8]) -> (Option<u32>, Option<u32>) {
    let (mut i, n) = (2usize, d.len());
    while i + 9 < n {
        if d[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = d[i + 1];
        if (0xC0..=0xCF).contains(&marker) && ![0xC4, 0xC8, 0xCC].contains(&marker) {
            let h = u16::from_be_bytes([d[i + 5], d[i + 6]]) as u32;
            let w = u16::from_be_bytes([d[i + 7], d[i + 8]]) as u32;
            return (Some(w), Some(h));
        }
        if marker == 0xD8 || marker == 0xD9 || (0xD0..=0xD7).contains(&marker) {
            i += 2;
            continue;
        }
        let seg = u16::from_be_bytes([d[i + 2], d[i + 3]]) as usize;
        i += 2 + seg;
    }
    (None, None)
}

async fn load(state: &AppState, design_id: &str) -> ApiResult<Option<Design>> {
    let Ok(oid) = ObjectId::parse_str(design_id) else { return Ok(None) };
    let doc = coll(state).find_one(doc! {"_id": oid}).await.map_err(internal)?;
    Ok(doc.as_ref().and_then(Design::from_doc))
}

/// `_owned`.
async fn owned(state: &AppState, actor: ObjectId, design_id: &str) -> ApiResult<Design> {
    load(state, design_id).await?.filter(|d| d.owner_id == actor).ok_or_else(not_found)
}

pub async fn list(state: &AppState, actor: ObjectId) -> ApiResult<Value> {
    let docs: Vec<Document> = coll(state)
        .find(doc! {"owner_id": actor})
        .sort(doc! {"created_at": -1})
        .await
        .map_err(internal)?
        .try_collect()
        .await
        .map_err(internal)?;
    let asset = state.settings().asset_url();
    let items: Vec<Value> = docs.iter().filter_map(Design::from_doc).map(|d| d.dto(&asset)).collect();
    Ok(json!({"items": items}))
}

pub async fn get(state: &AppState, actor: ObjectId, design_id: &str) -> ApiResult<Value> {
    Ok(owned(state, actor, design_id).await?.dto(&state.settings().asset_url()))
}

pub async fn create(state: &AppState, actor: ObjectId, body: &DesignBody, now: DateTime<Utc>) -> ApiResult<Value> {
    let count = coll(state).count_documents(doc! {"owner_id": actor}).await.map_err(internal)?;
    if count >= MAX_DESIGNS_PER_USER {
        return Err(ApiError::bad_request(format!("You can have at most {MAX_DESIGNS_PER_USER} image designs.")));
    }
    let mut d = Design::create(ObjectId::new(), actor, body, now);
    d.clamp(contexts::is_bindable);
    let mut stored = doc! {"_id": d.id};
    stored.extend(d.fields());
    coll(state).insert_one(stored).await.map_err(internal)?;
    Ok(d.dto(&state.settings().asset_url()))
}

pub async fn update(
    state: &AppState,
    actor: ObjectId,
    design_id: &str,
    body: &DesignBody,
    now: DateTime<Utc>,
) -> ApiResult<Value> {
    let mut d = owned(state, actor, design_id).await?;
    d.assign(body);
    d.clamp(contexts::is_bindable);
    d.updated_at = now;
    let mut set = doc! {"_id": d.id};
    set.extend(d.fields());
    coll(state).update_one(doc! {"_id": d.id}, doc! {"$set": set}).upsert(true).await.map_err(internal)?;
    Ok(d.dto(&state.settings().asset_url()))
}

pub async fn delete(state: &AppState, actor: ObjectId, design_id: &str) -> ApiResult<()> {
    let d = owned(state, actor, design_id).await?;
    coll(state).delete_one(doc! {"_id": d.id}).await.map_err(internal)?;
    Ok(())
}

/// `upload_image`.
pub async fn upload(state: &AppState, data: &[u8]) -> ApiResult<Value> {
    let max = state.settings().mods_image_max_bytes;
    if data.len() as i64 > max {
        return Err(ApiError::new(
            axum::http::StatusCode::PAYLOAD_TOO_LARGE,
            "bad_request",
            format!("Image exceeds the {max}-byte limit."),
        ));
    }
    let Some((ct, w, h)) = sniff_image(data) else {
        return Err(ApiError::bad_request("Unsupported image - use PNG, JPEG, WebP or GIF."));
    };
    let (sha, _) = put_blob(&state.settings().mods_store_dir, data).await.map_err(internal)?;
    Ok(json!({"sha": sha, "content_type": ct, "width": w, "height": h}))
}

/// Every image blob a design references (`_load_blobs`).
pub fn blob_shas(background: &Background, layers: &[Layer]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |s: &Option<String>| {
        if let Some(s) = s.as_ref().filter(|s| !s.is_empty())
            && !out.contains(s)
        {
            out.push(s.clone());
        }
    };
    if background.kind == "image" {
        add(&background.image_sha);
    }
    for l in layers {
        if l.kind == "image" {
            add(&l.image_sha);
        }
    }
    out
}

async fn load_blobs(state: &AppState, shas: &[String]) -> HashMap<String, Vec<u8>> {
    let root = &state.settings().mods_store_dir;
    let mut out = HashMap::new();
    for sha in shas {
        if let Some(data) = get_blob(root, sha).await {
            out.insert(sha.clone(), data);
        }
    }
    out
}

/// Whether a blob is something [`render`] reads like Pillow: absent, or a PNG
/// Rust converts. Reads only the header.
pub async fn blob_renderable(state: &AppState, sha: &str) -> bool {
    use tokio::io::AsyncReadExt;
    let Some(path) = blob_path(&state.settings().mods_store_dir, sha) else { return false };
    let Ok(mut f) = tokio::fs::File::open(&path).await else { return true };
    let mut head = [0u8; 33];
    let mut n = 0;
    while n < head.len() {
        match f.read(&mut head[n..]).await {
            Ok(0) | Err(_) => break,
            Ok(k) => n += k,
        }
    }
    n == 0 || kiwi_pil::decode::png_supported(&head[..n])
}

static CACHE: LazyLock<Mutex<HashMap<String, (f64, Bytes)>>> = LazyLock::new(Default::default);
const CACHE_TTL: f64 = 60.0;
const CACHE_MAX: usize = 256;

fn now_f64() -> f64 {
    crate::activity::unix_now_f64()
}

fn cache_get(key: &str) -> Option<Bytes> {
    let cache = CACHE.lock().ok()?;
    let (at, png) = cache.get(key)?;
    (now_f64() - at < CACHE_TTL).then(|| png.clone())
}

fn cache_put(key: String, png: Bytes) {
    let Ok(mut cache) = CACHE.lock() else { return };
    if cache.len() >= CACHE_MAX
        && let Some(oldest) = cache.iter().min_by(|a, b| a.1.0.total_cmp(&b.1.0)).map(|(k, _)| k.clone())
    {
        cache.remove(&oldest);
    }
    cache.insert(key, (now_f64(), png));
}

async fn draw_png(
    state: &AppState,
    bg: Background,
    layers: Vec<Layer>,
    size: (i64, i64),
    ctx: Ctx,
) -> ApiResult<Bytes> {
    let shas = blob_shas(&bg, &layers);
    let blobs = load_blobs(state, &shas).await;
    let fonts = crate::og::text::fonts().ok_or_else(|| internal("DejaVu fonts missing"))?;
    let now = (now_f64()).trunc() as i64;
    tokio::task::spawn_blocking(move || {
        let lib = kiwi_pil::Library::new().map_err(|e| internal(format!("{e}")))?;
        let scene = Scene { width: size.0, height: size.1, background: &bg, layers: &layers };
        let img = render::draw(&lib, fonts, &scene, &ctx, &blobs, now);
        Ok(Bytes::from(render::encode(&img)))
    })
    .await
    .map_err(internal)?
}

/// `render_public`: `None` for a missing design.
pub async fn render_public(state: &AppState, design_id: &str, kind: Option<&str>) -> ApiResult<Option<Bytes>> {
    let Some(d) = load(state, design_id).await? else { return Ok(None) };
    let bind = kind.filter(|k| !k.is_empty()).map(str::to_owned).or_else(|| d.bind_type.clone());
    let now_us = crate::dt::now().timestamp_micros();
    let ctx = match bind.as_deref().filter(|b| !b.is_empty() && contexts::is_bindable(b)) {
        Some(b) => contexts::live(state, b, now_us).await,
        None => Ctx::new(),
    };
    let sig = ["ends_at", "starts_at"]
        .iter()
        .filter_map(|k| ctx.get(*k))
        .find(|v| crate::features::py_truthy(v))
        .map(crate::feeds::pyval::py_str)
        .unwrap_or_default();
    let key = format!(
        "{}:{}:{}:{sig}:{}",
        d.id.to_hex(),
        d.updated_at.timestamp_micros(),
        bind.as_deref().unwrap_or("None"),
        (now_f64() / 60.0).floor() as i64
    );
    if let Some(png) = cache_get(&key) {
        return Ok(Some(png));
    }
    let png = draw_png(state, d.background, d.layers, (d.width, d.height), ctx).await?;
    cache_put(key, png.clone());
    Ok(Some(png))
}

/// The studio preview: an unsaved design with sample data.
pub async fn preview(state: &AppState, body: DesignBody, kind: Option<&str>) -> ApiResult<Bytes> {
    let bind = kind.filter(|k| !k.is_empty()).map(str::to_owned).or(body.bind_type.clone());
    let ctx = match bind.as_deref().filter(|b| !b.is_empty() && contexts::is_bindable(b)) {
        Some(b) => contexts::sample(b),
        None => Ctx::new(),
    };
    draw_png(state, body.background, body.layers, (body.width, body.height), ctx).await
}

/// The image blobs a design document refers to (for the defer check).
pub async fn design_blobs(state: &AppState, design_id: &str) -> Option<Vec<String>> {
    let d = load(state, design_id).await.ok().flatten()?;
    Some(blob_shas(&d.background, &d.layers))
}

pub fn design_body_blobs(v: &Value) -> Vec<String> {
    let s = |v: Option<&Value>| match v {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    };
    let mut out = Vec::new();
    if let Some(bg) = v.get("background")
        && bg.get("type").and_then(Value::as_str) == Some("image")
        && let Some(sha) = s(bg.get("image_sha"))
    {
        out.push(sha);
    }
    if let Some(Value::Array(layers)) = v.get("layers") {
        for l in layers {
            if l.get("type").and_then(Value::as_str) == Some("image")
                && let Some(sha) = s(l.get("image_sha"))
            {
                out.push(sha);
            }
        }
    }
    out
}
