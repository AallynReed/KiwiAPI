//! Decoded game data, read like `app/trove/decode/store.py`: a file rebuilt
//! into `GAMEDATA_DIR` wins over the repo baseline (`app/trove/gamedata`),
//! and every load is keyed on the files' mtimes, so a rebuild is picked up
//! without a restart.
//!
//! - [`Gamedata::global`]: the process-wide store (baseline + `settings.gamedata_dir`).
//! - [`Gamedata::path`]: the file readers should use (`path(name)`).
//! - [`Gamedata::signature`]: what changes whenever one of the files is rebuilt.
//! - [`Gamedata::load`]: parsed JSON, `None` when missing or unreadable (`load(name)`).
//! - [`Gamedata::cached`]: a value built once per key until its files change (`@cached`).
//! - [`read_json`]: `json.loads(path.read_text("utf-8"))` without the caching.

use std::{
    any::Any,
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, OnceLock},
    time::SystemTime,
};

use serde_json::Value;

use crate::{config::Settings, pyjson};

/// The repo baseline, relative to the working directory (`/app` in the image).
pub const BASELINE_DIR: &str = "app/trove/gamedata";

/// Each file's resolved path and mtime (`None` when it can't be stat'ed).
pub type Signature = Vec<(PathBuf, Option<SystemTime>)>;

type Loaded = (Signature, Option<Arc<Value>>);
type Memo = (Signature, Arc<dyn Any + Send + Sync>);

pub struct Gamedata {
    baseline: PathBuf,
    runtime: Option<PathBuf>,
    files: Mutex<HashMap<String, Loaded>>,
    memo: Mutex<HashMap<String, Memo>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Gamedata {
    pub fn new(baseline: impl Into<PathBuf>, runtime: Option<PathBuf>) -> Self {
        Gamedata { baseline: baseline.into(), runtime, files: Mutex::default(), memo: Mutex::default() }
    }

    pub fn global(settings: &Settings) -> &'static Gamedata {
        static G: OnceLock<Gamedata> = OnceLock::new();
        G.get_or_init(|| {
            let runtime = Some(&settings.gamedata_dir).filter(|d| !d.is_empty()).map(PathBuf::from);
            Gamedata::new(BASELINE_DIR, runtime)
        })
    }

    pub fn baseline_dir(&self) -> &Path {
        &self.baseline
    }

    pub fn path(&self, name: &str) -> PathBuf {
        if let Some(rt) = &self.runtime {
            let p = rt.join(name);
            if p.is_file() {
                return p;
            }
        }
        self.baseline.join(name)
    }

    pub fn signature(&self, names: &[&str]) -> Signature {
        names
            .iter()
            .map(|name| {
                let p = self.path(name);
                let mtime = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
                (p, mtime)
            })
            .collect()
    }

    pub fn load(&self, name: &str) -> Option<Arc<Value>> {
        let sig = self.signature(&[name]);
        if let Some((s, v)) = lock(&self.files).get(name)
            && *s == sig
        {
            return v.clone();
        }
        let value = read_json(&sig[0].0).map(Arc::new);
        lock(&self.files).insert(name.to_owned(), (sig, value.clone()));
        value
    }

    /// `build` runs again once any of `names` changes; `key` carries the arguments.
    pub fn cached<T: Send + Sync + 'static>(&self, key: &str, names: &[&str], build: impl FnOnce() -> T) -> Arc<T> {
        let sig = self.signature(names);
        if let Some((s, v)) = lock(&self.memo).get(key)
            && *s == sig
            && let Ok(t) = v.clone().downcast::<T>()
        {
            return t;
        }
        let value = Arc::new(build());
        lock(&self.memo).insert(key.to_owned(), (sig, value.clone()));
        value
    }
}

/// Python's text read rejects bad UTF-8, and `json.loads(str)` a BOM or NUL;
/// `NaN`/`Infinity` (which Python accepts) also read as `None` here.
pub fn read_json(path: &Path) -> Option<Value> {
    let bytes = std::fs::read(path).ok()?;
    let text = std::str::from_utf8(&bytes).ok()?;
    if text.starts_with('\u{feff}') || text.contains('\0') {
        return None;
    }
    pyjson::loads(text.as_bytes()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kiwi-gamedata-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn runtime_wins_and_reloads() {
        let base = tmp("base");
        let rt = tmp("rt");
        std::fs::write(base.join("a.json"), r#"{"v": 1}"#).unwrap();
        std::fs::write(base.join("bad.json"), "{").unwrap();
        std::fs::write(base.join("bom.json"), "\u{feff}{}").unwrap();
        let gd = Gamedata::new(&base, Some(rt.clone()));
        assert_eq!(gd.load("a.json").unwrap()["v"], 1);
        assert!(gd.load("bad.json").is_none() && gd.load("bom.json").is_none() && gd.load("nope.json").is_none());

        let builds = std::sync::atomic::AtomicUsize::new(0);
        let get = || {
            gd.cached("k", &["a.json"], || {
                builds.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                gd.load("a.json").unwrap()["v"].as_i64().unwrap()
            })
        };
        assert_eq!(*get(), 1);
        assert_eq!(*get(), 1);
        assert_eq!(builds.load(std::sync::atomic::Ordering::SeqCst), 1);

        std::fs::write(rt.join("a.json"), r#"{"v": 2}"#).unwrap();
        assert_eq!(gd.path("a.json"), rt.join("a.json"));
        assert_eq!(*get(), 2);
        assert_eq!(builds.load(std::sync::atomic::Ordering::SeqCst), 2);

        std::fs::remove_file(rt.join("a.json")).unwrap();
        assert_eq!(*get(), 1);
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_dir_all(&rt);
    }
}
