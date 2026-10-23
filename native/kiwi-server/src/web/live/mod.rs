//! The live-data pages of `app/web/pages.py` with their server-rendered first
//! paint (`ssr`): the homepage, status, calendar, streams, releases, mod stats,
//! giveaways, player activity, class activity, player profiles, leaderboards
//! and updates.

pub mod ssr;

#[cfg(test)]
mod tests;

use std::sync::Arc;

use axum::{
    Router,
    extract::State,
    http::{HeaderMap, HeaderValue, Uri, header},
    response::Response,
    routing::get,
};
use minijinja::Value;
use serde_json::Value as Json;

use super::pages::{appearance, render};
use super::{Gate, Web};
use crate::middleware::pyurl::{query_param, scope_path};

pub fn router() -> Router<Arc<Web>> {
    Router::new()
        .route("/", get(home))
        .route("/status", get(status))
        .route("/calendar", get(calendar))
        .route("/streams", get(streams))
        .route("/releases", get(releases))
        .route("/zakros-ui-stats", get(mod_stats))
        .route("/giveaways", get(giveaways))
        .route("/activity", get(activity))
        .route("/class-activity", get(class_activity))
        .route("/player/{name}", get(player))
        .route("/leaderboards", get(leaderboards))
        .route("/updates", get(updates))
}

fn ssr(model: &Json) -> (&'static str, Value) {
    ("ssr", Value::from_serialize(model))
}

async fn home(Gate(flags): Gate, State(web): State<Arc<Web>>, uri: Uri, headers: HeaderMap) -> Response {
    let (design, ..) = appearance(&headers, chrono::Utc::now());
    let template = if design == "classic" { "index.html" } else { "home_voxel.html" };
    let model = ssr::home_view(&web.api, &flags).await;
    let install = web.settings.discord_install_link().map_or(Value::from(()), Value::from);
    let ctx = vec![("discord_install_url", install), ssr(&model)];
    let mut res = render(&web, &flags, &uri, &headers, template, ctx);
    res.headers_mut().insert(header::VARY, HeaderValue::from_static("Cookie"));
    res
}

macro_rules! ssr_page {
    ($name:ident, $template:literal, $view:path) => {
        async fn $name(Gate(flags): Gate, State(web): State<Arc<Web>>, uri: Uri, headers: HeaderMap) -> Response {
            let model = $view(&web.api).await;
            render(&web, &flags, &uri, &headers, $template, vec![ssr(&model)])
        }
    };
}

ssr_page!(status, "status.html", ssr::status_view);
ssr_page!(calendar, "calendar.html", ssr::calendar_view);
ssr_page!(streams, "streams.html", ssr::streams_view);
ssr_page!(releases, "releases.html", ssr::releases_view);
ssr_page!(giveaways, "giveaways.html", ssr::giveaways_view);
ssr_page!(class_activity, "class-activity.html", ssr::class_activity_view);
ssr_page!(updates, "updates.html", ssr::updates_view);

async fn mod_stats(Gate(flags): Gate, State(web): State<Arc<Web>>, uri: Uri, headers: HeaderMap) -> Response {
    let model = ssr::mod_stats_view(&web.api, web.app_base()).await;
    render(&web, &flags, &uri, &headers, "mod-stats.html", vec![ssr(&model)])
}

async fn activity(Gate(flags): Gate, State(web): State<Arc<Web>>, uri: Uri, headers: HeaderMap) -> Response {
    let p = query_param(&uri, "period").unwrap_or_default().to_lowercase();
    let (p, label) = match p.as_str() {
        "7d" => ("7d", Some("Last 7 days")),
        "1m" => ("1m", Some("Last 30 days")),
        _ => ("", None),
    };
    let qs = if p.is_empty() { String::new() } else { format!("?period={p}") };
    let (title, desc) = match label {
        Some(l) => (
            format!("Trove Player Activity · {l}"),
            format!(
                "How many players were active each day over the {}, from the leaderboard captures.",
                l.to_lowercase()
            ),
        ),
        None => (
            "Trove Player Activity".to_owned(),
            "How many players are active in Trove each day and week, from the leaderboard captures.".to_owned(),
        ),
    };
    let model = ssr::activity_view(&web.api).await;
    let ctx = vec![
        ("og_title", Value::from(title)),
        ("og_desc", Value::from(desc)),
        ("og_image_url", Value::from(format!("{}/activity/og.png{qs}", web.api_base()))),
        ("og_page_url", Value::from(format!("{}/activity{qs}", web.app_base()))),
        ssr(&model),
    ];
    render(&web, &flags, &uri, &headers, "activity.html", ctx)
}

async fn player(Gate(flags): Gate, State(web): State<Arc<Web>>, uri: Uri, headers: HeaderMap) -> Response {
    let path = scope_path(&uri);
    let name = path.strip_prefix("/player/").unwrap_or("").to_owned();
    let model = ssr::player_view(&web.api, &name).await;
    let ctx = vec![
        ("og_title", Value::from(format!("{name} · Trove player profile"))),
        ("og_desc", Value::from(format!("{name}'s Trove leaderboard ranks and recent appearances."))),
        ("og_page_url", Value::from(format!("{}/player/{name}", web.app_base()))),
        ("player_name", Value::from(name)),
        ssr(&model),
    ];
    render(&web, &flags, &uri, &headers, "player.html", ctx)
}

async fn leaderboards(Gate(flags): Gate, State(web): State<Arc<Web>>, uri: Uri, headers: HeaderMap) -> Response {
    let model = ssr::leaderboards_view(&web.api).await;
    let mut ctx: Vec<(&str, Value)> =
        ["cheater_detection_enabled", "alt_clusters_enabled", "renames_enabled", "duplicates_enabled"]
            .into_iter()
            .map(|k| (k, Value::from_serialize(flags.value(k))))
            .collect();
    ctx.push(ssr(&model));
    render(&web, &flags, &uri, &headers, "leaderboards.html", ctx)
}
