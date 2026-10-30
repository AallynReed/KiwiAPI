//! The game-file version archiver (`app/trove/updates/` sync side): the
//! `update_archiver` loop mirroring Trion's update CDN into the shared
//! content-addressed store and the `update_*` collections, and the master's
//! `last_ordinal` backfill routes. The pure formats live in
//! `kiwi_core::update_archive`.

pub mod cas;
pub mod cdn;
pub mod from_live;
pub mod ingest;
pub mod maintenance;
pub mod repo;
pub mod worker;

#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod tests;

use axum::{
    Router,
    routing::{get, post},
};

pub use worker::{set_codex_hook, set_decode_hook, start};

use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/updates/backfill-modified", post(maintenance::backfill_modified))
        .route("/admin/updates/backfill-modified/status", get(maintenance::backfill_modified_status))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub added: i64,
    pub modified: i64,
    pub removed: i64,
    pub bytes_added: i64,
}

/// `sync_branch`'s summary, handed to the post-sync hooks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub branch: String,
    pub version: String,
    pub changed: bool,
    pub ordinal: Option<i64>,
    pub counts: Counts,
}

/// Why a branch sync stopped; `Display` is Python's `str(exc)`, which is what
/// `update_branches.last_error` keeps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncError {
    /// `httpx.HTTPStatusError` (non-2xx).
    Http {
        status: u16,
        message: String,
    },
    /// Any other httpx error.
    Transport(String),
    /// `CdnError`.
    Cdn(String),
    /// `ArchiveError`.
    Archive(String),
    Db(String),
    /// `OSError`.
    Io(String),
}

impl SyncError {
    pub fn db(e: impl std::fmt::Display) -> Self {
        SyncError::Db(e.to_string())
    }

    /// `str(OSError)`: `[Errno N] strerror` where the OS gave a code.
    pub fn io(e: std::io::Error) -> Self {
        let text = e.to_string();
        SyncError::Io(match e.raw_os_error() {
            Some(code) => {
                let msg = text.strip_suffix(&format!(" (os error {code})")).unwrap_or(&text);
                format!("[Errno {code}] {msg}")
            }
            None => text,
        })
    }
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncError::Http { message, .. } => f.write_str(message),
            SyncError::Transport(m)
            | SyncError::Cdn(m)
            | SyncError::Archive(m)
            | SyncError::Db(m)
            | SyncError::Io(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for SyncError {}
