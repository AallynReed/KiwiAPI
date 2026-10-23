//! A release's PopcornFX effects (`list_release_vfx`,
//! `get_release_vfx_manifest`, `get_release_vfx_asset`): references resolve
//! to a file bundled in the mod first, else the live game tree
//! (`trove_layout.game_file_map` + the updates CAS), else the dev folder.

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{Arc, LazyLock, Mutex},
};

use kiwi_core::updates::pkfx;
use mongodb::bson::{Bson, Document, doc};
use serde_json::{Map, Value, json};

use super::{
    access::{self, internal},
    artifact::{self, lower},
    model::Doc,
    read::{blob, is_tmod, not_found},
};
use crate::{error::ApiError, media, state::AppState};

pub const LIVE_BRANCH: &str = "live-us";
const DEPSET_MAX: usize = 256;

pub type Index = Vec<(String, Vec<u8>)>;

fn index_get<'a>(index: &'a Index, bn: &str) -> Option<&'a Vec<u8>> {
    index.iter().find(|(k, _)| k == bn).map(|(_, v)| v)
}

/// `_tmod_pkfx_and_index(tmod_bytes)`.
pub fn pkfx_and_index(data: &[u8]) -> Result<(Vec<(String, u32)>, Index), String> {
    let t = artifact::read(data, true)?;
    let mut items = Vec::new();
    let mut index: Index = Vec::new();
    for f in t.files {
        let bn = lower(&artifact::basename(&f.path));
        if lower(&f.path).ends_with(".pkfx") {
            items.push((f.path.clone(), f.size));
        }
        if index_get(&index, &bn).is_none() {
            index.push((bn, f.data));
        }
    }
    Ok((items, index))
}

async fn release_index(state: &AppState, r: &Doc) -> Result<(Vec<(String, u32)>, Index), ApiError> {
    if !is_tmod(r) {
        return Err(not_found("This release has no VFX."));
    }
    let data = blob(state, r.str("tmod_sha")).await?.ok_or_else(|| not_found("Release artifact not found"))?;
    tokio::task::spawn_blocking(move || pkfx_and_index(&data))
        .await
        .map_err(internal)?
        .map_err(|e| internal(format!("TmodError: {e}")))
}

/// `list_release_vfx`' items.
pub fn list_value(mut items: Vec<(String, u32)>) -> Value {
    items.sort_by_cached_key(|(p, _)| lower(p));
    let items: Vec<Value> = items.into_iter().map(|(p, s)| json!({"path": p, "size": s})).collect();
    json!({"items": items})
}

/// `list_release_vfx(release)`.
pub async fn list(state: &AppState, r: &Doc) -> Result<Value, ApiError> {
    if !is_tmod(r) {
        return Ok(json!({"items": []}));
    }
    let Some(data) = blob(state, r.str("tmod_sha")).await? else { return Ok(json!({"items": []})) };
    match tokio::task::spawn_blocking(move || pkfx_and_index(&data)).await.map_err(internal)? {
        Ok((items, _)) => Ok(list_value(items)),
        Err(_) => Ok(json!({"items": []})),
    }
}

// --- the game tree ---------------------------------------------------------

type FileMap = Arc<HashMap<String, String>>;
static GAME_MAP: LazyLock<Mutex<Option<(i64, FileMap)>>> = LazyLock::new(Default::default);

