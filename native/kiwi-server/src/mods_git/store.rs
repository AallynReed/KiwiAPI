//! `app/trove/mods_hub/gitstore.py`: per-project repositories under
//! `<mods_store_dir>/git/<project_id>.git`, with dulwich's tree iteration,
//! `commit_tree`, commit serialization and date-ordered walker. Blocking: run
//! it off the async runtime.

use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashSet, VecDeque},
    path::{Path, PathBuf},
};

use indexmap::IndexMap;

use super::{
    odb::{GitError, GitResult, Kind, Oid, Repo, from_hex, to_hex},
    refs,
};

pub const AUTHOR_DOMAIN: &str = "users.noreply.kiwi";
pub const S_IFDIR: u32 = 0o040000;
const MAX_EXTRA_COMMITS: usize = 5;

pub fn root(mods_store_dir: &str) -> PathBuf {
    Path::new(mods_store_dir).join("git")
}

pub fn repo_dir(mods_store_dir: &str, project_id: &str) -> PathBuf {
    root(mods_store_dir).join(format!("{project_id}.git"))
}

fn is_dir(mode: u32) -> bool {
    mode & 0o170000 == S_IFDIR
}

#[derive(Debug)]
pub enum StoreError {
    Git(GitError),
    NothingToCommit,
    /// `GitStoreError(message)`.
    Refused(String),
}

impl From<GitError> for StoreError {
    fn from(e: GitError) -> Self {
        StoreError::Git(e)
    }
}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        StoreError::Git(e.into())
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub path: Vec<u8>,
    pub mode: u32,
    pub sha: Oid,
}

/// `parse_tree`; a repeated name keeps the last entry, as dulwich's dict does.
pub fn parse_tree(data: &[u8]) -> GitResult<Vec<(Vec<u8>, u32, Oid)>> {
    let bad = || GitError::Invalid("malformed tree".into());
    let mut out: IndexMap<Vec<u8>, (u32, Oid)> = IndexMap::new();
    let mut i = 0;
    while i < data.len() {
        let sp = data[i..].iter().position(|&c| c == b' ').ok_or_else(bad)? + i;
        let mode_text = std::str::from_utf8(&data[i..sp]).map_err(|_| bad())?;
        let mode = u32::from_str_radix(mode_text.trim(), 8).map_err(|_| bad())?;
        let nul = data[sp..].iter().position(|&c| c == 0).ok_or_else(bad)? + sp;
        let name = data[sp + 1..nul].to_vec();
        let sha: Oid = data.get(nul + 1..nul + 21).ok_or_else(bad)?.try_into().map_err(|_| bad())?;
        out.insert(name, (mode, sha));
        i = nul + 21;
    }
    Ok(out.into_iter().map(|(n, (m, s))| (n, m, s)).collect())
}

fn tree_of(repo: &Repo, oid: &Oid) -> GitResult<Vec<(Vec<u8>, u32, Oid)>> {
    let obj = repo.get(oid)?;
    if obj.kind != Kind::Tree {
        return Err(GitError::Invalid("not a tree".into()));
    }
    let mut items = parse_tree(&obj.data)?;
    items.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(items)
}

/// `iter_tree_contents`: depth-first, entries in name order.
pub fn iter_tree(repo: &Repo, tree: &Oid, mut visit: impl FnMut(Entry) -> GitResult<bool>) -> GitResult<()> {
    let mut todo = vec![Entry { path: Vec::new(), mode: S_IFDIR, sha: *tree }];
    while let Some(e) = todo.pop() {
        if is_dir(e.mode) {
            let items = tree_of(repo, &e.sha)?;
            for (name, mode, sha) in items.into_iter().rev() {
                let path = if e.path.is_empty() { name } else { [&e.path[..], b"/", &name].concat() };
                todo.push(Entry { path, mode, sha });
            }
        } else if !visit(e)? {
            break;
        }
    }
    Ok(())
}

pub fn tree_entries(repo: &Repo, tree: &Oid) -> GitResult<Vec<Entry>> {
    let mut out = Vec::new();
    iter_tree(repo, tree, |e| {
        out.push(e);
        Ok(true)
    })?;
    Ok(out)
}

#[derive(Clone, Debug, Default)]
pub struct Commit {
    pub tree: Option<Vec<u8>>,
    pub parents: Vec<Vec<u8>>,
    pub author: Option<Vec<u8>>,
    pub commit_time: Option<i64>,
    pub message: Option<Vec<u8>>,
}

