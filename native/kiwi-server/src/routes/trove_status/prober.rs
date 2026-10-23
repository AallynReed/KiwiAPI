//! The status prober loop of `app/trove/status.py` (`status_prober`):
//! probe, record transitions, mirror the snapshot to Redis for every other
//! process, then publish `server_status` on the events bus.

use std::{
    future::Future,
    io,
    sync::RwLock,
    time::{Duration, Instant},
};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Request, Uri, header};
use hyper_util::rt::TokioIo;
use kiwi_core::{gems::py_round, status as core};
use mongodb::bson::{Bson, Document, doc};
use serde_json::{Map, Value, json};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
};
use tokio_rustls::{TlsConnector, rustls::pki_types::ServerName};

use super::{COLLECTION, REDIS_KEY, segment_from_doc, unix_now, unknown_snapshot};
use crate::{
    auth::pyutil::{bson_int, int_of, strip},
    features::py_truthy,
    jobs, pyfloat, pyjson, runtime_config,
    state::AppState,
};

pub const LOOP: &str = "status_prober";
const REDIS_TTL: u64 = 600;
const AUTH_PROBE_UA: &str = "Glyph (stable-251-1-a-335833)";
const AUTH_FORM: &str = "username=&password=&channel=131&includeStoreToken=&publicMachine=0&macAddr=000000000000";
const GLS_MAX_READ_BYTES: usize = 32768;
const GLS_SUBSTANTIVE_BYTES: usize = 256;
const GLS_OPENER_HEADER: [u8; 5] = [0x20, 0, 0, 0, 0];

static CURRENT: RwLock<Option<Value>> = RwLock::new(None);

/// This process's last snapshot (`_state`), when the prober runs here.
pub fn current() -> Option<Value> {
    CURRENT.read().ok().and_then(|s| s.clone())
}

/// Spawns the loop when this server owns it.
pub fn start(state: &AppState) {
    jobs::spawn(state, LOOP, run);
}

async fn run(state: AppState) {
    tokio::time::sleep(Duration::from_secs(3)).await;
    loop {
        match probe_once(&state).await {
            Ok(r) => {
                let s = |env: &str| r.pointer(&format!("/environments/{env}/status")).cloned().unwrap_or_default();
                let auth = r.pointer("/auth/online").cloned().unwrap_or_default();
                tracing::info!("trove status: eu={} us={} pts={} (auth={auth})", s("eu"), s("us"), s("pts"));
                if let Err(e) = crate::events::publish_type(&state, "server_status").await {
                    tracing::warn!("trove status: event publish failed: {e:#}");
                }
            }
            Err(e) => tracing::warn!("trove status probe failed: {e:#}"),
        }
        let interval = state.settings().trove_status_probe_interval_seconds.max(0) as u64;
        tokio::time::sleep(Duration::from_secs(interval)).await;
    }
}

/// `asyncio.wait_for` timeout seconds as a tokio duration.
fn secs(x: f64) -> Duration {
    if x.is_nan() || x <= 0.0 { Duration::ZERO } else { Duration::try_from_secs_f64(x).unwrap_or(Duration::MAX) }
}

fn latency_ms(t0: Instant) -> f64 {
    py_round(t0.elapsed().as_secs_f64() * 1000.0, 1)
}

async fn setting(state: &AppState, key: &str) -> anyhow::Result<Value> {
    runtime_config::get_setting(state, key).await
}

/// `int(value)`.
async fn int_setting(state: &AppState, key: &str) -> anyhow::Result<i64> {
    let v = setting(state, key).await?;
    int_of(&v).and_then(|i| i64::try_from(i).ok()).ok_or_else(|| anyhow::anyhow!("int({v}) failed for {key}"))
}

/// `float(value)`.
async fn float_setting(state: &AppState, key: &str) -> anyhow::Result<f64> {
    let v = setting(state, key).await?;
    match &v {
        Value::Bool(b) => Some(f64::from(u8::from(*b))),
        Value::Number(n) => n.as_f64(),
        Value::String(s) => strip(s).replace('_', "").parse().ok(),
        _ => None,
    }
    .ok_or_else(|| anyhow::anyhow!("float({v}) failed for {key}"))
}

/// `str(value)`.
fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "None".into(),
        Value::Bool(b) => if *b { "True" } else { "False" }.into(),
        Value::Number(n) if n.is_f64() => pyfloat::repr(n.as_f64().unwrap_or(f64::NAN)),
        Value::Number(n) => n.to_string(),
        other => pyjson::dumps_spaced(other),
    }
}

