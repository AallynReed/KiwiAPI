//! The `btt_releases` loop: GitHub releases upserted into `btt_releases`, then
//! tags + commits grouped into the `btt_changelogs` singleton, each half
//! failing on its own.

use std::{collections::HashMap, time::Duration};

use anyhow::{Context, anyhow};
use kiwi_core::btt::{conventional_prefix, py_strip};
use mongodb::{
    Collection,
    bson::{Bson, Document, doc, oid::ObjectId},
};
use serde_json::Value;

use super::{http_client, json_to_bson, py_str, releases::CHANGELOGS, releases::RELEASES, truthy};
use crate::{auth::pyutil::int_of, config::Settings, dt, state::AppState};

const GITHUB: &str = "https://api.github.com";
pub const UNRELEASED: &str = "Unreleased";

pub async fn refresh_cycle(state: AppState) -> anyhow::Result<()> {
    let client = http_client(Duration::from_secs(15))?;
    match refresh_releases(&state, &client).await {
        Ok(n) => tracing::info!("BTT releases refreshed: {n} release(s)"),
        Err(e) => tracing::warn!("BTT releases refresh failed: {e:#}"),
    }
    match refresh_changelog(&state, &client).await {
        Ok((n, limited)) => {
            tracing::info!("BTT changelog refreshed: {n} group(s){}", if limited { " (rate-limited)" } else { "" })
        }
        Err(e) => tracing::warn!("BTT changelog refresh failed: {e:#}"),
    }
    Ok(())
}

enum GhError {
    /// `httpx.HTTPStatusError` (a non-2xx answer).
    Status(reqwest::StatusCode),
    Other(anyhow::Error),
}

impl From<GhError> for anyhow::Error {
    fn from(e: GhError) -> Self {
        match e {
            GhError::Status(s) => anyhow!("GitHub answered {s}"),
            GhError::Other(e) => e,
        }
    }
}

/// `_gh_get`: GET a path under the configured repo, parsed JSON.
async fn gh_get(client: &reqwest::Client, s: &Settings, path: &str) -> Result<Value, GhError> {
    let url = format!("{GITHUB}/repos/{}/{path}", s.btt_releases_repo);
    let mut req = client
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header("User-Agent", "KiwiAPI/1.0");
    if let Some(token) = s.btt_releases_token.as_deref().filter(|t| !t.is_empty()) {
        req = req.header("Authorization", format!("Bearer {token}"));
    }
    let resp = req.send().await.map_err(|e| GhError::Other(e.into()))?;
    if !resp.status().is_success() {
        return Err(GhError::Status(resp.status()));
    }
    let bytes = resp.bytes().await.map_err(|e| GhError::Other(e.into()))?;
    serde_json::from_slice(&bytes).map_err(|e| GhError::Other(e.into()))
}

fn list_or_empty(v: Value) -> Vec<Value> {
    match v {
        Value::Array(items) => items,
        _ => Vec::new(),
    }
}

#[cfg(test)]
pub async fn fetch_list(client: &reqwest::Client, s: &Settings, path: &str) -> anyhow::Result<Vec<Value>> {
    Ok(list_or_empty(gh_get(client, s, path).await?))
}

fn mongo(state: &AppState) -> anyhow::Result<&mongodb::Database> {
    state.inner.db.mongo.as_ref().context("MONGO_URI not configured")
}

/// `_parse_dt`: `Ok(None)` for a falsy or unparseable value.
fn parse_dt(v: Option<&Value>) -> Result<Option<Bson>, String> {
    match v {
        _ if !truthy(v) => Ok(None),
        Some(Value::String(s)) => Ok(dt::parse_utc(&s.replace('Z', "+00:00")).map(|d| Bson::DateTime(dt::to_bson(d)))),
        _ => Err("published_at is not a string".into()),
    }
}

/// `int(x or 0)`.
fn py_int_or_zero(v: Option<&Value>, what: &str) -> Result<Bson, String> {
    if !truthy(v) {
        return Ok(Bson::Int32(0));
    }
    let n = v.and_then(int_of).ok_or_else(|| format!("{what} is not an int"))?;
    let n = i64::try_from(n).map_err(|_| format!("{what} overflows BSON"))?;
    Ok(crate::auth::pyutil::bson_int(n))
}

/// A `str` field that Python fills with `x or ""` (non-str truthy values fail
/// validation there).
fn str_or_empty(v: Option<&Value>, what: &str) -> Result<String, String> {
    match v {
        _ if !truthy(v) => Ok(String::new()),
        Some(Value::String(s)) => Ok(s.clone()),
        _ => Err(format!("{what} is not a string")),
    }
}

