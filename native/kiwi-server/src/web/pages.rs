//! The page routes of `app/web/pages.py` whose view model is plain: static
//! pages, settings/flag/meta-only pages, the hubs, swf-docs, the support /
//! clubs / browse first paints and the legacy redirects. Each runs the flag
//! gate first, then renders with the context processors' keys on top of the
//! route's own (`context.update`, so `theme` and `design` always win).

use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::get,
};
use chrono::Timelike;
use minijinja::Value;
use serde_json::Value as Json;

use super::{Gate, Web, flags::Flags, nav};
use crate::error::{ApiError, http_exception};
use crate::middleware::pyurl::{header_value, query_param, quote, scope_path, url_path};

pub fn router() -> Router<Arc<Web>> {
    let plain = |template: &'static str| {
        move |g: Gate, s: State<Arc<Web>>, uri: Uri, h: HeaderMap| plain_page(g, s, uri, h, template)
    };
    Router::new()
        .route("/embed/viewer", get(embed_viewer))
        .route("/app", get(app_showcase))
        .route("/browse", get(browse))
        .route("/documentation", get(plain("docs.html")))
        .route("/swf-docs", get(swf_docs_index))
        .route("/swf-docs/{doc}", get(swf_doc))
        .route("/support", get(support))
        .route("/embed/status.svg", get(status_badge))
        .route("/status/og.png", get(status_og))
        .route("/tomes", get(plain("tomes.html")))
        .route("/server-time", get(plain("server-time.html")))
        .route("/search", get(plain("search.html")))
        .route("/star-chart", get(plain("star-chart.html")))
        .route("/sound-studio", get(plain("sound-studio.html")))
        .route("/blueprint-editor", get(plain("blueprint-editor.html")))
        .route("/mod-workshop", get(plain("mod-workshop.html")))
        .route("/unlock-debug", get(plain("unlock-debug.html")))
        .route("/unlock_debug", get(unlock_debug_legacy))
        .route("/drop/{slug}", get(file_drop))
        .route("/zakros-ui-requests", get(plain("custom-art.html")))
        .route("/gems-guide", get(plain("gems-guide.html")))
        .route("/guides", get(guides))
        .route("/gem-tools", get(gem_tools))
        .route("/gem-abilities", get(gem_abilities))
        .route("/gear-guide", get(plain("gear-guide.html")))
        .route("/fishing-guide", get(plain("fishing-guide.html")))
        .route("/loot-collector-guide", get(plain("loot-collector-guide.html")))
        .route("/community-sites", get(plain("community-sites.html")))
        .route("/dressing-room", get(plain("dressing-room.html")))
        .route("/gem-simulator", get(plain("gem-simulator.html")))
        .route("/gem-evaluator", get(plain("gem-evaluator.html")))
        .route("/gem-builds", get(plain("gem-builds.html")))
        .route("/calculators", get(plain("calculators.html")))
        .route("/terms", get(plain("terms.html")))
        .route("/privacy", get(plain("privacy.html")))
        .route("/accessibility", get(plain("accessibility.html")))
        .route("/changelog", get(changelog))
        .route("/login", get(login))
        .route("/dashboard", get(plain("dashboard.html")))
        .route("/codexes/crafting", get(plain("codexes-crafting.html")))
        .route("/mods/why", get(plain("mods_why.html")))
        .route("/clubs", get(clubs))
        .route("/activity/og.png", get(activity_og))
}

/// `appearance.resolve(request.cookies)`.
pub fn appearance(headers: &HeaderMap, now: chrono::DateTime<chrono::Utc>) -> (String, String, &'static str) {
    let cookies = crate::auth::site::parse_cookies(headers);
    let design = cookies.get("btt_design").map(String::as_str).unwrap_or("");
    let theme = cookies.get("btt_theme").map(String::as_str).unwrap_or("");
    let design = if ["voxel", "classic"].contains(&design) { design } else { "voxel" };
    let theme = if ["auto", "light", "dark"].contains(&theme) { theme } else { "auto" };
    let tod = match theme {
        "light" => "day",
        "dark" => "night",
        _ => match (now - chrono::Duration::hours(11)).hour() {
            5..=8 => "morning",
            9..=16 => "day",
            17..=19 => "dusk",
            _ => "night",
        },
    };
    (design.to_owned(), theme.to_owned(), tod)
}