fn parse_time_entry(value: &[u8]) -> GitResult<(Vec<u8>, Option<i64>)> {
    let Some(sep) = value.windows(2).rposition(|w| w == b"> ") else { return Ok((value.to_vec(), None)) };
    let person = value[..=sep].to_vec();
    let rest = &value[sep + 2..];
    let bad = || GitError::Invalid("malformed time entry".into());
    let sp = rest.iter().rposition(|&c| c == b' ').ok_or_else(bad)?;
    let time = std::str::from_utf8(&rest[..sp]).ok().and_then(|t| t.trim().parse::<i64>().ok()).ok_or_else(bad)?;
    Ok((person, Some(time)))
}

/// `_parse_message` + `_parse_commit`, keeping what the store reads.
pub fn parse_commit(data: &[u8]) -> GitResult<Commit> {
    let mut c = Commit::default();
    let mut headers: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    let mut key: Option<Vec<u8>> = None;
    let mut val: Vec<u8> = Vec::new();
    let mut pos = 0;
    let mut eof = true;
    let strip_nl = |v: &mut Vec<u8>| {
        if v.last() == Some(&b'\n') {
            v.pop();
        }
    };
    while pos < data.len() {
        let end = data[pos..].iter().position(|&b| b == b'\n').map(|i| pos + i + 1).unwrap_or(data.len());
        let line = &data[pos..end];
        pos = end;
        if let Some(cont) = line.strip_prefix(b" ") {
            val.extend_from_slice(cont);
            continue;
        }
        if let Some(k) = key.take() {
            let mut v = std::mem::take(&mut val);
            strip_nl(&mut v);
            headers.push((k, v));
        }
        if line == b"\n" {
            eof = false;
            break;
        }
        let sp = line.iter().position(|&b| b == b' ').ok_or_else(|| GitError::Invalid("malformed header".into()))?;
        key = Some(line[..sp].to_vec());
        val = line[sp + 1..].to_vec();
    }
    if eof {
        if let Some(k) = key.take() {
            strip_nl(&mut val);
            headers.push((k, val));
        }
    } else {
        c.message = Some(data[pos..].to_vec());
    }
    for (k, v) in headers {
        match &k[..] {
            b"tree" => c.tree = Some(v),
            b"parent" => c.parents.push(v),
            b"author" => c.author = Some(parse_time_entry(&v)?.0),
            b"committer" => c.commit_time = parse_time_entry(&v)?.1,
            _ => {}
        }
    }
    Ok(c)
}

fn commit_at(repo: &Repo, oid: &Oid) -> GitResult<Option<Commit>> {
    match repo.read(oid)? {
        Some(o) if o.kind == Kind::Commit => Ok(Some(parse_commit(&o.data)?)),
        _ => Ok(None),
    }
}

fn hex_oid(v: &[u8]) -> GitResult<Oid> {
    from_hex(v).ok_or_else(|| GitError::Invalid(format!("bad sha {:?}", String::from_utf8_lossy(v))))
}

fn commit_tree_oid(c: &Commit) -> GitResult<Oid> {
    hex_oid(c.tree.as_deref().ok_or_else(|| GitError::Invalid("commit without tree".into()))?)
}

/// `_commit_meta`.
#[derive(Clone, Debug, PartialEq)]
pub struct Meta {
    pub sha: String,
    pub parents: Vec<String>,
    pub author: String,
    pub message: String,
    pub time: i64,
    pub file_count: usize,
}

fn author_name(ident: &[u8]) -> String {
    let s = String::from_utf8_lossy(ident).into_owned();
    match s.find(" <") {
        Some(i) => s[..i].to_owned(),
        None => s,
    }
}

fn meta(repo: &Repo, oid: &Oid, c: &Commit) -> GitResult<Meta> {
    let mut file_count = 0;
    iter_tree(repo, &commit_tree_oid(c)?, |_| {
        file_count += 1;
        Ok(true)
    })?;
    let author = c.author.as_deref().ok_or_else(|| GitError::Invalid("commit without author".into()))?;
    let message = c.message.as_deref().ok_or_else(|| GitError::Invalid("commit without message".into()))?;
    Ok(Meta {
        sha: to_hex(oid),
        parents: c.parents.iter().map(|p| String::from_utf8_lossy(p).into_owned()).collect(),
        author: author_name(author),
        message: kiwi_core::pystr::strip(&String::from_utf8_lossy(message)).to_owned(),
        time: c.commit_time.ok_or_else(|| GitError::Invalid("commit without time".into()))?,
        file_count,
    })
}