/// `_probe_with_retries`: the first online result, else the last one.
async fn with_retries<F, Fut>(mut make: F, attempts: i64, delay: f64) -> Value
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Value>,
{
    let n = attempts.max(1);
    let mut result = json!({"online": false, "error": "no_attempt"});
    for i in 0..n {
        result = make().await;
        if result.get("online").is_some_and(py_truthy) {
            return result;
        }
        if i < n - 1 && delay > 0.0 {
            tokio::time::sleep(secs(delay)).await;
        }
    }
    result
}

/// `probe_once`: probe, record transitions, update the snapshot and its Redis mirror.
pub async fn probe_once(state: &AppState) -> anyhow::Result<Value> {
    let attempts = int_setting(state, "trove_status_probe_attempts").await?;
    let delay = float_setting(state, "trove_status_probe_retry_delay_seconds").await?;
    let settings = state.settings();
    let timeout = settings.trove_status_timeout_seconds;
    let url = settings.trove_status_auth_url.as_str();

    let auth = with_retries(|| probe_auth(url, timeout), attempts, delay).await;
    let auth_online = auth["online"] == true;

    let deep = py_truthy(&setting(state, "trove_status_game_deep_probe").await?);
    let hold = float_setting(state, "trove_status_game_hold_seconds").await?;
    let random_opener = py_truthy(&setting(state, "trove_status_game_random_opener").await?);

    let mut environments = Map::new();
    let mut statuses = Vec::new();
    for env in core::ENVIRONMENTS {
        let host = strip(&py_str(&setting(state, &format!("trove_status_{env}_host")).await?)).to_owned();
        let port = int_setting(state, &format!("trove_status_{env}_port")).await?;
        let hello = setting(state, &format!("trove_status_{env}_hello_hex")).await?;
        let hello_hex = if py_truthy(&hello) { py_str(&hello) } else { String::new() };
        let game =
            with_retries(|| probe_game(&host, port, deep, &hello_hex, hold, random_opener, timeout), attempts, delay)
                .await;
        let online = game["online"] == true;
        let status = core::verdict(auth_online, online);
        environments.insert(env.into(), json!({"status": status, "online": game["online"], "game": game}));
        statuses.push((env, status));
    }

    let blackout = !auth_online && statuses.iter().all(|(_, s)| *s != "online");
    if blackout && py_truthy(&setting(state, "trove_status_local_check_enabled").await?) {
        let hosts = py_str(&setting(state, "trove_status_local_check_hosts").await?);
        let anchor_timeout = float_setting(state, "trove_status_local_check_timeout_seconds").await?;
        if !local_network_is_up(&hosts, anchor_timeout).await {
            tracing::warn!(
                "trove status: every probe failed and no uplink anchor answered - local connectivity loss, \
                 discarding this round (no downtime recorded)"
            );
            let mut discarded = current().unwrap_or_else(unknown_snapshot);
            discarded["auth"] = auth;
            discarded["local_network"] = Value::Bool(false);
            return Ok(discarded);
        }
    }

    for (env, status) in &statuses {
        let online = environments[*env]["online"] == true;
        record_transition(state, env, status, online).await;
    }
    let snapshot = json!({
        "overall": core::overall(&statuses),
        "auth": auth,
        "environments": environments,
        "checked_at": unix_now(),
    });
    if let Ok(mut cur) = CURRENT.write() {
        *cur = Some(snapshot.clone());
    }
    publish_snapshot(state, &snapshot).await;
    Ok(snapshot)
}

/// `_record_transition`: open a segment when `env` changes status, closing the
/// open one. Best-effort.
async fn record_transition(state: &AppState, env: &str, status: &str, online: bool) {
    match try_record(state, env, status, online).await {
        Ok(true) => tracing::info!("trove status: {env} -> {status}"),
        Ok(false) => {}
        Err(e) => tracing::warn!("trove status: failed to record {env} transition: {e:#}"),
    }
}

pub(super) async fn try_record(state: &AppState, env: &str, status: &str, online: bool) -> anyhow::Result<bool> {
    let now = unix_now();
    let db = state.inner.db.mongo.as_ref().ok_or_else(|| anyhow::anyhow!("MONGO_URI not configured"))?;
    let coll = db.collection::<Document>(COLLECTION);
    if let Some(last) = coll.find_one(doc! { "env": env }).sort(doc! { "started_at": -1 }).await? {
        let seg = segment_from_doc(&last)?;
        if seg.ended_at.is_none() {
            if seg.status == status {
                return Ok(false);
            }
            let id = last.get("_id").cloned().unwrap_or(Bson::Null);
            // Beanie's save(): $set of every field, so stored types come out normalized.
            let set = doc! {
                "env": env,
                "status": seg.status,
                "online": seg.online,
                "started_at": bson_int(seg.started_at),
                "ended_at": bson_int(now),
            };
            coll.update_one(doc! { "_id": id }, doc! { "$set": set }).upsert(true).await?;
        }
    }
    coll.insert_one(doc! {
        "env": env,
        "status": status,
        "online": online,
        "started_at": bson_int(now),
        "ended_at": Bson::Null,
    })
    .await?;
    Ok(true)
}

