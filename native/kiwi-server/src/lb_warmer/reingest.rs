//! `backlog.reingest`: replay every backlog dump oldest-first as a pure insert,
//! then the activity and class-activity replays once, then wake the warmer.
//! Same single-flight lock and status key as Python.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use super::guarded;
use crate::{
    activity,
    lb_ingest::{
        backlog::{self, RUNNING_KEY, STATUS_KEY},
        ingest::{self, Outcome},
        signals, store,
    },
    pyjson,
    state::AppState,
};

pub const LOCK_TTL: i64 = 900;
pub const STATUS_TTL: i64 = 86400;
const MAX_ERRORS: usize = 25;
pub const BACKFILL_DAYS: i64 = 730;

#[allow(async_fn_in_trait)]
pub trait Runner: Sync {
    fn has_redis(&self) -> bool;
    /// `SET lb:reingest:running 1 NX EX 900`.
    async fn lock(&self) -> anyhow::Result<bool>;
    /// `SET lb:reingest:status <json> EX 86400`, failures dropped.
    async fn put_status(&self, json: &str);
    /// `_local_status`.
    fn keep_local(&self, status: &Value);
    async fn heartbeat(&self);
    async fn unlock(&self);
    fn files(&self) -> Vec<(i64, PathBuf)>;
    fn now(&self) -> i64;
    async fn reset_all(&self) -> anyhow::Result<()>;
    async fn read_text(&self, path: &Path) -> Result<String, String>;
    /// `insert_dump(text, timestamp=anchor, allow_backfill=True)["created_at"]`.
    async fn insert(&self, text: String, anchor: i64) -> Result<Option<i64>, String>;
    async fn backfill_activity(&self) -> anyhow::Result<()>;
    async fn backfill_class(&self) -> anyhow::Result<()>;
    fn trigger(&self);
}

async fn set_status<R: Runner>(r: &R, status: &Map<String, Value>) {
    let v = Value::Object(status.clone());
    r.keep_local(&v);
    if r.has_redis() {
        r.put_status(&pyjson::dumps_spaced(&v)).await;
    }
}

fn bump(status: &mut Map<String, Value>, key: &str) {
    let n = status.get(key).and_then(Value::as_i64).unwrap_or(0) + 1;
    status.insert(key.into(), n.into());
}

/// `reingest(clear_first=)`.
pub async fn run<R: Runner>(r: &R, clear_first: bool) -> anyhow::Result<()> {
    if r.has_redis() && !r.lock().await? {
        tracing::info!("backlog re-ingest already running - skipping duplicate");
        return Ok(());
    }
    let files = r.files();
    let mut status = Map::new();
    for (k, v) in [
        ("running", json!(true)),
        ("total", json!(files.len())),
        ("done", json!(0)),
        ("ok", json!(0)),
        ("failed", json!(0)),
        ("started_at", json!(r.now())),
        ("finished_at", Value::Null),
        ("last_anchor", Value::Null),
        ("clear_first", json!(clear_first)),
        ("errors", json!([])),
    ] {
        status.insert(k.into(), v);
    }
    set_status(r, &status).await;
    let body = guarded(async {
        if clear_first {
            r.reset_all().await?;
        }
        for (anchor, path) in &files {
            let one = async {
                let text = r.read_text(path).await.map_err(anyhow::Error::msg)?;
                r.insert(text, *anchor).await.map_err(anyhow::Error::msg)
            };
            let res = guarded(one).await.map_err(|e| e.to_string());
            match res {
                Ok(created_at) => {
                    bump(&mut status, "ok");
                    status.insert("last_anchor".into(), created_at.into());
                }
                Err(e) => {
                    bump(&mut status, "failed");
                    if let Some(Value::Array(errors)) = status.get_mut("errors")
                        && errors.len() < MAX_ERRORS
                    {
                        errors.push(json!({"anchor": anchor, "error": kiwi_core::pystr::prefix(&e, 200)}));
                    }
                    tracing::warn!("backlog re-ingest failed for {}: {e}", path.display());
                }
            }
            bump(&mut status, "done");
            set_status(r, &status).await;
            if r.has_redis() {
                r.heartbeat().await;
            }
        }
        status.insert("phase".into(), json!("recomputing"));
        set_status(r, &status).await;
        if let Err(e) = guarded(r.backfill_activity()).await {
            tracing::warn!("backlog re-ingest: activity backfill failed: {e:#}");
        }
        if let Err(e) = guarded(r.backfill_class()).await {
            tracing::warn!("backlog re-ingest: class activity backfill failed: {e:#}");
        }
        r.trigger();
        anyhow::Ok(())
    })
    .await;
    status.insert("running".into(), json!(false));
    status.insert("phase".into(), json!("done"));
    status.insert("finished_at".into(), json!(r.now()));
    set_status(r, &status).await;
    if r.has_redis() {
        r.unlock().await;
    }
    tracing::info!("backlog re-ingest done: {} ok, {} failed of {}", status["ok"], status["failed"], status["total"]);
    body
}