/// `trove_layout.game_file_map(branch)`: `{basename.lower(): path}`, first
/// sighting wins, rebuilt when the branch's ordinal moves.
pub async fn game_file_map(state: &AppState, branch: &str) -> Result<FileMap, ApiError> {
    let ub = access::find_one(state, "update_branches", doc! {"branch": branch}).await?;
    let ordinal = match ub.as_ref().and_then(|d| d.get("current_ordinal")) {
        Some(Bson::Int32(n)) => i64::from(*n),
        Some(Bson::Int64(n)) => *n,
        _ => 0,
    };
    if ordinal == 0 {
        return Ok(Arc::default());
    }
    if let Some((o, m)) = GAME_MAP.lock().ok().and_then(|g| g.clone())
        && o == ordinal
        && !m.is_empty()
    {
        return Ok(m);
    }
    let db = state.inner.db.mongo.as_ref().ok_or_else(|| internal("no mongo"))?;
    let mut cursor = db
        .collection::<Document>("update_state")
        .find(doc! {"branch": branch})
        .projection(doc! {"path": 1, "_id": 0})
        .await
        .map_err(internal)?;
    let mut map = HashMap::new();
    while cursor.advance().await.map_err(internal)? {
        let d = cursor.deserialize_current().map_err(internal)?;
        let Some(Bson::String(path)) = d.get("path") else { continue };
        if path.is_empty() {
            continue;
        }
        let name = lower(path.rsplit('/').next().unwrap_or(path));
        map.entry(name).or_insert_with(|| path.clone());
    }
    let map = Arc::new(map);
    if let Ok(mut g) = GAME_MAP.lock() {
        *g = Some((ordinal, map.clone()));
    }
    Ok(map)
}

/// `os.walk(pkfx_dev_vfx_dir)`: `{name.lower(): path}`, first wins.
fn dev_index(root: &str) -> HashMap<String, PathBuf> {
    let mut out = HashMap::new();
    if root.is_empty() {
        return out;
    }
    let mut dirs = VecDeque::from([PathBuf::from(root)]);
    while let Some(dir) = dirs.pop_front() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let mut subdirs = Vec::new();
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                subdirs.push(p);
            } else {
                out.entry(lower(&e.file_name().to_string_lossy())).or_insert(p);
            }
        }
        for s in subdirs.into_iter().rev() {
            dirs.push_front(s);
        }
    }
    out
}

/// `_game_vfx_resolver()`.
struct Resolver {
    fmap: FileMap,
    dev: HashMap<String, PathBuf>,
}

pub enum Where {
    Game(String),
    Dev(PathBuf),
}

impl Resolver {
    async fn new(state: &AppState) -> Result<Resolver, ApiError> {
        let fmap = game_file_map(state, LIVE_BRANCH).await?;
        let root = state.settings().pkfx_dev_vfx_dir.clone();
        let dev = tokio::task::spawn_blocking(move || dev_index(&root)).await.map_err(internal)?;
        Ok(Resolver { fmap, dev })
    }

    fn available(&self) -> bool {
        !self.fmap.is_empty() || !self.dev.is_empty()
    }

    fn lookup(&self, bn: &str) -> Option<Where> {
        if let Some(p) = self.fmap.get(bn) {
            return Some(Where::Game(p.clone()));
        }
        self.dev.get(bn).map(|p| Where::Dev(p.clone()))
    }

    async fn read(&self, state: &AppState, w: Where) -> Result<Option<Vec<u8>>, ApiError> {
        match w {
            Where::Dev(p) => tokio::task::spawn_blocking(move || std::fs::read(p))
                .await
                .map_err(internal)?
                .map(Some)
                .map_err(internal),
            Where::Game(path) => match media::file_sha(state, LIVE_BRANCH, &path).await? {
                Some(sha) => media::read_blob(media::update_store(state), sha).await,
                None => Ok(None),
            },
        }
    }
}

/// `_build_vfx_depset(index, lookup, read)`.
pub async fn depset<L, R, Fut>(index: &Index, lookup: L, read: R) -> Result<HashSet<String>, ApiError>
where
    L: Fn(&str) -> Option<Where>,
    R: Fn(Where) -> Fut,
    Fut: std::future::Future<Output = Result<Option<Vec<u8>>, ApiError>>,
{
    let mut deps = HashSet::new();
    let mut seen = HashSet::new();
    let mut queue: Vec<String> = index.iter().filter(|(k, _)| k.ends_with(".pkfx")).map(|(k, _)| k.clone()).collect();
    while let Some(bn) = queue.pop() {
        if !seen.insert(bn.clone()) {
            continue;
        }
        let raw = match index_get(index, &bn) {
            Some(r) => Some(r.clone()),
            None => match lookup(&bn) {
                Some(w) => read(w).await?,
                None => None,
            },
        };
        let Some(raw) = raw else { continue };
        for r in pkfx::extract_refs(&String::from_utf8_lossy(&raw)) {
            let rbn = lower(&pkfx::basename(&r));
            deps.insert(rbn.clone());
            if rbn.ends_with(".pkfx") && !seen.contains(&rbn) {
                queue.push(rbn);
            }
        }
    }
    Ok(deps)
}