/// `_publish_snapshot`: `SET kiwi:status:current json.dumps(snapshot) EX 600`.
pub(super) async fn publish_snapshot(state: &AppState, snapshot: &Value) {
    let Some(mut conn) = state.inner.db.redis.clone() else { return };
    let r: redis::RedisResult<()> = redis::cmd("SET")
        .arg(REDIS_KEY)
        .arg(pyjson::dumps_spaced(snapshot))
        .arg("EX")
        .arg(REDIS_TTL)
        .query_async(&mut conn)
        .await;
    if let Err(e) = r {
        tracing::warn!("status: failed to cache snapshot in Redis: {e}");
    }
}

// ---------------------------------------------------------------------------
// auth gateway

pub(super) struct Reply {
    pub status: u16,
    pub headers: Vec<String>,
    pub body: String,
}

/// `_probe_auth`: the Glyph login POST, judged by `auth_is_reachable`; a
/// failure carries the httpx exception name.
pub async fn probe_auth(url: &str, timeout: f64) -> Value {
    let t0 = Instant::now();
    let reply = post_login(url, secs(timeout)).await;
    let latency = latency_ms(t0);
    match reply {
        Ok(r) => {
            let (online, error) = core::auth_is_reachable(r.status, r.headers.iter().map(String::as_str), &r.body);
            json!({"online": online, "http_status": r.status, "latency_ms": latency, "error": error})
        }
        Err(name) => json!({"online": false, "http_status": null, "latency_ms": latency, "error": name}),
    }
}

pub(super) async fn post_login(url: &str, timeout: Duration) -> Result<Reply, &'static str> {
    let uri: Uri = url.parse().map_err(|_| "InvalidURL")?;
    let tls = match uri.scheme_str() {
        Some("https") => true,
        Some("http") => false,
        _ => return Err("UnsupportedProtocol"),
    };
    let host = uri.host().filter(|h| !h.is_empty()).ok_or("UnsupportedProtocol")?;
    let port = uri.port_u16().unwrap_or(if tls { 443 } else { 80 });
    let authority = uri.authority().map(|a| a.as_str()).unwrap_or(host);
    let authority = authority.rsplit_once('@').map_or(authority, |(_, a)| a);
    let path = uri.path_and_query().map_or("/", |p| p.as_str());
    let req = Request::post(path)
        .header(header::HOST, authority)
        .header(header::ACCEPT, "*/*")
        .header(header::USER_AGENT, AUTH_PROBE_UA)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::CONTENT_LENGTH, AUTH_FORM.len())
        .body(Full::new(Bytes::from_static(AUTH_FORM.as_bytes())))
        .map_err(|_| "InvalidURL")?;
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let connect = async {
        let tcp = TcpStream::connect((bare, port)).await.map_err(|_| "ConnectError")?;
        Ok::<_, &'static str>(tcp)
    };
    let tcp = tokio::time::timeout(timeout, connect).await.map_err(|_| "ConnectTimeout")??;
    if !tls {
        return exchange(tcp, req, timeout).await;
    }
    let name = ServerName::try_from(bare.to_owned()).map_err(|_| "ConnectError")?;
    let handshake = TlsConnector::from(crate::http::tls_config()).connect(name, tcp);
    let stream =
        tokio::time::timeout(timeout, handshake).await.map_err(|_| "ConnectTimeout")?.map_err(|_| "ConnectError")?;
    exchange(stream, req, timeout).await
}

async fn exchange<S>(io: S, req: Request<Full<Bytes>>, timeout: Duration) -> Result<Reply, &'static str>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sender, conn) =
        hyper::client::conn::http1::handshake(TokioIo::new(io)).await.map_err(|_| "ConnectError")?;
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let protocol =
        |e: hyper::Error| if e.is_parse() || e.is_incomplete_message() { "RemoteProtocolError" } else { "ReadError" };
    let resp =
        tokio::time::timeout(timeout, sender.send_request(req)).await.map_err(|_| "ReadTimeout")?.map_err(protocol)?;
    let status = resp.status().as_u16();
    let headers = resp.headers().keys().map(|k| k.as_str().to_owned()).collect();
    let body = tokio::time::timeout(timeout, resp.into_body().collect())
        .await
        .map_err(|_| "ReadTimeout")?
        .map_err(protocol)?;
    Ok(Reply { status, headers, body: String::from_utf8_lossy(&body.to_bytes()).into_owned() })
}

