//! The page-view collector `POST /s` (`app/pageviews/collect.py`) and its
//! beacon to the api's internal ingest (`app/pageviews/beacon.py`). Always 204;
//! only the visitor digest and coarse labels leave this process.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use axum::{
    body::Bytes,
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use http_body_util::BodyExt;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use tokio::sync::Notify;

use super::Web;
use crate::middleware::ClientAddr;
use crate::middleware::pyurl::{first, latin1};
use kiwi_core::web::classify;
use kiwi_core::web::urlsplit::{unquote, urlsplit};

const MAX_BODY: usize = 4096;
const PER_MINUTE: u32 = 120;
const MAX_ENGAGED: i64 = 6 * 3600;
const PERF_KEYS: [&str; 5] = ["ttfb", "fcp", "lcp", "dcl", "load"];
const MAX_MS: i64 = 120_000;
const MAX_CLS: i64 = 10_000;
const LANGS: [&str; 10] = ["en", "de", "es", "fr", "ja", "ko", "pt-PT", "ru", "th", "zh-CN"];

const FLUSH_INTERVAL: Duration = Duration::from_secs(5);
const FLUSH_AT: usize = 50;
const MAX_BUFFER: usize = 5_000;
const POST_SIZE: usize = 500;

/// Page route templates in the website's registration order (the static
/// mount, robots/sitemap/Bing, the page router, `/s`, `/health`,
/// `/gamedata/{name}`): `page_template` takes the first that matches.
pub const ROUTES: [&str; 74] = [
    "/static",
    "/robots.txt",
    "/BingSiteAuth.xml",
    "/sitemap.xml",
    "/embed/viewer",
    "/",
    "/app",
    "/browse",
    "/documentation",
    "/swf-docs",
    "/swf-docs/{doc}",
    "/commands",
    "/support",
    "/status",
    "/embed/status.svg",
    "/status/og.png",
    "/tomes",
    "/server-time",
    "/search",
    "/calendar",
    "/streams",
    "/releases",
    "/classes",
    "/star-chart",
    "/sound-studio",
    "/blueprint-editor",
    "/mod-workshop",
    "/unlock-debug",
    "/unlock_debug",
    "/drop/{slug}",
    "/zakros-ui-requests",
    "/zakros-ui-stats",
    "/gems-guide",
    "/abilities",
    "/allies",
    "/guides",
    "/gem-tools",
    "/gem-abilities",
    "/gear-guide",
    "/fishing-guide",
    "/loot-collector-guide",
    "/community-sites",
    "/dressing-room",
    "/gem-simulator",
    "/gem-evaluator",
    "/gem-builds",
    "/calculators",
    "/terms",
    "/privacy",
    "/accessibility",
    "/changelog",
    "/login",
    "/dashboard",
    "/market",
    "/store",
    "/codexes",
    "/codexes/crafting",
    "/mods",
    "/mods/why",
    "/mods/{handle}/{slug}",
    "/mods/{handle}",
    "/modpacks",
    "/modpacks/{handle}/{slug}",
    "/giveaways",
    "/clubs",
    "/activity",
    "/activity/og.png",
    "/class-activity",
    "/player/{name}",
    "/leaderboards",
    "/updates",
    "/s",
    "/health",
    "/gamedata/{name}",
];

fn route_matches(template: &str, path: &str) -> bool {
    if template == "/static" {
        return path.starts_with("/static/");
    }
    let t: Vec<&str> = template.split('/').collect();
    let p: Vec<&str> = path.split('/').collect();
    t.len() == p.len() && t.iter().zip(&p).all(|(a, b)| if a.starts_with('{') { !b.is_empty() } else { a == b })
}

/// `page_template(request, path)`: the first route matching `path` for a GET,
/// if it is a page route (`/s` is POST-only and never matches).
pub fn page_template(path: &str) -> Option<&'static str> {
    let bare = path.strip_suffix('\n');
    let t =
        ROUTES.iter().find(|t| **t != "/s" && (route_matches(t, path) || bare.is_some_and(|b| route_matches(t, b))))?;
    let page = crate::middleware::paths::is_page_path(t)
        || crate::middleware::paths::PAGE_PREFIXES.iter().any(|p| t.starts_with(p));
    page.then_some(*t)
}

/// Python `str()` of a JSON value.
fn py_str(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::String(s) => s.clone(),
        Value::Number(n) => match n.as_f64() {
            Some(f) if !n.is_i64() && !n.is_u64() => crate::pyfloat::repr(f),
            _ => n.to_string(),
        },
        other => crate::web::templates::py_repr(&minijinja::Value::from_serialize(other)),
    }
}

