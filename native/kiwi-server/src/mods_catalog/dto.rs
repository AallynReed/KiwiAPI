//! The hand-built response dicts of `mods_hub/service.py`, key for key.

use chrono::{DateTime, Duration, Utc};
use kiwi_core::{mod_categories, pystr};
use serde_json::{Map, Value, json};

use super::model::{Claim, Project, Release, RemixClaim, map_json};
use crate::{auth::models::SiteUser, config::Settings, dt};

pub const REMIX_AUTO_APPROVE_DAYS: i64 = 7;
const DISCORD_CDN: &str = "https://cdn.discordapp.com";

/// `settings.api_url` / `settings.app_url` with the trailing `/` stripped.
#[derive(Clone, Debug)]
pub struct Urls {
    pub api: String,
    pub app: String,
}

impl Urls {
    pub fn of(s: &Settings) -> Self {
        Urls { api: s.api_url.trim_end_matches('/').to_owned(), app: s.app_url.trim_end_matches('/').to_owned() }
    }
}

pub fn iso(d: &DateTime<Utc>) -> Value {
    Value::String(dt::isoformat(d))
}

pub fn opt_iso(d: &Option<DateTime<Utc>>) -> Value {
    d.as_ref().map_or(Value::Null, iso)
}

fn opt(v: &Option<String>) -> Value {
    v.as_ref().map_or(Value::Null, |s| Value::String(s.clone()))
}

fn strs(v: &[String]) -> Value {
    Value::Array(v.iter().cloned().map(Value::String).collect())
}

pub fn remix_auto_approve() -> Duration {
    Duration::days(REMIX_AUTO_APPROVE_DAYS)
}

/// `effective_visibility(project)` at `now`.
pub fn effective_visibility(p: &Project, now: DateTime<Utc>) -> &str {
    if !p.is_remix || !matches!(p.visibility.as_str(), "public" | "public_website_only") {
        return &p.visibility;
    }
    if p.remix_status == "denied" {
        return "hidden";
    }
    if p.remix_status == "approved" || p.remix_requested_at.is_some_and(|r| now - r >= remix_auto_approve()) {
        return "public_website_only";
    }
    "hidden"
}

fn lineage(out: &mut Map<String, Value>, p: &Project) {
    let forked_from = if p.forked_from_slug.as_deref().is_some_and(|s| !s.is_empty()) {
        json!({"slug": opt(&p.forked_from_slug), "handle": opt(&p.forked_from_handle),
               "title": opt(&p.forked_from_title), "owner": opt(&p.forked_from_owner)})
    } else {
        Value::Null
    };
    let inspired_by = if p.inspired_by_slug.as_deref().is_some_and(|s| !s.is_empty()) {
        json!({"slug": opt(&p.inspired_by_slug), "handle": opt(&p.inspired_by_handle),
               "title": opt(&p.inspired_by_title), "owner": opt(&p.inspired_by_owner)})
    } else {
        Value::Null
    };
    out.insert("forked_from".into(), forked_from);
    out.insert("inspired_by".into(), inspired_by);
    out.insert("fork_count".into(), p.fork_count.into());
}

fn author(p: &Project) -> String {
    if p.author.is_empty() { p.owner_username.clone() } else { p.author.clone() }
}

/// `project_card(p, reveal=)`.
pub fn project_card(p: &Project, reveal: bool) -> Map<String, Value> {
    let shown = if !reveal && p.visibility == "public_website_only" { "public" } else { p.visibility.as_str() };
    let mut m = Map::new();
    m.insert("slug".into(), p.slug.clone().into());
    m.insert("handle".into(), p.owner_handle.clone().into());
    m.insert("title".into(), p.title.clone().into());
    m.insert("title_i18n".into(), map_json(&p.title_i18n));
    m.insert("summary".into(), p.summary.clone().into());
    m.insert("summary_i18n".into(), map_json(&p.summary_i18n));
    m.insert("tags".into(), strs(&p.tags));
    m.insert("owner_username".into(), p.owner_username.clone().into());
    m.insert("visibility".into(), shown.into());
    m.insert("taken_down".into(), p.taken_down.into());
    m.insert("is_beta".into(), p.is_beta.into());
    m.insert("mode".into(), p.mode.clone().into());
    m.insert("banner_sha".into(), opt(&p.banner_sha));
    m.insert("preview_sha".into(), p.preview_shas.first().map_or(Value::Null, |s| s.clone().into()));
    m.insert("download_count".into(), p.download_count.into());
    m.insert("star_count".into(), p.star_count.into());
    m.insert("is_stray".into(), p.is_stray.into());
    m.insert("uploaded_on_behalf".into(), p.uploaded_on_behalf.into());
    m.insert("author".into(), author(p).into());
    m.insert("updated_at".into(), iso(&p.updated_at));
    m.insert("last_release_at".into(), opt_iso(&p.last_release_at));
    m.insert("created_at".into(), iso(&p.created_at));
    lineage(&mut m, p);
    m
}

