//! The outbound clients, configured like the `httpx.AsyncClient`s they
//! replace: HTTP/1.1, gzip/deflate, certifi-style bundled roots, httpx's
//! per-phase timeouts and redirect policy.

use std::{sync::OnceLock, time::Duration};

use axum::http::{HeaderMap, HeaderValue, header};
use reqwest::{Client, redirect::Policy};

/// `app/core/http.py` `KIWI_UA`.
pub const KIWI_UA: &str = "KiwiAPI/1.0";
/// `app/trove/feeds.py` `_UA`.
pub const FEEDS_UA: &str = "KiwiAPI/1.0 (+https://api.aallyn.net)";
/// `app/trove/feeds.py` `_BILIBILI_UA`.
pub const BILIBILI_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0 Safari/537.36";

pub struct Clients {
    /// `app.core.http.fetch`: follows redirects.
    pub fetch: Client,
    /// Twitch and YouTube: `_UA`, no redirects.
    pub api: Client,
    /// The Bilibili search scrape.
    pub bilibili: Client,
    /// The thumbnail proxy: 10s, Referer, no redirects.
    pub image: Client,
}

fn tls() -> rustls::ClientConfig {
    let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("ring supports the default TLS versions")
        .with_root_certificates(roots)
        .with_no_client_auth()
}

fn build(timeout: u64, follow: bool, headers: HeaderMap) -> Client {
    let t = Duration::from_secs(timeout);
    Client::builder()
        .tls_backend_preconfigured(tls())
        .default_headers(headers)
        .connect_timeout(t)
        .read_timeout(t)
        .redirect(if follow { Policy::limited(20) } else { Policy::none() })
        .no_proxy()
        .build()
        .expect("reqwest client")
}

fn ua(value: &'static str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(header::USER_AGENT, HeaderValue::from_static(value));
    h
}

pub fn clients() -> &'static Clients {
    static CLIENTS: OnceLock<Clients> = OnceLock::new();
    CLIENTS.get_or_init(|| {
        let mut bili = ua(BILIBILI_UA);
        bili.insert(
            header::ACCEPT,
            HeaderValue::from_static(
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
            ),
        );
        bili.insert(header::ACCEPT_LANGUAGE, HeaderValue::from_static("en-US,en;q=0.5"));
        let mut image = ua(BILIBILI_UA);
        image.insert(header::REFERER, HeaderValue::from_static("https://www.bilibili.com/"));
        Clients {
            fetch: build(15, true, ua(KIWI_UA)),
            api: build(15, false, ua(FEEDS_UA)),
            bilibili: build(15, true, bili),
            image: build(10, false, image),
        }
    })
}

/// `resp.raise_for_status()` for the relays (any non-2xx is an error).
pub fn check_status(resp: reqwest::Response) -> anyhow::Result<reqwest::Response> {
    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!("HTTP {status} for url '{}'", resp.url());
    }
    Ok(resp)
}

/// `resp.json()`: `json.loads` of the body bytes.
pub async fn json(resp: reqwest::Response) -> anyhow::Result<serde_json::Value> {
    let bytes = resp.bytes().await?;
    crate::pyjson::loads(&bytes).map_err(|e| anyhow::anyhow!("invalid JSON body: {e:?}"))
}
