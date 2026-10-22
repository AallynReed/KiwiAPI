//! `app/trove/leaderboards/backlog.py`: the gzip-per-anchor dump store and the
//! re-ingest progress read. The re-ingest job itself stays in Python.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

use crate::{config::Settings, pyjson, state::AppState};

pub const STATUS_KEY: &str = "lb:reingest:status";
pub const RUNNING_KEY: &str = "lb:reingest:running";

/// `str(Path(p))` on POSIX: empty and `.` parts dropped, a lone `//` prefix kept.
pub fn posix_path(p: &str) -> String {
    let lead = if p.starts_with("//") && !p.starts_with("///") {
        "//"
    } else if p.starts_with('/') {
        "/"
    } else {
        ""
    };
    let parts: Vec<&str> = p.split('/').filter(|s| !s.is_empty() && *s != ".").collect();
    let joined = parts.join("/");
    match (lead, joined.is_empty()) {
        ("", true) => ".".into(),
        (lead, _) => format!("{lead}{joined}"),
    }
}

/// `str(Path(settings.backlog_dir) / "leaderboards")`.
pub fn dir_text(settings: &Settings) -> String {
    let base = posix_path(&settings.backlog_dir);
    match base.as_str() {
        "." => "leaderboards".into(),
        b if b.ends_with('/') => format!("{b}leaderboards"),
        b => format!("{b}/leaderboards"),
    }
}

fn dir(settings: &Settings) -> PathBuf {
    PathBuf::from(dir_text(settings))
}

/// `_anchor_of`: the first run of 9-11 decimal digits, if a plausible anchor.
pub fn anchor_of(name: &str) -> Option<i64> {
    let chars: Vec<char> = name.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let run = chars[i..].iter().take_while(|c| kiwi_core::pynum::decimal(**c).is_some()).count();
        if run >= 9 {
            let digits: String = chars[i..i + run.min(11)].iter().collect();
            let n = kiwi_core::pynum::int_of_digits(&digits).ok()?;
            return (1_577_836_800..=2_051_222_400).contains(&n).then_some(n);
        }
        i += run.max(1);
    }
    None
}

/// `list_files`: `(anchor, path)` oldest first.
pub fn list_files(settings: &Settings) -> Vec<(i64, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir(settings)) else { return Vec::new() };
    let mut out: Vec<(i64, PathBuf)> = rd
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if !path.is_file() || name.starts_with('.') {
                return None;
            }
            Some((anchor_of(&name)?, path))
        })
        .collect();
    out.sort_by_key(|(a, _)| *a);
    out
}

/// `info()`.
pub fn info(settings: &Settings) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("backlog_dir".into(), Value::String(dir_text(settings)));
    m.insert("backlog_dir_exists".into(), Value::Bool(dir(settings).is_dir()));
    m.insert("backlog_files".into(), Value::from(list_files(settings).len()));
    m
}

fn write_gz(dir: &Path, anchor: i64, text: &str, retention_days: i64) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp_name = format!(".{anchor}.cfg.gz.tmp");
    let mtime = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as u32).unwrap_or(0);
    let gz = kiwi_core::zlib::gzip_file(text.as_bytes(), 6, &tmp_name, mtime)
        .ok_or_else(|| std::io::Error::other("deflate failed"))?;
    let tmp = dir.join(&tmp_name);
    std::fs::write(&tmp, gz)?;
    std::fs::rename(&tmp, dir.join(format!("{anchor}.cfg.gz")))?;
    if retention_days > 0 {
        prune(dir, retention_days);
    }
    Ok(())
}

/// `_prune`: drop files older than `days` by mtime.
fn prune(dir: &Path, days: i64) {
    let cutoff = SystemTime::now() - Duration::from_secs(days.max(0) as u64 * 86400);
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.filter_map(Result::ok) {
        let p = e.path();
        if p.is_file() && p.metadata().and_then(|m| m.modified()).is_ok_and(|m| m < cutoff) {
            let _ = std::fs::remove_file(&p);
        }
    }
}

/// `save(anchor, text)`: best effort, never fails the ingest.
pub async fn save(state: &AppState, anchor: i64, text: std::sync::Arc<String>) {
    let s = state.settings();
    if !s.backlog_enabled || anchor == 0 {
        return;
    }
    let (d, days) = (dir(s), s.backlog_retention_days);
    let res = tokio::task::spawn_blocking(move || write_gz(&d, anchor, &text, days)).await;
    match res {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!("backlog save failed for anchor={anchor}: {e}"),
        Err(e) => tracing::warn!("backlog save failed for anchor={anchor}: {e}"),
    }
}

/// `get_status()` from what Redis holds: the shared copy, else this
/// process's (which never runs a re-ingest, so `{"running": false}`).
pub fn status_from(raw: Option<&[u8]>) -> Value {
    raw.filter(|r| !r.is_empty()).and_then(|r| pyjson::loads(r).ok()).unwrap_or_else(|| json!({"running": false}))
}

pub async fn get_status(state: &AppState) -> Value {
    let mut raw = None;
    if let Some(mut con) = state.inner.db.redis.clone() {
        let got: redis::RedisResult<Option<Vec<u8>>> = redis::cmd("GET").arg(STATUS_KEY).query_async(&mut con).await;
        raw = got.ok().flatten();
    }
    status_from(raw.as_deref())
}
