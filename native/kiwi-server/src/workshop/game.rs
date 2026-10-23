//! `trove_layout.game_file_map` / `game_file_paths`: the live tree's file
//! names, rebuilt when the branch's `current_ordinal` moves.

use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};

use anyhow::{Context, bail};
use futures::TryStreamExt;
use mongodb::bson::{Bson, Document, doc};

use crate::state::AppState;

#[derive(Default)]
pub struct GameIndex {
    pub ordinal: i64,
    /// `{filename.lower(): first path}`.
    pub map: HashMap<String, String>,
    /// `{filename.lower(): [every path]}`.
    pub all: HashMap<String, Vec<String>>,
}

static BUILT: LazyLock<Mutex<HashMap<String, Arc<GameIndex>>>> = LazyLock::new(Default::default);
static BUILD: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(Default::default);

#[cfg(test)]
pub static TEST_PATHS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

pub fn from_paths(ordinal: i64, paths: impl IntoIterator<Item = String>) -> GameIndex {
    let mut idx = GameIndex { ordinal, ..Default::default() };
    for path in paths {
        let name = path.rsplit('/').next().unwrap_or(&path).to_lowercase();
        idx.map.entry(name.clone()).or_insert_with(|| path.clone());
        idx.all.entry(name).or_default().push(path);
    }
    idx
}

fn hit(branch: &str, ordinal: i64) -> Option<Arc<GameIndex>> {
    BUILT.lock().ok()?.get(branch).filter(|i| i.ordinal == ordinal && !i.map.is_empty()).cloned()
}

/// The branch's index; empty when the archive has no current build.
pub async fn index(state: &AppState, branch: &str) -> anyhow::Result<Arc<GameIndex>> {
    #[cfg(test)]
    if let Some(paths) = TEST_PATHS.get() {
        return Ok(Arc::new(from_paths(1, paths.clone())));
    }
    let db = state.inner.db.mongo.as_ref().context("mongo not configured")?;
    let ub = db.collection::<Document>("update_branches").find_one(doc! {"branch": branch}).await?;
    let ordinal = match ub.as_ref().map(|d| d.get("current_ordinal")) {
        None => return Ok(Arc::default()),
        Some(None) => 0,
        Some(Some(Bson::Int32(i))) => i64::from(*i),
        Some(Some(Bson::Int64(i))) => *i,
        Some(Some(v)) => bail!("update_branches.current_ordinal: unexpected {v:?}"),
    };
    if ordinal == 0 {
        return Ok(Arc::default());
    }
    if let Some(h) = hit(branch, ordinal) {
        return Ok(h);
    }
    let _guard = BUILD.lock().await;
    if let Some(h) = hit(branch, ordinal) {
        return Ok(h);
    }
    let mut cursor = db
        .collection::<Document>("update_state")
        .find(doc! {"branch": branch})
        .projection(doc! {"path": 1, "_id": 0})
        .await?;
    let mut paths = Vec::new();
    while let Some(d) = cursor.try_next().await? {
        match d.get("path") {
            None | Some(Bson::Null) => {}
            Some(Bson::String(s)) if s.is_empty() => {}
            Some(Bson::String(s)) => paths.push(s.clone()),
            Some(v) => bail!("update_state.path: unexpected {v:?}"),
        }
    }
    let built = Arc::new(from_paths(ordinal, paths));
    if let Ok(mut m) = BUILT.lock() {
        m.insert(branch.to_owned(), built.clone());
    }
    Ok(built)
}
