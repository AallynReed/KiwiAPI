//! Starlette 1.9's router (`starlette/routing.py`) for what axum's can't decide
//! alone ([`rewrite`], [`gate`], [`fallback`]). `routes.json` holds the api's
//! and the website's route tables in registration order, frozen from the
//! Python apps (native/port/parity/gen_routing_fixtures.py). A request is
//! matched on its decoded path: the first route matching path and method wins,
//! else the first matching the path alone answers 405 with that route's
//! `Allow`, else a trailing-slash twin that matches anything is a 307 to it,
//! else the framework 404.

#[cfg(test)]
mod tests;

use std::{collections::HashSet, sync::OnceLock};

use axum::{
    body::Body,
    extract::{MatchedPath, Request, State},
    http::{HeaderValue, Method, StatusCode, Uri, header, uri::PathAndQuery},
    middleware::Next,
    response::{IntoResponse, Response},
};
use regex::{Regex, RegexSet};
use serde_json::Value;

use crate::error::{ApiError, method_not_allowed, not_found};
use crate::middleware::ClientAddr;
use crate::middleware::pyurl::{first, header_value, quote, scope_path, url_path, valid_host};

struct Route {
    methods: Option<Vec<String>>,
    strict: Regex,
}

pub struct Table {
    routes: Vec<Route>,
    set: RegexSet,
    shapes: HashSet<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Full(usize),
    Partial(usize),
    Redirect(String),
    NotFound,
}

fn param_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\{([a-zA-Z_][a-zA-Z0-9_]*)(:[a-zA-Z_][a-zA-Z0-9_]*)?\}").expect("regex"))
}

/// `compile_path` without the anchor at the end.
fn compile(template: &str) -> String {
    let mut out = String::from("^");
    let mut idx = 0;
    for c in param_re().captures_iter(template) {
        let whole = c.get(0).expect("match");
        out.push_str(&regex::escape(&template[idx..whole.start()]));
        let pat = match c.get(2).map_or("str", |m| &m.as_str()[1..]) {
            "str" => "[^/]+",
            "int" => "[0-9]+",
            "path" => ".*",
            "float" => r"[0-9]+(?:\.[0-9]+)?",
            "uuid" => "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
            other => panic!("unknown convertor {other} in {template}"),
        };
        out.push_str("(?:");
        out.push_str(pat);
        out.push(')');
        idx = whole.end();
    }
    out.push_str(&regex::escape(&template[idx..]));
    out
}

