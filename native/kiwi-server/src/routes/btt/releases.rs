//! `/v1/btt/releases|latest|check|latest/{platform}|changelog` over the
//! `btt_releases` and `btt_changelogs` collections.

use axum::{extract::State, response::IntoResponse};
use chrono::{DateTime, Utc};
use kiwi_core::btt::{CHANNELS, PLATFORMS, channel_prerelease, compare_versions, platform_order};
use mongodb::bson::{Bson, Document};
use serde_json::Value;

use super::{Mongo, Store, bool_field, dt_field, int_field, internal, obj, opt_str_field, str_field};
use crate::{
    auth::{Public, scopes::BTT_READ},
    dt,
    error::{ApiError, ApiResult},
    pyjson::PydanticJson,
    state::AppState,
    validation::{FromParams, Params, Path, Query},
};

pub const RELEASES: &str = "btt_releases";
pub const CHANGELOGS: &str = "btt_changelogs";

/// A stored `BttRelease` (assets stay raw: each route validates only the
/// ones it returns).
pub struct Release {
    pub release_id: i64,
    pub tag_name: String,
    pub name: String,
    pub body: String,
    pub html_url: String,
    pub prerelease: bool,
    pub published_at: DateTime<Utc>,
    pub fetched_at: DateTime<Utc>,
    pub assets: Vec<Bson>,
}

impl Release {
    pub fn from_doc(d: &Document) -> ApiResult<Release> {
        let assets = match d.get("assets") {
            None => Vec::new(),
            Some(Bson::Array(a)) if a.iter().all(|x| matches!(x, Bson::Document(_))) => a.clone(),
            Some(_) => return Err(internal("bad assets")),
        };
        Ok(Release {
            release_id: int_field(d, "release_id", None)?,
            tag_name: str_field(d, "tag_name", None)?,
            name: str_field(d, "name", Some(""))?,
            body: str_field(d, "body", Some(""))?,
            html_url: str_field(d, "html_url", None)?,
            prerelease: bool_field(d, "prerelease", false)?,
            published_at: dt_field(d, "published_at", false)?,
            fetched_at: dt_field(d, "fetched_at", true)?,
            assets,
        })
    }

    fn asset_names(&self) -> ApiResult<Vec<&str>> {
        self.assets
            .iter()
            .map(|a| match a.as_document().and_then(|d| d.get("name")) {
                None => Ok(""),
                Some(Bson::String(s)) => Ok(s.as_str()),
                Some(_) => Err(internal("bad asset name")),
            })
            .collect()
    }
}

/// `BttReleaseMeta`.
pub fn release_meta(r: &Release) -> Value {
    obj(vec![
        ("release_id", r.release_id.into()),
        ("tag_name", r.tag_name.clone().into()),
        ("name", r.name.clone().into()),
        ("body", r.body.clone().into()),
        ("html_url", r.html_url.clone().into()),
        ("prerelease", r.prerelease.into()),
        ("channel", if r.prerelease { "beta" } else { "release" }.into()),
        ("published_at", dt::pydantic(&r.published_at).into()),
        ("fetched_at", dt::pydantic(&r.fetched_at).into()),
    ])
}

/// `BttAsset(**a)`.
fn asset(a: &Bson) -> ApiResult<Value> {
    let d = a.as_document().ok_or_else(|| internal("bad asset"))?;
    Ok(obj(vec![
        ("name", str_field(d, "name", None)?.into()),
        ("url", str_field(d, "url", None)?.into()),
        ("size", int_field(d, "size", None)?.into()),
        ("content_type", opt_str_field(d, "content_type")?),
        ("download_count", int_field(d, "download_count", Some(0))?.into()),
    ]))
}

/// `BttReleaseInfo`: the meta plus every asset.
pub fn release_info(r: &Release) -> ApiResult<Value> {
    let mut v = release_meta(r);
    let assets = r.assets.iter().map(asset).collect::<ApiResult<Vec<_>>>()?;
    if let Value::Object(m) = &mut v {
        m.insert("assets".into(), Value::Array(assets));
    }
    Ok(v)
}

/// `walk_latest`: the first release (newest first) with an asset for
/// `platform`, and those assets in priority order.
pub fn walk_latest<'a>(releases: &'a [Release], platform: &str) -> ApiResult<Option<(&'a Release, Vec<&'a Bson>)>> {
    for r in releases {
        let order = platform_order(&r.asset_names()?, platform);
        if !order.is_empty() {
            return Ok(Some((r, order.into_iter().map(|i| &r.assets[i]).collect())));
        }
    }
    Ok(None)
}

/// `BttPlatformLatest`.
pub fn platform_latest(platform: &str, r: &Release, matched: &[&Bson]) -> ApiResult<Value> {
    let assets = matched.iter().map(|a| asset(a)).collect::<ApiResult<Vec<_>>>()?;
    Ok(obj(vec![("platform", platform.into()), ("release", release_meta(r)), ("assets", Value::Array(assets))]))
}

