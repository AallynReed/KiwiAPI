//! `/v1/codexes/render` and `/site/codexes/render` (`render/service.py`): a
//! blueprint resolved from the update archive (or a local install), drawn and
//! cached in Redis as base64 PNG. A `?prefab=` creature is served here only
//! once Python has assembled and cached it.

use axum::{
    extract::{FromRequestParts, Request, State},
    http::{HeaderValue, StatusCode, Uri, header, request::Parts},
    response::{IntoResponse, Response},
};
use base64::Engine;
use mongodb::bson::{Bson, Document, doc};

use super::{SiteGate, cache};
use crate::{
    auth::{extract::public_scope, scopes},
    error::{self, ApiError},
    features::{self, Codexes},
    state::AppState,
    validation::{FromParams, Params, Query},
};

pub const RENDER_VERSION: &str = "r2";
pub const STILL_VERSION: &str = "s2";
const BRANCHES: [&str; 2] = ["live-us", "pts"];
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

pub fn blueprint_key(branch: &str, name: &str, dim: i64) -> String {
    format!("render:bp:{RENDER_VERSION}:{branch}:{dim}:{}", name.replace('\\', "/").to_lowercase())
}

pub fn creature_key(branch: &str, prefab: &str, dim: i64) -> String {
    format!(
        "render:rig:{}:{STILL_VERSION}:{branch}:{dim}:{}",
        cache::ASSEMBLY_VERSION,
        prefab.replace('\\', "/").to_lowercase()
    )
}

async fn redis_get(state: &AppState, key: &str) -> Option<Option<String>> {
    let mut conn = state.inner.db.redis.clone()?;
    match redis::cmd("GET").arg(key).query_async::<Option<String>>(&mut conn).await {
        Ok(v) => Some(v),
        Err(e) => {
            tracing::warn!("render: redis get failed for {key}: {e}");
            None
        }
    }
}

/// A cached PNG: `Some(bytes)` for a non-empty, decodable value.
async fn cached_png(state: &AppState, key: &str) -> Option<Vec<u8>> {
    let v = redis_get(state, key).await.flatten().filter(|s| !s.is_empty())?;
    B64.decode(v.as_bytes()).ok()
}

/// The last value of a query parameter.
fn query_last(uri: &Uri, name: &str) -> Option<String> {
    form_urlencoded::parse(uri.query()?.as_bytes()).filter(|(k, _)| k == name).map(|(_, v)| v.into_owned()).last()
}

/// `routes::defer_to_python`: a non-empty `prefab` whose creature PNG isn't in
/// Redis yet goes to Python, which assembles it; anything Rust would reject
/// first (bad branch or `dim`) stays here.
pub async fn creature_uncached(state: &AppState, uri: &Uri, default_dim: i64) -> bool {
    let Some(prefab) = query_last(uri, "prefab").filter(|p| !p.is_empty()) else { return false };
    let branch = query_last(uri, "branch").unwrap_or_else(|| "live-us".into());
    if !BRANCHES.contains(&branch.as_str()) {
        return false;
    }
    let dim = match query_last(uri, "dim") {
        None => default_dim,
        Some(d) => match crate::validation::str_as_int(&d) {
            Ok(v) if (32..=512).contains(&v) => v as i64,
            _ => return false,
        },
    };
    cached_png(state, &creature_key(&branch, &prefab, dim)).await.is_none()
}

struct RenderQuery {
    blueprint: String,
    branch: String,
    dim: i64,
    prefab: Option<String>,
}

struct V1Query(RenderQuery);
struct SiteQuery(RenderQuery);

impl FromParams for V1Query {
    fn from_params(p: &mut Params<'_>) -> Self {
        V1Query(RenderQuery {
            blueprint: p.query("blueprint").str().required(),
            branch: p.query("branch").str().default("live-us".into()),
            dim: p.query("dim").int().ge(32).le(512).default(256),
            prefab: p.query("prefab").str().optional(),
        })
    }
}