/// A template with every parameter segment blanked, so a Starlette route and
/// the axum route serving it compare equal (`{slug}.git` and `{slug}`,
/// `{p:path}` and `{*p}`).
pub fn shape(template: &str) -> String {
    template
        .split('/')
        .map(|seg| {
            if seg.contains("{*") || seg.contains(":path}") {
                "{*}"
            } else if seg.contains('{') {
                "{}"
            } else {
                seg
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

impl Table {
    fn parse(rows: &Value) -> Table {
        let mut routes = Vec::new();
        let mut lenient = Vec::new();
        let mut shapes = HashSet::new();
        for row in rows.as_array().expect("route table") {
            let path = row[0].as_str().expect("path");
            let methods = row[1].as_array().map(|m| m.iter().map(|x| x.as_str().expect("method").to_owned()).collect());
            let template = if methods.is_none() { format!("{path}/{{path:path}}") } else { path.to_owned() };
            let body = compile(&template);
            // Python's `$` also matches before one final newline.
            lenient.push(format!(r"{body}\n?$"));
            shapes.insert(shape(&template));
            routes.push(Route { methods, strict: Regex::new(&format!("{body}$")).expect("route regex") });
        }
        Table { set: RegexSet::new(&lenient).expect("route set"), routes, shapes }
    }

    fn load(key: &str) -> Table {
        static RAW: OnceLock<Value> = OnceLock::new();
        let raw = RAW.get_or_init(|| serde_json::from_str(include_str!("routes.json")).expect("routes.json"));
        Table::parse(&raw[key])
    }

    /// `app.main:app`.
    pub fn api() -> &'static Table {
        static T: OnceLock<Table> = OnceLock::new();
        T.get_or_init(|| Table::load("api"))
    }

    /// `app.web.main:app`.
    pub fn web() -> &'static Table {
        static T: OnceLock<Table> = OnceLock::new();
        T.get_or_init(|| Table::load("web"))
    }

    /// `Router.app` for an http request; `path` is the ASGI (decoded) path.
    pub fn resolve(&self, method: &Method, path: &str) -> Outcome {
        let mut partial = None;
        for i in self.set.matches(path).iter() {
            match &self.routes[i].methods {
                Some(m) if !m.iter().any(|x| x == method.as_str()) => {
                    partial.get_or_insert(i);
                }
                _ => return Outcome::Full(i),
            }
        }
        if let Some(i) = partial {
            return Outcome::Partial(i);
        }
        if path != "/" {
            let other = match path.strip_suffix('/') {
                Some(_) => path.trim_end_matches('/').to_owned(),
                None => format!("{path}/"),
            };
            if self.set.is_match(&other) {
                return Outcome::Redirect(other);
            }
        }
        Outcome::NotFound
    }

    /// `", ".join(route.methods)`.
    pub fn allow(&self, i: usize) -> String {
        self.routes[i].methods.as_deref().unwrap_or_default().join(", ")
    }

    /// Whether a table route has this axum template's shape.
    pub fn knows(&self, axum_template: &str) -> bool {
        self.shapes.contains(&shape(axum_template))
    }
}

/// `HeadMethodMiddleware`: HEAD routes as GET, except under `/v1/events`.
pub fn routing_method(method: &Method, path: &str) -> Method {
    if method == Method::HEAD && !path.starts_with("/v1/events") { Method::GET } else { method.clone() }
}

/// A decoded path percent-encoded so axum splits it where Starlette does and
/// decodes every segment back to the same text.
pub fn canonical(decoded: &str) -> String {
    let mut out = String::with_capacity(decoded.len());
    for &b in decoded.as_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=:@/".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `scope["server"]` as a netloc; the bind address stands in for the socket's.
fn server_netloc(scheme: &str) -> String {
    let bind = std::env::var("KIWI_BIND").unwrap_or_else(|_| "0.0.0.0:8000".into());
    let default = if scheme == "https" { 443 } else { 80 };
    match bind.parse::<std::net::SocketAddr>() {
        Ok(a) if a.port() == default => match a.ip() {
            std::net::IpAddr::V6(ip) => format!("[{ip}]"),
            ip => ip.to_string(),
        },
        Ok(a) => a.to_string(),
        Err(_) => bind,
    }
}

/// `URL(scope=redirect_scope)` quoted the way `RedirectResponse` does.
pub fn location(scheme: &str, host: Option<&str>, path: &str, query: &str) -> String {
    let path = path.replace('?', "%3F").replace('#', "%23").replace('\\', "%5C");
    let url = match host.filter(|h| valid_host(h)).map(str::to_owned).unwrap_or_else(|| server_netloc(scheme)) {
        netloc if !netloc.is_empty() => {
            let mut u = format!("{scheme}://{netloc}{path}");
            if !query.is_empty() {
                u.push('?');
                u.push_str(query);
            }
            u
        }
        _ => {
            let p = match path.strip_prefix("//") {
                Some(rest) => format!("/%2F{rest}"),
                None => path,
            };
            if query.is_empty() { p } else { format!("{p}?{query}") }
        }
    };
    quote(&url, ":/%#?=@[]!$&'()*+,;")
}

fn redirect(req: &Request, path: &str) -> Response {
    let scheme = req.extensions().get::<ClientAddr>().map_or("http", |c| c.scheme);
    let host = first(req.headers(), "host");
    let target = location(scheme, host.as_deref(), path, req.uri().query().unwrap_or(""));
    let mut res = Response::new(Body::empty());
    *res.status_mut() = StatusCode::TEMPORARY_REDIRECT;
    res.headers_mut().insert(header::CONTENT_LENGTH, HeaderValue::from_static("0"));
    res.headers_mut().insert(header::LOCATION, header_value(&target));
    res
}

/// What Starlette answers for an outcome no route handler serves.
pub fn answer(t: &Table, outcome: Outcome, req: &Request) -> Response {
    match outcome {
        Outcome::Partial(i) => method_not_allowed(&t.allow(i)),
        Outcome::Redirect(path) => redirect(req, &path),
        Outcome::Full(_) | Outcome::NotFound => not_found(req.method(), &url_path(req.uri()), req.headers()),
    }
}

/// Inside the stack, around every route: a match axum made that Starlette
/// would not make is answered the way Starlette answers the path.
pub async fn gate(State(t): State<&'static Table>, req: Request, next: Next) -> Response {
    let known = req.extensions().get::<MatchedPath>().is_some_and(|m| t.knows(m.as_str()));
    if !known {
        return next.run(req).await;
    }
    match t.resolve(req.method(), &scope_path(req.uri())) {
        Outcome::Full(_) => next.run(req).await,
        other => answer(t, other, &req),
    }
}

/// Marks the answer to a request only a table route without a Rust handler could serve.
#[derive(Clone, Copy, Debug)]
pub struct NoRoute;

/// The router's default for a path axum matched nowhere.
pub async fn fallback(t: &'static Table, req: Request) -> Response {
    match t.resolve(req.method(), &scope_path(req.uri())) {
        Outcome::Full(i) => {
            tracing::error!("{} {}: no Rust route for Python's #{i}", req.method(), req.uri().path());
            let mut res = ApiError::internal().body().into_response();
            *res.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
            res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
            res.extensions_mut().insert(NoRoute);
            res
        }
        other => answer(t, other, &req),
    }
}

/// Before routing: a percent-encoded path is routed by what it decodes to.
/// A trailing newline only a Python `$` let match is dropped.
pub async fn rewrite(State(t): State<&'static Table>, mut req: Request, next: Next) -> Response {
    if req.uri().path().contains('%') {
        let decoded = scope_path(req.uri());
        let mut path = decoded.as_str();
        if decoded.ends_with('\n')
            && let Outcome::Full(i) = t.resolve(&routing_method(req.method(), &decoded), &decoded)
            && !t.routes[i].strict.is_match(&decoded)
        {
            path = &decoded[..decoded.len() - 1];
        }
        let canon = canonical(path);
        if canon != req.uri().path() {
            let pq = match req.uri().query() {
                Some(q) => format!("{canon}?{q}"),
                None => canon,
            };
            if let Ok(pq) = PathAndQuery::try_from(pq) {
                let mut parts = req.uri().clone().into_parts();
                parts.path_and_query = Some(pq);
                if let Ok(uri) = Uri::from_parts(parts) {
                    *req.uri_mut() = uri;
                }
            }
        }
    }
    next.run(req).await
}