/// `normalize_release`: the stored fields of one GitHub release (no
/// `fetched_at`), `None` for drafts and incomplete entries.
pub fn normalize_release(raw: &Value) -> Result<Option<Document>, String> {
    let Some(r) = raw.as_object() else { return Ok(None) };
    if truthy(r.get("draft")) {
        return Ok(None);
    }
    let release_id = r.get("id").filter(|v| v.is_i64()).and_then(Value::as_i64);
    let (tag, html) = (r.get("tag_name"), r.get("html_url"));
    let published = parse_dt(r.get("published_at"))?;
    let (Some(release_id), true, true, Some(published)) = (release_id, truthy(tag), truthy(html), published) else {
        return Ok(None);
    };
    let tag = str_or_empty(tag, "tag_name")?;
    let html = str_or_empty(html, "html_url")?;
    let mut assets = Vec::new();
    match r.get("assets") {
        Some(Value::Array(items)) => {
            for a in items {
                let Some(a) = a.as_object() else { continue };
                if !truthy(a.get("name")) || !truthy(a.get("browser_download_url")) {
                    continue;
                }
                let mut d = Document::new();
                d.insert("name", json_to_bson(&a["name"]));
                d.insert("url", json_to_bson(&a["browser_download_url"]));
                d.insert("size", py_int_or_zero(a.get("size"), "size")?);
                d.insert("content_type", a.get("content_type").map_or(Bson::Null, json_to_bson));
                d.insert("download_count", py_int_or_zero(a.get("download_count"), "download_count")?);
                assets.push(Bson::Document(d));
            }
        }
        Some(Value::Number(_) | Value::Bool(true)) => return Err("assets is not iterable".into()),
        _ => {}
    }
    let name = py_strip(&str_or_empty(r.get("name"), "name")?).to_owned();
    Ok(Some(doc! {
        "release_id": crate::auth::pyutil::bson_int(release_id),
        "tag_name": tag,
        "name": name,
        "body": str_or_empty(r.get("body"), "body")?,
        "html_url": html,
        "prerelease": truthy(r.get("prerelease")),
        "published_at": published,
        "assets": assets,
    }))
}

/// `refresh_releases`: upsert by `release_id`, `fetched_at` stamped on both
/// paths. Beanie's insert writes the model fields in order; its save `$set`s
/// them all, which leaves the stored document the same as this.
async fn refresh_releases(state: &AppState, client: &reqwest::Client) -> anyhow::Result<usize> {
    let raw = list_or_empty(gh_get(client, state.settings(), "releases?per_page=100").await?);
    store_releases(&mongo(state)?.collection::<Document>(RELEASES), &raw).await
}

pub async fn store_releases(coll: &Collection<Document>, raw: &[Value]) -> anyhow::Result<usize> {
    let mut stored = 0;
    for r in raw {
        let Some(mut fields) = normalize_release(r).map_err(|e| anyhow!(e))? else { continue };
        fields.insert("fetched_at", dt::to_bson(dt::now()));
        let release_id = fields.get("release_id").cloned().unwrap_or(Bson::Null);
        match coll.find_one(doc! { "release_id": release_id }).projection(doc! { "_id": 1 }).await? {
            None => {
                let mut d = doc! { "_id": ObjectId::new() };
                d.extend(fields);
                coll.insert_one(d).await?;
            }
            Some(existing) => {
                let id = existing.get("_id").cloned().unwrap_or(Bson::Null);
                coll.update_one(doc! { "_id": id }, doc! { "$set": fields }).await?;
            }
        }
        stored += 1;
    }
    Ok(stored)
}

