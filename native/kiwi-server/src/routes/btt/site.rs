//! The BTT reads of `app/site/router.py` (`/site/btt/*`), the website source
//! changelog (`app/site/site_changelog.py`, `/site/changelog`) and the
//! master's "pull GitHub now" button (`POST /admin/btt/refresh-releases`).

use std::time::Duration;

use axum::{
    Router,
    extract::State,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures::TryStreamExt;
use kiwi_core::btt::{CHANNELS, PLATFORMS, channel_prerelease};
use mongodb::bson::{Bson, Document, doc};
use redis::AsyncCommands;
use serde_json::{Map, Value, json};

use super::{
    Mongo, Store, bool_field, dt_field, github, http_client, internal,
    releases::{CHANGELOGS, RELEASES, Release, walk_latest},
    str_field,
};
use crate::{
    auth::Superuser,
    config::Settings,
    dt,
    error::{ApiResult, status_phrase},
    pyjson::{self, LoadsError, PydanticJson},
    site_live::{cached, flags::SiteGate, jsonable},
    state::AppState,
    validation::{FromParams, Params, Query},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/site/btt/latest", get(latest))
        .route("/site/btt/releases", get(releases))
        .route("/site/btt/changelog", get(changelog))
        .route("/site/changelog", get(site_changelog))
        .route("/admin/btt/refresh-releases", post(refresh))
}

fn iso(d: &chrono::DateTime<chrono::Utc>) -> Value {
    Value::String(dt::isoformat(d))
}

fn assets(r: &Release) -> Value {
    Value::Array(r.assets.iter().map(jsonable).collect())
}

// --- /site/btt/latest -------------------------------------------------------------

pub struct LatestQuery {
    channel: String,
}

impl FromParams for LatestQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        LatestQuery { channel: p.query("channel").str().default("release".into()) }
    }
}

pub fn latest_body(channel: &str, releases: &[Release]) -> ApiResult<Value> {
    let mut platforms = Map::new();
    for p in PLATFORMS {
        let v = match walk_latest(releases, p)? {
            None => Value::Null,
            Some((r, matched)) => json!({
                "platform": p,
                "tag_name": r.tag_name,
                "published_at": iso(&r.published_at),
                "html_url": r.html_url,
                "assets": matched.iter().map(|a| jsonable(a)).collect::<Vec<_>>(),
            }),
        };
        platforms.insert(p.to_owned(), v);
    }
    Ok(json!({"channel": channel, "platforms": platforms}))
}

async fn channel_releases<S: Store>(s: &S, channel: &str) -> ApiResult<Vec<Release>> {
    let docs = s.channel_releases(channel_prerelease(channel).unwrap_or(false)).await?;
    docs.iter().map(Release::from_doc).collect()
}

async fn latest(_: SiteGate, State(state): State<AppState>, Query(q): Query<LatestQuery>) -> ApiResult<Response> {
    let channel = if channel_prerelease(&q.channel).is_some() { q.channel.as_str() } else { "release" };
    let releases = channel_releases(&Mongo(&state), channel).await?;
    Ok(cached(latest_body(channel, &releases)?, "public, max-age=180"))
}

// --- /site/btt/releases -----------------------------------------------------------

pub struct ReleasesQuery {
    channel: Option<String>,
    limit: i64,
    offset: i64,
}

impl FromParams for ReleasesQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        ReleasesQuery {
            channel: p.query("channel").str().optional(),
            limit: p.query("limit").int().ge(1).le(100).default(30),
            offset: p.query("offset").int().ge(0).default(0),
        }
    }
}

/// `_btt_release(d)`.
pub fn release_item(r: &Release) -> Value {
    json!({
        "tag_name": r.tag_name,
        "name": r.name,
        "body": r.body,
        "html_url": r.html_url,
        "channel": if r.prerelease { "beta" } else { "release" },
        "prerelease": r.prerelease,
        "published_at": iso(&r.published_at),
        "assets": assets(r),
    })
}

