//! Outbound HTTPS: ring as the process crypto provider and one rustls client
//! config (certifi-style bundled webpki roots) behind every client. Callers set
//! their own timeouts, redirects and headers to mirror their httpx client.

use std::sync::{Arc, LazyLock};

use rustls::ClientConfig;

/// `app/core/http.py` `KIWI_UA`.
pub const KIWI_UA: &str = "KiwiAPI/1.0";

static TLS: LazyLock<Arc<ClientConfig>> = LazyLock::new(|| {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    let cfg = ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("ring supports the default TLS versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    Arc::new(cfg)
});

pub fn tls_config() -> Arc<ClientConfig> {
    TLS.clone()
}

/// `reqwest::Client::builder()` on the shared TLS config.
pub fn client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder().tls_backend_preconfigured(ClientConfig::clone(&TLS))
}