impl FromParams for SiteQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        SiteQuery(RenderQuery {
            blueprint: p.query("blueprint").str().min_len(1).required(),
            branch: p.query("branch").str().default("live-us".into()),
            dim: p.query("dim").int().ge(32).le(512).default(160),
            prefab: p.query("prefab").str().optional(),
        })
    }
}

/// Python's `re.escape`.
fn re_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        if "()[]{}?*+-|^$\\.&~# \t\n\r\u{0b}\u{0c}".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

async fn state_sha(db: &mongodb::Database, filter: Document) -> mongodb::error::Result<Option<Option<String>>> {
    let d = db.collection::<Document>("update_state").find_one(filter).await?;
    Ok(d.map(|d| match d.get("content_sha256") {
        Some(Bson::String(s)) => Some(s.clone()),
        _ => None,
    }))
}

/// `name.replace("\\", "/").strip()` without a leading `blueprints/` (any case).
fn unprefixed(name: &str) -> String {
    let clean = kiwi_core::pystr::strip(&name.replace('\\', "/")).to_owned();
    if clean.to_lowercase().starts_with("blueprints/") { clean.chars().skip(11).collect() } else { clean }
}

/// `_from_store`: the archived blob of `blueprints/<name>` (exact, lowercase,
/// then case-insensitive).
async fn from_store(state: &AppState, name: &str, branch: &str) -> anyhow::Result<Option<Vec<u8>>> {
    let clean = unprefixed(name);
    let path = format!("blueprints/{clean}");
    let db = state.inner.db.mongo.as_ref().ok_or_else(|| anyhow::anyhow!("no mongo"))?;
    let mut found = state_sha(db, doc! {"branch": branch, "path": &path}).await?;
    if found.is_none() {
        found = state_sha(db, doc! {"branch": branch, "path": path.to_lowercase()}).await?;
    }
    if found.is_none() {
        let regex = doc! {"$regex": format!("^{}$", re_escape(&path)), "$options": "i"};
        found = state_sha(db, doc! {"branch": branch, "path": regex}).await?;
    }
    let Some(sha) = found else { return Ok(None) };
    let sha = sha.ok_or_else(|| anyhow::anyhow!("update_state {path} has no content_sha256"))?;
    let root = state.inner.settings.trove_update_store_dir.clone();
    Ok(tokio::task::spawn_blocking(move || cache::store_get(&root, &sha)).await??)
}

/// `_from_local_sync`: a local install's `blueprints/**/index.tfi` + `archiveN.tfa`.
pub fn from_local(root: &str, name: &str) -> Option<Vec<u8>> {
    if root.is_empty() {
        return None;
    }
    let bp = std::path::Path::new(root).join("blueprints");
    let clean = unprefixed(name);
    let parts: Vec<&str> = clean.split('/').filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return None;
    }
    let (mut dir, mut rel) = (None, String::new());
    for i in (0..parts.len()).rev() {
        let sub = parts[..i].join("/");
        let candidate = if sub.is_empty() { bp.clone() } else { bp.join(&sub) };
        if candidate.join("index.tfi").is_file() {
            dir = Some(candidate);
            rel = parts[i..].join("/");
            break;
        }
    }
    let dir = dir?;
    let entries = parse_tfi(&std::fs::read(dir.join("index.tfi")).ok()?)?;
    let entry = lookup(&entries, &rel)?;
    let base = entry.name.rsplit('/').next().unwrap_or(&entry.name);
    let override_path = bp.join("override").join(base);
    if override_path.is_file() {
        return std::fs::read(override_path).ok();
    }
    let tfa = std::fs::read(dir.join(format!("archive{}.tfa", entry.archive))).ok()?;
    let content = kiwi_core::zlib::inflate(&tfa, usize::MAX)?.data;
    let start = (entry.offset as usize).min(content.len());
    let end = (entry.offset as usize + entry.size as usize).min(content.len());
    Some(content[start..end].to_vec())
}