async fn releases(_: SiteGate, State(state): State<AppState>, Query(q): Query<ReleasesQuery>) -> ApiResult<Response> {
    let channel = q.channel.filter(|c| channel_prerelease(c).is_some());
    let prerelease = channel.as_deref().and_then(channel_prerelease);
    let (docs, total) = Mongo(&state).release_page(prerelease, q.offset as u64, q.limit).await?;
    let docs = docs.iter().map(Release::from_doc).collect::<ApiResult<Vec<_>>>()?;
    let body = json!({
        "channel": channel,
        "items": docs.iter().map(release_item).collect::<Vec<_>>(),
        "count": docs.len(),
        "total": total,
    });
    Ok(cached(body, "public, max-age=180"))
}

// --- /site/btt/changelog ----------------------------------------------------------

/// The stored `BttChangelog` as the page reads it, or the empty payload.
pub fn changelog_body(doc: Option<&Document>) -> ApiResult<Value> {
    let Some(d) = doc else {
        return Ok(json!({"groups": [], "rate_limited": false, "fetched_at": null}));
    };
    str_field(d, "repo", None)?;
    let groups = match d.get("groups") {
        None => Vec::new(),
        Some(Bson::Array(g)) if g.iter().all(|x| matches!(x, Bson::Document(_))) => g.iter().map(jsonable).collect(),
        Some(_) => return Err(internal("bad groups")),
    };
    Ok(json!({
        "groups": groups,
        "rate_limited": bool_field(d, "rate_limited", false)?,
        "fetched_at": iso(&dt_field(d, "fetched_at", true)?),
    }))
}

async fn changelog(_: SiteGate, State(state): State<AppState>) -> ApiResult<Response> {
    let doc = Mongo(&state).changelog(&state.settings().btt_releases_repo).await?;
    Ok(cached(changelog_body(doc.as_ref())?, "public, max-age=180"))
}

// --- GitHub, failing the way httpx does ----------------------------------------------

/// `str(exc)` of what the httpx call raised; `status` marks `HTTPStatusError`.
#[derive(Debug)]
pub struct GhError {
    pub message: String,
    pub status: bool,
}

/// `HTTPStatusError`'s message for a final non-2xx answer.
pub fn status_message(code: u16, url: &str, location: Option<&str>) -> String {
    let error_type = match code / 100 {
        1 => "Informational response",
        3 => "Redirect response",
        4 => "Client error",
        5 => "Server error",
        _ => "Invalid status code",
    };
    let phrase = status_phrase(code).unwrap_or("");
    let redirect = location.map_or_else(String::new, |l| format!("Redirect location: '{l}'\n"));
    format!(
        "{error_type} '{code} {phrase}' for url '{url}'\n{redirect}For more information check: https://developer.mozilla.org/en-US/docs/Web/HTTP/Status/{code}"
    )
}

/// `str(json.JSONDecodeError)` for a body `json.loads` rejected.
pub fn decode_message(body: &[u8], e: &LoadsError) -> String {
    match e {
        LoadsError::Syntax { msg, pos } => {
            let text = String::from_utf8_lossy(body);
            let before: String = text.chars().take(*pos).collect();
            let line = before.matches('\n').count() + 1;
            let col = match before.rfind('\n') {
                Some(i) => before[i + 1..].chars().count() + 1,
                None => pos + 1,
            };
            format!("{msg}: line {line} column {col} (char {pos})")
        }
        LoadsError::Decode => "'utf-8' codec can't decode the response body".into(),
        LoadsError::NonFinite => "out of range float values are not JSON compliant".into(),
    }
}

/// `_gh_get(path)` against `repo`: GET, `raise_for_status()`, `.json()`.
pub async fn gh_get(client: &reqwest::Client, s: &Settings, repo: &str, path: &str) -> Result<Value, GhError> {
    let url = format!("https://api.github.com/repos/{repo}/{path}");
    let mut req = client
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "KiwiAPI/1.0");
    if let Some(token) = s.btt_releases_token.as_deref().filter(|t| !t.is_empty()) {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    let other = |e: reqwest::Error| GhError { message: e.to_string(), status: false };
    let resp = req.send().await.map_err(other)?;
    let code = resp.status().as_u16();
    if !(200..300).contains(&code) {
        let location = resp.headers().get("location").and_then(|v| v.to_str().ok()).map(str::to_owned);
        let location = location.filter(|_| matches!(code, 301 | 302 | 303 | 307 | 308));
        return Err(GhError { message: status_message(code, resp.url().as_str(), location.as_deref()), status: true });
    }
    let body = resp.bytes().await.map_err(other)?;
    pyjson::loads(&body).map_err(|e| GhError { message: decode_message(&body, &e), status: false })
}

