//! `app/web/meta.py`: robots.txt (host-aware), BingSiteAuth.xml and the
//! sitemap (memoised 10 minutes). Not feature-gated.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    extract::{Request, State},
    http::{HeaderValue, header},
    response::{IntoResponse, Response},
};
use serde_json::Value;

use super::Web;
use super::internal::InternalApi;
use crate::middleware::pyurl::hostname;

const BING_VERIFY: &str =
    "<?xml version=\"1.0\"?>\n<users>\n\t<user>FC86658CF71BBCB1184266DE6480D237</user>\n</users>\n";
const SITEMAP_TTL: Duration = Duration::from_secs(600);

fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split('/').next().unwrap_or("").to_lowercase()
}

/// `robots_body(host)`.
pub fn robots_body(web: &Web, host: &str) -> String {
    let host = host.to_lowercase();
    if web.mw_cfg.api_side_hosts.contains(&host) {
        return "User-agent: *\nAllow: /\nDisallow: /v1/\nDisallow: /site/\nDisallow: /git/\n".to_owned();
    }
    if host == host_of(&web.settings.api_url) || host == host_of(&web.settings.dev_url) {
        return "User-agent: *\nDisallow: /\n".to_owned();
    }
    format!(
        "User-agent: *\nAllow: /\nDisallow: /dashboard\nDisallow: /login\nDisallow: /site/\n\nSitemap: {}/sitemap.xml\n",
        web.settings.app_url.trim_end_matches('/')
    )
}

fn text(body: String, media: &'static str, cache: &'static str) -> Response {
    let mut res = Response::new(axum::body::Body::from(body));
    let h = res.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(media));
    res
}

pub async fn robots(State(web): State<Arc<Web>>, req: Request) -> Response {
    let host = hostname(req.headers(), req.uri()).unwrap_or_default();
    text(robots_body(&web, &host), "text/plain; charset=utf-8", "public, max-age=86400")
}

pub async fn bing() -> Response {
    text(BING_VERIFY.to_owned(), "application/xml", "public, max-age=86400")
}

/// `_all_public_cards(path)`: every card of a tokenless catalog, 100 a page,
/// capped at 25,000.
pub async fn all_public_cards(api: &InternalApi, path: &str) -> Vec<Value> {
    const CAP: usize = 25_000;
    let mut out = Vec::new();
    let mut offset: i64 = 0;
    loop {
        let params = [("limit", "100".to_owned()), ("offset", offset.to_string())];
        let Some(data) = api.get_obj(path, &params).await else { break };
        let rows = match data.get("items") {
            Some(Value::Array(a)) => a.clone(),
            _ => Vec::new(),
        };
        let total = match data.get("total") {
            Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
            Some(v) if super::flags::truthy(v) => f64::INFINITY,
            _ => 0.0,
        };
        let empty = rows.is_empty();
        out.extend(rows);
        offset += 100;
        if empty || offset as f64 >= total || out.len() >= CAP {
            break;
        }
    }
    if out.len() > CAP {
        tracing::warn!("{path} catalog ({}) exceeds cap {CAP} - truncating", out.len());
        out.truncate(CAP);
    }
    out
}

fn xml_loc(url: &str) -> String {
    url.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => crate::web::templates::py_str(&minijinja::Value::from_serialize(other)),
    }
}

async fn render_sitemap(web: &Web) -> String {
    let base = web.settings.app_url.trim_end_matches('/');
    let flags = web.flags.get().await;
    let mut entries: Vec<(String, Option<String>)> = super::flags::SITEMAP_PAGES
        .iter()
        .filter(|(_, attr)| attr.is_none_or(|a| flags.on(a)))
        .map(|(path, _)| (format!("{base}{path}"), None))
        .collect();
    entries.extend(web.swf_docs.doc_paths().into_iter().map(|p| (format!("{base}{p}"), None)));
    if flags.on("mods_hub_enabled") {
        let (mods, modders, packs) = tokio::join!(
            all_public_cards(&web.api, "/site/mods/projects"),
            web.api.get("/site/mods/modders", &[], 8.0, &[]),
            all_public_cards(&web.api, "/site/modpacks/projects"),
        );
        let lastmod = |c: &Value| c.get("updated_at").filter(|v| super::flags::truthy(v)).map(py_str);
        let present = |c: &Value, k: &str| c.get(k).is_some_and(super::flags::truthy);
        for c in &mods {
            if present(c, "handle") && present(c, "slug") {
                entries.push((format!("{base}/mods/{}/{}", py_str(&c["handle"]), py_str(&c["slug"])), lastmod(c)));
            }
        }
        if let Some(Value::Object(m)) = modders {
            let items = m.get("items").filter(|v| super::flags::truthy(v)).and_then(Value::as_array).cloned();
            for c in items.unwrap_or_default() {
                if present(&c, "handle") {
                    entries.push((format!("{base}/mods/{}", py_str(&c["handle"])), lastmod(&c)));
                }
            }
        }
        for c in &packs {
            if present(c, "handle") && present(c, "slug") {
                entries.push((format!("{base}/modpacks/{}/{}", py_str(&c["handle"]), py_str(&c["slug"])), lastmod(c)));
            }
        }
    }
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n",
    );
    for (loc, lastmod) in entries {
        out.push_str("  <url><loc>");
        out.push_str(&xml_loc(&loc));
        out.push_str("</loc>");
        if let Some(lm) = lastmod {
            out.push_str("<lastmod>");
            out.push_str(&lm);
            out.push_str("</lastmod>");
        }
        out.push_str("</url>\n");
    }
    out.push_str("</urlset>\n");
    out
}

pub async fn sitemap(State(web): State<Arc<Web>>) -> Response {
    let body = {
        let mut memo = web.sitemap.lock().await;
        match memo.as_ref() {
            Some((at, body)) if at.elapsed() <= SITEMAP_TTL => body.clone(),
            _ => {
                let body = render_sitemap(&web).await;
                *memo = Some((Instant::now(), body.clone()));
                body
            }
        }
    };
    text(body, "application/xml", "public, max-age=600").into_response()
}
