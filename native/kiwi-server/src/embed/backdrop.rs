//! `embed/backdrop.py`: what an aura is worn on, placed in effect space.

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
};

use axum::response::{IntoResponse, Response};
use kiwi_core::{dressing as kd, rig::Placement};
use serde_json::{Value, json};

use crate::{
    dressing::{EYE, catalogue, service as dress},
    error::ApiError,
    state::AppState,
};

pub const KINDS: [&str; 6] = ["melee", "pistol", "staff", "spear", "fist", "hat"];
const HAT_CLASS: &str = "knight";

type Key = (String, String, i64, Option<i64>);
static CACHE: LazyLock<Mutex<HashMap<Key, Option<Value>>>> = LazyLock::new(Default::default);

fn family(kind: &str) -> &'static str {
    match kind {
        "melee" => "Melee",
        "pistol" => "Gun",
        "staff" => "Staff",
        "spear" => "Spear",
        _ => "Fist",
    }
}

fn internal(e: impl std::fmt::Display) -> Response {
    tracing::error!("embed backdrop: {e}");
    ApiError::internal().into_response()
}

fn round6(v: f64) -> f64 {
    crate::pyfloat::round(v, 6)
}

fn coords(p: &kiwi_core::rig::Part, v: &[f64]) -> Value {
    if p.shifted { json!(v) } else { Value::Array(v.iter().map(|&f| json!(f as i64)).collect()) }
}

fn part_value(m: Vec<f64>, p: &kiwi_core::rig::Part) -> Value {
    json!({"m": m, "x": coords(p, &p.x), "y": coords(p, &p.y), "z": coords(p, &p.z), "rgb": p.rgb})
}

async fn read(
    state: &AppState,
    branch: &str,
    basename: &str,
    hint: &str,
    reference: &str,
) -> Result<Option<Vec<u8>>, Response> {
    match dress::blueprint_path(state, basename, hint, branch, reference).await.map_err(IntoResponse::into_response)? {
        Some(path) => Ok(dress::blueprint_bytes(state, &path, branch).await),
        None => Ok(None),
    }
}

async fn weapon(state: &AppState, fam: &str, cat: &kd::Catalogue, branch: &str) -> Result<Option<Value>, Response> {
    let mut starters: Vec<_> =
        cat.styles_of("weapon").iter().filter(|o| o.key.starts_with("starterweapon_") && o.family == fam).collect();
    starters.sort_by(|a, b| a.key.cmp(&b.key));
    for opt in starters {
        let Some(raw) = read(state, branch, &opt.blueprint, &opt.prefab, &opt.reference).await? else { continue };
        if !raw.starts_with(b"kiwib") {
            continue;
        }
        match raw.get(5) {
            Some(5) => {}
            Some(_) => continue,
            None => return Err(internal("IndexError: index out of range")),
        }
        let Some(part) = kiwi_core::rig::part_at("model", &raw, 1.0).map_err(internal)? else { continue };
        let m = kd::weapon_matrix(fam == "Gun", round6);
        return Ok(Some(json!({"label": opt.name, "parts": [part_value(m, &part)]})));
    }
    Ok(None)
}

async fn head(state: &AppState, branch: &str) -> Result<Option<Value>, Response> {
    let cat = catalogue::get(state).await.map_err(internal)?;
    let sel = dress::Selection {
        class_key: HAT_CLASS.into(),
        colors: vec![("eye_color".into(), Some(EYE[0].into()))],
        ..Default::default()
    };
    let Some(outfit) = dress::resolve(state, &cat, &sel).await.map_err(IntoResponse::into_response)? else {
        return Ok(None);
    };
    let skeleton = outfit.cls.skeleton.clone();
    let mut placements = Vec::new();
    for slot in ["head", "eyes"] {
        let Some(r) = outfit.blueprints.iter().find(|(s, _)| s == slot).map(|(_, r)| r.clone()) else { continue };
        let ap = dress::attach_point(slot, &skeleton);
        let Some(ap) = ap.filter(|_| !r.is_empty() && r != dress::NONE) else { continue };
        let base = r.rsplit('/').next().unwrap_or(&r).to_owned();
        let exact = if r.contains('/') { r.clone() } else { String::new() };
        if let Some(raw) = read(state, branch, &base, &r, &exact).await? {
            placements.push(Placement {
                ap: ap.to_owned(),
                raw,
                scale: outfit.piece_scale(slot, ap),
                tint: outfit.tint_for(slot),
            });
        }
    }
    let Some(rig) = crate::rigs::rig(&skeleton) else { return Ok(None) };
    if placements.is_empty() {
        return Ok(None);
    }
    let parts = tokio::task::spawn_blocking(move || kiwi_core::rig::placement_parts(rig, &placements))
        .await
        .map_err(internal)?
        .map_err(internal)?;
    if parts.is_empty() {
        return Ok(None);
    }
    let Some(hat) = rig.rest.get("hat") else { return Ok(None) };
    let mut out = Vec::new();
    for p in &parts {
        let size = rig.voxel_scale * p.scale.unwrap_or(1.0);
        let rest = rig.rest.get(&p.name).ok_or_else(|| internal(format!("KeyError: {}", p.name)))?;
        let m = kd::head_matrix(hat, rest, size, round6).ok_or_else(|| internal("LinAlgError: Singular matrix"))?;
        out.push(part_value(m, p));
    }
    Ok(Some(json!({"label": "Head", "parts": out})))
}

/// `backdrop(kind)`: `None` when there is nothing authoritative to draw.
pub async fn backdrop(state: &AppState, kind: &str) -> Result<Option<Value>, Response> {
    let branch = state.settings().trove_render_branch.clone();
    #[cfg(test)]
    let test = catalogue::TEST_CATALOGUE.get().is_some();
    #[cfg(not(test))]
    let test = false;
    if !KINDS.contains(&kind) || !(test || state.settings().postgres_enabled()) {
        return Ok(None);
    }
    let sig = if test {
        (0, None)
    } else {
        crate::codexes::store::meta_signature(state.pg(), &branch).await.map_err(internal)?
    };
    let key: Key = (kind.to_owned(), branch.clone(), sig.0, sig.1.map(|d| d.timestamp_micros()));
    if let Some(hit) = CACHE.lock().ok().and_then(|c| c.get(&key).cloned()) {
        return Ok(hit);
    }
    let cat = catalogue::get(state).await.map_err(internal)?;
    let built =
        if kind == "hat" { head(state, &branch).await? } else { weapon(state, family(kind), &cat, &branch).await? };
    if let Ok(mut c) = CACHE.lock() {
        c.insert(key, built.clone());
    }
    Ok(built)
}