fn list_or_empty(v: Value) -> Vec<Value> {
    match v {
        Value::Array(items) => items,
        _ => Vec::new(),
    }
}

// --- /site/changelog ---------------------------------------------------------------

const CHANGELOG_KEY: &str = "site:changelog";
const CHANGELOG_TTL: u64 = 900;

/// `_build()` from what the two GitHub calls gave.
pub fn changelog_payload(s: &Settings, tags: Option<Value>, commits: Option<Value>) -> ApiResult<Value> {
    let tags = tags.map(list_or_empty).unwrap_or_default();
    let commits = commits.unwrap_or(Value::Array(Vec::new()));
    let rate_limited = commits.is_object() && github::is_rate_limited(&commits);
    let commits = list_or_empty(commits);
    let groups = github::build_changelog_groups(&tags, &commits).map_err(internal)?;
    Ok(json!({
        "repo": s.site_source_repo,
        "repo_url": format!("https://github.com/{}", s.site_source_repo),
        "groups": groups.iter().map(jsonable).collect::<Vec<_>>(),
        "rate_limited": rate_limited,
    }))
}

async fn build_changelog(state: &AppState) -> ApiResult<Value> {
    let s = state.settings();
    let client = http_client(Duration::from_secs(15)).map_err(internal)?;
    let fetch = |path: &'static str| {
        let client = client.clone();
        async move {
            gh_get(&client, s, &s.site_source_repo, path)
                .await
                .inspect_err(|e| tracing::warn!("site changelog: {path} fetch failed: {}", e.message))
                .ok()
        }
    };
    let tags = fetch("tags?per_page=100").await;
    let commits = fetch("commits?per_page=100").await;
    changelog_payload(s, tags, commits)
}

/// `get_changelog()`: Redis first, GitHub on a miss; an empty rate-limited
/// build is not cached.
pub async fn get_changelog(state: &AppState) -> ApiResult<Value> {
    let redis = state.inner.db.redis.clone();
    if let Some(mut r) = redis.clone() {
        match r.get::<_, Option<Vec<u8>>>(CHANGELOG_KEY).await {
            Ok(Some(raw)) if !raw.is_empty() => match pyjson::loads(&raw) {
                Ok(v) => return Ok(v),
                Err(e) => tracing::warn!("site changelog: Redis read failed: {e:?}"),
            },
            Ok(_) => {}
            Err(e) => tracing::warn!("site changelog: Redis read failed: {e}"),
        }
    }
    let data = build_changelog(state).await?;
    let empty_limited =
        data["rate_limited"] == Value::Bool(true) && data["groups"].as_array().is_none_or(Vec::is_empty);
    if let Some(mut r) = redis
        && !empty_limited
        && let Err(e) = r.set_ex::<_, _, ()>(CHANGELOG_KEY, pyjson::dumps_spaced(&data), CHANGELOG_TTL).await
    {
        tracing::warn!("site changelog: Redis write failed: {e}");
    }
    Ok(data)
}

async fn site_changelog(_: SiteGate, State(state): State<AppState>) -> ApiResult<Response> {
    Ok(cached(get_changelog(&state).await?, "public, max-age=300"))
}

// --- POST /admin/btt/refresh-releases ------------------------------------------------

fn mongo_err(e: impl std::fmt::Display) -> GhError {
    GhError { message: e.to_string(), status: false }
}

async fn refresh_releases(state: &AppState, client: &reqwest::Client) -> Result<usize, GhError> {
    let s = state.settings();
    let raw = list_or_empty(gh_get(client, s, &s.btt_releases_repo, "releases?per_page=100").await?);
    github::store_releases(&state.mongo().collection::<Document>(RELEASES), &raw).await.map_err(mongo_err)
}