fn flag_value(v: &Json) -> Value {
    Value::from_serialize(v)
}

/// `TemplateResponse(request, template, ctx)`: the route context, `request`,
/// then the flag and appearance context processors.
pub fn render(
    web: &Web,
    flags: &Flags,
    uri: &Uri,
    headers: &HeaderMap,
    template: &str,
    ctx: Vec<(&str, Value)>,
) -> Response {
    let path = url_path(uri);
    let mut m: indexmap::IndexMap<String, Value> = ctx.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
    m.entry("request".to_owned())
        .or_insert_with(|| Value::from_iter([("url", Value::from_iter([("path", Value::from(path.clone()))]))]));
    for attr in super::flags::SITE_FLAGS {
        m.insert(attr.to_owned(), flag_value(&flags.value(attr)));
    }
    m.insert("nav_menu".to_owned(), nav::nav_menu(flags));
    m.insert("nav_active".to_owned(), Value::from(nav::nav_is_active(&path, flags)));
    let (design, theme, tod) = appearance(headers, chrono::Utc::now());
    m.insert("design".to_owned(), Value::from(design));
    m.insert("theme".to_owned(), Value::from(theme));
    m.insert("vx_tod".to_owned(), Value::from(tod));
    match web.templates.render(template, Value::from_iter(m)) {
        Ok(html) => html_response(html),
        Err(e) => {
            tracing::error!("template {template}: {e:#}");
            ApiError::internal().into_response()
        }
    }
}

pub fn html_response(html: String) -> Response {
    let mut res = Response::new(Body::from(html));
    res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"));
    res
}

/// Starlette `RedirectResponse(url, status_code)`.
pub fn redirect(url: &str, status: StatusCode) -> Response {
    let mut res = Response::new(Body::empty());
    *res.status_mut() = status;
    let h = res.headers_mut();
    h.insert(header::CONTENT_LENGTH, HeaderValue::from_static("0"));
    h.insert(header::LOCATION, header_value(&quote(url, ":/%#?=@[]!$&'()*+,;")));
    res
}

async fn plain_page(
    Gate(flags): Gate,
    State(web): State<Arc<Web>>,
    uri: Uri,
    headers: HeaderMap,
    template: &'static str,
) -> Response {
    render(&web, &flags, &uri, &headers, template, Vec::new())
}