fn branch_ref(name: &str) -> Vec<u8> {
    [b"refs/heads/", name.as_bytes()].concat()
}

/// `_resolve`: a branch name, else a 40-character commit id.
pub fn resolve(repo: &Repo, r: &str) -> GitResult<Option<Oid>> {
    let all = refs::all(&repo.dir)?;
    if let Some(v) = all.get(&branch_ref(r)) {
        return hex_oid(v).map(Some);
    }
    let cand = r.to_lowercase();
    if cand.len() != 40 {
        return Ok(None);
    }
    if cand == super::odb::ZERO_HEX {
        return Ok(None);
    }
    let oid = from_hex(cand.as_bytes()).ok_or_else(|| GitError::Invalid("Non-hexadecimal digit found".into()))?;
    match repo.read(&oid)? {
        Some(o) if o.kind == Kind::Commit => Ok(Some(oid)),
        _ => Ok(None),
    }
}

pub struct TreeEntryDto {
    pub path: String,
    pub blob_sha: String,
    pub size: usize,
}

/// `_read_tree`: the commit's meta and its files sorted by path.
pub fn read_tree(dir: &Path, r: &str) -> GitResult<Option<(Meta, Vec<TreeEntryDto>)>> {
    let Some(repo) = Repo::open(dir) else { return Ok(None) };
    let Some(oid) = resolve(&repo, r)? else { return Ok(None) };
    let c = commit_at(&repo, &oid)?.ok_or_else(|| GitError::Invalid("not a commit".into()))?;
    let mut entries = Vec::new();
    for e in tree_entries(&repo, &commit_tree_oid(&c)?)? {
        let size = repo.get(&e.sha)?.data.len();
        entries.push(TreeEntryDto {
            path: String::from_utf8_lossy(&e.path).into_owned(),
            blob_sha: to_hex(&e.sha),
            size,
        });
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Some((meta(&repo, &oid, &c)?, entries)))
}

/// `_read_blob`.
pub fn read_blob(dir: &Path, r: &str, path: &str) -> GitResult<Option<Vec<u8>>> {
    let Some(repo) = Repo::open(dir) else { return Ok(None) };
    let Some(oid) = resolve(&repo, r)? else { return Ok(None) };
    let c = commit_at(&repo, &oid)?.ok_or_else(|| GitError::Invalid("not a commit".into()))?;
    let mut hit = None;
    iter_tree(&repo, &commit_tree_oid(&c)?, |e| {
        if e.path == path.as_bytes() {
            hit = Some(e.sha);
            return Ok(false);
        }
        Ok(true)
    })?;
    match hit {
        Some(sha) => Ok(Some(repo.get(&sha)?.data)),
        None => Ok(None),
    }
}

/// `_list_branches`: `(name, head)` sorted by name.
pub fn list_branches(dir: &Path) -> GitResult<Vec<(String, String)>> {
    if Repo::open(dir).is_none() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for (name, sha) in refs::all(dir)? {
        if let Some(n) = name.strip_prefix(b"refs/heads/") {
            let n = String::from_utf8(n.to_vec()).map_err(|_| GitError::Invalid("branch name not UTF-8".into()))?;
            let h = String::from_utf8(sha).map_err(|_| GitError::Invalid("ref value not UTF-8".into()))?;
            out.push((n, h));
        }
    }
    out.sort();
    Ok(out)
}

/// dulwich's `Walker` over one head in date order (ties: lower id first),
/// reading five commits ahead of what it returns as dulwich does.
struct Walker<'a> {
    repo: &'a Repo,
    pq: BinaryHeap<(i64, Reverse<Oid>)>,
    commits: std::collections::HashMap<Oid, Commit>,
    queued: HashSet<Oid>,
    done: HashSet<Oid>,
    out: VecDeque<(Oid, Commit)>,
    finished: bool,
}

