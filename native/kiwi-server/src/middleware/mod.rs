//! The Python middleware stack (`app/main.py`, `app/core/middleware.py`) as
//! tower layers, applied to the ported router only: proxied responses already
//! went through Python's copy. Outer to inner:
//!
//! client address (uvicorn `--proxy-headers`) -> gzip -> public-asset CORS ->
//! CORS -> request id + panic boundary -> api-host 301 -> security (body cap,
//! headers, CSP, cache rules) -> idempotency -> [usage] -> HEAD as GET -> route.

pub mod compression;
pub mod config;
pub mod cors;
pub mod errors;
pub mod forwarded;
pub mod head;
pub mod headers;
pub mod idempotency;
pub mod paths;
pub mod pyurl;
pub mod redirect;
pub mod request_id;
pub mod security;

#[cfg(test)]
mod tests;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Router,
    extract::{ConnectInfo, DefaultBodyLimit, Request},
    http::{HeaderValue, header},
    middleware::{Next, from_fn, from_fn_with_state, map_response},
    response::Response,
};
use tower::ServiceBuilder;

pub use config::MwConfig;
pub use forwarded::ClientAddr;
pub use idempotency::IdemStore;
pub use request_id::current as request_id;
pub use security::EmbedSource;

use security::{EmbedOrigins, StaticVersions};

/// Shared state of the layers.
pub struct Mw {
    pub cfg: MwConfig,
    pub idem: Option<IdemStore>,
    pub trusted: forwarded::TrustedHosts,
    embed: EmbedOrigins,
    static_versions: StaticVersions,
}

impl Mw {
    pub fn new(cfg: MwConfig, idem: Option<IdemStore>, embed: EmbedSource, trusted: forwarded::TrustedHosts) -> Self {
        let static_versions = StaticVersions::new(&cfg.static_root);
        Mw { cfg, idem, trusted, embed: EmbedOrigins::new(embed), static_versions }
    }

    pub fn from_state(state: &crate::state::AppState) -> Self {
        let trusted = forwarded::TrustedHosts::parse(
            &std::env::var("FORWARDED_ALLOW_IPS").unwrap_or_else(|_| "127.0.0.1".into()),
        );
        Mw::new(
            MwConfig::from_env(),
            state.inner.db.redis.clone().map(IdemStore::Redis),
            EmbedSource::Setting(state.clone()),
            trusted,
        )
    }

    async fn embed_origins(&self) -> Vec<String> {
        self.embed.get().await
    }
}

pub(crate) async fn client_addr(
    axum::extract::State(mw): axum::extract::State<Arc<Mw>>,
    mut req: Request,
    next: Next,
) -> Response {
    let peer = req.extensions().get::<ConnectInfo<SocketAddr>>().map(|c| c.0);
    let client = forwarded::resolve(&mw.trusted, peer, req.headers());
    req.extensions_mut().insert(client);
    next.run(req).await
}

/// What the outermost layer saw leave the stack, for [`finish`].
#[derive(Clone, Copy)]
struct Touched {
    had_allow: bool,
    method_not_allowed: bool,
}

pub(crate) async fn mark(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let touched = Touched {
        had_allow: res.headers().contains_key(header::ALLOW),
        method_not_allowed: res.extensions().get::<errors::MethodNotAllowed>().is_some(),
    };
    res.extensions_mut().insert(touched);
    res
}

/// Runs outside axum's own response handling: drops the `Allow` it adds to
/// answers the stack gave in place of a 405 (CORS preflights, 413s, ...),
/// writes a 405's `Allow` the way FastAPI does, keeps a 204/304 free of
/// Content-Length like Starlette, and gives the status line uvicorn's
/// Python 3.13 reason phrase (`422 Unprocessable Content`, not hyper's).
async fn finish(mut res: Response) -> Response {
    let Some(t) = res.extensions_mut().remove::<Touched>() else { return res };
    let h = res.headers_mut();
    if t.method_not_allowed {
        let methods: Vec<String> = h
            .get_all(header::ALLOW)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(','))
            .map(|m| m.trim().to_owned())
            .filter(|m| !m.is_empty() && m != "HEAD")
            .collect();
        if let Ok(v) = HeaderValue::from_str(&methods.join(", ")) {
            h.insert(header::ALLOW, v);
        }
    } else if !t.had_allow {
        h.remove(header::ALLOW);
    }
    if matches!(res.status().as_u16(), 204 | 304) {
        res.headers_mut().remove(header::CONTENT_LENGTH);
    }
    let status = res.status();
    if let Some(phrase) = crate::error::status_phrase(status.as_u16())
        && status.canonical_reason() != Some(phrase)
    {
        res.extensions_mut().insert(hyper::ext::ReasonPhrase::from_static(phrase.as_bytes()));
    }
    res
}

/// Apply the stack to the ported routes. Call before adding the fallback.
/// `usage` turns on API-token usage recording (Python's `add_usage_middleware`).
pub fn wrap<S: Clone + Send + Sync + 'static>(
    ported: Router<S>,
    mw: Arc<Mw>,
    usage: Option<crate::state::AppState>,
) -> Router<S> {
    let stack = ServiceBuilder::new()
        .layer(from_fn(mark))
        .layer(from_fn_with_state(mw.clone(), client_addr))
        .layer(from_fn(compression::compression))
        .layer(from_fn_with_state(mw.clone(), cors::public_asset_cors))
        .layer(from_fn_with_state(mw.clone(), cors::cors))
        .layer(from_fn(request_id::request_context))
        .layer(from_fn_with_state(mw.clone(), redirect::api_host_redirect))
        .layer(from_fn_with_state(mw.clone(), security::security))
        .layer(from_fn_with_state(mw, idempotency::idempotency))
        .option_layer(usage.map(|state| from_fn_with_state(state, crate::usage::record)))
        .layer(from_fn(head::head_as_get))
        .layer(DefaultBodyLimit::disable());
    ported.method_not_allowed_fallback(errors::method_not_allowed).layer(stack)
}

/// The finished app: `inner` is the wrapped ported router with its fallback.
pub fn outer(inner: Router) -> Router {
    Router::new().fallback_service(inner).layer(map_response(finish))
}
