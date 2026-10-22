//! `api_host_redirect`: a GET/HEAD for a site page on an api-side host is a
//! 301 to the same path on `app_url` (Starlette `RedirectResponse`).

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::Response,
};

use super::paths::is_site_page;
use super::pyurl::{header_value, hostname, quote, url_path_query};
use super::Mw;

pub async fn api_host_redirect(State(mw): State<Arc<Mw>>, req: Request, next: Next) -> Response {
    if !matches!(*req.method(), Method::GET | Method::HEAD) {
        return next.run(req).await;
    }
    let host = hostname(req.headers(), req.uri()).unwrap_or_default();
    if !mw.cfg.api_side_hosts.contains(&host) {
        return next.run(req).await;
    }
    let (path, query) = url_path_query(req.uri());
    if !is_site_page(&path) {
        return next.run(req).await;
    }
    let mut target = format!("{}{path}", mw.cfg.app_url);
    if !query.is_empty() {
        target.push('?');
        target.push_str(&query);
    }
    let mut res = Response::new(Body::empty());
    *res.status_mut() = StatusCode::MOVED_PERMANENTLY;
    let h = res.headers_mut();
    h.insert(header::LOCATION, header_value(&quote(&target, ":/%#?=@[]!$&'()*+,;")));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from_static("0"));
    res
}
