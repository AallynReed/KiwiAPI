//! `app/geo_maintenance`: the website's regional maintenance gate and the
//! DB-IP Lite lookups (country for the gate, ASN for the page-view bot test).
//!
//! Each database is a file in `GEOIP_DIR` checked once a minute: reloaded when
//! its mtime changes, re-downloaded (current month, else last month) when it is
//! older than 32 days, at most every 6h after a failure, by one process at a
//! time through the same `.lock` file the Python workers use. Lookups stay in
//! memory: nothing about the address is logged or stored.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    middleware::Next,
    response::Response,
};
use chrono::Datelike;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::Mutex as AsyncMutex;

use super::Web;
use crate::middleware::ClientAddr;

const URL: &str = "https://download.db-ip.com/free/dbip-{kind}-lite-{month}.mmdb.gz";
const MAX_AGE: f64 = 32.0 * 86400.0;
const RETRY_AFTER: f64 = 6.0 * 3600.0;
const LOCK_STALE: f64 = 600.0;
const CHECK_EVERY: Duration = Duration::from_secs(60);

/// The page `scripts/build_maintenance_page.py` builds, relative to the working
/// directory (`/app` in the image).
pub const PAGE_PATH: &str = "assets/maintenance.html";
const CSP: &str = "default-src 'none'; style-src 'self' 'unsafe-inline'; font-src 'self'; \
img-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";
const TTL: Duration = Duration::from_secs(30);

type Reader = maxminddb::Reader<Vec<u8>>;

#[derive(Default)]
struct DbState {
    reader: Option<Arc<Reader>>,
    mtime: Option<SystemTime>,
    failed_at: f64,
    running: bool,
}

pub struct Database {
    kind: &'static str,
    dir: PathBuf,
    state: Mutex<DbState>,
}

fn now() -> f64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

fn age_secs(t: SystemTime) -> f64 {
    SystemTime::now().duration_since(t).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

impl Database {
    pub fn new(kind: &'static str, dir: &Path) -> Arc<Self> {
        Arc::new(Database { kind, dir: dir.to_path_buf(), state: Mutex::new(DbState::default()) })
    }

    fn path(&self) -> PathBuf {
        self.dir.join(format!("dbip-{}-lite.mmdb", self.kind))
    }

    /// Load (or reload) the file now, without the refresh loop.
    pub fn load_now(&self) {
        self.load(&self.path());
    }

    fn reader(&self) -> Option<Arc<Reader>> {
        self.state.lock().ok()?.reader.clone()
    }

    /// `ensure_running()`: start this process's refresh loop once.
    pub fn ensure_running(self: &Arc<Self>) {
        {
            let Ok(mut st) = self.state.lock() else { return };
            if st.running {
                return;
            }
            st.running = true;
        }
        let db = self.clone();
        tokio::spawn(async move {
            loop {
                db.refresh().await;
                tokio::time::sleep(CHECK_EVERY).await;
            }
        });
    }

    async fn refresh(&self) {
        let path = self.path();
        let age = std::fs::metadata(&path).and_then(|m| m.modified()).map(age_secs).unwrap_or(f64::INFINITY);
        let failed_at = self.state.lock().map(|s| s.failed_at).unwrap_or(0.0);
        if age > MAX_AGE && now() - failed_at > RETRY_AFTER {
            let _ = std::fs::create_dir_all(&self.dir);
            let lock = path.with_extension("lock");
            if claim(&lock) {
                if let Err(e) = self.download(&path).await {
                    if let Ok(mut s) = self.state.lock() {
                        s.failed_at = now();
                    }
                    tracing::warn!("GeoIP {} database download failed: {e:#}", self.kind);
                }
                let _ = std::fs::remove_file(&lock);
            }
        }
        self.load(&path);
    }

    async fn download(&self, dest: &Path) -> anyhow::Result<()> {
        let today = chrono::Utc::now().date_naive();
        let first = today.with_day0(0).unwrap_or(today);
        let months = [first, first - chrono::Duration::days(1)];
        let client = crate::http::client_builder().timeout(Duration::from_secs(60)).build()?;
        for month in months {
            let url = URL.replace("{kind}", self.kind).replace("{month}", &month.format("%Y-%m").to_string());
            let resp = client.get(&url).send().await?;
            if resp.status() == StatusCode::NOT_FOUND {
                continue;
            }
            let gz = resp.error_for_status()?.bytes().await?;
            let dest = dest.to_path_buf();
            tokio::task::spawn_blocking(move || install(&gz, &dest)).await??;
            tracing::info!("GeoIP {} database updated ({})", self.kind, month.format("%Y-%m"));
            return Ok(());
        }
        anyhow::bail!("no DB-IP {} database published for this or last month", self.kind)
    }

    fn load(&self, path: &Path) {
        let Ok(mtime) = std::fs::metadata(path).and_then(|m| m.modified()) else { return };
        if self.state.lock().map(|s| s.mtime == Some(mtime)).unwrap_or(true) {
            return;
        }
        match Reader::open_readfile(path) {
            Ok(r) => {
                if let Ok(mut s) = self.state.lock() {
                    s.reader = Some(Arc::new(r));
                    s.mtime = Some(mtime);
                }
            }
            Err(e) => tracing::error!("GeoIP {} refresh failed: {e}", self.kind),
        }
    }
}

fn claim(lock: &Path) -> bool {
    match std::fs::OpenOptions::new().write(true).create_new(true).open(lock) {
        Ok(_) => true,
        Err(_) => {
            if std::fs::metadata(lock).and_then(|m| m.modified()).map(age_secs).is_ok_and(|a| a > LOCK_STALE) {
                let _ = std::fs::remove_file(lock);
            }
            false
        }
    }
}

fn install(gz: &[u8], dest: &Path) -> anyhow::Result<()> {
    use std::io::Read;
    let name = dest.file_name().and_then(|n| n.to_str()).unwrap_or("db");
    let tmp = dest.with_file_name(format!("{name}.{}.tmp", std::process::id()));
    let res = (|| {
        let mut raw = Vec::new();
        flate2::read::MultiGzDecoder::new(gz).read_to_end(&mut raw)?;
        std::fs::write(&tmp, &raw)?;
        Reader::from_source(raw)?;
        std::fs::rename(&tmp, dest)?;
        Ok(())
    })();
    let _ = std::fs::remove_file(&tmp);
    res
}

#[derive(Deserialize)]
struct CountryRec<'a> {
    #[serde(borrow)]
    country: Option<CountryInner<'a>>,
}