/// `build_changelog_groups`: commits (newest first) split into a group at
/// every tagged sha, led by "Unreleased"; empty groups dropped.
pub fn build_changelog_groups(tags: &[Value], commits: &[Value]) -> Result<Vec<Bson>, String> {
    let mut tag_map: HashMap<&str, &Value> = HashMap::new();
    for tag in tags {
        if !truthy(Some(tag)) {
            continue;
        }
        let t = tag.as_object().ok_or("tag is not a dict")?;
        let commit = t.get("commit");
        if !truthy(commit) {
            continue;
        }
        let c = commit.and_then(Value::as_object).ok_or("tag commit is not a dict")?;
        let (sha, name) = (c.get("sha"), t.get("name"));
        if !(truthy(sha) && truthy(name)) {
            continue;
        }
        match (sha, name) {
            (Some(Value::String(s)), Some(n)) => {
                tag_map.insert(s, n);
            }
            (Some(Value::Array(_) | Value::Object(_)), _) => return Err("unhashable tag sha".into()),
            _ => {}
        }
    }
    let mut groups: Vec<(Bson, Vec<Bson>)> = vec![(Bson::String(UNRELEASED.into()), Vec::new())];
    for c in commits {
        let Some(c) = c.as_object() else { continue };
        let sha = c.get("sha");
        let commit = c.get("commit");
        let message = if truthy(commit) {
            let o = commit.and_then(Value::as_object).ok_or("commit is not a dict")?;
            str_or_empty(o.get("message"), "message")?
        } else {
            String::new()
        };
        let raw_msg = py_strip(&message);
        if !(truthy(sha) && !raw_msg.is_empty()) {
            continue;
        }
        let Some(Value::String(sha)) = sha else { return Err("commit sha is not a string".into()) };
        if let Some(version) = tag_map.get(sha.as_str()) {
            groups.push((json_to_bson(version), Vec::new()));
        }
        let first_line = py_strip(raw_msg.split('\n').next().unwrap_or(""));
        let url = c.get("html_url").filter(|v| truthy(Some(v))).map_or(Bson::String(String::new()), json_to_bson);
        let entry = doc! {
            "sha": sha.as_str(),
            "short_sha": sha.chars().take(7).collect::<String>(),
            "message": first_line,
            "type": conventional_prefix(first_line).map_or(Bson::Null, Bson::String),
            "url": url,
        };
        if let Some(last) = groups.last_mut() {
            last.1.push(Bson::Document(entry));
        }
    }
    Ok(groups
        .into_iter()
        .filter(|(_, commits)| !commits.is_empty())
        .map(|(version, commits)| Bson::Document(doc! { "version": version, "commits": commits }))
        .collect())
}

/// `"rate limit" in str(body.get("message", "")).lower()` for a dict body.
pub fn is_rate_limited(body: &Value) -> bool {
    let Some(m) = body.as_object() else { return false };
    let text = m.get("message").map_or_else(String::new, py_str);
    text.to_lowercase().contains("rate limit")
}

/// `refresh_changelog`: a rate-limited cycle with no commits keeps the
/// groups already stored.
async fn refresh_changelog(state: &AppState, client: &reqwest::Client) -> anyhow::Result<(usize, bool)> {
    let s = state.settings();
    let tags = match gh_get(client, s, "tags?per_page=100").await {
        Ok(v) => list_or_empty(v),
        Err(GhError::Status(_)) => Vec::new(),
        Err(e) => return Err(e.into()),
    };
    let raw_commits = gh_get(client, s, "commits?per_page=100").await?;
    let coll = mongo(state)?.collection::<Document>(CHANGELOGS);
    store_changelog(&coll, &s.btt_releases_repo, &tags, raw_commits).await
}

pub async fn store_changelog(
    coll: &Collection<Document>,
    repo: &str,
    tags: &[Value],
    raw_commits: Value,
) -> anyhow::Result<(usize, bool)> {
    let rate_limited = is_rate_limited(&raw_commits);
    let commits = list_or_empty(raw_commits);
    let groups = build_changelog_groups(tags, &commits).map_err(|e| anyhow!(e))?;
    let count = groups.len();
    let now = dt::to_bson(dt::now());
    match coll.find_one(doc! { "repo": repo }).await? {
        None => {
            coll.insert_one(doc! {
                "_id": ObjectId::new(),
                "repo": repo,
                "groups": groups,
                "rate_limited": rate_limited,
                "fetched_at": now,
            })
            .await?;
        }
        Some(existing) => {
            let kept = existing.get("groups").cloned().unwrap_or_else(|| Bson::Array(Vec::new()));
            let had_groups = matches!(&kept, Bson::Array(g) if !g.is_empty());
            let groups = if rate_limited && groups.is_empty() && had_groups { kept } else { Bson::Array(groups) };
            let id = existing.get("_id").cloned().unwrap_or(Bson::Null);
            let stored_repo = existing.get("repo").cloned().unwrap_or_else(|| Bson::String(repo.to_owned()));
            coll.update_one(
                doc! { "_id": id },
                doc! { "$set": { "repo": stored_repo, "groups": groups, "rate_limited": rate_limited, "fetched_at": now } },
            )
            .await?;
        }
    }
    Ok((count, rate_limited))
}
