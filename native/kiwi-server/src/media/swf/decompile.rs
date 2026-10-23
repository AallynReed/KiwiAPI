//! `app/trove/swf/decompile.py` + the decompile half of `service.py`: JPEXS
//! FFDec (Java) run as a bounded subprocess over a scratch copy of the movie,
//! its exported `.as` tree read back under a file and byte cap. Cached per
//! movie sha under `c1:swfsrc:` ([`scripts`]), or not at all for uploads
//! ([`scripts_uncached`]), at most `ffdec_max_concurrent` JVMs at once.

use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
    time::Duration,
};

use axum::http::{StatusCode, request::Parts};
use serde_json::{Value, json};
use tokio::sync::Semaphore;

use super::extract;
use crate::{
    auth::{Rejection, client::client_ip},
    config::Settings,
    error::ApiError,
    media::bpcache,
    ratelimit, runtime_config,
    state::AppState,
};

const EXPORT_SUBDIR: &str = "scripts";
const FALLBACK_JARS: [&str; 4] = [
    "/opt/ffdec/ffdec.jar",
    "/usr/share/ffdec/ffdec.jar",
    r"C:\Program Files (x86)\FFDec\ffdec-cli.jar",
    r"C:\Program Files\FFDec\ffdec-cli.jar",
];

/// `DecompileError` (a 422 with its text) or `DecompilerUnavailable` (503).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fail {
    Error(String),
    Unavailable(String),
}

fn find_jar(s: &Settings) -> Option<PathBuf> {
    let configured = s.ffdec_jar.trim();
    let configured = (!configured.is_empty()).then_some(configured);
    configured.into_iter().chain(FALLBACK_JARS).map(PathBuf::from).find(|p| p.is_file())
}

/// `shutil.which(name)`.
fn which(name: &str) -> Option<PathBuf> {
    let p = Path::new(name);
    if p.components().count() > 1 {
        return p.is_file().then(|| p.to_path_buf());
    }
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE".into()).split(';').map(str::to_owned).collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let cand = dir.join(format!("{name}{ext}"));
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

fn find_java(s: &Settings) -> Option<PathBuf> {
    which(if s.ffdec_java.is_empty() { "java" } else { &s.ffdec_java })
}

/// `available()`: both a jar and a JVM are present.
pub fn available(s: &Settings) -> bool {
    find_jar(s).is_some() && find_java(s).is_some()
}

fn config_flags(s: &Settings) -> String {
    [
        format!("decompilationTimeoutSingleMethod={}", s.ffdec_method_timeout),
        format!("decompilationTimeoutFile={}", s.ffdec_file_timeout),
        format!("exportTimeout={}", s.ffdec_timeout),
        "parallelSpeedUp=true".into(),
        "parallelSpeedUpThreadCount=2".into(),
        "autoDeobfuscate=true".into(),
    ]
    .join(",")
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, out);
        } else if p.is_file() {
            out.push(p);
        }
    }
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root).unwrap_or(p).to_string_lossy().replace('\\', "/")
}

/// `_read_export`: `[{path, size, source}]` sorted by lowercased path.
pub fn read_export(root: &Path, max_scripts: usize, max_bytes: usize) -> (Vec<Value>, bool) {
    let mut files = Vec::new();
    walk(root, &mut files);
    files.sort_by_cached_key(|p| rel(root, p).to_lowercase());
    let (mut out, mut total, mut truncated) = (Vec::new(), 0usize, false);
    for p in files {
        if out.len() >= max_scripts {
            truncated = true;
            break;
        }
        let Ok(data) = std::fs::read(&p) else { continue };
        total += data.len();
        if total > max_bytes {
            truncated = true;
            break;
        }
        out.push(json!({"path": rel(root, &p), "size": data.len(), "source": String::from_utf8_lossy(&data)}));
    }
    (out, truncated)
}

/// `_version(stdout)`.
pub fn version(stdout: &str) -> String {
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r"Free Flash Decompiler\s+v\.?\s*([\d.]+)").expect("version regex"));
    re.captures(stdout).map_or_else(|| "ffdec".into(), |c| format!("ffdec {}", &c[1]))
}

fn scratch_dir() -> std::io::Result<PathBuf> {
    const CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789_";
    for _ in 0..100 {
        let mut raw = [0u8; 8];
        let _ = getrandom::fill(&mut raw);
        let name: String = raw.iter().map(|b| CHARS[usize::from(*b) % CHARS.len()] as char).collect();
        let dir = std::env::temp_dir().join(format!("ffdec-{name}"));
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::other("no temporary directory name available"))
}

struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `decompile_scripts(raw)`: `{scripts, count, truncated, decompiler}`.
pub async fn decompile_scripts(s: &Settings, raw: &[u8]) -> Result<Value, Fail> {
    let (Some(jar), Some(java)) = (find_jar(s), find_java(s)) else {
        return Err(Fail::Unavailable("The Flash decompiler is not installed here.".into()));
    };
    if raw.len() as i64 > s.ffdec_max_swf_bytes {
        return Err(Fail::Error("This movie is too large to decompile.".into()));
    }
    if let Err(extract::Fail::Swf(e) | extract::Fail::Crash(e)) = extract::read_header(raw) {
        return Err(Fail::Error(format!("This file could not be read as a Flash movie ({e}).")));
    }
    let work = Scratch(scratch_dir().map_err(|e| Fail::Unavailable(e.to_string()))?);
    let movie = work.0.join("movie.swf");
    let outdir = work.0.join("out");
    std::fs::write(&movie, raw).map_err(|e| Fail::Unavailable(e.to_string()))?;
    let mut cmd = tokio::process::Command::new(&java);
    cmd.arg(format!("-Xmx{}m", s.ffdec_max_heap_mb))
        .arg("-Djava.awt.headless=true")
        .arg(format!("-Duser.home={}", work.0.display()))
        .arg("-jar")
        .arg(&jar)
        .arg("-config")
        .arg(config_flags(s))
        .arg("-export")
        .arg("script")
        .arg(&outdir)
        .arg(&movie)
        .env("HOME", &work.0)
        .env("USERPROFILE", &work.0)
        .current_dir(&work.0)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let limit = Duration::from_secs((s.ffdec_timeout + 30).max(0) as u64);
    let out = match tokio::time::timeout(limit, cmd.output()).await {
        Err(_) => {
            tracing::warn!("swf: decompile timed out after {}s", s.ffdec_timeout + 30);
            return Err(Fail::Error("This movie took too long to decompile.".into()));
        }
        Ok(Err(e)) => return Err(Fail::Unavailable(e.to_string())),
        Ok(Ok(o)) => o,
    };
    let stdout = String::from_utf8_lossy(&out.stdout);
    let root = outdir.join(EXPORT_SUBDIR);
    if !root.is_dir() {
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            tracing::info!("swf: ffdec exited {:?}: {}", out.status.code(), err.chars().take(400).collect::<String>());
            return Err(Fail::Error("This file could not be read as a Flash movie.".into()));
        }
        return Ok(json!({"scripts": [], "count": 0, "truncated": false, "decompiler": version(&stdout)}));
    }
    let (max_scripts, max_bytes) = (s.ffdec_max_scripts.max(0) as usize, s.ffdec_max_source_bytes.max(0) as usize);
    let (scripts, truncated) =
        tokio::task::spawn_blocking(move || read_export(&root, max_scripts, max_bytes)).await.unwrap_or_default();
    let count = scripts.len();
    Ok(json!({"scripts": scripts, "count": count, "truncated": truncated, "decompiler": version(&stdout)}))
}

fn gate(s: &Settings) -> &'static Semaphore {
    static G: OnceLock<Semaphore> = OnceLock::new();
    G.get_or_init(|| Semaphore::new(s.ffdec_max_concurrent.max(1) as usize))
}

fn unavailable() -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "service_unavailable",
        "The Flash decompiler is not available on this server.",
    )
}

/// `service._decompile`.
async fn run(state: &AppState, raw: Vec<u8>, label: &str) -> Result<Value, ApiError> {
    let s = state.settings();
    if !available(s) {
        return Err(unavailable());
    }
    let _permit = gate(s).acquire().await.map_err(|_| ApiError::internal())?;
    match decompile_scripts(s, &raw).await {
        Ok(v) => Ok(v),
        Err(Fail::Unavailable(e)) => {
            tracing::warn!("swf: decompiler unavailable: {e}");
            Err(unavailable())
        }
        Err(Fail::Error(e)) => {
            tracing::info!("swf: cannot decompile {}: {e}", label.chars().take(12).collect::<String>());
            Err(ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, "bad_request", e))
        }
    }
}

/// `service.scripts(raw, sha)`: cached per movie.
pub async fn scripts(state: &AppState, raw: Vec<u8>, sha: &str) -> Result<Value, ApiError> {
    bpcache::get_or_build(state, &bpcache::key_for_swf_scripts(sha), || run(state, raw, sha)).await
}

/// `service.scripts_uncached(raw)`: for uploads, nothing kept.
pub async fn scripts_uncached(state: &AppState, raw: Vec<u8>) -> Result<Value, ApiError> {
    run(state, raw, "upload").await
}

/// `service.decompile_throttle(request)`: the per-IP `swfcode:` bucket.
pub async fn throttle(state: &AppState, parts: &Parts) -> Result<(), Rejection> {
    let (max, window) =
        runtime_config::get_rate_limit(state, "swf_decompile_rate_limit").await.map_err(Rejection::internal)?;
    let ip = client_ip(parts).unwrap_or_else(|| "unknown".into());
    ratelimit::check_rate_limit(state, &format!("swfcode:{ip}"), max, window).await.map(|_| ())
}
