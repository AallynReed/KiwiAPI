//! The reads `create_release_from_commit` makes of a project's bare repo
//! (`gitstore.resolve`, `list_branches`, `read_tree`, `read_blob`), through
//! the `git` binary. Repos live at `<mods_store_dir>/git/<project_id>.git`.

use std::{
    collections::HashMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub fn repo_dir(mods_store_dir: &str, project_id: &str) -> PathBuf {
    Path::new(mods_store_dir).join("git").join(format!("{project_id}.git"))
}

/// `_open`: a repo exists once it has `objects/` or `HEAD`.
pub fn exists(dir: &Path) -> bool {
    dir.join("objects").is_dir() || dir.join("HEAD").is_file()
}

fn git(dir: &Path, args: &[&str], stdin: Option<Vec<u8>>) -> std::io::Result<Vec<u8>> {
    let mut cmd = Command::new("git");
    cmd.arg("-c").arg("safe.directory=*").arg("--git-dir").arg(dir).args(args);
    cmd.env("GIT_CONFIG_NOSYSTEM", "1").env_remove("GIT_DIR");
    cmd.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = cmd.spawn()?;
    let writer = match (stdin, child.stdin.take()) {
        (Some(data), Some(mut pipe)) => Some(std::thread::spawn(move || {
            let _ = pipe.write_all(&data);
        })),
        _ => None,
    };
    let mut out = Vec::new();
    if let Some(mut so) = child.stdout.take() {
        so.read_to_end(&mut out)?;
    }
    let status = child.wait()?;
    if let Some(w) = writer {
        let _ = w.join();
    }
    if !status.success() {
        return Err(std::io::Error::other(format!("git {} failed: {status}", args.join(" "))));
    }
    Ok(out)
}

/// Every `refs/heads/*` as `(name, sha)`.
pub fn branches(dir: &Path) -> std::io::Result<Vec<(Vec<u8>, String)>> {
    if !exists(dir) {
        return Ok(Vec::new());
    }
    let out = git(dir, &["for-each-ref", "--format=%(objectname) %(refname)", "refs/heads/"], None)?;
    Ok(out
        .split(|b| *b == b'\n')
        .filter_map(|line| {
            let sp = line.iter().position(|b| *b == b' ')?;
            let name = line[sp + 1..].strip_prefix(b"refs/heads/")?;
            Some((name.to_vec(), String::from_utf8_lossy(&line[..sp]).into_owned()))
        })
        .collect())
}

fn object_type(dir: &Path, sha: &str) -> std::io::Result<Option<String>> {
    let out = git(dir, &["cat-file", "--batch-check"], Some(format!("{sha}\n").into_bytes()))?;
    let line = String::from_utf8_lossy(&out);
    let mut parts = line.split_whitespace();
    let (_, kind) = (parts.next(), parts.next());
    Ok(kind.filter(|k| *k != "missing").map(str::to_owned))
}

/// `gitstore.resolve(project_id, ref)`: a branch name, else a 40-hex commit id.
pub fn resolve(dir: &Path, r: &str) -> std::io::Result<Option<String>> {
    if !exists(dir) {
        return Ok(None);
    }
    if let Some((_, sha)) = branches(dir)?.into_iter().find(|(n, _)| n.as_slice() == r.as_bytes()) {
        return Ok(Some(sha));
    }
    let cand = r.to_lowercase();
    if cand.len() != 40 || !cand.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(None);
    }
    Ok((object_type(dir, &cand)?.as_deref() == Some("commit")).then_some(cand))
}

pub struct Entry {
    pub path: String,
    pub sha: String,
    pub is_blob: bool,
    pub utf8: bool,
}

/// `read_tree`'s entries, sorted by path.
pub fn tree(dir: &Path, commit: &str) -> std::io::Result<Vec<Entry>> {
    let out = git(dir, &["ls-tree", "-r", "-z", "--full-tree", commit], None)?;
    let mut entries: Vec<Entry> = out
        .split(|b| *b == 0)
        .filter(|r| !r.is_empty())
        .filter_map(|rec| {
            let tab = rec.iter().position(|b| *b == b'\t')?;
            let meta = String::from_utf8_lossy(&rec[..tab]).into_owned();
            let mut m = meta.split(' ');
            let (_, kind, sha) = (m.next()?, m.next()?, m.next()?);
            let raw = &rec[tab + 1..];
            Some(Entry {
                path: String::from_utf8_lossy(raw).into_owned(),
                sha: sha.to_owned(),
                is_blob: kind == "blob",
                utf8: std::str::from_utf8(raw).is_ok(),
            })
        })
        .collect();
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

/// Blob contents by sha (`cat-file --batch`).
pub fn blobs(dir: &Path, shas: &[String]) -> std::io::Result<HashMap<String, Vec<u8>>> {
    let mut out = HashMap::new();
    if shas.is_empty() {
        return Ok(out);
    }
    let input: String = shas.iter().map(|s| format!("{s}\n")).collect();
    let raw = git(dir, &["cat-file", "--batch"], Some(input.into_bytes()))?;
    let mut pos = 0;
    while pos < raw.len() {
        let Some(nl) = raw[pos..].iter().position(|b| *b == b'\n') else { break };
        let header = String::from_utf8_lossy(&raw[pos..pos + nl]).into_owned();
        pos += nl + 1;
        let mut h = header.split(' ');
        let (sha, kind, size) = (h.next().unwrap_or(""), h.next().unwrap_or(""), h.next());
        if kind == "missing" {
            continue;
        }
        let size: usize = size.and_then(|s| s.parse().ok()).unwrap_or(0);
        let end = (pos + size).min(raw.len());
        if kind == "blob" {
            out.insert(sha.to_owned(), raw[pos..end].to_vec());
        }
        pos = end + 1;
    }
    Ok(out)
}