pub async fn health() -> Response {
    crate::pyjson::json_response(r#"{"status":"ok"}"#)
}

/// `GET /gamedata/{name}`: the post-patch rebuild when there is one.
pub async fn gamedata(State(web): State<Arc<Web>>, req: Request) -> Response {
    let path = scope_path(req.uri());
    let name = path.strip_prefix("/gamedata/").unwrap_or("");
    if !crate::decode::routes::PUBLIC.contains(&name) {
        return http_exception(StatusCode::NOT_FOUND, None, req.method(), &url_path(req.uri()), req.headers());
    }
    let file = crate::gamedata::Gamedata::global(&web.settings).path(name);
    match crate::drops::download::file_response_with(
        &file,
        "application/json",
        None,
        "public, max-age=300",
        req.headers(),
    )
    .await
    {
        Ok(r) if r.status() == StatusCode::OK => {
            let (parts, _) = r.into_parts();
            Response::from_parts(parts, super::static_files::file_frames(file))
        }
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

const BACKDROPS: [&str; 6] = ["melee", "pistol", "staff", "spear", "fist", "hat"];
const MODES: [&str; 5] = ["auto", "blueprint", "assembled", "vfx", "audio"];

async fn embed_viewer(Gate(flags): Gate, State(web): State<Arc<Web>>, uri: Uri, headers: HeaderMap) -> Response {
    let q = |name: &str| query_param(&uri, name);
    let s = |name: &str| Value::from(q(name).unwrap_or_default());
    let backdrop = q("backdrop").filter(|b| BACKDROPS.contains(&b.as_str())).unwrap_or_default();
    let mode = q("mode").unwrap_or_else(|| "auto".into());
    let mode = if MODES.contains(&mode.as_str()) { mode } else { "auto".into() };
    let theme = q("theme").unwrap_or_else(|| "dark".into());
    let theme = if theme == "dark" || theme == "light" { theme } else { "dark".into() };
    let ctx = vec![
        ("release", s("release")),
        ("tmod", s("tmod")),
        ("game", s("game")),
        ("prefab", s("prefab")),
        ("dress", s("dress")),
        ("path", s("path")),
        ("sound", s("sound")),
        ("backdrop", Value::from(backdrop)),
        ("mode", Value::from(mode)),
        ("theme", Value::from(theme)),
        ("api_base", Value::from(web.api_base())),
        ("app_url", Value::from(web.app_base())),
    ];
    render(&web, &flags, &uri, &headers, "embed_viewer.html", ctx)
}

fn discord_install(web: &Web) -> Value {
    web.settings.discord_install_link().map_or(Value::from(()), Value::from)
}

async fn app_showcase(Gate(flags): Gate, State(web): State<Arc<Web>>, uri: Uri, headers: HeaderMap) -> Response {
    let ctx = vec![("discord_install_url", discord_install(&web))];
    render(&web, &flags, &uri, &headers, "app.html", ctx)
}

/// `re.sub(r"^\W+", "", title or slug or "").lower()`.
fn browse_key(card: &Json) -> String {
    let pick = |k: &str| card.get(k).filter(|v| super::flags::truthy(v));
    let raw = match pick("title").or_else(|| pick("slug")) {
        Some(Json::String(s)) => s.clone(),
        _ => String::new(),
    };
    raw.trim_start_matches(|c: char| !kiwi_core::pystr::is_word(c)).to_lowercase()
}

async fn browse(Gate(flags): Gate, State(web): State<Arc<Web>>, uri: Uri, headers: HeaderMap) -> Response {
    let (mut mods, mut packs) = (Vec::new(), Vec::new());
    if flags.on("mods_hub_enabled") {
        (mods, packs) = tokio::join!(
            super::meta::all_public_cards(&web.api, "/site/mods/projects"),
            super::meta::all_public_cards(&web.api, "/site/modpacks/projects"),
        );
    }
    for cards in [&mut mods, &mut packs] {
        cards.sort_by_cached_key(browse_key);
    }
    let ctx = vec![("mods", Value::from_serialize(&mods)), ("modpacks", Value::from_serialize(&packs))];
    render(&web, &flags, &uri, &headers, "browse.html", ctx)
}

async fn swf_docs_index(Gate(flags): Gate, State(web): State<Arc<Web>>, uri: Uri, headers: HeaderMap) -> Response {
    let ctx = web.swf_docs.page_context(None, &web.settings.app_url).unwrap_or_default();
    render(&web, &flags, &uri, &headers, "swf-docs.html", ctx)
}

async fn swf_doc(
    Gate(flags): Gate,
    State(web): State<Arc<Web>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let path = scope_path(&uri);
    let doc = path.strip_prefix("/swf-docs/").unwrap_or("");
    if doc.to_lowercase() == super::swf_docs::OVERVIEW.to_lowercase() {
        return redirect("/swf-docs", StatusCode::MOVED_PERMANENTLY);
    }
    let Some(ctx) = web.swf_docs.page_context(Some(doc), &web.settings.app_url) else {
        return http_exception(StatusCode::NOT_FOUND, Some("No such document."), &method, &url_path(&uri), &headers);
    };
    let slug = ctx.iter().find(|(k, _)| *k == "doc_slug").map(|(_, v)| v.to_string()).unwrap_or_default();
    if doc != slug {
        return redirect(&format!("/swf-docs/{slug}"), StatusCode::MOVED_PERMANENTLY);
    }
    render(&web, &flags, &uri, &headers, "swf-docs.html", ctx)
}

async fn support(Gate(flags): Gate, State(web): State<Arc<Web>>, uri: Uri, headers: HeaderMap) -> Response {
    let data = web.api.get_obj("/v1/misc/supporters", &[]).await;
    let supporters = data.and_then(|d| d.get("supporters").cloned()).filter(super::flags::truthy);
    let ctx = vec![("supporters", supporters.map_or(Value::from(Vec::<Value>::new()), |s| Value::from_serialize(&s)))];
    render(&web, &flags, &uri, &headers, "support.html", ctx)
}

async fn status_badge(_: Gate, State(web): State<Arc<Web>>) -> Response {
    redirect(&format!("{}/embed/status.svg", web.api_base()), StatusCode::FOUND)
}

async fn status_og(_: Gate, State(web): State<Arc<Web>>) -> Response {
    redirect(&format!("{}/status/og.png", web.api_base()), StatusCode::FOUND)
}

async fn unlock_debug_legacy(_: Gate) -> Response {
    redirect("/unlock-debug", StatusCode::MOVED_PERMANENTLY)
}

async fn file_drop(Gate(flags): Gate, State(web): State<Arc<Web>>, uri: Uri, headers: HeaderMap) -> Response {
    let path = scope_path(&uri);
    let slug = path.strip_prefix("/drop/").unwrap_or("").to_owned();
    render(&web, &flags, &uri, &headers, "drop.html", vec![("slug", Value::from(slug))])
}

async fn hub(
    web: &Web,
    flags: &Flags,
    uri: &Uri,
    headers: &HeaderMap,
    hub: &nav::Hub,
    title: &str,
    description: &str,
) -> Response {
    let view = nav::hub_view(hub, flags, title, description);
    render(web, flags, uri, headers, "hub.html", vec![("hub", view)])
}

async fn guides(Gate(flags): Gate, State(web): State<Arc<Web>>, uri: Uri, headers: HeaderMap) -> Response {
    let desc = "Our written guides to Trove: how gems work, fishing, and more as we write them.";
    hub(&web, &flags, &uri, &headers, &nav::GUIDES, "Trove guides", desc).await
}

async fn gem_tools(Gate(flags): Gate, State(web): State<Arc<Web>>, uri: Uri, headers: HeaderMap) -> Response {
    let desc = "Roll a gem, rate the one you own, or optimise the build around it.";
    hub(&web, &flags, &uri, &headers, &nav::GEM_TOOLS, "Trove gem tools", desc).await
}

async fn gem_abilities(_: Gate) -> Response {
    redirect("/abilities?tab=gems", StatusCode::PERMANENT_REDIRECT)
}

async fn changelog(Gate(flags): Gate, State(web): State<Arc<Web>>, uri: Uri, headers: HeaderMap) -> Response {
    let repo = format!("https://github.com/{}", web.settings.site_source_repo);
    render(&web, &flags, &uri, &headers, "changelog.html", vec![("repo_url", Value::from(repo))])
}

async fn login(Gate(flags): Gate, State(web): State<Arc<Web>>, uri: Uri, headers: HeaderMap) -> Response {
    let ctx = vec![("discord_oauth_enabled", Value::from(web.settings.discord_oauth_enabled()))];
    render(&web, &flags, &uri, &headers, "login.html", ctx)
}

async fn clubs(Gate(flags): Gate, State(web): State<Arc<Web>>, uri: Uri, headers: HeaderMap) -> Response {
    let data = web.api.get_obj("/site/clubs", &[]).await;
    let items = data.and_then(|d| d.get("items").cloned()).filter(super::flags::truthy);
    let ctx = vec![("clubs", items.map_or(Value::from(Vec::<Value>::new()), |s| Value::from_serialize(&s)))];
    render(&web, &flags, &uri, &headers, "clubs.html", ctx)
}

async fn activity_og(_: Gate, State(web): State<Arc<Web>>, uri: Uri) -> Response {
    let p = query_param(&uri, "period").unwrap_or_default().to_lowercase();
    let qs = ["7d", "1m"].iter().find(|k| **k == p).map_or(String::new(), |k| format!("?period={k}"));
    redirect(&format!("{}/activity/og.png{qs}", web.api_base()), StatusCode::FOUND)
}