struct TfiEntry {
    name: String,
    archive: u32,
    offset: u32,
    size: u32,
}

fn parse_tfi(data: &[u8]) -> Option<Vec<TfiEntry>> {
    let leb = |pos: usize| kiwi_core::bpedit::codec::read_uleb(data, pos).ok();
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        let (len, p) = leb(pos)?;
        let raw = data.get(p..(p + len as usize).min(data.len()))?;
        let raw = raw.split(|&b| b == 0).next().unwrap_or(&[]);
        let name = std::str::from_utf8(raw).ok()?.replace('\\', "/");
        let (archive, p) = leb(p + len as usize)?;
        let (offset, p) = leb(p)?;
        let (size, p) = leb(p)?;
        let (_, p) = leb(p)?;
        out.push(TfiEntry { name, archive, offset, size });
        pos = p;
    }
    Some(out)
}

/// `_lookup`: the exact name (with or without `.blueprint`), then the basename.
fn lookup<'a>(entries: &'a [TfiEntry], name: &str) -> Option<&'a TfiEntry> {
    let n = kiwi_core::pystr::strip(&name.replace('\\', "/")).to_lowercase();
    let n = n.strip_prefix("blueprints/").map(str::to_owned).unwrap_or(n);
    let mut cands = vec![n.clone()];
    if !n.ends_with(".blueprint") {
        cands.push(format!("{n}.blueprint"));
    }
    let by_name = |c: &str| entries.iter().rev().find(|e| e.name.to_lowercase() == c);
    let by_base = |c: &str| entries.iter().find(|e| e.name.rsplit('/').next().unwrap_or("").to_lowercase() == c);
    cands
        .iter()
        .find_map(|c| by_name(c))
        .or_else(|| cands.iter().find_map(|c| by_base(c.rsplit('/').next().unwrap_or(c))))
}

/// `get_blueprint_bytes(name, branch)`.
pub async fn blueprint_bytes(state: &AppState, name: &str, branch: &str) -> Option<Vec<u8>> {
    match from_store(state, name, branch).await {
        Ok(Some(d)) if !d.is_empty() => return Some(d),
        Ok(_) => {}
        Err(e) => tracing::warn!("render: store lookup failed for {name:?}: {e:#}"),
    }
    let (root, name) = (state.inner.settings.trove_local_game_dir.clone(), name.to_owned());
    tokio::task::spawn_blocking(move || from_local(&root, &name)).await.ok().flatten()
}

/// Why a render failed: the codec's `BlueprintError` text, or a 500.
pub enum RenderError {
    Blueprint(String),
    Unhandled,
}

/// `render_blueprint_cached(name, dim, branch)`: `Ok(None)` when not found.
pub async fn blueprint_png(
    state: &AppState,
    name: &str,
    dim: i64,
    branch: &str,
) -> Result<Option<Vec<u8>>, RenderError> {
    let key = blueprint_key(branch, name, dim);
    if let Some(png) = cached_png(state, &key).await {
        return Ok(Some(png));
    }
    let Some(data) = blueprint_bytes(state, name, branch).await else { return Ok(None) };
    let png = tokio::task::spawn_blocking(move || {
        kiwi_core::voxel::render_thumbnail(&data, dim as usize)
            .map(|rgba| kiwi_pil::png::encode_rgba_optimized(&rgba, dim as usize, dim as usize))
    })
    .await
    .map_err(|_| RenderError::Unhandled)?
    .map_err(|f| match f {
        kiwi_core::bpedit::Fault::Blueprint(m) => RenderError::Blueprint(m),
        other => {
            tracing::error!("render: {}", other.message());
            RenderError::Unhandled
        }
    })?;
    if let Some(mut conn) = state.inner.db.redis.clone() {
        let ttl = state.inner.settings.trove_render_cache_ttl;
        let set: redis::RedisResult<()> =
            redis::cmd("SET").arg(&key).arg(B64.encode(&png)).arg("EX").arg(ttl).query_async(&mut conn).await;
        if let Err(e) = set {
            tracing::warn!("render: redis set failed for {key}: {e}");
        }
    }
    Ok(Some(png))
}

