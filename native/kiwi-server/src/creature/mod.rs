//! Creature assembly over the live rig map: a Mods Hub release's parts
//! (`service.assemble_release_model`, `.../assembled`), a game creature from
//! its prefab (`embed.service._assemble_creature`) and the creature thumbnail
//! (`render.service.render_creature_cached`).

use std::{collections::HashMap, future::Future};

use axum::{
    Router,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use base64::Engine;
use serde_json::{Map, Value};

use crate::{
    auth::OptionalSiteUser,
    blueprints::cache::{self, BuildError, Cached, Fmt},
    dressing::service as dress,
    error::{ApiError, method_not_allowed},
    features::{ModsHub, RequireFeature},
    mod_releases::{ModsAsset, SiteMods, access::release_with_project, artifact, model::Doc, read::blob},
    state::AppState,
    validation::{FromParams, Params, Path},
};

pub const LIVE_BRANCH: &str = "live-us";
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

pub fn router() -> Router<AppState> {
    let get_only = || async { method_not_allowed("GET") };
    Router::new()
        .route("/v1/mods/hub/releases/{release_id}/assembled", get(v1_assembled).fallback(get_only))
        .route("/site/mods/releases/{release_id}/assembled", get(site_assembled).fallback(get_only))
}

/// `bp_cache.NoPayload`, carried through `get_or_build` as a marked response.
#[derive(Clone, Copy)]
struct NoPayload;

pub fn no_payload() -> BuildError {
    let mut r = Response::new(axum::body::Body::empty());
    r.extensions_mut().insert(NoPayload);
    BuildError::Other(r)
}

pub fn internal(e: impl std::fmt::Display) -> Response {
    tracing::error!("creature: {e}");
    ApiError::internal().into_response()
}

/// A built payload, `None` for `NoPayload`, or the response to send.
pub async fn cached<F, Fut>(
    state: &AppState,
    sig: Option<&str>,
    ident: &str,
    fmt: Fmt,
    build: F,
) -> Result<Option<Cached>, Response>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Map<String, Value>, BuildError>>,
{
    let out = match sig {
        None => cache::build_uncached(fmt, build()).await,
        Some(sig) => cache::get_or_build(state, &cache::key_for_assembly(sig, ident), fmt, build).await,
    };
    match out {
        Ok(c) => Ok(Some(c)),
        Err(BuildError::Other(r)) if r.extensions().get::<NoPayload>().is_some() => Ok(None),
        Err(e) => Err(e.into_response()),
    }
}

pub async fn signature(state: &AppState) -> Result<Option<String>, Response> {
    crate::rigs::index_signature(state, &state.settings().trove_render_branch).await.map_err(internal)
}

#[cfg(test)]
pub static TEST_ROWS: std::sync::OnceLock<Vec<kiwi_core::rig::Row>> = std::sync::OnceLock::new();

/// `rig_index._rig_map(branch)`.
pub async fn rig_map(state: &AppState, branch: &str) -> anyhow::Result<std::sync::Arc<kiwi_core::rig::RigMap>> {
    #[cfg(test)]
    if let Some(rows) = TEST_ROWS.get() {
        return Ok(std::sync::Arc::new(kiwi_core::rig::RigMap::build(rows)));
    }
    crate::rigs::rig_map(state, branch).await
}

pub fn render_branch(state: &AppState) -> String {
    state.settings().trove_render_branch.clone()
}

/// `assembly.assemble(files, rig_name, ap_overrides, part_scales)` off the runtime.
async fn assemble(
    skeleton: Option<String>,
    files: Vec<(String, Vec<u8>)>,
    overrides: HashMap<String, String>,
    scales: HashMap<String, f64>,
) -> Result<Map<String, Value>, BuildError> {
    let built = tokio::task::spawn_blocking(move || {
        let Some(name) = skeleton else { return Ok(None) };
        let Some(rig) = crate::rigs::rig(&name) else { return Ok(None) };
        kiwi_core::rig::assemble(rig, &name, &files, &overrides, &scales)
    })
    .await
    .map_err(|e| BuildError::Other(internal(e)))?
    .map_err(|e| BuildError::Other(internal(e)))?;
    match built {
        Some(Value::Object(m)) => Ok(m),
        _ => Err(no_payload()),
    }
}

/// The build of a `.tmod`'s parts on the rig `rig_index.resolve` names.
pub async fn build_tmod(state: &AppState, data: Vec<u8>) -> Result<Map<String, Value>, BuildError> {
    let t = tokio::task::spawn_blocking(move || artifact::read(&data, true))
        .await
        .map_err(|e| BuildError::Other(internal(e)))?
        .map_err(|e| BuildError::Other(internal(format!("TmodError: {e}"))))?;
    let files: Vec<(String, Vec<u8>)> = t.files.into_iter().map(|f| (f.path, f.data)).collect();
    let names: Vec<String> = files
        .iter()
        .filter(|(p, _)| p.to_lowercase().ends_with(".blueprint"))
        .map(|(p, _)| {
            let last = p.rsplit('/').next().unwrap_or(p);
            let stem: String = last.chars().take(last.chars().count().saturating_sub(10)).collect();
            stem.to_lowercase()
        })
        .collect();
    let rm = rig_map(state, &render_branch(state)).await.map_err(|e| BuildError::Other(internal(e)))?;
    let (skeleton, attach) = rm.resolve(&names);
    let scales = rm.scales_for(&attach, skeleton.as_deref().and_then(crate::rigs::rig));
    assemble(skeleton, files, attach.into_iter().collect(), scales.parts).await
}

/// `assemble_release_model(release, fmt)`.
pub async fn release_model(state: &AppState, release: &Doc, fmt: Fmt) -> Result<Option<Cached>, Response> {
    if release.str("release_format") != "tmod" {
        return Ok(None);
    }
    let sha = release.str("tmod_sha").to_owned();
    let st = state.clone();
    let s2 = sha.clone();
    let build = || async move {
        let data = blob(&st, &s2).await.map_err(BuildError::Api)?.ok_or_else(no_payload)?;
        build_tmod(&st, data).await
    };
    let sig = signature(state).await?;
    cached(state, sig.as_deref(), &format!("tmod:{sha}"), fmt, build).await
}

/// `embed.service._read_game_file(path)`.
pub async fn read_game_file(state: &AppState, path: &str) -> Result<Option<Vec<u8>>, ApiError> {
    #[cfg(test)]
    if let Some(hit) = dress::test_game_file(path) {
        return Ok(hit);
    }
    let Some(sha) = crate::media::file_sha(state, LIVE_BRANCH, path).await? else { return Ok(None) };
    crate::media::read_blob(crate::media::update_store(state), sha).await
}

/// `_assemble_creature(prefab, skeleton, parts, fmt)`.
pub async fn game_creature(
    state: &AppState,
    prefab: &str,
    skeleton: &str,
    parts: Vec<(String, String)>,
    fmt: Fmt,
) -> Result<Option<Cached>, Response> {
    if !crate::rigs::has_baked_rig(skeleton) {
        return Ok(None);
    }
    let st = state.clone();
    let (p, s) = (prefab.to_owned(), skeleton.to_owned());
    let build = || async move {
        let paths = dress::game_paths(&st, LIVE_BRANCH).await.map_err(BuildError::Api)?;
        let mut files = Vec::new();
        for (basename, _) in &parts {
            let cands = paths.all.get(&format!("{basename}.blueprint")).cloned().unwrap_or_default();
            let Some(gp) = kiwi_core::workshop::nearest_path(&cands, &p) else { continue };
            if let Some(raw) = read_game_file(&st, gp).await.map_err(BuildError::Api)? {
                files.push((format!("{basename}.blueprint"), raw));
            }
        }
        if files.is_empty() {
            return Err(no_payload());
        }
        let rm = rig_map(&st, &render_branch(&st)).await.map_err(|e| BuildError::Other(internal(e)))?;
        let scales = rm.creature_scales(&p, crate::rigs::rig);
        assemble(Some(s), files, parts.into_iter().collect(), scales.parts).await
    };
    let sig = signature(state).await?;
    cached(state, sig.as_deref(), &format!("game:{prefab}"), fmt, build).await
}

// --- the creature thumbnail ---------------------------------------------------------

/// `render_creature_cached(prefab, dim, branch)`.
pub async fn creature_png(state: &AppState, prefab: &str, dim: i64, branch: &str) -> Result<Option<Vec<u8>>, Response> {
    let key = crate::blueprints::render::creature_key(branch, prefab, dim);
    if let Some(hit) = dress::redis_get(state, &key).await.filter(|s| !s.is_empty())
        && let Ok(png) = B64.decode(hit.as_bytes())
    {
        return Ok(Some(png).filter(|p| !p.is_empty()));
    }
    let png = build_creature_png(state, prefab, dim, branch).await?;
    dress::redis_set(state, &key, &B64.encode(png.as_deref().unwrap_or_default())).await;
    Ok(png)
}

async fn build_creature_png(
    state: &AppState,
    prefab: &str,
    dim: i64,
    branch: &str,
) -> Result<Option<Vec<u8>>, Response> {
    let rm = rig_map(state, branch).await.map_err(internal)?;
    let (skeleton, parts) = rm.creature_by_prefab(prefab);
    let Some(skeleton) = skeleton.filter(|s| !s.is_empty() && !parts.is_empty() && crate::rigs::has_baked_rig(s))
    else {
        return Ok(None);
    };
    let scales = rm.creature_scales(prefab, crate::rigs::rig);
    let index = dress::blueprint_by_basename(state, branch).await.map_err(IntoResponse::into_response)?;
    let paths = dress::game_paths(state, branch).await.map_err(IntoResponse::into_response)?;
    let mut wanted = Vec::new();
    for (basename, ap) in &parts {
        let low = basename.to_lowercase();
        let cands = paths.all.get(&format!("{low}.blueprint")).cloned().unwrap_or_default();
        let path = index.get(&low).cloned().or_else(|| kiwi_core::workshop::nearest_path(&cands, prefab).cloned());
        let Some(path) = path else { return Ok(None) };
        let Some(data) = dress::blueprint_bytes(state, &path, branch).await else { return Ok(None) };
        wanted.push((ap.clone(), data, Some(scales.parts.get(basename).copied().unwrap_or(1.0))));
    }
    let dim = dim as usize;
    tokio::task::spawn_blocking(move || {
        let rig = crate::rigs::rig(&skeleton).ok_or("rig vanished")?;
        let cells = kiwi_core::rig::assemble_voxels(rig, &wanted)?;
        if cells.is_empty() {
            return Ok(None);
        }
        Ok(kiwi_core::voxel::render_baked(&cells, dim, kiwi_core::voxel::CATALOG_AZ, kiwi_core::voxel::CATALOG_EL)
            .map(|rgba| kiwi_pil::png::encode_rgba_optimized(&rgba, dim, dim)))
    })
    .await
    .map_err(internal)?
    .map_err(|e: String| internal(e))
}

// --- routes ------------------------------------------------------------------------------

struct Assembled {
    id: String,
    fmt: Fmt,
}

impl FromParams for Assembled {
    fn from_params(p: &mut Params<'_>) -> Self {
        let id = p.path("release_id").str().required();
        let fmt = p.query("fmt").str().one_of_pattern(&["json", "bin"]).default("json".into());
        Assembled { id, fmt: if fmt == "bin" { Fmt::Bin } else { Fmt::Json } }
    }
}

fn reply(headers: &HeaderMap, built: Result<Option<Cached>, Response>) -> Response {
    match built {
        Ok(Some(c)) => cache::respond(headers, &c, 300),
        Ok(None) => ApiError::not_found("No assemblable creature for this mod.").into_response(),
        Err(r) => r,
    }
}

async fn v1_assembled(
    State(s): State<AppState>,
    _: RequireFeature<ModsHub>,
    _: ModsAsset,
    headers: HeaderMap,
    Path(q): Path<Assembled>,
) -> Response {
    let built = async {
        let (release, _) = release_with_project(&s, &q.id, None).await.map_err(IntoResponse::into_response)?;
        release_model(&s, &release, q.fmt).await
    };
    reply(&headers, built.await)
}

async fn site_assembled(
    State(s): State<AppState>,
    _: SiteMods,
    OptionalSiteUser(v): OptionalSiteUser,
    headers: HeaderMap,
    Path(q): Path<Assembled>,
) -> Response {
    let built = async {
        let (release, _) = release_with_project(&s, &q.id, v.as_ref()).await.map_err(IntoResponse::into_response)?;
        release_model(&s, &release, q.fmt).await
    };
    reply(&headers, built.await)
}

/// A PNG with a day of cacheability.
pub fn png_response(png: Vec<u8>) -> Response {
    let mut res = (StatusCode::OK, png).into_response();
    res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=86400"));
    res
}
