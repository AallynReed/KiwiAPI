//! Full-tree parity harness: the same rows `codexes_tree_dump.py` captures from
//! the Python indexer, from an extracted game tree. Run with
//! `KIWI_CODEX_TREE=E:/Trove KIWI_CODEX_EXE=.../Trove_x64.exe KIWI_CODEX_OUT=rs.jsonl
//! cargo test -p kiwi-server --release codexes::tree_tests -- --ignored`, then
//! `codexes_tree_diff.py py.jsonl rs.jsonl`.

use std::{io::Write, path::PathBuf};

use kiwi_core::codex::{extract::Blobs, types::classify};
use serde_json::{Value, json};

use super::{
    archive::{Archive, Change},
    indexer::{self, Job},
    pg::P,
    store,
};

const EXE: &str = "Trove_x64.exe";

#[derive(Clone)]
struct DirBlobs {
    root: PathBuf,
    exe: Option<PathBuf>,
}

impl Blobs for DirBlobs {
    fn get(&self, sha: &str) -> Option<Vec<u8>> {
        let p = if sha == EXE { self.exe.clone()? } else { self.root.join(sha) };
        if p.is_file() { std::fs::read(p).ok() } else { None }
    }
}

struct DirArchive {
    blobs: DirBlobs,
    paths: Vec<String>,
}

fn walk(dir: &std::path::Path, root: &std::path::Path, out: &mut Vec<String>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, root, out);
        } else if let Ok(rel) = p.strip_prefix(root) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

impl DirArchive {
    fn new(root: PathBuf, exe: Option<PathBuf>) -> Self {
        let mut paths = Vec::new();
        for top in ["prefabs", "languages", "blueprints"] {
            walk(&root.join(top), &root, &mut paths);
        }
        if exe.is_some() {
            paths.push(EXE.into());
        }
        paths.sort();
        DirArchive { blobs: DirBlobs { root, exe }, paths }
    }
}

impl Archive for DirArchive {
    type Blobs = DirBlobs;

    fn blobs(&self) -> DirBlobs {
        self.blobs.clone()
    }

    async fn prefix(&self, prefix: &str) -> anyhow::Result<Vec<(String, Option<String>)>> {
        Ok(self.paths.iter().filter(|p| p.starts_with(prefix)).map(|p| (p.clone(), Some(p.clone()))).collect())
    }

    async fn regex(&self, pattern: &str, ci: bool) -> anyhow::Result<Vec<(String, Option<String>)>> {
        let re = regex::RegexBuilder::new(pattern).case_insensitive(ci).build()?;
        Ok(self.paths.iter().filter(|p| re.is_match(p)).map(|p| (p.clone(), Some(p.clone()))).collect())
    }

    async fn file_sha(&self, path: &str) -> anyhow::Result<Option<String>> {
        Ok(self.paths.binary_search_by(|p| p.as_str().cmp(path)).ok().map(|_| path.to_owned()))
    }

    async fn changes(&self, _: i64) -> anyhow::Result<Vec<Change>> {
        Ok(Vec::new())
    }
}

/// A write argument as the JSON the Python harness dumps.
fn arg_json(p: &P) -> Option<Value> {
    Some(match p {
        P::Null => Value::Null,
        P::Bool(b) => json!(b),
        P::Int(i) => json!(i),
        P::Float(f) => json!(f),
        P::Text(s) => json!(s),
        P::Json(s) => crate::pyjson::loads(s.as_bytes()).expect("json arg"),
        P::TextList(v) => json!(v),
        P::Ts(_) => return None,
    })
}

fn line(out: &mut impl Write, kind: &str, args: &[P]) {
    let row: Vec<Value> = args.iter().filter_map(arg_json).collect();
    writeln!(out, "{}", crate::pyjson::dumps(&json!({"t": kind, "row": row}))).expect("write");
}

