//! `app/trove/updates/cas.py`'s `ContentStore`: blobs at
//! `<root>/objects/<sha[:2]>/<sha>`, written atomically (temp + fsync +
//! rename) and only when absent, so Python and Rust share the stores.

use std::{
    io::Write,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

#[derive(Clone, Debug)]
pub struct ContentStore {
    root: PathBuf,
}

impl ContentStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        ContentStore { root: root.into() }
    }

    pub fn path_for(&self, sha: &str) -> PathBuf {
        let shard: String = sha.chars().take(2).collect();
        self.root.join("objects").join(shard).join(sha)
    }

    pub fn has(&self, sha: &str) -> bool {
        self.path_for(sha).is_file()
    }

    /// `get`: None when there is no such blob.
    pub fn get(&self, sha: &str) -> std::io::Result<Option<Vec<u8>>> {
        let p = self.path_for(sha);
        if !p.is_file() {
            return Ok(None);
        }
        std::fs::read(p).map(Some)
    }

    /// `put`: (sha256 hex, created).
    pub fn put(&self, data: &[u8]) -> std::io::Result<(String, bool)> {
        let sha = hex::encode(Sha256::digest(data));
        let dest = self.path_for(&sha);
        if dest.is_file() {
            return Ok((sha, false));
        }
        let dir = dest.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let tmp = temp_name(dir);
        let written = (|| {
            let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
            f.write_all(data)?;
            f.flush()?;
            f.sync_all()?;
            drop(f);
            std::fs::rename(&tmp, &dest)
        })();
        if let Err(e) = written {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
        Ok((sha, true))
    }
}

/// `tempfile.mkstemp(dir=..., suffix=".part")`'s name shape.
fn temp_name(dir: &Path) -> PathBuf {
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789_";
    let mut raw = [0u8; 8];
    let _ = getrandom::fill(&mut raw);
    let name: String = raw.iter().map(|b| CHARS[usize::from(*b) % CHARS.len()] as char).collect();
    dir.join(format!("tmp{name}.part"))
}