fn png_response(png: Vec<u8>) -> Response {
    let mut res = Response::new(axum::body::Body::from(png));
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=86400"));
    res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    res
}

/// The creature PNG when Python has cached a non-empty one.
async fn creature(state: &AppState, q: &RenderQuery) -> Option<Vec<u8>> {
    let prefab = q.prefab.as_deref().filter(|p| !p.is_empty())?;
    cached_png(state, &creature_key(&q.branch, prefab, q.dim)).await.filter(|p| !p.is_empty())
}

/// `GET /v1/codexes/render`.
pub async fn v1_render(State(state): State<AppState>, req: Request) -> Response {
    let (mut parts, _) = req.into_parts();
    if let Err(e) = features::check::<Codexes>(&state).await {
        return e.into_response();
    }
    let multiplier = state.inner.settings.codexes_rate_limit_multiplier;
    if let Err(r) = public_scope(&state, &parts, scopes::CODEXES_READ, multiplier, None).await {
        return r.into_response();
    }
    let q = match Query::<V1Query>::from_request_parts(&mut parts, &state).await {
        Ok(Query(V1Query(q))) => q,
        Err(r) => return r.into_response(),
    };
    if !BRANCHES.contains(&q.branch.as_str()) {
        return ApiError::not_found(format!("Unknown branch '{}' (known: {})", q.branch, BRANCHES.join(", ")))
            .into_response();
    }
    if let Some(png) = creature(&state, &q).await {
        return png_response(png);
    }
    match blueprint_png(&state, &q.blueprint, q.dim, &q.branch).await {
        Ok(Some(png)) => png_response(png),
        Ok(None) => {
            ApiError::not_found(format!("No blueprint '{}' on branch '{}'", q.blueprint, q.branch)).into_response()
        }
        Err(RenderError::Blueprint(m)) => ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            crate::error::ErrorCode::BadRequest,
            format!("Blueprint not renderable: {m}"),
        )
        .into_response(),
        Err(RenderError::Unhandled) => ApiError::internal().into_response(),
    }
}

/// `GET /site/codexes/render`.
pub async fn site_render(State(state): State<AppState>, req: Request) -> Response {
    let (mut parts, _) = req.into_parts();
    if let Err(r) = SiteGate::from_request_parts(&mut parts, &state).await {
        return r;
    }
    let q = match Query::<SiteQuery>::from_request_parts(&mut parts, &state).await {
        Ok(Query(SiteQuery(q))) => q,
        Err(r) => return r.into_response(),
    };
    let http = |p: &Parts, status: StatusCode, detail: String| {
        error::http_exception(status, Some(&detail), &p.method, p.uri.path(), &p.headers)
    };
    if !BRANCHES.contains(&q.branch.as_str()) {
        let detail = format!("Unknown branch '{}' (known: {})", q.branch, BRANCHES.join(", "));
        return http(&parts, StatusCode::NOT_FOUND, detail);
    }
    if let Some(png) = creature(&state, &q).await {
        return png_response(png);
    }
    match blueprint_png(&state, &q.blueprint, q.dim, &q.branch).await {
        Ok(Some(png)) => png_response(png),
        Ok(None) => {
            http(&parts, StatusCode::NOT_FOUND, format!("No blueprint '{}' on branch '{}'", q.blueprint, q.branch))
        }
        Err(RenderError::Blueprint(m)) => {
            tracing::info!("codex render: {:?} is not renderable: {m}", q.blueprint);
            http(&parts, StatusCode::UNPROCESSABLE_ENTITY, format!("Blueprint not renderable: {m}"))
        }
        Err(RenderError::Unhandled) => ApiError::internal().into_response(),
    }
}