// ---------------------------------------------------------------------------
// game sockets

/// Python's exception class name for a failed connect.
fn os_error_name(e: &io::Error) -> &'static str {
    match e.kind() {
        io::ErrorKind::ConnectionRefused => "ConnectionRefusedError",
        io::ErrorKind::ConnectionReset => "ConnectionResetError",
        io::ErrorKind::ConnectionAborted => "ConnectionAbortedError",
        io::ErrorKind::TimedOut => "TimeoutError",
        io::ErrorKind::PermissionDenied => "PermissionError",
        _ => "OSError",
    }
}

/// `asyncio.open_connection(host, port)`: every resolved address in turn.
async fn connect(host: &str, port: i64) -> Result<TcpStream, &'static str> {
    let port = u16::try_from(port).map_err(|_| "OverflowError")?;
    let addrs: Vec<_> = tokio::net::lookup_host((host, port)).await.map_err(|_| "gaierror")?.collect();
    let mut names = Vec::new();
    for addr in addrs {
        match TcpStream::connect(addr).await {
            Ok(s) => return Ok(s),
            Err(e) => names.push(os_error_name(&e)),
        }
    }
    match names.first() {
        None => Err("gaierror"),
        Some(first) if names.iter().all(|n| n == first) => Err(first),
        Some(_) => Err("OSError"),
    }
}

/// `_random_opener`: the frame header plus 32 random bytes.
fn random_opener() -> Vec<u8> {
    let mut body = [0u8; 32];
    let _ = getrandom::fill(&mut body);
    [&GLS_OPENER_HEADER[..], &body].concat()
}

/// `_glsserver_holds_session`: `Some(true)` held, `Some(false)` dropped after
/// ~nothing, `None` inconclusive.
async fn holds_session(stream: &mut TcpStream, hello: &[u8], hold: f64) -> Option<bool> {
    stream.write_all(hello).await.ok()?;
    let mut buf = [0u8; 4096];
    let mut total = 0;
    loop {
        match tokio::time::timeout(secs(hold), stream.read(&mut buf)).await {
            Err(_) => return Some(true),
            Ok(Ok(0)) => return Some(total > GLS_SUBSTANTIVE_BYTES),
            Ok(Ok(n)) => {
                total += n;
                if total >= GLS_MAX_READ_BYTES {
                    return Some(true);
                }
            }
            Ok(Err(_)) => return None,
        }
    }
}

/// `_probe_game`.
pub async fn probe_game(
    host: &str,
    port: i64,
    deep: bool,
    hello_hex: &str,
    hold: f64,
    random: bool,
    timeout: f64,
) -> Value {
    let t0 = Instant::now();
    if host.is_empty() || port <= 0 {
        return json!({"online": false, "host": host, "port": port, "latency_ms": 0.0, "error": "not_configured"});
    }
    let connected = tokio::time::timeout(secs(timeout), connect(host, port)).await.unwrap_or(Err("TimeoutError"));
    let mut stream = match connected {
        Ok(s) => s,
        Err(name) => {
            return json!({"online": false, "host": host, "port": port, "latency_ms": latency_ms(t0), "error": name});
        }
    };
    let (opener, mode) = match (deep && !hello_hex.is_empty(), random) {
        (false, _) => (Vec::new(), None),
        (true, true) => (random_opener(), Some("random")),
        (true, false) => (core::fromhex(hello_hex).unwrap_or_default(), Some("replay")),
    };
    if opener.is_empty() {
        let latency = latency_ms(t0);
        return json!({"online": true, "host": host, "port": port, "latency_ms": latency, "error": null, "probe": "tcp"});
    }
    let held = holds_session(&mut stream, &opener, hold).await;
    let latency = latency_ms(t0);
    match held {
        None => json!({
            "online": true, "host": host, "port": port, "latency_ms": latency, "error": null, "probe": "tcp_fallback",
        }),
        Some(held) => json!({
            "online": held,
            "host": host,
            "port": port,
            "latency_ms": latency,
            "error": if held { None } else { Some("glsserver_dropped") },
            "probe": "glsserver",
            "opener": mode,
        }),
    }
}

/// `_local_network_is_up`: any anchor accepting a TCP connection.
pub async fn local_network_is_up(hosts: &str, timeout: f64) -> bool {
    let anchors = core::parse_anchors(hosts);
    if anchors.is_empty() {
        return true;
    }
    let probes = anchors.iter().map(|(host, port)| async move {
        let port = i64::try_from(*port).unwrap_or(i64::MAX);
        matches!(tokio::time::timeout(secs(timeout), connect(host, port)).await, Ok(Ok(_)))
    });
    futures::future::join_all(probes).await.into_iter().any(|up| up)
}
