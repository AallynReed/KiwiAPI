//! Creature rigs: the baked rig data shipped in `assets/rigs`
//! (`assembly.py`), the live prefab binding map (`rig_index.py`), the clip
//! and graph routes, and the Blueprint Editor's model projects.

pub mod model;

#[cfg(test)]
mod tests;

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, LazyLock, Mutex, OnceLock},
};

use axum::{
    Router,
    extract::{RawPathParams, Request, State},
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::{DateTime, Utc};
use kiwi_core::rig::{Rig, RigMap, valid_name};

use crate::{
    auth::{extract::public_scope, scopes::MODS_READ},
    error::ApiError,
    features::{self, ModsHub},
    state::AppState,
};

/// The baked rigs, relative to the working directory (`/app` in the image).
pub const RIG_DIR: &str = "assets/rigs";

pub fn rig_dir() -> PathBuf {
    let p = PathBuf::from(RIG_DIR);
    if cfg!(test) && !p.is_dir() {
        return PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").join(RIG_DIR);
    }
    p
}

/// `_rigs()`: every `*.rig.json`, loaded once.
pub fn rigs() -> &'static HashMap<String, Rig> {
    static RIGS: OnceLock<HashMap<String, Rig>> = OnceLock::new();
    RIGS.get_or_init(|| {
        let mut out = HashMap::new();
        let Ok(dir) = std::fs::read_dir(rig_dir()) else { return out };
        for entry in dir.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(stem) = name.strip_suffix(".rig.json") else { continue };
            let parsed = std::fs::read(entry.path()).ok().and_then(|b| crate::pyjson::loads(&b).ok());
            match parsed {
                Some(serde_json::Value::Object(m)) => {
                    out.insert(stem.to_owned(), Rig::parse(m));
                }
                _ => tracing::error!("rig {name}: unreadable"),
            }
        }
        out
    })
}

pub fn rig(name: &str) -> Option<&'static Rig> {
    rigs().get(name)
}

/// `has_baked_rig(name)`.
pub fn has_baked_rig(name: &str) -> bool {
    rigs().contains_key(name)
}

fn read_file(p: PathBuf) -> Option<Vec<u8>> {
    if p.is_file() { std::fs::read(p).ok() } else { None }
}

/// `load_animation(skeleton, name)`: the raw `TANIM1`/`TANIM2` clip.
pub fn load_animation(skeleton: &str, name: &str) -> Option<Vec<u8>> {
    if !(valid_name(skeleton) && valid_name(name)) {
        return None;
    }
    read_file(rig_dir().join("anim").join(skeleton).join(format!("{name}.anim")))
}

/// `load_animation_graph(skeleton)`.
pub fn load_animation_graph(skeleton: &str) -> Option<Vec<u8>> {
    if !valid_name(skeleton) {
        return None;
    }
    read_file(rig_dir().join("graph").join(format!("{skeleton}.graph.json")))
}

// --- rig_index -----------------------------------------------------------------

type Sig = (i64, Option<DateTime<Utc>>);
type Entry = (Sig, Arc<RigMap>);
static MAPS: LazyLock<Mutex<HashMap<String, Entry>>> = LazyLock::new(Default::default);
static BUILD: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(Default::default);

#[cfg(test)]
pub static TEST_ROWS: OnceLock<Vec<kiwi_core::rig::Row>> = OnceLock::new();

fn cached(branch: &str, sig: &Sig) -> Option<Arc<RigMap>> {
    MAPS.lock().ok()?.get(branch).filter(|(s, _)| s == sig).map(|(_, m)| m.clone())
}

/// `_rig_map(branch)`: rebuilt when the codex signature changes.
pub async fn rig_map(state: &AppState, branch: &str) -> anyhow::Result<Arc<RigMap>> {
    #[cfg(test)]
    if let Some(rows) = TEST_ROWS.get() {
        return Ok(Arc::new(RigMap::build(rows)));
    }
    if !state.settings().postgres_enabled() {
        return Ok(Arc::default());
    }
    let pool = state.pg();
    let sig = crate::codexes::store::meta_signature(pool, branch).await?;
    if let Some(m) = cached(branch, &sig) {
        return Ok(m);
    }
    let _guard = BUILD.lock().await;
    if let Some(m) = cached(branch, &sig) {
        return Ok(m);
    }
    let map = Arc::new(RigMap::build(&crate::codexes::store::load_rig_bindings(pool, branch).await?));
    if let Ok(mut m) = MAPS.lock() {
        m.insert(branch.to_owned(), (sig, map.clone()));
    }
    Ok(map)
}

/// `index_signature(branch)`: what anything derived from the map keys its cache on.
pub async fn index_signature(state: &AppState, branch: &str) -> anyhow::Result<Option<String>> {
    if !state.settings().postgres_enabled() {
        return Ok(None);
    }
    let (version, updated) = crate::codexes::store::meta_signature(state.pg(), branch).await?;
    Ok(updated.map(|u| format!("{branch}:{version}:{}", u.timestamp())))
}

// --- routes ----------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/mods/hub/rigs/{skeleton}/anim/{name}", get(v1_anim))
        .route("/site/rigs/{skeleton}/anim/{name}", get(site_anim))
        .route("/site/rigs/{skeleton}/graph", get(site_graph))
        .merge(model::router())
}

fn clip(data: Vec<u8>, media_type: &'static str) -> Response {
    let mut r = Response::new(axum::body::Body::from(data));
    let h = r.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(media_type));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=3600"));
    r
}

pub async fn anim_reply(skeleton: String, name: String) -> Response {
    match tokio::task::spawn_blocking(move || load_animation(&skeleton, &name)).await {
        Ok(Some(d)) => clip(d, "application/octet-stream"),
        Ok(None) => ApiError::not_found("No such rig animation").into_response(),
        Err(_) => ApiError::internal().into_response(),
    }
}

pub async fn v1_anim(State(state): State<AppState>, raw: RawPathParams, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    if let Err(e) = features::check::<ModsHub>(&state).await {
        return e.into_response();
    }
    let mult = state.settings().mods_asset_rate_limit_multiplier;
    if let Err(r) = public_scope(&state, &parts, MODS_READ, mult, Some("mods:asset")).await {
        return r.into_response();
    }
    let [skeleton, name] = params(&raw);
    anim_reply(skeleton, name).await
}

/// Starlette's path params: percent-decoded, invalid UTF-8 replaced.
fn params<const N: usize>(raw: &RawPathParams) -> [String; N] {
    let v: Vec<String> =
        raw.iter().map(|(_, v)| percent_encoding::percent_decode_str(v).decode_utf8_lossy().into_owned()).collect();
    std::array::from_fn(|i| v.get(i).cloned().unwrap_or_default())
}

pub async fn site_anim(State(state): State<AppState>, raw: RawPathParams, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    if let Err(r) = crate::workshop::site_gate(&state, &parts, None).await {
        return r;
    }
    let [skeleton, name] = params(&raw);
    anim_reply(skeleton, name).await
}

pub async fn site_graph(State(state): State<AppState>, raw: RawPathParams, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    if let Err(r) = crate::workshop::site_gate(&state, &parts, None).await {
        return r;
    }
    let [skeleton] = params(&raw);
    graph_reply(skeleton).await
}

pub async fn graph_reply(skeleton: String) -> Response {
    match tokio::task::spawn_blocking(move || load_animation_graph(&skeleton)).await {
        Ok(Some(d)) => clip(d, "application/json"),
        Ok(None) => ApiError::not_found("No such rig animation graph").into_response(),
        Err(_) => ApiError::internal().into_response(),
    }
}
