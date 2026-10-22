//! The two CORS layers: Starlette's `CORSMiddleware` as `app/main.py`
//! configures it (credentials, every method, every header, an origin list plus
//! a regex), and `PublicAssetCorsMiddleware` outside it, which answers the 3D
//! viewer's public assets with a wildcard for origins that aren't ours.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::Response,
};

use super::Mw;
use super::headers::{add_vary_header, set, set_static};
use super::paths::is_public_asset;
use super::pyurl::{first, header_value, scope_path};

const ALL_METHODS: [&str; 7] = ["DELETE", "GET", "HEAD", "OPTIONS", "PATCH", "POST", "PUT"];
const EXPOSE: &str = "X-Request-ID, X-RateLimit-Limit, X-RateLimit-Remaining, X-RateLimit-Reset, Retry-After, X-Dressing-Dropped, Content-Disposition";

fn plain_text(status: StatusCode, text: &'static str, headers: Vec<(&'static str, String)>) -> Response {
    let mut res = Response::new(Body::from(text));
    *res.status_mut() = status;
    let h = res.headers_mut();
    for (k, v) in headers {
        h.insert(k, header_value(&v));
    }
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    res
}

fn preflight(mw: &Mw, req: &HeaderMap, origin: &str) -> Response {
    let mut headers: Vec<(&'static str, String)> = vec![
        ("vary", "Origin".into()),
        ("access-control-allow-methods", ALL_METHODS.join(", ")),
        ("access-control-max-age", "600".into()),
        ("access-control-allow-credentials", "true".into()),
    ];
    let mut failures: Vec<&str> = Vec::new();
    if mw.cfg.is_allowed_origin(origin) {
        headers.push(("access-control-allow-origin", origin.to_owned()));
    } else {
        failures.push("origin");
    }
    let method = first(req, "access-control-request-method").unwrap_or_default();
    if !ALL_METHODS.contains(&method.as_str()) {
        failures.push("method");
    }
    if let Some(requested) = first(req, "access-control-request-headers") {
        headers.push(("access-control-allow-headers", requested));
    }
    if req.contains_key("access-control-request-private-network") {
        failures.push("private-network");
    }
    if failures.is_empty() {
        return plain_text(StatusCode::OK, "OK", headers);
    }
    let text: &'static str = match failures.join(", ").as_str() {
        "origin" => "Disallowed CORS origin",
        "method" => "Disallowed CORS method",
        "private-network" => "Disallowed CORS private-network",
        "origin, method" => "Disallowed CORS origin, method",
        "origin, private-network" => "Disallowed CORS origin, private-network",
        "method, private-network" => "Disallowed CORS method, private-network",
        _ => "Disallowed CORS origin, method, private-network",
    };
    plain_text(StatusCode::BAD_REQUEST, text, headers)
}

pub async fn cors(State(mw): State<Arc<Mw>>, req: Request, next: Next) -> Response {
    let Some(origin) = first(req.headers(), "origin") else {
        return next.run(req).await;
    };
    if req.method() == Method::OPTIONS && req.headers().contains_key("access-control-request-method") {
        return preflight(&mw, req.headers(), &origin);
    }
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    set_static(h, "access-control-allow-credentials", "true");
    set_static(h, "access-control-expose-headers", EXPOSE);
    if mw.cfg.is_allowed_origin(&origin) {
        set(h, "access-control-allow-origin", &origin);
        add_vary_header(h, "Origin");
    }
    res
}

pub async fn public_asset_cors(State(mw): State<Arc<Mw>>, req: Request, next: Next) -> Response {
    if !is_public_asset(&scope_path(req.uri())) {
        return next.run(req).await;
    }
    let origin = first(req.headers(), "origin").unwrap_or_default();
    if mw.cfg.is_own_origin(&origin) {
        return next.run(req).await;
    }
    if req.method() == Method::OPTIONS {
        let mut res = Response::new(Body::empty());
        *res.status_mut() = StatusCode::NO_CONTENT;
        let h = res.headers_mut();
        set_static(h, "access-control-allow-origin", "*");
        set_static(h, "access-control-allow-methods", "GET, HEAD, OPTIONS");
        set_static(h, "access-control-allow-headers", "*");
        set_static(h, "access-control-max-age", "86400");
        return res;
    }
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    let mut vary: Vec<u8> = Vec::new();
    for v in h.get_all(header::VARY) {
        if !vary.is_empty() {
            vary.extend_from_slice(b", ");
        }
        vary.extend_from_slice(v.as_bytes());
    }
    if !vary.to_ascii_lowercase().windows(6).any(|w| w == b"origin") {
        vary = if vary.is_empty() { b"Origin".to_vec() } else { [vary.as_slice(), b", Origin"].concat() };
    }
    h.remove(header::ACCESS_CONTROL_ALLOW_ORIGIN);
    h.remove(header::ACCESS_CONTROL_ALLOW_CREDENTIALS);
    h.remove(header::ACCESS_CONTROL_EXPOSE_HEADERS);
    h.insert(header::VARY, HeaderValue::from_bytes(&vary).unwrap_or(HeaderValue::from_static("Origin")));
    set_static(h, "access-control-allow-origin", "*");
    set_static(h, "access-control-expose-headers", "X-Dressing-Dropped");
    res
}