async fn release_depset(state: &AppState, index: &Index, res: &Resolver) -> Result<HashSet<String>, ApiError> {
    depset(index, |bn| res.lookup(bn), |w| res.read(state, w)).await
}

type DepCache = (HashMap<String, Arc<HashSet<String>>>, VecDeque<String>);
static DEPSETS: LazyLock<Mutex<DepCache>> = LazyLock::new(Default::default);

fn remember(sha: &str, deps: Arc<HashSet<String>>) {
    let Ok(mut c) = DEPSETS.lock() else { return };
    if c.0.contains_key(sha) {
        return;
    }
    c.0.insert(sha.to_owned(), deps);
    c.1.push_back(sha.to_owned());
    while c.1.len() > DEPSET_MAX {
        if let Some(old) = c.1.pop_front() {
            c.0.remove(&old);
        }
    }
}

fn cached(sha: &str) -> Option<Arc<HashSet<String>>> {
    DEPSETS.lock().ok()?.0.get(sha).cloned()
}

/// The deps of a manifest: each ref as `mod` / `game` / `missing`.
pub fn manifest_deps(index: &Index, text: &str, in_game: impl Fn(&str) -> bool) -> (Vec<Value>, Vec<Value>) {
    let mut deps = Vec::new();
    let mut missing = Vec::new();
    for rf in pkfx::extract_refs(text) {
        let base = pkfx::basename(&rf);
        let bn = lower(&base);
        let source = if index_get(index, &bn).is_some() {
            "mod"
        } else if in_game(&bn) {
            "game"
        } else {
            "missing"
        };
        if source == "missing" {
            missing.push(Value::String(base.clone()));
        }
        let mut d = Map::new();
        d.insert("ref".into(), json!(rf));
        d.insert("basename".into(), json!(base));
        d.insert("source".into(), json!(source));
        deps.push(Value::Object(d));
    }
    (deps, missing)
}

/// `get_release_vfx_manifest(release, path)`.
pub async fn manifest(state: &AppState, r: &Doc, path: &str) -> Result<Value, ApiError> {
    let (_, index) = release_index(state, r).await?;
    let raw = index_get(&index, &lower(&pkfx::basename(path)))
        .ok_or_else(|| not_found("VFX file not found in this release."))?;
    let text = String::from_utf8_lossy(raw).into_owned();
    let res = Resolver::new(state).await?;
    let (deps, missing) = manifest_deps(&index, &text, |bn| res.lookup(bn).is_some());
    let set = release_depset(state, &index, &res).await?;
    remember(r.str("tmod_sha"), Arc::new(set));
    Ok(json!({"path": path, "pkfx": text, "deps": deps, "missing": missing, "game_available": res.available()}))
}

/// `get_release_vfx_asset(release, ref)`: `(bytes, media type)`.
pub async fn asset(state: &AppState, r: &Doc, rf: &str) -> Result<(Vec<u8>, &'static str), ApiError> {
    let (_, index) = release_index(state, r).await?;
    let bn = lower(&pkfx::basename(rf));
    let res = Resolver::new(state).await?;
    let set = match cached(r.str("tmod_sha")) {
        Some(s) => s,
        None => {
            let s = Arc::new(release_depset(state, &index, &res).await?);
            remember(r.str("tmod_sha"), s.clone());
            s
        }
    };
    if !set.contains(&bn) && index_get(&index, &bn).is_none() {
        return Err(not_found("Asset not referenced by this release's VFX."));
    }
    let raw = match index_get(&index, &bn) {
        Some(d) => Some(d.clone()),
        None => match res.lookup(&bn) {
            Some(w) => res.read(state, w).await?,
            None => None,
        },
    };
    let raw = raw.ok_or_else(|| not_found("Asset not found (not bundled and not in the game tree)."))?;
    Ok((raw, pkfx::media_type_for(rf)))
}
