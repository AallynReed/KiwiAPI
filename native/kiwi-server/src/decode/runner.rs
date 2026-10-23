//! `app/trove/decode/runner.py`: rebuild the decoded game data after each patch
//! and report how it went. A decoder runs when the patch it last tried differs
//! from the current one, or its code changed; one that fails, decodes nothing
//! or shrinks sharply keeps the previous file, and the failure lands in the dev
//! panel's Game data tab.
//!
//! The lock (`kiwi:decode:running`), the `decoder_runs` rows and the output
//! files are shared with Python's runner, the fallback. Unlike Python's, this
//! lock is renewed while a run lasts.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use serde_json::{Value, json};

use super::{
    json::J,
    registry::{self, DECODERS, DecodeError, Decoder},
    tree::{Archive, Tree},
};
use crate::{auth::pyutil::bson_int, config::Settings, dt, gamedata::Gamedata, state::AppState};

pub const LOCK_KEY: &str = "kiwi:decode:running";
pub const LOCK_TTL: i64 = 1800;
pub const SHRINK_LIMIT: f64 = 0.8;
pub const COLLECTION: &str = "decoder_runs";

/// One `DecoderRun` document.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    pub id: ObjectId,
    pub name: String,
    pub trigger: String,
    pub ordinal: i64,
    pub version_tag: Option<String>,
    pub code_version: String,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub ok: Option<bool>,
    pub entries: Option<i64>,
    pub error: Option<String>,
    pub traceback: Option<String>,
}

fn opt_str(v: &Option<String>) -> Bson {
    v.as_ref().map_or(Bson::Null, |s| Bson::String(s.clone()))
}

impl Run {
    /// Beanie's document: every model field, model order, explicit nulls.
    pub fn to_doc(&self) -> Document {
        doc! {
            "_id": self.id,
            "name": &self.name,
            "trigger": &self.trigger,
            "ordinal": bson_int(self.ordinal),
            "version_tag": opt_str(&self.version_tag),
            "code_version": &self.code_version,
            "started_at": dt::to_bson(self.started_at),
            "finished_at": self.finished_at.map_or(Bson::Null, |d| Bson::DateTime(dt::to_bson(d))),
            "ok": self.ok.map_or(Bson::Null, Bson::Boolean),
            "entries": self.entries.map_or(Bson::Null, bson_int),
            "error": opt_str(&self.error),
            "traceback": opt_str(&self.traceback),
        }
    }

    /// A stored row, `None` where Beanie's validation would raise.
    pub fn from_doc(d: &Document) -> Option<Run> {
        let opt_s = |k: &str| match d.get(k) {
            None | Some(Bson::Null) => Some(None),
            Some(Bson::String(s)) => Some(Some(s.clone())),
            _ => None,
        };
        let int = |b: &Bson| match b {
            Bson::Int32(i) => Some(i64::from(*i)),
            Bson::Int64(i) => Some(*i),
            Bson::Double(f) if f.fract() == 0.0 => Some(*f as i64),
            _ => None,
        };
        let date = |b: &Bson| match b {
            Bson::DateTime(t) => Some(dt::from_bson(*t)),
            _ => None,
        };
        Some(Run {
            id: d.get_object_id("_id").ok()?,
            name: d.get_str("name").ok()?.to_owned(),
            trigger: d.get_str("trigger").ok()?.to_owned(),
            ordinal: int(d.get("ordinal")?)?,
            version_tag: opt_s("version_tag")?,
            code_version: d.get_str("code_version").ok()?.to_owned(),
            started_at: date(d.get("started_at")?)?,
            finished_at: match d.get("finished_at") {
                None | Some(Bson::Null) => None,
                Some(b) => Some(date(b)?),
            },
            ok: match d.get("ok") {
                None | Some(Bson::Null) => None,
                Some(Bson::Boolean(b)) => Some(*b),
                Some(_) => return None,
            },
            entries: match d.get("entries") {
                None | Some(Bson::Null) => None,
                Some(b) => Some(int(b)?),
            },
            error: opt_s("error")?,
            traceback: opt_s("traceback")?,
        })
    }
}

pub fn interrupted(run: &Run, now: DateTime<Utc>) -> bool {
    run.finished_at.is_none() && now - run.started_at > chrono::Duration::seconds(LOCK_TTL)
}

pub fn enabled(settings: &Settings) -> bool {
    !settings.gamedata_dir.is_empty()
}