/// `_safe_filename`.
pub fn safe_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | '\0'..='\x1f'))
        .collect();
    let cleaned = pystr::strip(&cleaned).trim_matches('.');
    let cut = pystr::prefix(cleaned, 120);
    if cut.is_empty() { "mod".into() } else { cut.to_owned() }
}

fn prop<'a>(r: &'a Release, key: &str) -> Option<&'a str> {
    r.tmod_properties.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// `release_download_filename`.
pub fn release_download_filename(r: &Release) -> String {
    if r.release_format == "zip" {
        return r.tmod_filename.clone();
    }
    let title = pystr::strip(prop(r, "title").unwrap_or(""));
    if title.is_empty() { r.tmod_filename.clone() } else { format!("{}.tmod", safe_filename(title)) }
}

/// `_release_dto`.
pub fn release_dto(r: &Release) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("id".into(), r.id.to_hex().into());
    m.insert("tag".into(), r.tag.clone().into());
    m.insert("branch".into(), r.branch.clone().into());
    m.insert("title".into(), r.title.clone().into());
    m.insert("title_i18n".into(), map_json(&r.title_i18n));
    m.insert("changelog".into(), r.changelog.clone().into());
    m.insert("changelog_i18n".into(), map_json(&r.changelog_i18n));
    m.insert("status".into(), r.status.clone().into());
    m.insert("silent".into(), r.silent.into());
    m.insert("tmod_filename".into(), release_download_filename(r).into());
    m.insert("tmod_size".into(), r.tmod_size.into());
    m.insert("download_count".into(), r.download_count.into());
    m.insert("format".into(), r.release_format.clone().into());
    m.insert("source_commit_sha".into(), opt(&r.source_commit_sha));
    m.insert("banner_sha".into(), opt(&r.banner_sha));
    m.insert("published_at".into(), opt_iso(&r.published_at));
    m.insert("created_at".into(), iso(&r.created_at));
    m
}

/// `_public_img_url`.
pub fn public_img_url(u: &Urls, sha: Option<&str>) -> Option<String> {
    sha.filter(|s| !s.is_empty()).map(|s| format!("{}/v1/mods/hub/image/{s}", u.api))
}

fn url_value(v: Option<String>) -> Value {
    v.map_or(Value::Null, Value::String)
}

/// `public_release_dto`.
pub fn public_release_dto(u: &Urls, r: &Release) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("tag".into(), r.tag.clone().into());
    m.insert("branch".into(), r.branch.clone().into());
    m.insert("title".into(), r.title.clone().into());
    m.insert("title_i18n".into(), map_json(&r.title_i18n));
    m.insert("changelog".into(), r.changelog.clone().into());
    m.insert("changelog_i18n".into(), map_json(&r.changelog_i18n));
    m.insert("format".into(), r.release_format.clone().into());
    m.insert("filename".into(), release_download_filename(r).into());
    m.insert("size".into(), r.tmod_size.into());
    m.insert("sha256".into(), r.tmod_sha.clone().into());
    m.insert("download_count".into(), r.download_count.into());
    m.insert("download_url".into(), format!("{}/v1/mods/hub/releases/{}/download", u.api, r.id.to_hex()).into());
    m.insert("published_at".into(), opt_iso(&r.published_at));
    m
}

/// `public_mod_dto(p, releases=)`.
pub fn public_mod_dto(u: &Urls, p: &Project, releases: Option<&[Release]>) -> Map<String, Value> {
    let flags = mod_categories::flags_from_tags(p.tags.iter().map(String::as_str));
    let mut m = Map::new();
    m.insert("slug".into(), p.slug.clone().into());
    m.insert("handle".into(), p.owner_handle.clone().into());
    m.insert("title".into(), p.title.clone().into());
    m.insert("title_i18n".into(), map_json(&p.title_i18n));
    m.insert("summary".into(), p.summary.clone().into());
    m.insert("summary_i18n".into(), map_json(&p.summary_i18n));
    m.insert("description".into(), p.description.clone().into());
    m.insert("description_i18n".into(), map_json(&p.description_i18n));
    m.insert("readme_text".into(), p.readme_text.clone().into());
    m.insert("readme_i18n".into(), map_json(&p.readme_i18n));
    m.insert("warnings".into(), p.warnings.clone().into());
    m.insert("warnings_i18n".into(), map_json(&p.warnings_i18n));
    m.insert("tags".into(), strs(&p.tags));
    m.insert("categories".into(), json!(mod_categories::tags_from_flags(flags)));
    m.insert("flags".into(), flags.into());
    m.insert("author".into(), author(p).into());
    m.insert("is_beta".into(), p.is_beta.into());
    m.insert("is_stray".into(), p.is_stray.into());
    m.insert("uploaded_on_behalf".into(), p.uploaded_on_behalf.into());
    m.insert("banner_url".into(), url_value(public_img_url(u, p.banner_sha.as_deref())));
    let previews: Vec<Value> =
        p.preview_shas.iter().filter_map(|s| public_img_url(u, Some(s))).map(Value::String).collect();
    m.insert("preview_urls".into(), Value::Array(previews));
    m.insert("download_count".into(), p.download_count.into());
    m.insert("downloads_7d".into(), p.downloads_7d.into());
    m.insert("star_count".into(), p.star_count.into());
    m.insert("popularity_score".into(), json!(p.popularity_score));
    m.insert("discord_url".into(), opt(&p.discord_url));
    m.insert("website_url".into(), opt(&p.website_url));
    m.insert("donation_urls".into(), strs(&p.donation_urls));
    m.insert("page_url".into(), format!("{}/mods/{}/{}", u.app, p.owner_handle, p.slug).into());
    m.insert("created_at".into(), iso(&p.created_at));
    m.insert("updated_at".into(), iso(&p.updated_at));
    m.insert("last_release_at".into(), opt_iso(&p.last_release_at));
    lineage(&mut m, p);
    if let Some(rs) = releases {
        m.insert("releases".into(), Value::Array(rs.iter().map(|r| Value::Object(public_release_dto(u, r))).collect()));
    }
    m
}