async fn refresh_changelog(state: &AppState, client: &reqwest::Client) -> Result<(usize, bool), GhError> {
    let s = state.settings();
    let tags = match gh_get(client, s, &s.btt_releases_repo, "tags?per_page=100").await {
        Ok(v) => list_or_empty(v),
        Err(e) if e.status => Vec::new(),
        Err(e) => return Err(e),
    };
    let commits = gh_get(client, s, &s.btt_releases_repo, "commits?per_page=100").await?;
    let coll = state.mongo().collection::<Document>(CHANGELOGS);
    github::store_changelog(&coll, &s.btt_releases_repo, &tags, commits).await.map_err(mongo_err)
}

/// `int(a.get("download_count") or 0)`; `None` where Python's `int()` raises.
fn download_count(a: &Bson) -> Option<i64> {
    match a.as_document()?.get("download_count") {
        None | Some(Bson::Null) => Some(0),
        Some(Bson::Int32(i)) => Some((*i).into()),
        Some(Bson::Int64(i)) => Some(*i),
        Some(Bson::Boolean(b)) => Some((*b).into()),
        Some(Bson::Double(f)) if f.is_finite() => Some(f.trunc() as i64),
        Some(Bson::String(s)) if s.is_empty() => Some(0),
        Some(Bson::String(s)) => crate::middleware::pyurl::py_int(s).and_then(|n| i64::try_from(n).ok()),
        Some(_) => None,
    }
}

async fn total_downloads(state: &AppState) -> Option<i64> {
    let docs: Vec<Document> =
        state.mongo().collection::<Document>(RELEASES).find(doc! {}).await.ok()?.try_collect().await.ok()?;
    let mut total = 0i64;
    for d in &docs {
        for a in Release::from_doc(d).ok()?.assets {
            total = total.checked_add(download_count(&a)?)?;
        }
    }
    Some(total)
}

/// The panel's result, in the order Python fills the dict.
pub fn refresh_body(
    releases: Result<usize, String>,
    changelog: Result<(usize, bool), String>,
    latest: Vec<(String, String)>,
    downloads: Option<i64>,
) -> Value {
    let mut errors = Vec::new();
    let mut out = Map::new();
    out.insert("releases".into(), Value::Null);
    out.insert("changelog".into(), Value::Null);
    match releases {
        Ok(n) => {
            out.insert("releases".into(), n.into());
        }
        Err(e) => errors.push(format!("releases: {e}")),
    }
    let mut rate_limited = None;
    match changelog {
        Ok((n, limited)) => {
            out.insert("changelog".into(), n.into());
            rate_limited = Some(limited);
        }
        Err(e) => errors.push(format!("changelog: {e}")),
    }
    let ok = errors.is_empty();
    out.insert("errors".into(), json!(errors));
    if let Some(limited) = rate_limited {
        out.insert("rate_limited".into(), limited.into());
    }
    out.insert("latest".into(), Value::Object(latest.into_iter().map(|(k, v)| (k, Value::String(v))).collect()));
    out.insert("downloads".into(), downloads.map_or(Value::Null, Value::from));
    out.insert("ok".into(), ok.into());
    Value::Object(out)
}

async fn refresh(Superuser(_): Superuser, State(state): State<AppState>) -> Response {
    let client = match http_client(Duration::from_secs(15)) {
        Ok(c) => c,
        Err(e) => return internal(e).into_response(),
    };
    let releases = refresh_releases(&state, &client).await.map_err(|e| e.message);
    let changelog = refresh_changelog(&state, &client).await.map_err(|e| e.message);
    let mut latest = Vec::new();
    for channel in CHANNELS {
        let Ok(found) = channel_releases(&Mongo(&state), channel).await else { continue };
        let Ok(hits) = PLATFORMS.iter().map(|p| walk_latest(&found, p)).collect::<ApiResult<Vec<_>>>() else {
            continue;
        };
        for (p, hit) in PLATFORMS.iter().zip(hits) {
            if let Some((r, _)) = hit
                && !r.tag_name.is_empty()
            {
                latest.push((format!("{channel}/{p}"), r.tag_name.clone()));
            }
        }
    }
    let downloads = total_downloads(&state).await;
    PydanticJson(refresh_body(releases, changelog, latest, downloads)).into_response()
}

#[cfg(test)]
#[path = "site_tests.rs"]
mod tests;
