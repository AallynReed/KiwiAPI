//! `catalogue.get(branch)`: the option lists read out of the archive, built
//! once per codex signature.

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};

use kiwi_core::dressing::{self as kd, Catalogue};

use crate::{
    codexes::archive::{Archive, Cas, MongoArchive},
    state::AppState,
};

type Sig = (i64, Option<chrono::DateTime<chrono::Utc>>);
type Built = HashMap<String, (Sig, Arc<Catalogue>)>;
static BUILT: LazyLock<Mutex<Built>> = LazyLock::new(Default::default);
static BUILD: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(Default::default);

#[cfg(test)]
pub static TEST_CATALOGUE: std::sync::OnceLock<Arc<Catalogue>> = std::sync::OnceLock::new();

fn hit(branch: &str, sig: &Sig) -> Option<Arc<Catalogue>> {
    BUILT.lock().ok()?.get(branch).filter(|(s, _)| s == sig).map(|(_, c)| c.clone())
}

/// The branch's catalogue, rebuilt only when the codex signature moves.
pub async fn get(state: &AppState) -> anyhow::Result<Arc<Catalogue>> {
    #[cfg(test)]
    if let Some(c) = TEST_CATALOGUE.get() {
        return Ok(c.clone());
    }
    let branch = state.settings().trove_render_branch.clone();
    if !state.settings().postgres_enabled() {
        return Ok(Arc::default());
    }
    let sig = crate::codexes::store::meta_signature(state.pg(), &branch).await?;
    if let Some(c) = hit(&branch, &sig) {
        return Ok(c);
    }
    let _guard = BUILD.lock().await;
    if let Some(c) = hit(&branch, &sig) {
        return Ok(c);
    }
    let built = Arc::new(build(state, &branch).await?);
    if let Ok(mut m) = BUILT.lock() {
        m.insert(branch, (sig, built.clone()));
    }
    Ok(built)
}

/// `_load(branch, prefix)`: every `.binfab` under `prefix` with its bytes.
async fn load<A: Archive>(a: &A, prefix: &str) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
    let rows: Vec<(String, String)> = a
        .prefix(prefix)
        .await?
        .into_iter()
        .filter_map(|(p, sha)| sha.filter(|s| !s.is_empty() && p.ends_with(".binfab")).map(|s| (p, s)))
        .collect();
    let blobs = a.blobs();
    Ok(tokio::task::spawn_blocking(move || {
        use kiwi_core::codex::extract::Blobs;
        rows.into_iter().filter_map(|(p, sha)| blobs.get(&sha).filter(|b| !b.is_empty()).map(|b| (p, b))).collect()
    })
    .await?)
}

async fn build(state: &AppState, branch: &str) -> anyhow::Result<Catalogue> {
    let db = state.inner.db.mongo.clone().ok_or_else(|| anyhow::anyhow!("mongo not configured"))?;
    let root = state.settings().trove_update_store_dir.clone();
    let a = MongoArchive { db, branch: branch.to_owned(), cas: Cas { root: root.into() } };
    let loc = crate::codexes::indexer::load_locale_map(&a).await?;
    let classes = load(&a, kd::CLASS_ROOT).await?;
    if kd::build_classes(&classes, &loc).is_empty() {
        return Ok(Catalogue::default());
    }
    let skins = load(&a, kd::SKIN_ROOT).await?;
    let rig_map = crate::creature::rig_map(state, branch).await?;
    let equipment = load(&a, kd::EQUIP_ROOT).await?;
    let heads = match a.file_sha(kd::CUSTOM_HEADS).await? {
        Some(sha) => a.blob(&sha).await?,
        None => None,
    };
    let built = tokio::task::spawn_blocking(move || {
        let inputs = kd::Inputs {
            loc: &loc,
            classes: &classes,
            skins: &skins,
            rig_map: &rig_map,
            equipment: &equipment,
            heads: heads.as_deref().filter(|h| !h.is_empty()),
        };
        kd::build(&inputs, crate::rigs::rig)
    })
    .await?
    .map_err(|e| anyhow::anyhow!(e))?;
    tracing::info!(
        "dressing[{branch}]: {} classes, {} costumes, {} races",
        built.classes.len(),
        built.costumes.iter().map(|(_, v)| v.len()).sum::<usize>(),
        built.races.len()
    );
    Ok(built)
}
