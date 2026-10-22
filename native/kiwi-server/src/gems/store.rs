//! The decoded game files the gem routes read, like `app/trove/decode/store.py`:
//! `GAMEDATA_DIR` wins over the repo baseline `app/trove/gamedata`, and every
//! derived table is rebuilt when one of its files' mtime changes.
// replace with crate::gamedata once merged

use std::{
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
    time::SystemTime,
};

use serde_json::Value;

pub struct Store {
    runtime: Option<PathBuf>,
    baseline: PathBuf,
}

type Sig = Vec<(PathBuf, Option<SystemTime>)>;

impl Store {
    pub fn new(runtime: Option<PathBuf>, baseline: PathBuf) -> Self {
        Store { runtime, baseline }
    }

    /// The process store: `GAMEDATA_DIR` (if set) over `app/trove/gamedata`.
    pub fn global(gamedata_dir: &str) -> &'static Store {
        static STORE: OnceLock<Store> = OnceLock::new();
        STORE.get_or_init(|| {
            let runtime = (!gamedata_dir.is_empty()).then(|| PathBuf::from(gamedata_dir));
            Store::new(runtime, PathBuf::from("app/trove/gamedata"))
        })
    }

    /// `store.path(name)`.
    pub fn path(&self, name: &str) -> PathBuf {
        if let Some(rt) = &self.runtime {
            let p = rt.join(name);
            if p.is_file() {
                return p;
            }
        }
        self.baseline.join(name)
    }

    fn signature(&self, names: &[&str]) -> Sig {
        names
            .iter()
            .map(|n| {
                let p = self.path(n);
                let m = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
                (p, m)
            })
            .collect()
    }

    /// `store.load(name)`: the parsed file, `None` when missing or unreadable.
    pub fn load(&self, name: &str) -> Option<Value> {
        let bytes = std::fs::read(self.path(name)).ok()?;
        match serde_json::from_slice(&bytes) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!("gamedata {name}: {e}");
                None
            }
        }
    }
}

/// A value derived from some files, rebuilt when any of them changes
/// (`@gamedata.cached(...)`).
pub struct Cached<T> {
    slot: Mutex<Option<(Sig, Arc<T>)>>,
}

impl<T> Default for Cached<T> {
    fn default() -> Self {
        Cached { slot: Mutex::new(None) }
    }
}

impl<T> Cached<T> {
    pub fn get(&self, store: &Store, names: &[&str], build: impl FnOnce(&Store) -> T) -> Arc<T> {
        let sig = store.signature(names);
        let mut slot = self.slot.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((s, v)) = slot.as_ref()
            && *s == sig
        {
            return v.clone();
        }
        let v = Arc::new(build(store));
        *slot = Some((sig, v.clone()));
        v
    }
}