pub fn check_channel(channel: &str) -> ApiResult<()> {
    if channel_prerelease(channel).is_none() {
        return Err(ApiError::bad_request(format!("channel must be one of: {}", CHANNELS.join(", "))));
    }
    Ok(())
}

pub fn check_platform(platform: &str) -> ApiResult<()> {
    if !PLATFORMS.contains(&platform) {
        return Err(ApiError::not_found(format!("unknown platform '{platform}' (known: {})", PLATFORMS.join(", "))));
    }
    Ok(())
}

async fn channel<S: Store>(s: &S, channel: &str) -> ApiResult<Vec<Release>> {
    let docs = s.channel_releases(channel_prerelease(channel).unwrap_or(false)).await?;
    docs.iter().map(Release::from_doc).collect()
}

// --- /v1/btt/releases ---------------------------------------------------------

pub struct ListQuery {
    pub channel: Option<String>,
    pub limit: i64,
    pub offset: i64,
}

impl FromParams for ListQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        ListQuery {
            channel: p.query("channel").str().optional(),
            limit: p.query("limit").int().ge(1).le(200).default(50),
            offset: p.query("offset").int().ge(0).default(0),
        }
    }
}

pub fn list_body(q: &ListQuery, docs: &[Release], total: u64) -> ApiResult<Value> {
    let items = docs.iter().map(release_info).collect::<ApiResult<Vec<_>>>()?;
    Ok(obj(vec![
        ("channel", q.channel.clone().map_or(Value::Null, Value::String)),
        ("items", Value::Array(items)),
        ("count", docs.len().into()),
        ("total", total.into()),
    ]))
}

pub(crate) async fn list_core<S: Store>(s: &S, q: &ListQuery) -> ApiResult<Value> {
    if let Some(c) = &q.channel {
        check_channel(c)?;
    }
    let prerelease = q.channel.as_deref().and_then(channel_prerelease);
    let (docs, total) = s.release_page(prerelease, q.offset as u64, q.limit).await?;
    let docs = docs.iter().map(Release::from_doc).collect::<ApiResult<Vec<_>>>()?;
    list_body(q, &docs, total)
}

pub async fn list(
    State(state): State<AppState>,
    Public(ctx): Public<{ BTT_READ }>,
    Query(q): Query<ListQuery>,
) -> ApiResult<impl IntoResponse> {
    Ok((ctx.rate_limit, PydanticJson(list_core(&Mongo(&state), &q).await?)))
}

// --- /v1/btt/latest -----------------------------------------------------------

pub struct ChannelQuery {
    pub channel: String,
}

impl FromParams for ChannelQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        ChannelQuery { channel: p.query("channel").str().default("release".into()) }
    }
}

pub fn latest_body(channel: &str, releases: &[Release]) -> ApiResult<Value> {
    let mut platforms = serde_json::Map::new();
    for p in PLATFORMS {
        let v = match walk_latest(releases, p)? {
            Some((r, matched)) => platform_latest(p, r, &matched)?,
            None => Value::Null,
        };
        platforms.insert(p.into(), v);
    }
    Ok(obj(vec![("channel", channel.into()), ("platforms", Value::Object(platforms))]))
}

pub(crate) async fn latest_core<S: Store>(s: &S, q: &ChannelQuery) -> ApiResult<Value> {
    check_channel(&q.channel)?;
    latest_body(&q.channel, &channel(s, &q.channel).await?)
}

pub async fn latest(
    State(state): State<AppState>,
    Public(ctx): Public<{ BTT_READ }>,
    Query(q): Query<ChannelQuery>,
) -> ApiResult<impl IntoResponse> {
    Ok((ctx.rate_limit, PydanticJson(latest_core(&Mongo(&state), &q).await?)))
}

// --- /v1/btt/check ------------------------------------------------------------

pub struct CheckQuery {
    pub installed: String,
    pub platform: String,
    pub channel: String,
}

impl FromParams for CheckQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        CheckQuery {
            installed: p.query("installed").str().min_len(1).required(),
            platform: p.query("platform").str().required(),
            channel: p.query("channel").str().default("release".into()),
        }
    }
}

pub fn check_body(q: &CheckQuery, releases: &[Release]) -> ApiResult<Value> {
    let (update, comparable, latest) = match walk_latest(releases, &q.platform)? {
        None => (false, false, Value::Null),
        Some((r, matched)) => {
            let cmp = compare_versions(&q.installed, &r.tag_name);
            (cmp.is_some_and(|c| c < 0), cmp.is_some(), platform_latest(&q.platform, r, &matched)?)
        }
    };
    Ok(obj(vec![
        ("installed", q.installed.clone().into()),
        ("channel", q.channel.clone().into()),
        ("platform", q.platform.clone().into()),
        ("update_available", update.into()),
        ("comparable", comparable.into()),
        ("latest", latest),
    ]))
}

pub(crate) async fn check_core<S: Store>(s: &S, q: &CheckQuery) -> ApiResult<Value> {
    check_platform(&q.platform)?;
    check_channel(&q.channel)?;
    check_body(q, &channel(s, &q.channel).await?)
}