fn baseline(settings: &Settings) -> PathBuf {
    Gamedata::global(settings).baseline_dir().to_path_buf()
}

/// `(UpdateBranch.current_ordinal, current_version)`, `(0, None)` without one.
pub async fn patch(state: &AppState, branch: &str) -> anyhow::Result<(i64, Option<String>)> {
    let Some(d) = state.mongo().collection::<Document>("update_branches").find_one(doc! {"branch": branch}).await?
    else {
        return Ok((0, None));
    };
    let ordinal = match d.get("current_ordinal") {
        Some(Bson::Int32(i)) => i64::from(*i),
        Some(Bson::Int64(i)) => *i,
        _ => 0,
    };
    Ok((ordinal, d.get_str("current_version").ok().map(str::to_owned)))
}

pub async fn latest(state: &AppState, name: &str, ok: Option<bool>) -> anyhow::Result<Option<Run>> {
    let mut filter = doc! {"name": name};
    if let Some(ok) = ok {
        filter.insert("ok", ok);
    }
    let found = state.mongo().collection::<Document>(COLLECTION).find_one(filter).sort(doc! {"started_at": -1}).await?;
    match found {
        None => Ok(None),
        Some(d) => {
            Run::from_doc(&d).map(Some).ok_or_else(|| anyhow::anyhow!("decoder_runs row for {name} doesn't validate"))
        }
    }
}

async fn needs_run(state: &AppState, d: &Decoder, ordinal: i64) -> anyhow::Result<bool> {
    let Some(last) = latest(state, d.name, None).await? else { return Ok(true) };
    if interrupted(&last, dt::now()) {
        return Ok(true);
    }
    Ok(last.ordinal != ordinal || last.code_version != d.code_version)
}

/// The post-sync hook: rebuild whatever is out of date for the live branch.
/// The update archiver calls it after each branch sync (Python's
/// `worker._sync_all_branches`).
pub async fn ensure_built(state: &AppState, branch: &str) -> anyhow::Result<()> {
    let settings = state.settings();
    if branch != settings.trove_render_branch || !enabled(settings) {
        return Ok(());
    }
    let (ordinal, _) = patch(state, branch).await?;
    if ordinal == 0 {
        return Ok(());
    }
    let mut todo = Vec::new();
    for d in DECODERS {
        if needs_run(state, d, ordinal).await? {
            todo.push(d.name);
        }
    }
    if !todo.is_empty() {
        run(state, &todo, "patch", false).await?;
    }
    Ok(())
}

pub async fn is_running(state: &AppState) -> anyhow::Result<bool> {
    let Some(mut r) = state.inner.db.redis.clone() else { return Ok(false) };
    let n: i64 = redis::cmd("EXISTS").arg(LOCK_KEY).query_async(&mut r).await?;
    Ok(n > 0)
}

fn token() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("os random");
    hex::encode(b)
}

const RENEW: &str =
    "if redis.call('GET', KEYS[1]) == ARGV[1] then return redis.call('EXPIRE', KEYS[1], ARGV[2]) end return 0";
const RELEASE: &str = "if redis.call('GET', KEYS[1]) == ARGV[1] then return redis.call('DEL', KEYS[1]) end return 0";

/// Load the archive tree for `prefixes` (`ArchiveTree.load`).
pub async fn archive_tree(state: &AppState, branch: &str, prefixes: &[&str]) -> anyhow::Result<Tree> {
    let coll = state.mongo().collection::<Document>("update_state");
    let mut shas: HashMap<String, String> = HashMap::new();
    let mut paths: Vec<String> = Vec::new();
    for prefix in prefixes {
        let filter = doc! {"branch": branch, "path": {"$gte": *prefix, "$lt": format!("{prefix}\u{ffff}")}};
        let mut cur = coll.find(filter).projection(doc! {"path": 1, "content_sha256": 1, "_id": 0}).await?;
        while let Some(row) = cur.try_next().await? {
            let (Ok(path), Ok(sha)) = (row.get_str("path"), row.get_str("content_sha256")) else { continue };
            if shas.insert(path.to_owned(), sha.to_owned()).is_none() {
                paths.push(path.to_owned());
            }
        }
    }
    let root = PathBuf::from(&state.settings().trove_update_store_dir);
    Ok(Tree::new(paths, Arc::new(Archive { shas, root })))
}

