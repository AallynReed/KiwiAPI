//! The website container (`app.web.main:app`, `trove.aallyn.net`) as the
//! `kiwi-web` binary: the same middleware stack, `/static`, the page-view
//! collector, robots/sitemap, `/gamedata/{name}` and every page whose view
//! model is plain. Anything else (the server-rendered first paints of
//! `app/site/ssr.py`) is proxied to the Python website (`PY_WEB_UPSTREAM`,
//! the `web-py` service), exactly as kiwi-server fronts api-py.
//!
//! Stack, outer to inner: client address -> static gzip -> request id ->
//! security -> regional maintenance gate -> HEAD as GET -> route.

pub mod flags;
pub mod geo;
pub mod gzip;
pub mod internal;
pub mod live;
pub mod meta;
pub mod nav;
pub mod pages;
pub mod pageviews;
pub mod static_files;
pub mod swf_docs;
pub mod templates;

#[cfg(test)]
mod tests;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use axum::{
    Router,
    extract::{ConnectInfo, DefaultBodyLimit, FromRequestParts, Request, State},
    http::{StatusCode, request::Parts},
    middleware::{Next, from_fn, from_fn_with_state},
    response::Response,
    routing::{any, get, post},
};
use tower::ServiceBuilder;

use crate::config::Settings;
use crate::middleware::{self, Mw, MwConfig, errors, head, request_id, security};
use crate::proxy::proxy;
use crate::state::AppState;

/// Everything the website's routes and layers share.
pub struct Web {
    pub settings: Settings,
    pub mw_cfg: MwConfig,
    pub api: internal::InternalApi,
    pub flags: flags::FlagCache,
    pub templates: templates::Templates,
    pub swf_docs: swf_docs::SwfDocs,
    pub internal_key: String,
    pub beacon_enabled: bool,
    pub own_suffix: String,
    pub beacon: pageviews::Beacon,
    pub rate: pageviews::RateLimit,
    pub country_db: Arc<geo::Database>,
    pub asn_db: Arc<geo::Database>,
    pub countries: geo::Countries,
    pub maintenance_page: Arc<str>,
    pub sitemap: tokio::sync::Mutex<Option<(Instant, String)>>,
    pub static_root: PathBuf,
}

impl Web {
    pub fn new(settings: &Settings, mw_cfg: MwConfig) -> Self {
        let site_root = PathBuf::from(&settings.site_root);
        let api = internal::InternalApi::new(&settings.internal_api_url);
        let geoip = PathBuf::from(&settings.geoip_dir);
        let page = std::fs::read_to_string(geo::PAGE_PATH).map(|s| s.replace("\r\n", "\n")).unwrap_or_else(|e| {
            tracing::error!("{}: {e}", geo::PAGE_PATH);
            String::new()
        });
        Web {
            mw_cfg,
            flags: flags::FlagCache::new(api.clone()),
            templates: templates::Templates::new(&site_root),
            swf_docs: swf_docs::SwfDocs::new(&site_root),
            internal_key: settings.internal_key.clone(),
            beacon_enabled: !settings.internal_key.is_empty() && settings.pageview_tracking_enabled,
            own_suffix: kiwi_core::web::classify::own_suffix(&settings.app_url),
            beacon: pageviews::Beacon::default(),
            rate: pageviews::RateLimit::default(),
            country_db: geo::Database::new("country", &geoip),
            asn_db: geo::Database::new("asn", &geoip),
            countries: geo::Countries::new(),
            maintenance_page: page.into(),
            sitemap: tokio::sync::Mutex::new(None),
            static_root: site_root.join("static"),
            api,
            settings: settings.clone(),
        }
    }

    /// `settings.internal_key or settings.secret_key`.
    pub fn salt(&self) -> &str {
        if self.internal_key.is_empty() { &self.settings.secret_key } else { &self.internal_key }
    }

    pub fn api_base(&self) -> &str {
        self.settings.api_url.trim_end_matches('/')
    }

    pub fn app_base(&self) -> &str {
        self.settings.app_url.trim_end_matches('/')
    }