pub async fn check(
    State(state): State<AppState>,
    Public(ctx): Public<{ BTT_READ }>,
    Query(q): Query<CheckQuery>,
) -> ApiResult<impl IntoResponse> {
    Ok((ctx.rate_limit, PydanticJson(check_core(&Mongo(&state), &q).await?)))
}

// --- /v1/btt/latest/{platform} --------------------------------------------------

pub struct PlatformQuery {
    pub platform: String,
    pub channel: String,
}

impl FromParams for PlatformQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        PlatformQuery {
            platform: p.path("platform").str().required(),
            channel: p.query("channel").str().default("release".into()),
        }
    }
}

pub fn platform_body(q: &PlatformQuery, releases: &[Release]) -> ApiResult<Value> {
    match walk_latest(releases, &q.platform)? {
        Some((r, matched)) => platform_latest(&q.platform, r, &matched),
        None => Err(ApiError::not_found(format!("no {} release carries a {} asset yet", q.channel, q.platform))),
    }
}

pub(crate) async fn platform_core<S: Store>(s: &S, q: &PlatformQuery) -> ApiResult<Value> {
    check_platform(&q.platform)?;
    check_channel(&q.channel)?;
    platform_body(q, &channel(s, &q.channel).await?)
}

pub async fn latest_for_platform(
    State(state): State<AppState>,
    Public(ctx): Public<{ BTT_READ }>,
    Path(q): Path<PlatformQuery>,
) -> ApiResult<impl IntoResponse> {
    Ok((ctx.rate_limit, PydanticJson(platform_core(&Mongo(&state), &q).await?)))
}

// --- /v1/btt/changelog ----------------------------------------------------------

pub struct ChangelogQuery {
    pub limit_groups: i64,
    pub commits_per_group: i64,
}

impl FromParams for ChangelogQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        ChangelogQuery {
            limit_groups: p.query("limit_groups").int().ge(0).le(50).default(0),
            commits_per_group: p.query("commits_per_group").int().ge(0).le(100).default(0),
        }
    }
}

/// `BttCommit(**c)`.
fn commit(c: &Bson) -> ApiResult<Value> {
    let d = c.as_document().ok_or_else(|| internal("bad commit"))?;
    Ok(obj(vec![
        ("sha", str_field(d, "sha", None)?.into()),
        ("short_sha", str_field(d, "short_sha", None)?.into()),
        ("message", str_field(d, "message", None)?.into()),
        ("type", opt_str_field(d, "type")?),
        ("url", str_field(d, "url", None)?.into()),
    ]))
}

/// The response for a stored changelog, or the empty one (`repo` from the
/// settings, `fetched_at` now) before the first refresh.
pub fn changelog_body(q: &ChangelogQuery, doc: Option<&Document>, repo: &str) -> ApiResult<Value> {
    let Some(d) = doc else {
        return Ok(obj(vec![
            ("repo", repo.into()),
            ("groups", Value::Array(Vec::new())),
            ("rate_limited", false.into()),
            ("fetched_at", dt::pydantic(&dt::now()).into()),
        ]));
    };
    let groups: &[Bson] = match d.get("groups") {
        None => &[],
        Some(Bson::Array(g)) if g.iter().all(|x| matches!(x, Bson::Document(_))) => g,
        Some(_) => return Err(internal("bad groups")),
    };
    let take = |n: i64, len: usize| if n > 0 { len.min(n as usize) } else { len };
    let mut out = Vec::new();
    for g in &groups[..take(q.limit_groups, groups.len())] {
        let g = g.as_document().ok_or_else(|| internal("bad group"))?;
        let commits: &[Bson] = match g.get("commits") {
            Some(Bson::Array(c)) => c,
            _ => return Err(internal("bad group commits")),
        };
        let commits = &commits[..take(q.commits_per_group, commits.len())];
        out.push(obj(vec![
            ("version", str_field(g, "version", None)?.into()),
            ("commits", Value::Array(commits.iter().map(commit).collect::<ApiResult<_>>()?)),
        ]));
    }
    Ok(obj(vec![
        ("repo", str_field(d, "repo", None)?.into()),
        ("groups", Value::Array(out)),
        ("rate_limited", bool_field(d, "rate_limited", false)?.into()),
        ("fetched_at", dt::pydantic(&dt_field(d, "fetched_at", true)?).into()),
    ]))
}

pub(crate) async fn changelog_core<S: Store>(s: &S, q: &ChangelogQuery, repo: &str) -> ApiResult<Value> {
    changelog_body(q, s.changelog(repo).await?.as_ref(), repo)
}

pub async fn changelog(
    State(state): State<AppState>,
    Public(ctx): Public<{ BTT_READ }>,
    Query(q): Query<ChangelogQuery>,
) -> ApiResult<impl IntoResponse> {
    let repo = &state.settings().btt_releases_repo;
    Ok((ctx.rate_limit, PydanticJson(changelog_core(&Mongo(&state), &q, repo).await?)))
}