impl<'a> Walker<'a> {
    fn new(repo: &'a Repo, head: &Oid) -> GitResult<Self> {
        let mut w = Walker {
            repo,
            pq: BinaryHeap::new(),
            commits: Default::default(),
            queued: HashSet::new(),
            done: HashSet::new(),
            out: VecDeque::new(),
            finished: false,
        };
        w.push(head)?;
        Ok(w)
    }

    fn push(&mut self, oid: &Oid) -> GitResult<()> {
        let mut oid = *oid;
        let obj = loop {
            let obj =
                self.repo.read(&oid)?.ok_or_else(|| GitError::Invalid(format!("missing commit {}", to_hex(&oid))))?;
            if obj.kind != Kind::Tag {
                break obj;
            }
            let target = obj
                .data
                .split(|&b| b == b'\n')
                .find_map(|l| l.strip_prefix(b"object "))
                .ok_or_else(|| GitError::Invalid("tag without object".into()))?;
            oid = hex_oid(target)?;
        };
        if obj.kind != Kind::Commit {
            return Ok(());
        }
        if !self.queued.contains(&oid) && !self.done.contains(&oid) {
            let c = parse_commit(&obj.data)?;
            let t = c.commit_time.ok_or_else(|| GitError::Invalid("commit without time".into()))?;
            self.pq.push((t, Reverse(oid)));
            self.queued.insert(oid);
            self.commits.insert(oid, c);
        }
        Ok(())
    }

    fn step(&mut self) -> GitResult<Option<(Oid, Commit)>> {
        while let Some((_, Reverse(oid))) = self.pq.pop() {
            self.queued.remove(&oid);
            if self.done.contains(&oid) {
                continue;
            }
            self.done.insert(oid);
            let c = self.commits.remove(&oid).unwrap_or_default();
            for p in &c.parents {
                self.push(&hex_oid(p)?)?;
            }
            return Ok(Some((oid, c)));
        }
        self.finished = true;
        Ok(None)
    }

    fn next_entry(&mut self) -> GitResult<Option<(Oid, Commit)>> {
        loop {
            let e = if self.finished { None } else { self.step()? };
            let ended = e.is_none();
            if let Some(e) = e {
                self.out.push_back(e);
            }
            if ended || self.out.len() > MAX_EXTRA_COMMITS {
                return Ok(self.out.pop_front());
            }
        }
    }
}

/// `_list_commits`: up to `limit` metas, newest first.
pub fn list_commits(dir: &Path, r: &str, limit: usize) -> GitResult<Vec<Meta>> {
    let Some(repo) = Repo::open(dir) else { return Ok(Vec::new()) };
    let Some(head) = resolve(&repo, r)? else { return Ok(Vec::new()) };
    let mut w = Walker::new(&repo, &head)?;
    let mut out = Vec::new();
    while out.len() < limit {
        let Some((oid, c)) = w.next_entry()? else { break };
        out.push(meta(&repo, &oid, &c)?);
    }
    Ok(out)
}

/// `_count_commits`.
pub fn count_commits(dir: &Path, r: &str) -> GitResult<usize> {
    let Some(repo) = Repo::open(dir) else { return Ok(0) };
    let Some(head) = resolve(&repo, r)? else { return Ok(0) };
    let mut w = Walker::new(&repo, &head)?;
    let mut n = 0;
    while w.next_entry()?.is_some() {
        n += 1;
    }
    Ok(n)
}

/// `author_ident(username)`.
pub fn author_ident(username: &str) -> Vec<u8> {
    let name = if username.is_empty() { "modder" } else { username };
    let mut safe: String =
        name.chars().filter(|&c| kiwi_core::pystr::is_alnum(c) || matches!(c, '-' | '_' | '.')).collect();
    if safe.is_empty() {
        safe = "modder".into();
    }
    format!("{username} <{safe}@{AUTHOR_DOMAIN}>").into_bytes()
}

enum Node {
    Dir,
    File(u32, Oid),
}

fn pathsplit(p: &[u8]) -> (&[u8], &[u8]) {
    match p.iter().rposition(|&c| c == b'/') {
        Some(i) => (&p[..i], &p[i + 1..]),
        None => (b"", p),
    }
}

fn pathjoin(a: &[u8], b: &[u8]) -> Vec<u8> {
    match (a.is_empty(), b.is_empty()) {
        (true, _) => b.to_vec(),
        (_, true) => a.to_vec(),
        _ => [a, b"/", b].concat(),
    }
}

