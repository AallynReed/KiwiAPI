//! `app/trove/updates/worker.py`: probe every branch each
//! `trove_update_probe_seconds`, under the Redis leader lock Python's workers
//! share (`kiwi:updates:leader`, SET NX EX 60, renewed every 20s, released by
//! token), then run the post-sync hooks.

use std::{future::Future, panic::AssertUnwindSafe, pin::Pin, sync::OnceLock, time::Duration};

use futures::FutureExt;
use kiwi_core::update_archive::BRANCHES;

use super::{Summary, SyncError, cas::ContentStore, cdn::CdnClient, from_live, ingest::sync_branch, repo::MongoRepo};
use crate::state::AppState;

pub const LOCK_KEY: &str = "kiwi:updates:leader";
pub const LOCK_TTL: u64 = 60;
const RENEW_EVERY: Duration = Duration::from_secs(20);
pub const RENEW_LUA: &str =
    "if redis.call('get', KEYS[1]) == ARGV[1] then return redis.call('pexpire', KEYS[1], ARGV[2]) else return 0 end";
pub const RELEASE_LUA: &str =
    "if redis.call('get', KEYS[1]) == ARGV[1] then return redis.call('del', KEYS[1]) else return 0 end";

pub type HookFuture = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>;
/// `codexes.indexer.ensure_indexed(branch, store, summary)`.
pub type CodexHook = fn(AppState, ContentStore, Summary) -> HookFuture;
/// `decode.runner.ensure_built(branch)`.
pub type DecodeHook = fn(AppState, String) -> HookFuture;

static CODEX_HOOK: OnceLock<CodexHook> = OnceLock::new();
static DECODE_HOOK: OnceLock<DecodeHook> = OnceLock::new();

/// Called once at startup by the codex port with its `ensure_indexed`.
pub fn set_codex_hook(hook: CodexHook) {
    let _ = CODEX_HOOK.set(hook);
}

/// Called once at startup by the decode port with its `ensure_built`.
pub fn set_decode_hook(hook: DecodeHook) {
    let _ = DECODE_HOOK.set(hook);
}

pub fn start(state: &AppState) {
    if !state.settings().trove_update_enabled {
        tracing::info!("updates: archiver disabled (set TROVE_UPDATE_ENABLED=true to run)");
        return;
    }
    let state = state.clone();
    tokio::spawn(async move {
        let repo = MongoRepo::new(state.clone());
        let store = ContentStore::new(&state.settings().trove_update_store_dir);
        loop {
            match AssertUnwindSafe(run_one_cycle(&state, &repo, &store)).catch_unwind().await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::warn!("updates: archiver cycle failed: {e:#}"),
                Err(_) => tracing::warn!("updates: archiver cycle failed: panicked"),
            }
            let secs = u64::try_from(state.settings().trove_update_probe_seconds).unwrap_or(0);
            tokio::time::sleep(Duration::from_secs(secs)).await;
        }
    });
}

/// A hook's failure, panics included, never derails the archiver.
async fn isolated(fut: HookFuture) -> Result<(), String> {
    match AssertUnwindSafe(fut).catch_unwind().await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(format!("{e:#}")),
        Err(_) => Err("panicked".into()),
    }
}

async fn run_hooks(state: &AppState, store: &ContentStore, summary: &Summary) {
    let branch = summary.branch.as_str();
    match CODEX_HOOK.get() {
        Some(hook) => {
            if let Err(e) = isolated(hook(state.clone(), store.clone(), summary.clone())).await {
                tracing::warn!("codexes[{branch}] index failed: {e}");
            }
        }
        None => tracing::warn!("codexes[{branch}] index skipped: no codex indexer registered"),
    }
    match DECODE_HOOK.get() {
        Some(hook) => {
            if let Err(e) = isolated(hook(state.clone(), branch.to_owned())).await {
                tracing::warn!("decode[{branch}] run failed: {e}");
            }
        }
        None => tracing::warn!("decode[{branch}] run skipped: no decode runner registered"),
    }
}

async fn sync_all_branches(state: &AppState, repo: &MongoRepo, store: &ContentStore) {
    let s = state.settings();
    let cdn = CdnClient::new(&s.trove_update_base_url, &s.trove_update_prefix);
    let concurrency = usize::try_from(s.trove_update_concurrency).unwrap_or(1);
    for (branch, pointer_file) in BRANCHES {
        match sync_branch(branch, pointer_file, &cdn, store, repo, concurrency).await {
            Ok(summary) => {
                let c = &summary.counts;
                if summary.changed {
                    let ord = summary.ordinal.map_or_else(|| "None".to_owned(), |o| o.to_string());
                    tracing::info!(
                        "updates[{branch}]: {} ordinal={ord}  +{} ~{} -{}  ({} new bytes)",
                        summary.version,
                        c.added,
                        c.modified,
                        c.removed,
                        c.bytes_added
                    );
                } else {
                    tracing::info!("updates[{branch}]: no change ({})", summary.version);
                }
                if let Some(db) = state.inner.db.mongo.as_ref()
                    && let Err(e) = from_live::tag_pending(db, branch).await
                {
                    tracing::warn!("updates[{branch}] from-live tagging failed: {e}");
                }
                run_hooks(state, store, &summary).await;
            }
            Err(SyncError::Http { status: 404, .. }) => {
                tracing::info!("updates[{branch}]: not published right now (404) - skipping");
            }
            Err(e) => {
                tracing::warn!("updates[{branch}] sync failed: {e}");
                let _ = repo.mark_branch_error(branch, &e.to_string()).await;
            }
        }
    }
}

/// The lock heartbeat stops with the cycle, however the cycle ends.
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// `secrets.token_hex(16)`.
fn token_hex() -> String {
    let mut raw = [0u8; 16];
    getrandom::fill(&mut raw).expect("OS randomness");
    hex::encode(raw)
}

async fn run_one_cycle(state: &AppState, repo: &MongoRepo, store: &ContentStore) -> anyhow::Result<()> {
    let Some(mut redis) = state.inner.db.redis.clone() else {
        sync_all_branches(state, repo, store).await;
        return Ok(());
    };
    let token = token_hex();
    let got: Option<String> =
        redis::cmd("SET").arg(LOCK_KEY).arg(&token).arg("NX").arg("EX").arg(LOCK_TTL).query_async(&mut redis).await?;
    if got.is_none() {
        tracing::debug!("updates: another worker holds the archiver lock; skipping cycle");
        return Ok(());
    }
    let heartbeat = {
        let (mut redis, token) = (redis.clone(), token.clone());
        AbortOnDrop(tokio::spawn(async move {
            loop {
                tokio::time::sleep(RENEW_EVERY).await;
                let renewed: i64 = redis::cmd("EVAL")
                    .arg(RENEW_LUA)
                    .arg(1)
                    .arg(LOCK_KEY)
                    .arg(&token)
                    .arg(LOCK_TTL * 1000)
                    .query_async(&mut redis)
                    .await
                    .unwrap_or(1);
                if renewed == 0 {
                    tracing::warn!("updates: lost the archiver leader lock mid-cycle");
                    return;
                }
            }
        }))
    };
    sync_all_branches(state, repo, store).await;
    drop(heartbeat);
    let _: Result<i64, _> =
        redis::cmd("EVAL").arg(RELEASE_LUA).arg(1).arg(LOCK_KEY).arg(&token).query_async(&mut redis).await;
    Ok(())
}