/// Run `names` one after another; `false` when a run is already going.
pub async fn run(state: &AppState, names: &[&str], trigger: &str, allow_shrink: bool) -> anyhow::Result<bool> {
    let redis = state.inner.db.redis.clone();
    let tok = token();
    if let Some(mut r) = redis.clone() {
        let got: Option<String> =
            redis::cmd("SET").arg(LOCK_KEY).arg(&tok).arg("NX").arg("EX").arg(LOCK_TTL).query_async(&mut r).await?;
        if got.is_none() {
            return Ok(false);
        }
    }
    let renew = redis.clone().map(|mut r| {
        let tok = tok.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(600)).await;
                let res: redis::RedisResult<i64> =
                    redis::Script::new(RENEW).key(LOCK_KEY).arg(&tok).arg(LOCK_TTL).invoke_async(&mut r).await;
                if !matches!(res, Ok(1)) {
                    return;
                }
            }
        })
    });
    let result = run_all(state, names, trigger, allow_shrink).await;
    if let Some(h) = renew {
        h.abort();
    }
    if let Some(mut r) = redis {
        let _: redis::RedisResult<i64> = redis::Script::new(RELEASE).key(LOCK_KEY).arg(&tok).invoke_async(&mut r).await;
    }
    result.map(|()| true)
}

async fn run_all(state: &AppState, names: &[&str], trigger: &str, allow_shrink: bool) -> anyhow::Result<()> {
    let settings = state.settings();
    let branch = settings.trove_render_branch.clone();
    let (ordinal, tag) = patch(state, &branch).await?;
    let mut prefixes: Vec<&str> =
        names.iter().filter_map(|n| registry::get(n)).flat_map(|d| d.prefixes.iter().copied()).collect();
    prefixes.sort();
    prefixes.dedup();
    let tree = archive_tree(state, &branch, &prefixes).await?;
    for name in names {
        if let Some(d) = registry::get(name) {
            run_one(state, d, &tree, ordinal, tag.clone(), trigger, allow_shrink).await?;
        }
    }
    Ok(())
}

/// Entries in the file being replaced: the last good rebuild, else the repo copy.
async fn previous_count(state: &AppState, d: &Decoder) -> anyhow::Result<Option<i64>> {
    if let Some(last) = latest(state, d.name, Some(true)).await?
        && let Some(n) = last.entries
    {
        return Ok(Some(n));
    }
    let path = baseline(state.settings()).join(d.output);
    let Some(v) = std::fs::read(path).ok().and_then(|b| crate::pyjson::loads(&b).ok()) else { return Ok(None) };
    Ok((d.count)(&super::json::from_serde(&v)).map(|n| n as i64))
}

/// Build on a thread of its own: the readers recurse, and a decoder can take
/// seconds of CPU.
async fn build(d: &'static Decoder, tree: Tree, baseline: PathBuf) -> Result<(String, usize), DecodeError> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let spawned =
        std::thread::Builder::new().name(format!("decode-{}", d.name)).stack_size(64 << 20).spawn(move || {
            let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let data: J = d.run(&tree, &baseline)?;
                let n = (d.count)(&data).ok_or_else(|| DecodeError::key("count"))?;
                Ok((d.dump(&data), n))
            }));
            let _ = tx.send(out.unwrap_or_else(|p| {
                let msg =
                    p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| (*s).to_owned()));
                Err(DecodeError { kind: "RuntimeError", msg: format!("decoder panicked: {}", msg.unwrap_or_default()) })
            }));
        });
    if spawned.is_err() {
        return Err(DecodeError { kind: "RuntimeError", msg: "could not start the decoder thread".into() });
    }
    rx.await.unwrap_or_else(|_| Err(DecodeError { kind: "RuntimeError", msg: "decoder thread vanished".into() }))
}