    /// The lifespan: the beacon loop and, when it runs, the ASN refresh loop.
    pub fn start(self: &Arc<Self>) {
        if self.beacon_enabled {
            pageviews::Beacon::start(self.clone());
            self.asn_db.ensure_running();
        }
    }
}

/// The page router's `Depends(web_flags.resolve)`: the resolved flags, or the
/// 404 of a switched-off feature.
pub struct Gate(pub flags::Flags);

impl FromRequestParts<Arc<Web>> for Gate {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, web: &Arc<Web>) -> Result<Self, Response> {
        let flags = web.flags.get().await;
        let path = middleware::pyurl::url_path(&parts.uri);
        if flags::feature_blocks(&path, &flags) {
            return Err(crate::error::http_exception(
                StatusCode::NOT_FOUND,
                None,
                &parts.method,
                &path,
                &parts.headers,
            ));
        }
        Ok(Gate(flags))
    }
}

fn ported(web: Arc<Web>) -> Router<AppState> {
    let static_root = web.static_root.clone();
    Router::new()
        .merge(pages::router())
        .merge(live::router())
        .route("/robots.txt", get(meta::robots))
        .route("/BingSiteAuth.xml", get(meta::bing))
        .route("/sitemap.xml", get(meta::sitemap))
        .route("/s", post(pageviews::collect))
        .route("/health", get(pages::health))
        .route("/gamedata/{name}", get(pages::gamedata))
        .route("/static/{*path}", any(move |req: Request| static_files::serve(static_root.clone(), req)))
        .with_state(web)
}

/// A `%2F` or `//` in the raw path routes differently in Starlette: Python's.
async fn encoded_slash(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let raw = req.uri().path();
    if (raw.contains("%2F") || raw.contains("%2f") || raw.contains("//"))
        && let Some(&ConnectInfo(peer)) = req.extensions().get::<ConnectInfo<SocketAddr>>()
    {
        return proxy(State(state), ConnectInfo(peer), req).await;
    }
    next.run(req).await
}

/// The website app. `KIWI_PROXY_ALL` sends every request to Python.
pub fn app(state: AppState, web: Arc<Web>) -> Router {
    let proxy_all = std::env::var("KIWI_PROXY_ALL").is_ok_and(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"));
    let routes = if proxy_all { Router::new() } else { ported(web.clone()) };
    let trusted = middleware::forwarded::TrustedHosts::parse(
        &std::env::var("FORWARDED_ALLOW_IPS").unwrap_or_else(|_| "127.0.0.1".into()),
    );
    let api = web.api.clone();
    let remote: security::RemoteOrigins = Arc::new(move || {
        let api = api.clone();
        Box::pin(async move { embed_allowlist(&api).await })
    });
    let mw = Arc::new(Mw::new(web.mw_cfg.clone(), None, security::EmbedSource::Remote(remote), trusted));
    let stack = ServiceBuilder::new()
        .layer(from_fn(middleware::mark))
        .layer(from_fn_with_state(mw.clone(), middleware::client_addr))
        .layer(from_fn(gzip::static_compression))
        .layer(from_fn(request_id::request_context))
        .layer(from_fn_with_state(mw, security::security))
        .layer(from_fn_with_state(web.clone(), geo::gate))
        .layer(from_fn(head::head_as_get))
        .layer(DefaultBodyLimit::disable());
    let inner = routes
        .method_not_allowed_fallback(errors::method_not_allowed)
        .layer(stack)
        .fallback(proxy)
        .layer(from_fn_with_state(state.clone(), encoded_slash))
        .with_state(state);
    middleware::outer(inner)
}

/// `_read_allowlist()` in the website: the api's `/site/embed/allowed-origins`.
async fn embed_allowlist(api: &internal::InternalApi) -> String {
    use serde_json::Value;
    if let Some(Value::Object(data)) = api.get("/site/embed/allowed-origins", &[], 8.0, &[]).await
        && let Some(Value::Array(origins)) = data.get("origins")
    {
        return origins
            .iter()
            .map(|o| match o {
                Value::String(s) => s.clone(),
                other => templates::py_str(&minijinja::Value::from_serialize(other)),
            })
            .collect::<Vec<_>>()
            .join(" ");
    }
    tracing::warn!("embed: allowlist unreadable - framing denied until it returns");
    String::new()
}
