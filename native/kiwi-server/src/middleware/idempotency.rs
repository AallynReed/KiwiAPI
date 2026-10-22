//! `app/core/idempotency.py`: an `Idempotency-Key` on POST/PUT/PATCH/DELETE
//! claims `idem:<sha256>` in Redis; a finished 2xx is replayed (body, status,
//! content type and `Idempotent-Replay: true` only), an in-flight one is a 409.
//! Fails open on any Redis or record error. Records use Python's
//! `json.dumps` spacing so both servers read each other's.
//!
//! One deliberate difference: Python rebuilt the first response from
//! `dict(response.headers)`, keeping only the first of repeated headers (two
//! Set-Cookie became one). Here every header is kept.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::Response,
};
use base64::Engine;
use sha2::{Digest, Sha256};

use super::Mw;
use super::errors::envelope;
use super::pyurl::{ascii_json_str, first, header_value, url_path};

const MAX_KEY_LEN: usize = 200;

pub enum IdemStore {
    Redis(redis::aio::ConnectionManager),
    /// For tests: key -> (value, ttl seconds).
    Memory(Mutex<HashMap<String, (String, u64)>>),
}

impl IdemStore {
    async fn claim(&self, key: &str, ttl: u64) -> anyhow::Result<bool> {
        match self {
            IdemStore::Redis(conn) => {
                let mut conn = conn.clone();
                let r: Option<String> = redis::cmd("SET")
                    .arg(key)
                    .arg(r#"{"s":"lock"}"#)
                    .arg("NX")
                    .arg("EX")
                    .arg(ttl)
                    .query_async(&mut conn)
                    .await?;
                Ok(r.is_some())
            }
            IdemStore::Memory(m) => {
                let mut m = m.lock().map_err(|_| anyhow::anyhow!("poisoned"))?;
                if m.contains_key(key) {
                    return Ok(false);
                }
                m.insert(key.to_owned(), (r#"{"s":"lock"}"#.to_owned(), ttl));
                Ok(true)
            }
        }
    }

    async fn get(&self, key: &str) -> anyhow::Result<Option<String>> {
        match self {
            IdemStore::Redis(conn) => {
                let mut conn = conn.clone();
                Ok(redis::cmd("GET").arg(key).query_async(&mut conn).await?)
            }
            IdemStore::Memory(m) => {
                Ok(m.lock().map_err(|_| anyhow::anyhow!("poisoned"))?.get(key).map(|v| v.0.clone()))
            }
        }
    }

    async fn put(&self, key: &str, value: &str, ttl: u64) -> anyhow::Result<()> {
        match self {
            IdemStore::Redis(conn) => {
                let mut conn = conn.clone();
                let _: () = redis::cmd("SET").arg(key).arg(value).arg("EX").arg(ttl).query_async(&mut conn).await?;
                Ok(())
            }
            IdemStore::Memory(m) => {
                m.lock().map_err(|_| anyhow::anyhow!("poisoned"))?.insert(key.to_owned(), (value.to_owned(), ttl));
                Ok(())
            }
        }
    }

    async fn release(&self, key: &str) {
        match self {
            IdemStore::Redis(conn) => {
                let mut conn = conn.clone();
                let _: Result<(), _> = redis::cmd("DEL").arg(key).query_async(&mut conn).await;
            }
            IdemStore::Memory(m) => {
                if let Ok(mut m) = m.lock() {
                    m.remove(key);
                }
            }
        }
    }
}

/// `_storage_key`: namespaced by the Authorization header, method and path.
pub fn storage_key(authorization: &str, method: &str, path: &str, key: &str) -> String {
    let cred = hex::encode(Sha256::digest(authorization.as_bytes()));
    let raw = format!("{}:{method}:{path}:{key}", &cred[..16]);
    format!("idem:{}", hex::encode(Sha256::digest(raw.as_bytes())))
}

/// Python's `base64.b64decode` (non-alphabet characters discarded).
fn b64decode(s: &str) -> Option<Vec<u8>> {
    let clean: String = s.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=')).collect();
    base64::engine::general_purpose::STANDARD.decode(clean).ok()
}

enum Lookup {
    Replay(Response),
    Conflict,
}

fn replay(stored: Option<&str>) -> Option<Lookup> {
    let data: serde_json::Value = match stored {
        Some(s) if !s.is_empty() => serde_json::from_str(s).ok()?,
        _ => return Some(Lookup::Conflict),
    };
    let obj = data.as_object()?;
    if obj.get("s").and_then(|v| v.as_str()) != Some("done") {
        return Some(Lookup::Conflict);
    }
    let body = b64decode(obj.get("b")?.as_str()?)?;
    let status = StatusCode::from_u16(u16::try_from(obj.get("c")?.as_u64()?).ok()?).ok()?;
    let ct = match obj.get("ct") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => Some(v.as_str()?.to_owned()),
    };
    let mut res = Response::new(Body::from(body));
    *res.status_mut() = status;
    let h = res.headers_mut();
    h.insert("idempotent-replay", HeaderValue::from_static("true"));
    if let Some(mut ct) = ct {
        if ct.starts_with("text/") && !ct.to_lowercase().contains("charset=") {
            ct.push_str("; charset=utf-8");
        }
        h.insert(header::CONTENT_TYPE, header_value(&ct));
    }
    Some(Lookup::Replay(res))
}

fn record(status: u16, ct: Option<&str>, body: &[u8]) -> String {
    format!(
        r#"{{"s": "done", "c": {status}, "ct": {}, "b": "{}"}}"#,
        ct.map_or_else(|| "null".to_owned(), ascii_json_str),
        base64::engine::general_purpose::STANDARD.encode(body)
    )
}

pub async fn idempotency(State(mw): State<Arc<Mw>>, req: Request, next: Next) -> Response {
    let Some(store) = mw.idem.as_ref() else { return next.run(req).await };
    let key = first(req.headers(), "idempotency-key").unwrap_or_default();
    let unsafe_method = matches!(*req.method(), Method::POST | Method::PUT | Method::PATCH | Method::DELETE);
    if key.is_empty() || key.chars().count() > MAX_KEY_LEN || !unsafe_method {
        return next.run(req).await;
    }
    let auth = first(req.headers(), "authorization").unwrap_or_default();
    let sk = storage_key(&auth, req.method().as_str(), &url_path(req.uri()), &key);

    match store.claim(&sk, mw.cfg.idempotency_lock_seconds).await {
        Ok(true) => {}
        Ok(false) => {
            let found = match store.get(&sk).await {
                Ok(stored) => replay(stored.as_deref()),
                Err(e) => {
                    tracing::warn!("idempotency lookup failed; proceeding without it: {e}");
                    return next.run(req).await;
                }
            };
            match found {
                Some(Lookup::Replay(res)) => return res,
                Some(Lookup::Conflict) => {
                    return envelope(409, "conflict", "A request with this Idempotency-Key is already in progress.");
                }
                None => {
                    tracing::warn!("idempotency lookup failed; proceeding without it: unreadable record");
                    return next.run(req).await;
                }
            }
        }
        Err(e) => {
            tracing::warn!("idempotency lookup failed; proceeding without it: {e}");
            return next.run(req).await;
        }
    }

    let res = next.run(req).await;
    let (parts, body) = res.into_parts();
    let bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(_) => {
            store.release(&sk).await;
            return Response::from_parts(parts, Body::empty());
        }
    };
    if parts.status.is_success() {
        let ct = parts.headers.get(header::CONTENT_TYPE).map(|v| super::pyurl::latin1(v.as_bytes()));
        let rec = record(parts.status.as_u16(), ct.as_deref(), &bytes);
        if let Err(e) = store.put(&sk, &rec, mw.cfg.idempotency_result_seconds).await {
            tracing::warn!("idempotency store failed: {e}");
        }
    } else {
        store.release(&sk).await;
    }
    Response::from_parts(parts, Body::from(bytes))
}
