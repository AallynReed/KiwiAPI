//! `app/trove/updates/cas.py`: the content-addressed blob store,
//! `<root>/objects/<sha[:2]>/<sha256>`, raw bytes, written once (temp file in
//! the same directory, fsync, rename) and never rewritten.

use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub struct ContentStore {
    pub root: PathBuf,
    pub objects: PathBuf,
}

impl ContentStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let objects = root.join("objects");
        ContentStore { root, objects }
    }

    pub fn path_for(&self, sha: &str) -> PathBuf {
        self.objects.join(sha.get(..2).unwrap_or(sha)).join(sha)
    }

    pub fn has(&self, sha: &str) -> bool {
        self.path_for(sha).is_file()
    }

    /// `(sha256_hex, created)`; `created` is false when the blob was already there.
    pub fn put(&self, data: &[u8]) -> io::Result<(String, bool)> {
        let sha = hex::encode(Sha256::digest(data));
        let dest = self.path_for(&sha);
        if dest.is_file() {
            return Ok((sha, false));
        }
        let dir = dest.parent().expect("blob path has a parent");
        fs::create_dir_all(dir)?;
        let (mut f, tmp) = mkstemp(dir, ".part")?;
        let res = (|| {
            f.write_all(data)?;
            f.flush()?;
            f.sync_all()?;
            drop(f);
            fs::rename(&tmp, &dest)
        })();
        if let Err(e) = res {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
        Ok((sha, true))
    }

    pub fn get(&self, sha: &str) -> Option<Vec<u8>> {
        let p = self.path_for(sha);
        if p.is_file() { fs::read(p).ok() } else { None }
    }
}

const NAME_CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789_";

/// `tempfile.mkstemp(dir=dir, suffix=suffix)`: `tmp` + 8 random name
/// characters + suffix, created exclusively with mode 0600.
fn mkstemp(dir: &Path, suffix: &str) -> io::Result<(fs::File, PathBuf)> {
    let mut last = None;
    for _ in 0..10_000 {
        let mut raw = [0u8; 8];
        getrandom::fill(&mut raw).map_err(io::Error::other)?;
        let name: String = raw.iter().map(|b| NAME_CHARS[usize::from(*b) % NAME_CHARS.len()] as char).collect();
        let path = dir.join(format!("tmp{name}{suffix}"));
        let mut opts = fs::OpenOptions::new();
        opts.read(true).write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
        match opts.open(&path) {
            Ok(f) => return Ok((f, path)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| io::Error::new(io::ErrorKind::AlreadyExists, "No usable temporary file name found")))
}