#[derive(Deserialize)]
struct CountryInner<'a> {
    iso_code: Option<&'a str>,
}

#[derive(Deserialize)]
struct AsnRec {
    autonomous_system_number: Option<u64>,
}

fn parse_ip(ip: &str) -> Option<IpAddr> {
    ip.parse().ok()
}

/// `geoip.country(ip)`.
pub fn country(db: &Database, ip: &str) -> Option<String> {
    let reader = db.reader()?;
    let rec: CountryRec = reader.lookup(parse_ip(ip)?).ok()?.decode().ok()??;
    rec.country?.iso_code.map(str::to_owned)
}

/// `geoip.asn(ip)`.
pub fn asn(db: &Database, ip: &str) -> Option<u64> {
    let reader = db.reader()?;
    let rec: AsnRec = reader.lookup(parse_ip(ip)?).ok()?.decode().ok()??;
    rec.autonomous_system_number
}

/// The gate's country list (`countries()`): memoised 30s, the last list kept
/// when the api cannot be read.
pub struct Countries {
    memo: AsyncMutex<(Option<Instant>, Vec<Value>)>,
}

impl Countries {
    pub fn new() -> Self {
        Countries { memo: AsyncMutex::new((None, Vec::new())) }
    }

    pub async fn get(&self, web: &Web) -> Vec<Value> {
        if web.internal_key.is_empty() {
            return Vec::new();
        }
        let mut memo = self.memo.lock().await;
        if memo.0.is_none_or(|at| at.elapsed() > TTL) {
            memo.0 = Some(Instant::now());
            let headers = [("X-Internal-Key", web.internal_key.as_str())];
            if let Some(Value::Object(data)) = web.api.get("/internal/geo-maintenance", &[], 3.0, &headers).await
                && let Some(Value::Array(list)) = data.get("countries")
            {
                memo.1 = list.clone();
            }
        }
        memo.1.clone()
    }
}

impl Default for Countries {
    fn default() -> Self {
        Self::new()
    }
}

pub async fn gate(State(web): State<Arc<Web>>, req: Request, next: Next) -> Response {
    let path = crate::middleware::pyurl::url_path(req.uri());
    if !(path.starts_with("/static/") || path.starts_with("/health")) {
        let blocked = web.countries.get(&web).await;
        if !blocked.is_empty() {
            web.country_db.ensure_running();
            let ip = req.extensions().get::<ClientAddr>().map(|c| c.host.clone()).unwrap_or_default();
            if let Some(code) = country(&web.country_db, &ip)
                && blocked.iter().any(|c| c.as_str() == Some(code.as_str()))
            {
                let page = web.maintenance_page.clone();
                let mut res = Response::new(Body::from(page.to_string()));
                *res.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
                let h = res.headers_mut();
                h.insert(header::CONTENT_LENGTH, HeaderValue::from(page.len()));
                h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
                h.insert(header::RETRY_AFTER, HeaderValue::from_static("120"));
                h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
                h.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static(CSP));
                return res;
            }
        }
    }
    next.run(req).await
}
