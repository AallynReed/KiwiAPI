//! `app/trove/updates/cdn.py` `CdnClient`: the httpx client (30s, `KiwiAPI/1.0`,
//! no redirects, `raise_for_status`) over Trion's plain-HTTP update CDN.

use std::{future::Future, sync::OnceLock, time::Duration};

use axum::http::{HeaderValue, StatusCode, header};
use bytes::Bytes;
use kiwi_core::update_archive::{self as ua, ManifestEntry, Pointer};
use reqwest::{Client, redirect::Policy};

use super::SyncError;
use crate::http::KIWI_UA;

/// What `sync_branch` needs from the CDN (a fake in tests).
pub trait Cdn: Sync {
    fn fetch_pointer(&self, pointer_file: &str) -> impl Future<Output = Result<Pointer, SyncError>> + Send;
    fn fetch_manifest(
        &self,
        content_path: &str,
        version: &str,
    ) -> impl Future<Output = Result<(String, Vec<ManifestEntry>), SyncError>> + Send;
    fn download_file(
        &self,
        content_path: &str,
        path: &str,
        sha1: &str,
        expected_size: Option<i64>,
    ) -> impl Future<Output = Result<Bytes, SyncError>> + Send;
}

pub struct CdnClient {
    base: String,
    prefix: String,
}

impl CdnClient {
    pub fn new(base: &str, prefix: &str) -> Self {
        CdnClient { base: base.to_owned(), prefix: prefix.to_owned() }
    }

    async fn get(&self, url: String) -> Result<reqwest::Response, SyncError> {
        let resp = client().get(&url).send().await.map_err(transport)?;
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let reason = resp
            .extensions()
            .get::<hyper::ext::ReasonPhrase>()
            .map(|r| String::from_utf8_lossy(r.as_bytes()).chars().filter(char::is_ascii).collect())
            .or_else(|| status.canonical_reason().map(str::to_owned))
            .unwrap_or_default();
        let location = resp.headers().get(header::LOCATION).map(latin1);
        Err(SyncError::Http { status: status.as_u16(), message: status_message(status, &reason, &url, location) })
    }

    async fn text(&self, url: String) -> Result<String, SyncError> {
        let resp = self.get(url).await?;
        let ct = resp.headers().get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_owned);
        let body = resp.bytes().await.map_err(transport)?;
        Ok(text_of(&body, ct.as_deref()))
    }
}

impl Cdn for CdnClient {
    async fn fetch_pointer(&self, pointer_file: &str) -> Result<Pointer, SyncError> {
        let text = self.text(ua::pointer_url(&self.base, &self.prefix, pointer_file)).await?;
        ua::parse_pointer(&text).map_err(SyncError::Cdn)
    }

    async fn fetch_manifest(
        &self,
        content_path: &str,
        version: &str,
    ) -> Result<(String, Vec<ManifestEntry>), SyncError> {
        let text = self.text(ua::manifest_url(&self.base, &self.prefix, content_path, version)).await?;
        ua::parse_manifest(&text).map_err(SyncError::Cdn)
    }

    async fn download_file(
        &self,
        content_path: &str,
        path: &str,
        sha1: &str,
        expected_size: Option<i64>,
    ) -> Result<Bytes, SyncError> {
        let resp = self.get(ua::file_url(&self.base, &self.prefix, content_path, path, sha1)).await?;
        let data = resp.bytes().await.map_err(transport)?;
        if let Some(want) = expected_size
            && i64::try_from(data.len()) != Ok(want)
        {
            return Err(SyncError::Cdn(format!("size mismatch for {path}: got {}, manifest {want}", data.len())));
        }
        Ok(data)
    }
}

fn client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        let t = Duration::from_secs(30);
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(header::USER_AGENT, HeaderValue::from_static(KIWI_UA));
        crate::http::client_builder()
            .default_headers(headers)
            .connect_timeout(t)
            .read_timeout(t)
            .redirect(Policy::none())
            .no_proxy()
            .build()
            .expect("reqwest client")
    })
}

fn latin1(v: &HeaderValue) -> String {
    v.as_bytes().iter().map(|&b| b as char).collect()
}

/// `str()` of the httpx error: a timeout's is empty, a failed connect is
/// anyio's "All connection attempts failed", a failed lookup the `gaierror`.
fn transport(e: reqwest::Error) -> SyncError {
    if e.is_timeout() {
        return SyncError::Transport(String::new());
    }
    let root = root_cause(&e);
    if !e.is_connect() {
        return SyncError::Transport(root);
    }
    let text = match root.strip_prefix("failed to lookup address information: ") {
        Some(msg) => {
            let code = match msg {
                "Temporary failure in name resolution" => -3,
                "No address associated with hostname" => -5,
                _ => -2,
            };
            format!("[Errno {code}] {msg}")
        }
        None => "All connection attempts failed".to_owned(),
    };
    SyncError::Transport(text)
}

fn root_cause(e: &(dyn std::error::Error + 'static)) -> String {
    let mut cur = e;
    while let Some(next) = cur.source() {
        cur = next;
    }
    cur.to_string()
}

/// httpx `Response.raise_for_status()`'s message.
pub fn status_message(status: StatusCode, reason: &str, url: &str, location: Option<String>) -> String {
    let code = status.as_u16();
    let kind = match code / 100 {
        1 => "Informational response",
        3 => "Redirect response",
        4 => "Client error",
        5 => "Server error",
        _ => "Invalid status code",
    };
    let redirect = matches!(code, 301 | 302 | 303 | 307 | 308);
    let loc = match location {
        Some(l) if redirect => format!("Redirect location: '{l}'\n"),
        _ => String::new(),
    };
    format!(
        "{kind} '{code} {reason}' for url '{url}'\n{loc}For more information check: https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/{code}"
    )
}

/// The `charset` parameter of a Content-Type, as httpx reads it.
fn charset_of(ct: &str) -> Option<String> {
    ct.split(';').skip(1).find_map(|p| {
        let (k, v) = p.split_once('=')?;
        k.trim().eq_ignore_ascii_case("charset").then(|| v.trim().trim_matches(['"', '\'']).to_owned())
    })
}

/// `Response.text`: the declared charset, else UTF-8, errors replaced.
pub fn text_of(body: &[u8], content_type: Option<&str>) -> String {
    if body.is_empty() {
        return String::new();
    }
    let charset = content_type.and_then(charset_of).map(|c| c.to_ascii_lowercase());
    match charset.as_deref() {
        Some("iso-8859-1" | "latin-1" | "latin1" | "l1" | "iso8859-1" | "8859") => {
            body.iter().map(|&b| b as char).collect()
        }
        Some("ascii" | "us-ascii") => body.iter().map(|&b| if b < 0x80 { b as char } else { '\u{fffd}' }).collect(),
        _ => String::from_utf8_lossy(body).into_owned(),
    }
}