async fn run_one(
    state: &AppState,
    d: &'static Decoder,
    tree: &Tree,
    ordinal: i64,
    tag: Option<String>,
    trigger: &str,
    allow_shrink: bool,
) -> anyhow::Result<()> {
    let settings = state.settings();
    let coll = state.mongo().collection::<Document>(COLLECTION);
    let mut rec = Run {
        id: ObjectId::new(),
        name: d.name.to_owned(),
        trigger: trigger.to_owned(),
        ordinal,
        version_tag: tag,
        code_version: d.code_version.to_owned(),
        started_at: dt::now(),
        finished_at: None,
        ok: None,
        entries: None,
        error: None,
        traceback: None,
    };
    coll.insert_one(rec.to_doc()).await?;
    let outcome: Result<usize, DecodeError> = async {
        let (text, n) = build(d, tree.clone(), baseline(settings)).await?;
        if n == 0 {
            return Err(DecodeError::value("decoded nothing - the game files may have moved or changed format"));
        }
        let prev = previous_count(state, d).await.map_err(|e| DecodeError { kind: "RuntimeError", msg: e.to_string() })?;
        if let Some(prev) = prev.filter(|p| *p != 0)
            && (n as f64) < prev as f64 * SHRINK_LIMIT
            && !allow_shrink
        {
            return Err(DecodeError::value(format!(
                "output shrank from {prev} to {n} entries; kept the previous data. If the game really removed them, run it again with \"Accept smaller output\"."
            )));
        }
        let target = Path::new(&settings.gamedata_dir).join(d.output);
        tokio::task::spawn_blocking(move || write(&target, &text))
            .await
            .map_err(|e| DecodeError { kind: "RuntimeError", msg: e.to_string() })?
            .map_err(|e| DecodeError { kind: "OSError", msg: e.to_string() })?;
        Ok(n)
    }
    .await;
    match outcome {
        Ok(n) => {
            rec.ok = Some(true);
            rec.entries = Some(n as i64);
            tracing::info!("decode[{}]: wrote {n} entries from ordinal {ordinal}", d.name);
        }
        Err(e) => {
            rec.ok = Some(false);
            rec.error = Some(e.to_string());
            rec.traceback = Some(format!("Traceback (kiwi-server, decoder {}):\n{e}\n", d.name));
            tracing::error!("decode[{}] failed: {e}", d.name);
        }
    }
    rec.finished_at = Some(dt::now());
    coll.update_one(doc! {"_id": rec.id}, doc! {"$set": rec.to_doc()}).upsert(true).await?;
    Ok(())
}

/// `store.write`: replace `target` atomically, readable by the other containers.
pub fn write(target: &Path, text: &str) -> std::io::Result<()> {
    let dir = target.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = target.file_name().and_then(|n| n.to_str()).unwrap_or("out");
    let tmp = dir.join(format!(".{name}.{}", token()));
    let res = (|| {
        std::fs::write(&tmp, text.as_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644))?;
        }
        std::fs::rename(&tmp, target)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

/// `_run_dto`.
pub fn run_dto(r: Option<&Run>) -> Value {
    let Some(r) = r else { return Value::Null };
    let duration = r.finished_at.map(|f| {
        let us = (f - r.started_at).num_microseconds().unwrap_or(0);
        ((us as f64 / 1_000_000.0) * 1000.0).trunc() as i64
    });
    json!({
        "trigger": r.trigger, "ordinal": r.ordinal, "version_tag": r.version_tag,
        "code_version": r.code_version, "ok": r.ok, "entries": r.entries,
        "error": r.error, "traceback": r.traceback,
        "started_at": dt::isoformat(&r.started_at),
        "finished_at": r.finished_at.as_ref().map(dt::isoformat),
        "duration_ms": duration,
    })
}

pub fn state_of(last: Option<&Run>, now: DateTime<Utc>) -> &'static str {
    match last {
        None => "never",
        Some(r) if r.finished_at.is_none() => {
            if interrupted(r, now) {
                "interrupted"
            } else {
                "running"
            }
        }
        Some(r) if r.ok == Some(true) => "ok",
        Some(_) => "failed",
    }
}

/// The Game data tab's status (`runner.status()`).
pub async fn status(state: &AppState) -> anyhow::Result<Value> {
    let settings = state.settings();
    let (ordinal, tag) = patch(state, &settings.trove_render_branch).await?;
    let store = Gamedata::global(settings);
    let now = dt::now();
    let mut rows = Vec::new();
    for d in DECODERS {
        let last = latest(state, d.name, None).await?;
        let good = match &last {
            Some(l) if l.ok == Some(true) => Some(l.clone()),
            _ => latest(state, d.name, Some(true)).await?,
        };
        let serving = if store.path(d.output) != store.baseline_dir().join(d.output) { "rebuild" } else { "repo copy" };
        rows.push(json!({
            "name": d.name, "title": d.title, "output": d.output,
            "state": state_of(last.as_ref(), now), "code_version": d.code_version,
            "serving": serving, "last": run_dto(last.as_ref()), "last_ok": run_dto(good.as_ref()),
        }));
    }
    Ok(json!({
        "enabled": enabled(settings), "running": is_running(state).await?,
        "branch": settings.trove_render_branch, "ordinal": ordinal, "version_tag": tag,
        "decoders": rows,
    }))
}