/// dulwich `commit_tree`: nested trees from flat `(path, sha, mode)` entries,
/// each written as it is built (children first).
pub fn commit_tree(repo: &Repo, blobs: &[(Vec<u8>, Oid, u32)]) -> GitResult<Oid> {
    let mut trees: IndexMap<Vec<u8>, IndexMap<Vec<u8>, Node>> = IndexMap::new();
    trees.insert(Vec::new(), IndexMap::new());
    fn add_tree(trees: &mut IndexMap<Vec<u8>, IndexMap<Vec<u8>, Node>>, path: &[u8]) {
        if trees.contains_key(path) {
            return;
        }
        let (dirname, basename) = pathsplit(path);
        let (dirname, basename) = (dirname.to_vec(), basename.to_vec());
        add_tree(trees, &dirname);
        if let Some(t) = trees.get_mut(&dirname) {
            t.insert(basename, Node::Dir);
        }
        trees.insert(path.to_vec(), IndexMap::new());
    }
    for (path, sha, mode) in blobs {
        let (tree_path, basename) = pathsplit(path);
        add_tree(&mut trees, tree_path);
        if let Some(t) = trees.get_mut(tree_path) {
            t.insert(basename.to_vec(), Node::File(*mode, *sha));
        }
    }
    fn build(repo: &Repo, trees: &IndexMap<Vec<u8>, IndexMap<Vec<u8>, Node>>, path: &[u8]) -> GitResult<Oid> {
        let mut items: Vec<(Vec<u8>, u32, Oid)> = Vec::new();
        for (name, node) in trees.get(path).into_iter().flatten() {
            match node {
                Node::Dir => items.push((name.clone(), S_IFDIR, build(repo, trees, &pathjoin(path, name))?)),
                Node::File(mode, sha) => items.push((name.clone(), *mode, *sha)),
            }
        }
        items.sort_by(|a, b| {
            let key = |e: &(Vec<u8>, u32, Oid)| if is_dir(e.1) { [&e.0[..], b"/"].concat() } else { e.0.clone() };
            key(a).cmp(&key(b))
        });
        let mut data = Vec::new();
        for (name, mode, sha) in &items {
            data.extend_from_slice(format!("{mode:04o} ").as_bytes());
            data.extend_from_slice(name);
            data.push(0);
            data.extend_from_slice(sha);
        }
        repo.write(Kind::Tree, &data)
    }
    build(repo, &trees, b"")
}

fn header_line(out: &mut Vec<u8>, field: &[u8], value: &[u8]) {
    let mut lines = value.split(|&c| c == b'\n');
    out.extend_from_slice(field);
    out.push(b' ');
    out.extend_from_slice(lines.next().unwrap_or_default());
    out.push(b'\n');
    for l in lines {
        out.push(b' ');
        out.extend_from_slice(l);
        out.push(b'\n');
    }
}

/// `Commit._serialize` for what the store writes: UTC times, UTF-8 encoding header.
pub fn serialize_commit(tree: &Oid, parents: &[Oid], author: &[u8], ts: i64, message: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    header_line(&mut out, b"tree", to_hex(tree).as_bytes());
    for p in parents {
        header_line(&mut out, b"parent", to_hex(p).as_bytes());
    }
    let who = [author, format!(" {ts} +0000").as_bytes()].concat();
    header_line(&mut out, b"author", &who);
    header_line(&mut out, b"committer", &who);
    header_line(&mut out, b"encoding", b"UTF-8");
    out.push(b'\n');
    out.extend_from_slice(message);
    out
}

