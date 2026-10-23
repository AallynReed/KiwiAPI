//! `/robots.txt`, `/BingSiteAuth.xml` and `/sitemap.xml` on the api app.

use std::{sync::LazyLock, time::Instant};

use axum::{
    extract::State,
    http::{HeaderMap, HeaderValue, Uri, header},
    response::{IntoResponse, Response},
};
use serde_json::Value;
use tokio::sync::Mutex;

use super::flags::{self, Flags, SITEMAP_PAGES, SiteGate};
use crate::{error::ApiError, middleware::pyurl::hostname, state::AppState};

pub const BING_VERIFY: &str =
    "<?xml version=\"1.0\"?>\n<users>\n\t<user>FC86658CF71BBCB1184266DE6480D237</user>\n</users>\n";

const SITEMAP_TTL_SECS: f64 = 600.0;
pub const SITEMAP_MAX_PER_SECTION: usize = 25_000;
const CATALOG_PAGE: i64 = 1000;

fn text(body: String, content_type: &'static str, cache: &'static str) -> Response {
    let mut res = body.into_response();
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    res
}

pub async fn robots(_: SiteGate, State(state): State<AppState>, headers: HeaderMap, uri: Uri) -> Response {
    let host = hostname(&headers, &uri).unwrap_or_default();
    text(flags::robots_body(state.settings(), &host), "text/plain; charset=utf-8", "public, max-age=86400")
}

pub async fn bing(_: SiteGate) -> Response {
    text(BING_VERIFY.into(), "application/xml", "public, max-age=86400")
}

fn xml_loc(url: &str) -> String {
    url.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// One catalog card's `(handle, slug, updated_at)` as the sitemap reads it.
fn card_parts(c: &Value) -> (Option<&str>, Option<&str>, Option<&str>) {
    let s = |k: &str| c.get(k).and_then(Value::as_str);
    (s("handle"), s("slug"), s("updated_at"))
}

/// `_render_sitemap` over the resolved flags and the catalog sections.
pub fn render(base: &str, f: &Flags, mods: &[Value], modders: &[Value], packs: &[Value]) -> String {
    let base = base.trim_end_matches('/');
    let mut entries: Vec<(String, Option<String>)> = SITEMAP_PAGES
        .iter()
        .filter(|(_, attr)| attr.is_none_or(|a| f.get_or(a, true)))
        .map(|(path, _)| (format!("{base}{path}"), None))
        .collect();
    if f.get_or("mods_hub_enabled", true) {
        let project = |root: &str, cards: &[Value], out: &mut Vec<(String, Option<String>)>| {
            for c in cards {
                if let (Some(h), Some(s), lastmod) = card_parts(c)
                    && !h.is_empty()
                    && !s.is_empty()
                {
                    out.push((format!("{base}/{root}/{h}/{s}"), lastmod.map(str::to_owned)));
                }
            }
        };
        project("mods", mods, &mut entries);
        for m in modders {
            let (h, _, lastmod) = card_parts(m);
            entries.push((format!("{base}/mods/{}", h.unwrap_or("None")), lastmod.map(str::to_owned)));
        }
        project("modpacks", packs, &mut entries);
    }
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n",
    );
    for (loc, lastmod) in entries {
        out.push_str("  <url><loc>");
        out.push_str(&xml_loc(&loc));
        out.push_str("</loc>");
        if let Some(lm) = lastmod.filter(|l| !l.is_empty()) {
            out.push_str("<lastmod>");
            out.push_str(&lm);
            out.push_str("</lastmod>");
        }
        out.push_str("</url>\n");
    }
    out.push_str("</urlset>\n");
    out
}

/// `_all_public_cards`: every page of a `list_public`, truncated at the cap.
pub async fn all_cards<F, Fut>(mut page: F, cap: usize, label: &str) -> Result<Vec<Value>, ApiError>
where
    F: FnMut(i64) -> Fut,
    Fut: Future<Output = Result<(Vec<Value>, u64), ApiError>>,
{
    let mut out = Vec::new();
    let mut offset = 0i64;
    loop {
        let (rows, total) = page(offset).await?;
        let empty = rows.is_empty();
        out.extend(rows);
        offset += CATALOG_PAGE;
        if empty || offset as u64 >= total || out.len() >= cap {
            break;
        }
    }
    if out.len() > cap {
        tracing::warn!("sitemap: {label} catalog ({}) exceeds cap {cap} - truncating", out.len());
        out.truncate(cap);
    }
    Ok(out)
}

async fn build(state: &AppState) -> Result<String, ApiError> {
    let f = flags::resolve(state).await?;
    let (mut mods, mut modders, mut packs) = (Vec::new(), Vec::new(), Vec::new());
    if f.get_or("mods_hub_enabled", true) {
        mods = all_cards(
            |offset| async move {
                let a = crate::mods_catalog::service::ListArgs {
                    q: None,
                    tag: None,
                    author: None,
                    sort: "recent".into(),
                    limit: CATALOG_PAGE,
                    offset,
                };
                crate::mods_catalog::service::list_public(state, &a).await
            },
            SITEMAP_MAX_PER_SECTION,
            "mods",
        )
        .await?;
        modders = crate::mods_catalog::service::public_modder_handles(state).await?;
        packs = all_cards(
            |offset| async move {
                let q = crate::modpacks::body::ListQuery {
                    q: None,
                    tag: None,
                    author: None,
                    sort: "recent".into(),
                    limit: CATALOG_PAGE,
                    offset,
                };
                let (rows, total) = crate::modpacks::service::browse(state, &q).await?;
                Ok((rows.iter().map(|p| Value::Object(crate::modpacks::service::pack_card(p))).collect(), total))
            },
            SITEMAP_MAX_PER_SECTION,
            "modpacks",
        )
        .await?;
    }
    Ok(render(&state.settings().app_url, &f, &mods, &modders, &packs))
}

/// The rendered body and when it was built, shared for `SITEMAP_TTL_SECS`.
static SITEMAP: LazyLock<Mutex<Option<(Instant, String)>>> = LazyLock::new(|| Mutex::new(None));

pub async fn sitemap(_: SiteGate, State(state): State<AppState>) -> Result<Response, ApiError> {
    let mut memo = SITEMAP.lock().await;
    let fresh = memo.as_ref().filter(|(at, _)| at.elapsed().as_secs_f64() <= SITEMAP_TTL_SECS).map(|(_, b)| b.clone());
    let body = match fresh {
        Some(b) => b,
        None => {
            let b = build(&state).await?;
            *memo = Some((Instant::now(), b.clone()));
            b
        }
    };
    Ok(text(body, "application/xml", "public, max-age=3600"))
}