/// `_int(value, lo, hi)`: `int(value)` clamped, `lo` where Python raises.
fn clamp_int(v: Option<&Value>, lo: i64, hi: i64) -> i64 {
    let n: Option<f64> = match v {
        Some(Value::Bool(b)) => Some(f64::from(u8::from(*b))),
        Some(Value::Number(n)) => match n.as_i64() {
            Some(i) => return i.clamp(lo, hi),
            None => n.as_f64().filter(|f| f.is_finite()).map(f64::trunc),
        },
        Some(Value::String(s)) => match kiwi_core::pynum::int(s) {
            Some(i) => return i.clamp(lo, hi),
            None => None,
        },
        _ => None,
    };
    match n {
        Some(f) if f < lo as f64 => lo,
        Some(f) if f > hi as f64 => hi,
        Some(f) => f as i64,
        None => lo,
    }
}

/// `^[A-Za-z0-9-]{8,40}$` (Python's `$` also matches before a final newline).
fn pid_ok(pid: &str) -> bool {
    let core = pid.strip_suffix('\n').unwrap_or(pid);
    (8..=40).contains(&core.len()) && core.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

pub struct Visitor<'a> {
    pub ip: &'a str,
    pub ua: &'a str,
    pub secret: &'a str,
    pub date: &'a str,
}

/// `visitor_hash(request)`.
pub fn visitor_hash(v: &Visitor<'_>) -> String {
    let text = format!("{}|{}|{}|{}", v.secret, v.date, v.ip, v.ua);
    hex::encode(Sha256::digest(text.as_bytes()))[..32].to_owned()
}

/// Where `build_event` raises: the whole beacon is dropped.
#[derive(Debug)]
pub struct Dropped;

/// `build_event(request, data)`: `Err` where Python raises, `Ok(None)` where
/// it returns None.
pub fn build_event(data: &Map<String, Value>, v: &Visitor<'_>, suffix: &str) -> Result<Option<Value>, Dropped> {
    let pid = data.get("id").map(py_str).unwrap_or_default();
    if !pid_ok(&pid) {
        return Ok(None);
    }
    let kind = data.get("t").and_then(Value::as_str).unwrap_or("");
    match kind {
        "end" => {
            return Ok(Some(json!({"kind": "end", "pid": pid, "engaged_s": clamp_int(data.get("s"), 0, MAX_ENGAGED)})));
        }
        "perf" => {
            let mut perf = Map::new();
            for k in PERF_KEYS {
                let n = clamp_int(data.get(k), 0, MAX_MS);
                if n != 0 {
                    perf.insert(k.to_owned(), json!(n));
                }
            }
            return Ok((!perf.is_empty()).then(|| json!({"kind": "perf", "pid": pid, "perf": perf})));
        }
        "cls" => {
            let cls = clamp_int(data.get("c"), -1, MAX_CLS);
            return Ok((cls >= 0).then(|| json!({"kind": "perf", "pid": pid, "perf": {"cls": cls}})));
        }
        "pv" => {}
        _ => return Ok(None),
    }
    let page_url = kiwi_core::pystr::prefix(&data.get("u").map(py_str).unwrap_or_default(), 2000).to_owned();
    let Ok(parts) = urlsplit(&page_url) else { return Ok(None) };
    let host = parts.hostname().unwrap_or_default().to_lowercase();
    if !classify::is_own_host(host.strip_prefix("www.").unwrap_or(&host), suffix) {
        return Ok(None);
    }
    let path =
        kiwi_core::pystr::prefix(&unquote(if parts.path.is_empty() { "/" } else { &parts.path }), 500).to_owned();
    let Some(template) = page_template(&path) else { return Ok(None) };
    let width = clamp_int(data.get("w"), 0, 20_000);
    let touch = clamp_int(data.get("tp"), 0, 20);
    let referrer = kiwi_core::pystr::prefix(&data.get("r").map(py_str).unwrap_or_default(), 2000).to_owned();
    let (channel, source) = classify::classify(&page_url, &referrer, suffix);
    let lang = match data.get("l") {
        Some(Value::Array(_) | Value::Object(_)) => return Err(Dropped),
        Some(Value::String(l)) if LANGS.contains(&l.as_str()) => Value::String(l.clone()),
        _ => Value::Null,
    };
    Ok(Some(json!({
        "kind": "pv",
        "pid": pid,
        "route": template,
        "path": path,
        "visitor_hash": visitor_hash(v),
        "channel": channel,
        "source": source,
        "device": classify::device(v.ua, width, touch),
        "browser": classify::browser(v.ua),
        "os": classify::os_name(v.ua, touch),
        "lang": lang,
    })))
}

