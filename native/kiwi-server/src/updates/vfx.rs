//! `app/trove/updates/vfx.py`: the PopcornFX effect browser over the archive.
//! The per-branch index (every `.pkfx` plus the pack's lowercased path map)
//! lives in-process and is rebuilt when the branch's ordinal moves.

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};

use kiwi_core::updates::pkfx;
use mongodb::bson::{Bson, doc};
use serde_json::{Value, json};

use super::read::{self, BRANCHES, STATE};
use crate::routes::store::service::{Fetch, Find, projection};

const PACK_ROOT: &str = "particles/vfx/";
const THUMB_DIR: &str = "particles/vfx/editor/thumbnails/particles/";
const BRANCH_FIELDS: &[&str] =
    &["branch", "content_path", "current_version", "current_ordinal", "last_probe_at", "status", "last_error"];

pub struct Effect {
    pub path: String,
    pub size: Value,
    pub thumb: Option<String>,
}

pub struct Index {
    pub ordinal: i64,
    pub effects: Vec<Effect>,
    pub paths: HashMap<String, String>,
}

/// `_CACHE` + `_BUILD_LOCK`.
#[derive(Default)]
pub struct Cache {
    built: Mutex<HashMap<String, Arc<Index>>>,
    lock: tokio::sync::Mutex<()>,
}

pub static CACHE: LazyLock<Cache> = LazyLock::new(Cache::default);

impl Cache {
    fn hit(&self, branch: &str, ordinal: i64) -> Option<Arc<Index>> {
        self.built.lock().ok()?.get(branch).filter(|i| i.ordinal == ordinal).cloned()
    }

    /// `_index(branch)`.
    pub async fn index<F: Fetch>(&self, f: &F, branch: &str) -> Arc<Index> {
        let ub = f.find_one(BRANCHES, doc! {"branch": branch}, projection(BRANCH_FIELDS)).await;
        let ordinal = match ub.as_ref().and_then(|d| d.get("current_ordinal")) {
            None | Some(Bson::Null) => 0,
            Some(Bson::Int32(i)) => i64::from(*i),
            Some(Bson::Int64(i)) => *i,
            Some(v) => read::unhandled(format!("current_ordinal: unexpected {v:?}")),
        };
        if let Some(hit) = self.hit(branch, ordinal) {
            return hit;
        }
        let _guard = self.lock.lock().await;
        if let Some(hit) = self.hit(branch, ordinal) {
            return hit;
        }
        let built = Arc::new(build(f, branch, ordinal).await);
        if let Ok(mut m) = self.built.lock() {
            m.insert(branch.to_owned(), built.clone());
        }
        built
    }
}

async fn build<F: Fetch>(f: &F, branch: &str, ordinal: i64) -> Index {
    let find = Find {
        filter: doc! {"branch": branch},
        projection: doc! {"path": 1, "size": 1, "_id": 0},
        sort: Vec::new(),
        skip: 0,
        limit: 0,
    };
    let mut effects = Vec::new();
    let mut paths: HashMap<String, String> = HashMap::new();
    for d in f.find(STATE, find).await {
        let path = match d.get("path") {
            None | Some(Bson::Null) => continue,
            Some(Bson::String(s)) if s.is_empty() => continue,
            Some(Bson::String(s)) => s.clone(),
            Some(v) => read::unhandled(format!("path: unexpected {v:?}")),
        };
        let low = path.to_lowercase();
        if low.starts_with(PACK_ROOT) {
            paths.insert(low.clone(), path.clone());
        }
        if low.ends_with(".pkfx") {
            let size = d.get("size").map_or(Value::from(0), read::raw_json);
            effects.push(Effect { path: path.clone(), size, thumb: None });
            paths.entry(low).or_insert(path);
        }
    }
    for e in &mut effects {
        let key = format!("{THUMB_DIR}{}.png", pkfx::basename(&e.path).to_lowercase());
        e.thumb = paths.get(&key).cloned();
    }
    effects.sort_by_cached_key(|e| (pkfx::basename(&e.path).to_lowercase(), e.path.to_lowercase()));
    Index { ordinal, effects, paths }
}

/// `_resolve(idx, ref)`.
pub fn resolve<'a>(idx: &'a Index, r: &str) -> Option<&'a String> {
    let r = r.replace('\\', "/");
    let r = r.trim_start_matches('/').to_lowercase();
    if r.is_empty() {
        return None;
    }
    idx.paths.get(&r).or_else(|| idx.paths.get(&format!("{PACK_ROOT}{r}")))
}

/// `list_effects(branch, q, limit, offset)`.
pub async fn list_effects<F: Fetch>(cache: &Cache, f: &F, branch: &str, q: &str, limit: i64, offset: i64) -> Value {
    let idx = cache.index(f, branch).await;
    let needle = kiwi_core::pystr::strip(q).to_lowercase();
    let items: Vec<&Effect> =
        idx.effects.iter().filter(|e| needle.is_empty() || e.path.to_lowercase().contains(&needle)).collect();
    let start = (offset.max(0) as usize).min(items.len());
    let end = start.saturating_add(limit.max(0) as usize).min(items.len());
    let page: Vec<Value> =
        items[start..end].iter().map(|e| json!({"path": e.path, "size": e.size, "thumb": e.thumb})).collect();
    json!({"branch": branch, "items": page, "count": page.len(), "total": items.len()})
}

async fn read_blob<F: Fetch>(f: &F, branch: &str, path: &str) -> Option<(Vec<u8>, String)> {
    let meta = read::get_file_meta(f, branch, path).await?;
    let raw = f.blob(&meta.content_sha256).await?;
    Some((raw, meta.content_sha256))
}

/// `manifest(branch, path)`.
pub async fn manifest<F: Fetch>(cache: &Cache, f: &F, branch: &str, path: &str) -> Option<Value> {
    let idx = cache.index(f, branch).await;
    let mut real = resolve(&idx, path).cloned();
    if real.is_none() && path.to_lowercase().ends_with(".pkfx") {
        let cand = path.replace('\\', "/").trim_start_matches('/').to_owned();
        if read::get_file_meta(f, branch, &cand).await.is_some() {
            real = Some(cand);
        }
    }
    let real = real.filter(|r| r.to_lowercase().ends_with(".pkfx"))?;
    let (raw, _) = read_blob(f, branch, &real).await?;
    let text = String::from_utf8_lossy(&raw).into_owned();
    let deps: Vec<(String, String, bool)> = pkfx::extract_refs(&text)
        .into_iter()
        .map(|r| {
            let base = pkfx::basename(&r);
            let game = resolve(&idx, &r).is_some();
            (r, base, game)
        })
        .collect();
    let missing: Vec<&str> = deps.iter().filter(|d| !d.2).map(|d| d.1.as_str()).collect();
    let deps_json: Vec<Value> = deps
        .iter()
        .map(|(r, b, game)| json!({"ref": r, "basename": b, "source": if *game { "game" } else { "missing" }}))
        .collect();
    Some(json!({
        "branch": branch, "path": real, "pkfx": text, "deps": deps_json,
        "missing": missing, "game_available": true,
    }))
}

/// `asset(branch, ref)`: `(bytes, media type, content sha)`.
pub async fn asset<F: Fetch>(cache: &Cache, f: &F, branch: &str, r: &str) -> Option<(Vec<u8>, &'static str, String)> {
    if !pkfx::is_asset(r) {
        return None;
    }
    let idx = cache.index(f, branch).await;
    let real = resolve(&idx, r)?.clone();
    let (raw, sha) = read_blob(f, branch, &real).await?;
    Some((raw, pkfx::media_type_for(&real), sha))
}