/// `_write_commit`: the parent tree, minus `deletes`, plus `adds` (mode
/// 100644), committed on `branch`.
pub fn write_commit(
    dir: &Path,
    branch: &str,
    adds: &[(String, Vec<u8>)],
    deletes: &[String],
    author: &[u8],
    message: &str,
    ts: i64,
) -> Result<String, StoreError> {
    let repo = Repo::ensure(dir)?;
    let r = branch_ref(branch);
    let parent = match refs::all(dir)?.get(&r) {
        Some(v) => Some(hex_oid(v)?),
        None => None,
    };
    let mut entries: IndexMap<Vec<u8>, (u32, Oid)> = IndexMap::new();
    let mut parent_tree = None;
    if let Some(p) = &parent {
        let c = commit_at(&repo, p)?.ok_or_else(|| GitError::Invalid("parent is not a commit".into()))?;
        let t = commit_tree_oid(&c)?;
        for e in tree_entries(&repo, &t)? {
            entries.insert(e.path, (e.mode, e.sha));
        }
        parent_tree = Some(t);
    }
    for d in deletes {
        entries.shift_remove(d.as_bytes());
    }
    for (path, content) in adds {
        let sha = repo.write(Kind::Blob, content)?;
        entries.insert(path.as_bytes().to_vec(), (0o100644, sha));
    }
    let blobs: Vec<(Vec<u8>, Oid, u32)> = entries.into_iter().map(|(p, (m, s))| (p, s, m)).collect();
    let tree = commit_tree(&repo, &blobs)?;
    if parent_tree == Some(tree) {
        return Err(StoreError::NothingToCommit);
    }
    let msg = if message.is_empty() { b"(no message)".as_slice() } else { message.as_bytes() };
    let parents: Vec<Oid> = parent.into_iter().collect();
    let data = serialize_commit(&tree, &parents, author, ts, msg);
    let oid = repo.write(Kind::Commit, &data)?;
    refs::set(dir, &r, to_hex(&oid).as_bytes())?;
    Ok(to_hex(&oid))
}

/// `_create_branch`: at `start_ref`, else at the first ref there is (the
/// Python set order is arbitrary; this takes the first by name).
pub fn create_branch(dir: &Path, name: &str, start_ref: &str) -> Result<String, StoreError> {
    let repo = Repo::ensure(dir)?;
    let mut start = if start_ref.is_empty() { None } else { resolve(&repo, start_ref)? };
    if start.is_none() {
        start = refs::all(dir)?.into_values().next().map(|v| hex_oid(&v)).transpose()?;
    }
    let Some(start) = start else {
        return Err(StoreError::Refused("Commit something before creating a branch.".into()));
    };
    refs::set(dir, &branch_ref(name), to_hex(&start).as_bytes())?;
    Ok(to_hex(&start))
}

/// `_delete_branch`.
pub fn delete_branch(dir: &Path, name: &str) -> GitResult<()> {
    if Repo::open(dir).is_none() {
        return Ok(());
    }
    refs::remove_if_equals(dir, &branch_ref(name), None).map(|_| ())
}

/// `_fork`: the source branch's files as one parentless commit.
pub fn fork(
    src: &Path,
    dst: &Path,
    src_branch: &str,
    branch: &str,
    author: &[u8],
    message: &str,
    ts: i64,
) -> Result<Option<String>, StoreError> {
    let dst_repo = Repo::ensure(dst)?;
    let Some(src_repo) = Repo::open(src) else { return Ok(None) };
    let Some(head) = resolve(&src_repo, src_branch)? else { return Ok(None) };
    let c = commit_at(&src_repo, &head)?.ok_or_else(|| GitError::Invalid("not a commit".into()))?;
    let mut blobs = Vec::new();
    for e in tree_entries(&src_repo, &commit_tree_oid(&c)?)? {
        let obj = src_repo.get(&e.sha)?;
        dst_repo.write(obj.kind, &obj.data)?;
        blobs.push((e.path, e.sha, e.mode));
    }
    let tree = commit_tree(&dst_repo, &blobs)?;
    let msg = if message.is_empty() { "fork" } else { message };
    let data = serialize_commit(&tree, &[], author, ts, msg.as_bytes());
    let oid = dst_repo.write(Kind::Commit, &data)?;
    refs::set(dst, &branch_ref(branch), to_hex(&oid).as_bytes())?;
    Ok(Some(to_hex(&oid)))
}

/// `_delete_repo`.
pub fn delete_repo(dir: &Path) {
    if dir.exists() {
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// `_blocked_in_commit`: the first `*.tmod` (any case) in a commit's tree.
pub fn blocked_in_commit(repo: &Repo, oid: &Oid) -> GitResult<Option<String>> {
    let Some(c) = commit_at(repo, oid)? else { return Ok(None) };
    let Some(tree) = c.tree.as_deref().and_then(from_hex) else { return Ok(None) };
    let mut hit = None;
    iter_tree(repo, &tree, |e| {
        let path = String::from_utf8_lossy(&e.path).into_owned();
        if path.to_lowercase().ends_with(".tmod") {
            hit = Some(path);
            return Ok(false);
        }
        Ok(true)
    })?;
    Ok(hit)
}
