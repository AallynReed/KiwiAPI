//! The slice of `mods_hub/gitstore.py` this component needs: bare repos at
//! `<mods_store_dir>/git/<project_id>.git`, laid out exactly as dulwich's
//! `Repo.init_bare` leaves them. Refs are read and written directly (dulwich
//! writes loose refs and no reflog); object reads and writes go through the
//! `git` binary.

use std::{
    path::{Path, PathBuf},
    process::Stdio,
};

use anyhow::{Context, bail};
use tokio::{io::AsyncWriteExt, process::Command};

use crate::state::AppState;

pub const AUTHOR_DOMAIN: &str = "users.noreply.kiwi";

pub fn repo_dir(state: &AppState, project_id: &str) -> PathBuf {
    Path::new(&state.settings().mods_store_dir).join("git").join(format!("{project_id}.git"))
}

fn exists(d: &Path) -> bool {
    d.join("objects").is_dir() || d.join("HEAD").is_file()
}

fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let lock = path.with_file_name(format!("{}.lock", path.file_name().and_then(|n| n.to_str()).unwrap_or("x")));
    std::fs::write(&lock, data)?;
    std::fs::rename(&lock, path)
}

/// `Repo.init_bare(d)` followed by `HEAD -> refs/heads/main`.
pub fn init_bare(d: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(d)?;
    for sub in ["branches", "refs", "refs/tags", "refs/heads", "hooks", "info", "objects/info", "objects/pack"] {
        std::fs::create_dir_all(d.join(sub))?;
    }
    write_atomic(&d.join("description"), b"Unnamed repository")?;
    let filemode = if cfg!(unix) { "true" } else { "false" };
    let config = format!(
        "[core]\n\trepositoryformatversion = 0\n\tfilemode = {filemode}\n\tbare = true\n\tlogallrefupdates = true\n"
    );
    write_atomic(&d.join("config"), config.as_bytes())?;
    write_atomic(&d.join("info").join("exclude"), b"")?;
    write_atomic(&d.join("HEAD"), b"ref: refs/heads/main\n")
}

/// `gitstore.ensure_repo(project_id)`.
pub async fn ensure_repo(state: &AppState, project_id: &str) -> anyhow::Result<()> {
    let d = repo_dir(state, project_id);
    tokio::task::spawn_blocking(move || if exists(&d) { Ok(()) } else { init_bare(&d) }).await??;
    Ok(())
}

/// `gitstore.delete_repo(project_id)`.
pub async fn delete_repo(state: &AppState, project_id: &str) {
    let d = repo_dir(state, project_id);
    let _ = tokio::task::spawn_blocking(move || {
        if d.exists() {
            let _ = std::fs::remove_dir_all(&d);
        }
    })
    .await;
}

fn valid_ref(name: &str) -> bool {
    !name.is_empty() && !name.ends_with(".lock") && !name.split('/').any(|s| s.is_empty() || s.starts_with('.'))
}

fn walk_loose(dir: &Path, prefix: &str, out: &mut Vec<(String, String)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        let full = format!("{prefix}{name}");
        if path.is_dir() {
            walk_loose(&path, &format!("{full}/"), out);
        } else if let Ok(raw) = std::fs::read(&path) {
            out.push((full, String::from_utf8_lossy(&raw).trim_end().to_owned()));
        }
    }
}

/// Every ref under `refs/` with its raw value (loose ones over packed ones).
fn raw_refs(d: &Path) -> Vec<(String, String)> {
    let mut refs: Vec<(String, String)> = Vec::new();
    if let Ok(packed) = std::fs::read_to_string(d.join("packed-refs")) {
        for line in packed.lines() {
            if line.starts_with('#') || line.starts_with('^') {
                continue;
            }
            if let Some((sha, name)) = line.split_once(' ') {
                refs.push((name.to_owned(), sha.to_owned()));
            }
        }
    }
    let mut loose = Vec::new();
    walk_loose(&d.join("refs"), "refs/", &mut loose);
    for (name, value) in loose {
        match refs.iter_mut().find(|(n, _)| *n == name) {
            Some(e) => e.1 = value,
            None => refs.push((name, value)),
        }
    }
    refs.retain(|(n, _)| valid_ref(n));
    refs
}

/// Follows `ref: ` values to a sha, as `RefsContainer.__getitem__` does.
fn follow(refs: &[(String, String)], value: &str) -> Option<String> {
    let mut v = value.to_owned();
    for _ in 0..5 {
        match v.strip_prefix("ref: ") {
            Some(target) => v = refs.iter().find(|(n, _)| n == target)?.1.clone(),
            None => return (v.len() == 40 && v.bytes().all(|b| b.is_ascii_hexdigit())).then_some(v),
        }
    }
    None
}