/// `_read_text` of a file's bytes: gunzipped for `.gz`, UTF-8 with
/// replacement, universal newlines.
pub fn decode_text(name: &str, data: &[u8]) -> Result<String, String> {
    let raw = if name.ends_with(".gz") { std::borrow::Cow::Owned(kiwi_core::zlib::gunzip(data)?) } else { data.into() };
    let text = String::from_utf8_lossy(&raw);
    Ok(if text.contains('\r') { text.replace("\r\n", "\n").replace('\r', "\n") } else { text.into_owned() })
}

/// `str(OSError)` for a failed read of `path`.
pub fn os_error(e: &std::io::Error, path: &Path) -> String {
    let shown = kiwi_core::pystr::repr(&path.to_string_lossy());
    match e.raw_os_error() {
        Some(code) => {
            let text = e.to_string();
            let msg = text.strip_suffix(&format!(" (os error {code})")).unwrap_or(&text);
            format!("[Errno {code}] {msg}: {shown}")
        }
        None => e.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Production

pub struct Live(pub AppState);

impl Live {
    fn redis(&self) -> Option<redis::aio::ConnectionManager> {
        self.0.inner.db.redis.clone()
    }
}

impl Runner for Live {
    fn has_redis(&self) -> bool {
        self.0.inner.db.redis.is_some()
    }

    async fn lock(&self) -> anyhow::Result<bool> {
        let Some(mut con) = self.redis() else { return Ok(true) };
        let got: Option<String> =
            redis::cmd("SET").arg(RUNNING_KEY).arg("1").arg("NX").arg("EX").arg(LOCK_TTL).query_async(&mut con).await?;
        Ok(got.is_some())
    }

    async fn put_status(&self, json: &str) {
        let Some(mut con) = self.redis() else { return };
        let _: redis::RedisResult<()> =
            redis::cmd("SET").arg(STATUS_KEY).arg(json).arg("EX").arg(STATUS_TTL).query_async(&mut con).await;
    }

    fn keep_local(&self, status: &Value) {
        backlog::set_local_status(status.clone());
    }

    async fn heartbeat(&self) {
        let Some(mut con) = self.redis() else { return };
        let _: redis::RedisResult<()> = redis::cmd("EXPIRE").arg(RUNNING_KEY).arg(LOCK_TTL).query_async(&mut con).await;
    }

    async fn unlock(&self) {
        let Some(mut con) = self.redis() else { return };
        let _: redis::RedisResult<()> = redis::cmd("DEL").arg(RUNNING_KEY).query_async(&mut con).await;
    }

    fn files(&self) -> Vec<(i64, PathBuf)> {
        backlog::list_files(self.0.settings())
    }

    fn now(&self) -> i64 {
        chrono::Utc::now().timestamp()
    }

    async fn reset_all(&self) -> anyhow::Result<()> {
        let pool = store::pool(&self.0).map_err(anyhow::Error::msg)?;
        let tally = store::reset_all(pool, false).await.map_err(anyhow::Error::msg)?;
        super::reset_memos();
        let cleared = signals::reset_cache(&self.0).await;
        tracing::warn!(
            "leaderboards: FULL RESET entries={} estimates={} boards={} redis_keys_cleared={cleared}",
            tally.entries,
            tally.estimates,
            tally.boards
        );
        Ok(())
    }

    async fn read_text(&self, path: &Path) -> Result<String, String> {
        let data = std::fs::read(path).map_err(|e| os_error(&e, path))?;
        decode_text(&path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default(), &data)
    }

    async fn insert(&self, text: String, anchor: i64) -> Result<Option<i64>, String> {
        let clock = || chrono::Utc::now().timestamp();
        Ok(match ingest::insert_dump(&ingest::Live(&self.0), &text, Some(anchor), true, &clock).await? {
            Outcome::Empty => None,
            Outcome::Skipped { created_at, .. } | Outcome::Stored { created_at, .. } => Some(created_at),
        })
    }

    async fn backfill_activity(&self) -> anyhow::Result<()> {
        activity::player::backfill_history_chunked(&self.0, BACKFILL_DAYS, true, false).await.map(drop)
    }

    async fn backfill_class(&self) -> anyhow::Result<()> {
        activity::class::backfill_class_history_chunked(&self.0, BACKFILL_DAYS, true, false).await.map(drop)
    }

    fn trigger(&self) {
        super::trigger();
    }
}
