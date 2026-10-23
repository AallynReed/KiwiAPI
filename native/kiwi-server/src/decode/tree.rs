//! `app/trove/decode/tree.py`: read-only views of a game file tree. Paths are
//! posix, relative to the client root; lookups ignore case, listings return
//! the stored spelling sorted by `str.lower`.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

/// Where the bytes of a listed path come from.
pub trait Source: Send + Sync {
    fn fetch(&self, path: &str) -> Option<Vec<u8>>;
}

#[derive(Clone)]
pub struct Tree {
    index: Arc<HashMap<String, String>>,
    sorted: Arc<Vec<(String, String)>>,
    src: Arc<dyn Source>,
}

impl Tree {
    pub fn new(paths: impl IntoIterator<Item = String>, src: Arc<dyn Source>) -> Tree {
        let mut index: HashMap<String, String> = HashMap::new();
        for p in paths {
            index.insert(p.to_lowercase(), p);
        }
        let mut sorted: Vec<(String, String)> = index.iter().map(|(l, p)| (l.clone(), p.clone())).collect();
        sorted.sort();
        Tree { index: Arc::new(index), sorted: Arc::new(sorted), src }
    }

    pub fn read(&self, path: &str) -> Option<Vec<u8>> {
        let actual = self.index.get(&path.to_lowercase())?;
        self.src.fetch(actual)
    }

    pub fn exists(&self, path: &str) -> bool {
        self.index.contains_key(&path.to_lowercase())
    }

    /// Every file under `prefix` (recursive), sorted, optionally by suffix.
    pub fn files(&self, prefix: &str, suffix: &str) -> Vec<String> {
        let (prefix, suffix) = (prefix.to_lowercase(), suffix.to_lowercase());
        self.sorted
            .iter()
            .filter(|(l, _)| l.starts_with(&prefix) && l.ends_with(&suffix))
            .map(|(_, p)| p.clone())
            .collect()
    }

    /// A view of just `prefixes`.
    pub fn only(&self, prefixes: &[&str]) -> Tree {
        let wanted: Vec<String> = prefixes.iter().map(|p| p.to_lowercase()).collect();
        let paths = self.sorted.iter().filter(|(l, _)| wanted.iter().any(|w| l.starts_with(w))).map(|(_, p)| p.clone());
        Tree::new(paths, self.src.clone())
    }

    /// Names of the immediate subdirectories of `prefix`.
    pub fn dirs(&self, prefix: &str) -> Vec<String> {
        let prefix = format!("{}/", prefix.to_lowercase().trim_end_matches('/'));
        let mut out: Vec<String> = Vec::new();
        for p in self.files(&prefix, "") {
            let rest = p.get(prefix.len()..).unwrap_or("");
            if let Some((head, _)) = rest.split_once('/')
                && !out.iter().any(|o| o == head)
            {
                out.push(head.to_owned());
            }
        }
        out.sort();
        out
    }

    pub fn len(&self) -> usize {
        self.sorted.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sorted.is_empty()
    }
}

/// The latest version of a branch in the update archive: path -> content sha,
/// bytes from the content store (`<root>/objects/<sha[:2]>/<sha>`).
pub struct Archive {
    pub shas: HashMap<String, String>,
    pub root: PathBuf,
}

impl Source for Archive {
    fn fetch(&self, path: &str) -> Option<Vec<u8>> {
        let sha = self.shas.get(path)?;
        let p = self.root.join("objects").join(sha.get(..2)?).join(sha);
        std::fs::read(p).ok()
    }
}

/// An extracted client tree on disk.
pub struct Dir(pub PathBuf);

impl Source for Dir {
    fn fetch(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.0.join(path)).ok()
    }
}

/// `DirTree(root, prefixes)`.
pub fn dir_tree(root: &Path, prefixes: &[&str]) -> Tree {
    let mut paths = Vec::new();
    for prefix in prefixes {
        if !prefix.ends_with('/') && root.join(prefix).is_file() {
            paths.push((*prefix).to_owned());
        }
    }
    for prefix in prefixes {
        walk(root, &root.join(prefix), &mut paths);
    }
    Tree::new(paths, Arc::new(Dir(root.to_path_buf())))
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(root, &p, out);
        } else if let Ok(rel) = p.strip_prefix(root) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// Files held in memory (tests).
pub struct Mem(pub HashMap<String, Vec<u8>>);

impl Source for Mem {
    fn fetch(&self, path: &str) -> Option<Vec<u8>> {
        self.0.get(path).cloned()
    }
}

pub fn mem_tree(files: Vec<(&str, Vec<u8>)>) -> Tree {
    let map: HashMap<String, Vec<u8>> = files.into_iter().map(|(p, b)| (p.to_owned(), b)).collect();
    let paths: Vec<String> = map.keys().cloned().collect();
    Tree::new(paths, Arc::new(Mem(map)))
}