/// `_list_branches`: `(name, head)` sorted by name.
pub fn branches_in(d: &Path) -> Vec<(String, String)> {
    if !exists(d) {
        return Vec::new();
    }
    let refs = raw_refs(d);
    let mut out: Vec<(String, String)> =
        refs.iter().filter_map(|(n, v)| Some((n.strip_prefix("refs/heads/")?.to_owned(), follow(&refs, v)?))).collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

pub async fn list_branches(state: &AppState, project_id: &str) -> anyhow::Result<Vec<(String, String)>> {
    let d = repo_dir(state, project_id);
    Ok(tokio::task::spawn_blocking(move || branches_in(&d)).await?)
}

async fn git(d: &Path, args: &[&str], stdin: Option<&[u8]>, index: Option<&Path>) -> anyhow::Result<Vec<u8>> {
    let mut cmd = Command::new("git");
    cmd.arg("-c").arg("safe.directory=*").arg("--git-dir").arg(d).args(args);
    if let Some(i) = index {
        cmd.env("GIT_INDEX_FILE", i);
    }
    cmd.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().context("spawn git")?;
    if let (Some(data), Some(mut sin)) = (stdin, child.stdin.take()) {
        sin.write_all(data).await?;
        drop(sin);
    }
    let out = child.wait_with_output().await?;
    if !out.status.success() {
        bail!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(out.stdout)
}

/// `_resolve(repo, ref)`: a branch head, else a 40-hex id naming a commit.
async fn resolve(d: &Path, r: &str) -> anyhow::Result<Option<String>> {
    let dd = d.to_path_buf();
    let name = format!("refs/heads/{r}");
    let refs = tokio::task::spawn_blocking(move || raw_refs(&dd)).await?;
    if let Some((_, v)) = refs.iter().find(|(n, _)| *n == name) {
        return Ok(follow(&refs, v));
    }
    let cand = r.to_lowercase();
    if cand.len() != 40 || !cand.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Ok(None);
    }
    match git(d, &["cat-file", "-t", &cand], None, None).await {
        Ok(kind) if kind.trim_ascii() == b"commit" => Ok(Some(cand)),
        _ => Ok(None),
    }
}

/// `_count_commits(project_id, ref)`.
pub async fn count_commits(state: &AppState, project_id: &str, r: &str) -> anyhow::Result<u64> {
    let d = repo_dir(state, project_id);
    if !exists(&d) {
        return Ok(0);
    }
    let Some(head) = resolve(&d, r).await? else { return Ok(0) };
    let out = git(&d, &["rev-list", "--count", &head], None, None).await?;
    Ok(String::from_utf8_lossy(&out).trim().parse()?)
}

/// `author_ident(username)`.
pub fn author_ident(username: &str) -> String {
    let base = if username.is_empty() { "modder" } else { username };
    let safe: String = base.chars().filter(|c| kiwi_core::pystr::is_alnum(*c) || "-_.".contains(*c)).collect();
    let safe = if safe.is_empty() { "modder".to_owned() } else { safe };
    format!("{username} <{safe}@{AUTHOR_DOMAIN}>")
}

/// The commit dulwich's `Commit()` serializes for a fork (no parents).
pub fn fork_commit(tree: &str, author: &str, ts: i64, message: &str) -> Vec<u8> {
    let msg = if message.is_empty() { "fork" } else { message };
    format!("tree {tree}\nauthor {author} {ts} +0000\ncommitter {author} {ts} +0000\nencoding UTF-8\n\n{msg}")
        .into_bytes()
}

/// Identity, message and time of a fork commit.
pub struct ForkCommit {
    pub author: String,
    pub message: String,
    pub ts: i64,
}

/// `gitstore.fork(...)`: the source branch head's files as one parentless
/// commit on `branch` of the destination repo.
pub async fn fork(
    state: &AppState,
    src_id: &str,
    dst_id: &str,
    src_branch: &str,
    branch: &str,
    c: &ForkCommit,
) -> anyhow::Result<Option<String>> {
    ensure_repo(state, dst_id).await?;
    fork_dirs(&repo_dir(state, src_id), &repo_dir(state, dst_id), src_branch, branch, c).await
}

pub async fn fork_dirs(
    src: &Path,
    dst: &Path,
    src_branch: &str,
    branch: &str,
    c: &ForkCommit,
) -> anyhow::Result<Option<String>> {
    let (src, dst) = (src.to_path_buf(), dst.to_path_buf());
    if !exists(&src) {
        return Ok(None);
    }
    let Some(head) = resolve(&src, src_branch).await? else { return Ok(None) };
    let listing = git(&src, &["ls-tree", "-r", "-z", "--full-tree", &head], None, None).await?;
    let mut shas = String::new();
    let mut index_info: Vec<u8> = Vec::new();
    for entry in listing.split(|b| *b == 0).filter(|e| !e.is_empty()) {
        let tab = entry.iter().position(|b| *b == b'\t').context("ls-tree line")?;
        let meta = String::from_utf8_lossy(&entry[..tab]);
        let mut it = meta.split(' ');
        let (mode, _kind, sha) = (it.next().unwrap_or(""), it.next().unwrap_or(""), it.next().unwrap_or(""));
        shas.push_str(sha);
        shas.push('\n');
        index_info.extend_from_slice(format!("{mode} {sha}\t").as_bytes());
        index_info.extend_from_slice(&entry[tab + 1..]);
        index_info.push(0);
    }
    if !shas.is_empty() {
        let pack = git(&src, &["pack-objects", "--stdout", "-q"], Some(shas.as_bytes()), None).await?;
        git(&dst, &["unpack-objects", "-q"], Some(&pack), None).await?;
    }
    let index = std::env::temp_dir().join(format!("kiwi-fork-{}.index", mongodb::bson::oid::ObjectId::new().to_hex()));
    let tree = async {
        git(&dst, &["update-index", "-z", "--add", "--index-info"], Some(&index_info), Some(&index)).await?;
        git(&dst, &["write-tree"], None, Some(&index)).await
    }
    .await;
    let _ = std::fs::remove_file(&index);
    let tree = String::from_utf8_lossy(&tree?).trim().to_owned();
    let raw = fork_commit(&tree, &c.author, c.ts, &c.message);
    let sha = git(&dst, &["hash-object", "-t", "commit", "-w", "--stdin", "--literally"], Some(&raw), None).await?;
    let sha = String::from_utf8_lossy(&sha).trim().to_owned();
    let ref_path = dst.join("refs").join("heads").join(branch);
    if let Some(parent) = ref_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_atomic(&ref_path, format!("{sha}\n").as_bytes())?;
    Ok(Some(sha))
}