/// `_discord_avatar_url(user, size)`.
pub fn discord_avatar_url(user: &SiteUser, size: u32) -> Option<String> {
    let id = user.discord_id?;
    match user.discord_avatar.as_deref().filter(|h| !h.is_empty()) {
        Some(hash) => {
            let ext = if hash.starts_with("a_") { "gif" } else { "png" };
            Some(format!("{DISCORD_CDN}/avatars/{id}/{hash}.{ext}?size={size}"))
        }
        None => Some(format!("{DISCORD_CDN}/embed/avatars/{}.png", (id >> 22).rem_euclid(6))),
    }
}

/// `_profile_avatar_url(user, profile, small=)`.
pub fn profile_avatar_url(u: &Urls, user: &SiteUser, avatar_sha: Option<&str>, small: bool) -> Option<String> {
    if let Some(sha) = avatar_sha.filter(|s| !s.is_empty()) {
        let url = public_img_url(u, Some(sha))?;
        return Some(if small { format!("{url}?w=200") } else { url });
    }
    discord_avatar_url(user, if small { 64 } else { 256 })
}

/// `_branch_dto`.
pub fn branch_dto(name: &str, head: Option<&str>) -> Value {
    json!({"name": name, "head_commit_id": head})
}

/// `_remix_card(p)`.
pub fn remix_card(p: &Project, now: DateTime<Utc>) -> Map<String, Value> {
    let mut card = project_card(p, true);
    card.insert("id".into(), p.id.to_hex().into());
    card.insert("remix_status".into(), p.remix_status.clone().into());
    card.insert("effective_visibility".into(), effective_visibility(p, now).into());
    card.insert("remix_requested_at".into(), opt_iso(&p.remix_requested_at));
    let auto = match p.remix_requested_at {
        Some(r) if p.remix_status == "pending" => iso(&(r + remix_auto_approve())),
        _ => Value::Null,
    };
    card.insert("remix_auto_at".into(), auto);
    card
}

/// `_claim_dto` (note `resolved_at` goes through `iso()`, which is null-safe).
pub fn claim_dto(c: &Claim) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("id".into(), c.id.to_hex().into());
    m.insert("project_id".into(), c.project_id.to_hex().into());
    m.insert("project_slug".into(), c.project_slug.clone().into());
    m.insert("project_title".into(), c.project_title.clone().into());
    m.insert("claimant_username".into(), c.claimant_username.clone().into());
    m.insert("message".into(), c.message.clone().into());
    m.insert("status".into(), c.status.clone().into());
    m.insert("created_at".into(), iso(&c.created_at));
    m.insert("resolved_at".into(), opt_iso(&c.resolved_at));
    m
}

/// `_remix_claim_dto`.
pub fn remix_claim_dto(c: &RemixClaim) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("id".into(), c.id.to_hex().into());
    m.insert("target".into(), json!({"id": c.target_id.to_hex(), "slug": c.target_slug, "title": c.target_title}));
    m.insert(
        "original".into(),
        json!({"id": c.original_id.to_hex(), "slug": c.original_slug, "title": c.original_title}),
    );
    m.insert("claimant_username".into(), c.claimant_username.clone().into());
    m.insert("message".into(), c.message.clone().into());
    m.insert("status".into(), c.status.clone().into());
    m.insert("created_at".into(), iso(&c.created_at));
    m.insert("resolved_at".into(), opt_iso(&c.resolved_at));
    m
}