/// The per-IP limit (`_over_limit`): 120 a minute, pruned past 20k keys.
#[derive(Default)]
pub struct RateLimit(Mutex<HashMap<String, (u64, u32)>>);

impl RateLimit {
    pub fn over(&self, ip: &str) -> bool {
        let minute = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() / 60).unwrap_or(0);
        let Ok(mut m) = self.0.lock() else { return false };
        let (seen, count) = m.get(ip).copied().unwrap_or((minute, 0));
        let count = if seen == minute { count + 1 } else { 1 };
        m.insert(ip.to_owned(), (minute, count));
        if m.len() > 20_000 {
            m.retain(|_, (mm, _)| *mm == minute);
        }
        count > PER_MINUTE
    }
}

/// `PageviewBeacon`: the buffer, flushed every 5s or at 50 events.
pub struct Beacon {
    buffer: Mutex<Vec<Value>>,
    wake: Notify,
}

impl Default for Beacon {
    fn default() -> Self {
        Beacon { buffer: Mutex::new(Vec::new()), wake: Notify::new() }
    }
}

impl Beacon {
    pub fn record(&self, event: Value) {
        let Ok(mut b) = self.buffer.lock() else { return };
        b.push(event);
        if b.len() > MAX_BUFFER {
            let dropped = b.len() - MAX_BUFFER;
            b.drain(..dropped);
            tracing::warn!("page-view beacon buffer full - dropped {dropped} event(s)");
        }
        if b.len() >= FLUSH_AT {
            self.wake.notify_one();
        }
    }

    /// The buffered events, emptied.
    pub fn take(&self) -> Vec<Value> {
        self.buffer.lock().map(|mut b| std::mem::take(&mut *b)).unwrap_or_default()
    }

    pub async fn flush(&self, web: &Web) {
        let batch = match self.buffer.lock() {
            Ok(mut b) if !b.is_empty() => std::mem::take(&mut *b),
            _ => return,
        };
        let headers = [("X-Internal-Key", web.internal_key.as_str())];
        for chunk in batch.chunks(POST_SIZE) {
            let body = crate::pyjson::dumps(&json!({"events": chunk}));
            if let Err(e) = web.api.post_json("/internal/pageviews", body, &headers, 8.0).await {
                tracing::warn!("page-view beacon failed for {} event(s): {e}", batch.len());
                return;
            }
        }
    }

    pub fn start(web: Arc<Web>) {
        tokio::spawn(async move {
            loop {
                let _ = tokio::time::timeout(FLUSH_INTERVAL, web.beacon.wake.notified()).await;
                web.beacon.flush(&web).await;
            }
        });
    }
}

fn origin_ok(headers: &HeaderMap, suffix: &str) -> Result<bool, ()> {
    let origin = first(headers, "origin").unwrap_or_default();
    if origin.is_empty() {
        return Ok(true);
    }
    let host = urlsplit(&origin).map_err(|_| ())?.hostname().unwrap_or_default().to_lowercase();
    Ok(classify::is_own_host(&host, suffix))
}

fn no_content() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

/// `POST /s`.
pub async fn collect(State(web): State<Arc<Web>>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let headers = &parts.headers;
    let ua = headers.get("user-agent").map(|v| latin1(v.as_bytes())).unwrap_or_default();
    if !web.beacon_enabled || classify::is_bot(&ua) {
        return no_content();
    }
    if !matches!(origin_ok(headers, &web.own_suffix), Ok(true)) {
        return no_content();
    }
    let ip = parts.extensions.get::<ClientAddr>().map(|c| c.host.clone()).filter(|h| !h.is_empty());
    let ip = ip.unwrap_or_else(|| "unknown".to_owned());
    if super::geo::asn(&web.asn_db, &ip).is_some_and(|n| classify::CLOUD_ASNS.contains(&n)) || web.rate.over(&ip) {
        return no_content();
    }
    let Ok(collected) = body.collect().await else { return no_content() };
    let bytes: Bytes = collected.to_bytes();
    if bytes.len() > MAX_BODY {
        return no_content();
    }
    let data = if bytes.is_empty() { Ok(Value::Object(Map::new())) } else { crate::pyjson::loads(&bytes) };
    if let Ok(Value::Object(data)) = data {
        let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
        let v = Visitor { ip: &ip, ua: &ua, secret: web.salt(), date: &date };
        if let Ok(Some(event)) = build_event(&data, &v, &web.own_suffix) {
            web.beacon.record(event);
        }
    }
    no_content()
}