/// The delta path (`codexes_tree_dump.py --delta`): every collection prefab
/// modified in one version, so the maps load without the recipe resolver.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs an extracted game tree (KIWI_CODEX_TREE)"]
async fn dump_delta() {
    let root = PathBuf::from(std::env::var("KIWI_CODEX_TREE").expect("KIWI_CODEX_TREE"));
    let exe = std::env::var("KIWI_CODEX_EXE").ok().map(PathBuf::from);
    let out_path = std::env::var("KIWI_CODEX_OUT").expect("KIWI_CODEX_OUT");
    let a = DirArchive::new(root, exe);
    let branch = "live-us";
    let jobs: Vec<Job> = a
        .prefix("prefabs/collections/")
        .await
        .unwrap()
        .into_iter()
        .filter_map(|(path, sha)| classify(&path).map(|ctype| Job { path, sha: sha.unwrap(), ctype }))
        .collect();
    assert!(jobs.iter().all(|j| j.ctype != "recipe"));
    let maps = indexer::load_maps(&a, false, "").await.expect("maps");
    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("out"));
    let now = chrono::Utc::now();
    for (job, r) in jobs.iter().zip(indexer::parse_jobs(&maps, &jobs)) {
        let Some(p) = r.expect("parse") else { continue };
        line(&mut out, "entry", &store::entry_args(branch, &p, &job.sha, now));
        for s in &p.stats {
            line(&mut out, "stat", &store::stat_args(branch, s));
        }
        for x in &p.abilities {
            line(&mut out, "ability", &store::ability_args(branch, x));
        }
    }
    let shared = indexer::shared_tables(&a, branch, &maps.loc).await.expect("shared");
    for r in shared.requirements.iter().flatten() {
        line(&mut out, "requirement", &store::requirement_args(branch, r));
    }
    for u in &shared.upgrades {
        line(&mut out, "upgrade", u);
    }
    for l in shared.cost_edges.iter().chain(shared.unlock_edges.iter().flatten()).chain(&shared.member_edges) {
        line(&mut out, "link", &store::link_args(branch, l));
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs an extracted game tree (KIWI_CODEX_TREE)"]
async fn dump_tree() {
    let root = PathBuf::from(std::env::var("KIWI_CODEX_TREE").expect("KIWI_CODEX_TREE"));
    let exe = std::env::var("KIWI_CODEX_EXE").ok().map(PathBuf::from);
    let out_path = std::env::var("KIWI_CODEX_OUT").expect("KIWI_CODEX_OUT");
    let a = DirArchive::new(root, exe);
    let branch = "live-us";
    let maps = indexer::load_maps(&a, true, "").await.expect("maps");
    let jobs: Vec<Job> = a
        .prefix("prefabs/")
        .await
        .unwrap()
        .into_iter()
        .filter_map(|(path, sha)| classify(&path).map(|ctype| Job { path, sha: sha.unwrap(), ctype }))
        .collect();
    let results = indexer::parse_jobs(&maps, &jobs);
    let mut out = std::io::BufWriter::new(std::fs::File::create(&out_path).expect("out"));
    let now = chrono::Utc::now();
    for (job, r) in jobs.iter().zip(results) {
        let Some(p) = r.expect("parse") else { continue };
        line(&mut out, "entry", &store::entry_args(branch, &p, &job.sha, now));
        for s in &p.stats {
            line(&mut out, "stat", &store::stat_args(branch, s));
        }
        for x in &p.abilities {
            line(&mut out, "ability", &store::ability_args(branch, x));
        }
        for l in &p.links {
            line(&mut out, "link", &store::link_args(branch, l));
        }
    }
    let shared = indexer::shared_tables(&a, branch, &maps.loc).await.expect("shared");
    for r in shared.requirements.iter().flatten() {
        line(&mut out, "requirement", &store::requirement_args(branch, r));
    }
    for u in &shared.upgrades {
        line(&mut out, "upgrade", u);
    }
    for l in shared.cost_edges.iter().chain(shared.unlock_edges.iter().flatten()).chain(&shared.member_edges) {
        line(&mut out, "link", &store::link_args(branch, l));
    }
    let candidates: Vec<(String, String)> =
        a.prefix("prefabs/").await.unwrap().into_iter().map(|(p, s)| (p, s.unwrap())).collect();
    for (prefab, bp, sk, ap, ms, hs) in indexer::rig_rows(&a.blobs(), &candidates) {
        line(
            &mut out,
            "rig",
            &[P::text(branch), P::Text(prefab), P::Text(bp), P::Text(sk), P::Text(ap), P::Float(ms), P::Float(hs)],
        );
    }
}
