//! `service._resolve_rig`: which baked creature rig a release's blueprint
//! basenames assemble onto (`rig_index.resolve` over the live `rig_binding`
//! map, cached per codex signature) and that rig's animation names
//! (`assembly.has_baked_rig` / `animations_for`, from `rigs/<name>.rig.json`).

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, LazyLock, Mutex},
};

use regex::Regex;
use serde_json::{Value, json};

use super::access::internal;
use crate::{codexes::store, error::ApiError, state::AppState};

pub type ByBlueprint = HashMap<String, (String, String)>;
type Sig = (i64, Option<chrono::DateTime<chrono::Utc>>);

type Maps = HashMap<String, (Sig, Arc<ByBlueprint>)>;
static MAPS: LazyLock<Mutex<Maps>> = LazyLock::new(Default::default);
static ANIMS: LazyLock<Mutex<HashMap<String, Option<Vec<String>>>>> = LazyLock::new(Default::default);
static STYLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[[^\]]*\]").expect("style regex"));
static TIER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"_lvl\d+").expect("tier regex"));

/// `_rig_map(branch).by_blueprint`.
async fn rig_map(state: &AppState, branch: &str) -> Result<Arc<ByBlueprint>, ApiError> {
    if !state.settings().postgres_enabled() {
        return Ok(Arc::default());
    }
    let pool = state.inner.db.pg.as_ref().ok_or_else(|| internal("no postgres"))?;
    let sig = store::meta_signature(pool, branch).await.map_err(internal)?;
    if let Some((s, m)) = MAPS.lock().ok().and_then(|g| g.get(branch).cloned())
        && s == sig
    {
        return Ok(m);
    }
    let rows = store::load_rig_bindings(pool, branch).await.map_err(internal)?;
    let mut map = ByBlueprint::new();
    for (_prefab, blueprint, skeleton, ap, _, _) in rows {
        map.entry(blueprint).or_insert((skeleton, ap));
    }
    let map = Arc::new(map);
    if let Ok(mut g) = MAPS.lock() {
        g.insert(branch.to_owned(), (sig, map.clone()));
    }
    Ok(map)
}

/// `_known(basename, rig_map)`.
fn known(b: &str, map: &ByBlueprint) -> Option<String> {
    if map.contains_key(b) {
        return Some(b.to_owned());
    }
    let mut seen: Vec<String> = vec![b.to_owned()];
    let styled = STYLE.replace_all(b, "").into_owned();
    let tiered = TIER.replace_all(b, "").into_owned();
    let both = TIER.replace_all(&styled, "").into_owned();
    for cand in [styled, tiered, both] {
        let cand = kiwi_core::pystr::strip(&cand).trim_matches('_').to_owned();
        if cand.is_empty() || seen.contains(&cand) {
            continue;
        }
        seen.push(cand.clone());
        if map.contains_key(&cand) {
            return Some(cand);
        }
    }
    None
}

/// `rig_index.resolve(basenames)`: `(skeleton, {basename: ap})`.
pub fn resolve(fns: &[String], map: &ByBlueprint) -> Option<(String, Vec<(String, String)>)> {
    let mut hits: Vec<(String, (String, String))> = Vec::new();
    for b in fns {
        if let Some(k) = known(b, map) {
            let v = map[&k].clone();
            match hits.iter_mut().find(|(n, _)| n == b) {
                Some(slot) => slot.1 = v,
                None => hits.push((b.clone(), v)),
            }
        }
    }
    if hits.is_empty() {
        return None;
    }
    let mut counts: Vec<(String, usize)> = Vec::new();
    for (_, (skel, _)) in &hits {
        match counts.iter_mut().find(|(s, _)| s == skel) {
            Some(c) => c.1 += 1,
            None => counts.push((skel.clone(), 1)),
        }
    }
    let mut best = &counts[0];
    for c in &counts[1..] {
        if c.1 > best.1 {
            best = c;
        }
    }
    let skeleton = best.0.clone();
    let attach = hits.into_iter().filter(|(_, (s, _))| *s == skeleton).map(|(b, (_, ap))| (b, ap)).collect();
    Some((skeleton, attach))
}

fn rig_dir() -> PathBuf {
    let root = if cfg!(test) {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
    } else {
        std::env::current_dir().unwrap_or_default()
    };
    root.join("app/trove/mods_hub/rigs")
}

/// `animations_for(name)` when `has_baked_rig(name)`, else `None`.
fn baked_animations(name: &str) -> Option<Vec<String>> {
    if let Some(hit) = ANIMS.lock().ok().and_then(|g| g.get(name).cloned()) {
        return hit;
    }
    let path = rig_dir().join(format!("{name}.rig.json"));
    let found = if name.contains(['/', '\\']) || !path.is_file() {
        None
    } else {
        let text = std::fs::read(&path).ok()?;
        let v: Value = serde_json::from_slice(&text).ok()?;
        Some(v.get("animations").and_then(Value::as_object).map(|m| m.keys().cloned().collect()).unwrap_or_default())
    };
    if let Ok(mut g) = ANIMS.lock() {
        g.insert(name.to_owned(), found.clone());
    }
    found
}

/// `_resolve_rig(fns)`: `(rig, animations, component basenames)`.
pub async fn resolve_release(state: &AppState, fns: &[String]) -> Result<(Value, Value, HashSet<String>), ApiError> {
    let none = || (Value::Null, json!([]), HashSet::new());
    if fns.is_empty() {
        return Ok(none());
    }
    let map = rig_map(state, &state.settings().trove_render_branch).await?;
    let Some((skeleton, attach)) = resolve(fns, &map) else { return Ok(none()) };
    let name = skeleton.clone();
    let anims = tokio::task::spawn_blocking(move || baked_animations(&name)).await.map_err(internal)?;
    match anims {
        Some(a) => Ok((json!(skeleton), json!(a), attach.into_iter().map(|(b, _)| b).collect())),
        None => Ok(none()),
    }
}
