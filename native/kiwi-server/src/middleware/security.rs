//! `security` of `app/core/middleware.py`: the Content-Length body cap (a 413
//! that skips every header below), then setdefault security headers, the
//! per-path CSP, static/page cache rules and the credential rule.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

use axum::{extract::Request, extract::State, http::HeaderMap, middleware::Next, response::Response};
use regex::Regex;
use sha2::{Digest, Sha256};

use super::Mw;
use super::errors::envelope;
use super::headers::{add_vary, set, setdefault};
use super::paths::{
    API_CSP, EMBED_PATH, PAGE_PREFIXES, SITE_CSP, embed_csp, is_page_path, is_public_asset, is_site_path,
};
use super::pyurl::{first, py_int, query_param, url_path};

const FOREVER: &str = "public, max-age=31536000, immutable";
const FONT_TTL: &str = "public, max-age=604800";

fn has_credential(h: &HeaderMap) -> bool {
    first(h, "authorization").is_some_and(|v| !v.is_empty()) || super::pyurl::has_cookie(h, "kiwi_site_access")
}

pub async fn security(State(mw): State<Arc<Mw>>, req: Request, next: Next) -> Response {
    let path = url_path(req.uri());
    let cap = mw.cfg.body_cap(&path);
    if let Some(cl) = first(req.headers(), "content-length")
        && py_int(&cl).is_some_and(|n| n > cap as i128)
    {
        return envelope(413, "bad_request", &format!("Request body exceeds the {cap}-byte limit"));
    }
    let credential = has_credential(req.headers());
    let v = path.starts_with("/static/").then(|| query_param(req.uri(), "v")).flatten();

    let mut res = next.run(req).await;
    let h = res.headers_mut();
    setdefault(h, "x-content-type-options", "nosniff");
    setdefault(h, "referrer-policy", "strict-origin-when-cross-origin");
    setdefault(h, "strict-transport-security", "max-age=31536000; includeSubDomains");
    setdefault(h, "permissions-policy", "geolocation=(), microphone=(), camera=()");
    if path == EMBED_PATH {
        h.remove("x-frame-options");
        if !h.contains_key("content-security-policy") {
            let origins = mw.embed_origins().await;
            setdefault(res.headers_mut(), "content-security-policy", &embed_csp(&origins));
        }
        setdefault(res.headers_mut(), "cache-control", "no-cache");
        return res;
    }
    setdefault(h, "x-frame-options", "DENY");
    setdefault(h, "content-security-policy", if is_site_path(&path) { SITE_CSP } else { API_CSP });
    if path.starts_with("/static/") {
        let cc = mw.static_versions.cache_control(&path, v.as_deref());
        set(h, "cache-control", cc);
        return res;
    }
    if is_page_path(&path) || PAGE_PREFIXES.iter().any(|p| path.starts_with(p)) {
        setdefault(h, "cache-control", "no-cache");
    }
    if credential && !is_public_asset(&path) {
        set(h, "cache-control", "private, no-store");
        add_vary(h, &["Cookie", "Authorization"]);
    }
    res
}

/// `app/core/static_version.py`: sha256[:10] of a /static file, cached per
/// file by (mtime, size).
pub struct StaticVersions {
    root: PathBuf,
    cache: Mutex<HashMap<String, (SystemTime, u64, String)>>,
}

impl StaticVersions {
    pub fn new(root: &Path) -> Self {
        let root =
            std::fs::canonicalize(root).unwrap_or_else(|_| std::path::absolute(root).unwrap_or(root.to_path_buf()));
        StaticVersions { root, cache: Mutex::new(HashMap::new()) }
    }

    pub fn version(&self, url_path: &str) -> Option<String> {
        let rel = url_path.strip_prefix("/static/").unwrap_or(url_path);
        let file = std::fs::canonicalize(self.root.join(rel)).ok()?;
        if !file.starts_with(&self.root) {
            return None;
        }
        let meta = std::fs::metadata(&file).ok()?;
        let mtime = meta.modified().ok()?;
        if let Some((m, s, d)) = self.cache.lock().ok()?.get(rel)
            && *m == mtime
            && *s == meta.len()
        {
            return Some(d.clone());
        }
        let bytes = std::fs::read(&file).ok()?;
        let digest = hex::encode(Sha256::digest(&bytes))[..10].to_owned();
        if let Ok(mut c) = self.cache.lock() {
            c.insert(rel.to_owned(), (mtime, meta.len(), digest.clone()));
        }
        Some(digest)
    }

    pub fn cache_control(&self, url_path: &str, v: Option<&str>) -> &'static str {
        if let Some(v) = v.filter(|v| !v.is_empty())
            && self.version(url_path).as_deref() == Some(v)
        {
            return FOREVER;
        }
        if url_path.ends_with(".woff2") {
            return FONT_TTL;
        }
        "no-cache"
    }
}

/// `app.embed.service.allowed_origins`: memoised 30s, fails closed.
pub struct EmbedOrigins {
    source: EmbedSource,
    cache: tokio::sync::Mutex<Option<(Instant, Vec<String>)>>,
}

pub enum EmbedSource {
    Setting(crate::state::AppState),
    Fixed(Vec<String>),
}

const EMBED_TTL_SECS: u64 = 30;

impl EmbedOrigins {
    pub fn new(source: EmbedSource) -> Self {
        EmbedOrigins { source, cache: tokio::sync::Mutex::new(None) }
    }

    pub async fn get(&self) -> Vec<String> {
        let state = match &self.source {
            EmbedSource::Fixed(v) => return v.clone(),
            EmbedSource::Setting(state) => state,
        };
        let mut cache = self.cache.lock().await;
        if let Some((at, v)) = cache.as_ref()
            && at.elapsed().as_secs_f64() <= EMBED_TTL_SECS as f64
        {
            return v.clone();
        }
        let raw = match crate::runtime_config::get_setting(state, "embed.allowed_origins").await {
            Ok(serde_json::Value::String(s)) => s,
            Ok(serde_json::Value::Null) | Ok(serde_json::Value::Bool(false)) => String::new(),
            Ok(other) => other.to_string(),
            Err(e) => {
                tracing::warn!("embed: allowlist unreadable ({e}) - framing denied until it returns");
                String::new()
            }
        };
        let parsed = parse_origins(&raw);
        *cache = Some((Instant::now(), parsed.clone()));
        parsed
    }
}

fn py_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `_parse_origins`: space- or comma-separated, malformed entries dropped.
pub fn parse_origins(raw: &str) -> Vec<String> {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"(?i)^https?://(\*\.)?[a-z0-9-]+(\.[a-z0-9-]+)*(:\d{1,5})?$").expect("static regex")
    });
    let mut seen: Vec<String> = Vec::new();
    for part in raw.replace(',', " ").split(py_space).filter(|p| !p.is_empty()) {
        let origin = part.trim_end_matches('/');
        if re.is_match(origin) && !seen.iter().any(|s| s == origin) {
            seen.push(origin.to_owned());
        }
    }
    seen
}
