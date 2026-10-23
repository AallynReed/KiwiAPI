//! The `mod_release` event (`_emit_release_event`): a published build of a
//! public mod announced once on the live stream, with the app-facing mod card
//! (`public_mod_dto`), the build, the one it supersedes and how they differ.

use mongodb::bson::{Bson, Document, doc};
use serde_json::{Map, Value, json};

use super::{
    access::{self, internal},
    model::{Doc, V},
    read::download_filename,
};
use crate::{dt, error::ApiError, pyfloat, state::AppState};

const OMIT: [&str; 4] = ["description", "description_i18n", "readme_text", "readme_i18n"];

/// Beanie `save()`: `$set` of every model field.
pub async fn save(state: &AppState, coll: &str, d: &Doc) -> Result<(), ApiError> {
    let db = state.inner.db.mongo.as_ref().ok_or_else(|| internal("no mongo"))?;
    db.collection::<Document>(coll)
        .update_one(doc! {"_id": d.id}, doc! {"$set": d.set_doc()})
        .upsert(true)
        .await
        .map_err(internal)?;
    Ok(())
}

/// `settings.api_url` / `settings.app_url`, trailing slash dropped.
pub struct Urls {
    pub api: String,
    pub site: String,
}

impl Urls {
    pub fn of(state: &AppState) -> Urls {
        let s = state.settings();
        Urls { api: s.api_url.trim_end_matches('/').to_owned(), site: s.app_url.trim_end_matches('/').to_owned() }
    }
}

/// `_public_img_url(sha)`.
fn img_url(u: &Urls, sha: Option<&str>) -> Value {
    match sha {
        Some(s) if !s.is_empty() => json!(format!("{}/v1/mods/hub/image/{s}", u.api)),
        _ => Value::Null,
    }
}

fn lineage(p: &Doc, m: &mut Map<String, Value>) {
    let block = |pre: &str| {
        let slug = p.opt_str(&format!("{pre}_slug")).filter(|s| !s.is_empty());
        slug.map_or(Value::Null, |_| {
            json!({
                "slug": p.opt_json(&format!("{pre}_slug")),
                "handle": p.opt_json(&format!("{pre}_handle")),
                "title": p.opt_json(&format!("{pre}_title")),
                "owner": p.opt_json(&format!("{pre}_owner")),
            })
        })
    };
    m.insert("forked_from".into(), block("forked_from"));
    m.insert("inspired_by".into(), block("inspired_by"));
    m.insert("fork_count".into(), json!(p.int("fork_count")));
}

/// `public_mod_dto(project)`.
pub fn public_mod_dto(u: &Urls, p: &Doc) -> Map<String, Value> {
    let tags: Vec<&str> = p.list("tags").iter().map(String::as_str).collect();
    let flags = kiwi_core::mod_categories::flags_from_tags(tags.iter().copied());
    let author = if p.str("author").is_empty() { p.str("owner_username") } else { p.str("author") };
    let previews: Vec<Value> =
        p.list("preview_shas").iter().map(|s| img_url(u, Some(s))).filter(|v| !v.is_null()).collect();
    let mut m = Map::new();
    m.insert("slug".into(), json!(p.str("slug")));
    m.insert("handle".into(), json!(p.str("owner_handle")));
    m.insert("title".into(), json!(p.str("title")));
    m.insert("title_i18n".into(), p.map_json("title_i18n"));
    m.insert("summary".into(), json!(p.str("summary")));
    m.insert("summary_i18n".into(), p.map_json("summary_i18n"));
    m.insert("description".into(), json!(p.str("description")));
    m.insert("description_i18n".into(), p.map_json("description_i18n"));
    m.insert("readme_text".into(), json!(p.str("readme_text")));
    m.insert("readme_i18n".into(), p.map_json("readme_i18n"));
    m.insert("warnings".into(), json!(p.str("warnings")));
    m.insert("warnings_i18n".into(), p.map_json("warnings_i18n"));
    m.insert("tags".into(), p.list_json("tags"));
    m.insert("categories".into(), json!(kiwi_core::mod_categories::tags_from_flags(flags)));
    m.insert("flags".into(), json!(flags));
    m.insert("author".into(), json!(author));
    m.insert("is_beta".into(), json!(p.bool("is_beta")));
    m.insert("is_stray".into(), json!(p.bool("is_stray")));
    m.insert("uploaded_on_behalf".into(), json!(p.bool("uploaded_on_behalf")));
    m.insert("banner_url".into(), img_url(u, p.opt_str("banner_sha")));
    m.insert("preview_urls".into(), Value::Array(previews));
    m.insert("download_count".into(), json!(p.int("download_count")));
    m.insert("downloads_7d".into(), json!(p.int("downloads_7d")));
    m.insert("star_count".into(), json!(p.int("star_count")));
    m.insert("popularity_score".into(), json!(p.float("popularity_score")));
    m.insert("discord_url".into(), p.opt_json("discord_url"));
    m.insert("website_url".into(), p.opt_json("website_url"));
    m.insert("donation_urls".into(), p.list_json("donation_urls"));
    m.insert("page_url".into(), json!(format!("{}/mods/{}/{}", u.site, p.str("owner_handle"), p.str("slug"))));
    m.insert("created_at".into(), p.iso("created_at"));
    m.insert("updated_at".into(), p.iso("updated_at"));
    m.insert("last_release_at".into(), p.iso("last_release_at"));
    lineage(p, &mut m);
    m
}

/// `public_release_dto(release)`.
pub fn public_release_dto(u: &Urls, r: &Doc) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("tag".into(), json!(r.str("tag")));
    m.insert("branch".into(), json!(r.str("branch")));
    m.insert("title".into(), json!(r.str("title")));
    m.insert("title_i18n".into(), r.map_json("title_i18n"));
    m.insert("changelog".into(), json!(r.str("changelog")));
    m.insert("changelog_i18n".into(), r.map_json("changelog_i18n"));
    m.insert("format".into(), json!(r.str("release_format")));
    m.insert("filename".into(), json!(download_filename(r)));
    m.insert("size".into(), json!(r.int("tmod_size")));
    m.insert("sha256".into(), json!(r.str("tmod_sha")));
    m.insert("download_count".into(), json!(r.int("download_count")));
    m.insert("download_url".into(), json!(format!("{}/v1/mods/hub/releases/{}/download", u.api, r.id.to_hex())));
    m.insert("published_at".into(), r.iso("published_at"));
    m
}

/// `_event_release(release)`.
pub fn event_release(u: &Urls, r: &Doc) -> Map<String, Value> {
    let props = r.map("tmod_properties");
    let prop = |k: &str| props.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str()).filter(|v| !v.is_empty());
    let mut m = public_release_dto(u, r);
    m.insert("id".into(), json!(r.id.to_hex()));
    m.insert("status".into(), json!(r.str("status")));
    m.insert("source_commit_sha".into(), r.opt_json("source_commit_sha"));
    m.insert("properties".into(), r.map_json("tmod_properties"));
    m.insert("mod_version".into(), prop("modVersion").map_or(Value::Null, |v| json!(v)));
    m.insert("mod_author".into(), prop("author").map_or(Value::Null, |v| json!(v)));
    m.insert("image_url".into(), img_url(u, r.opt_str("banner_sha")));
    m.insert("created_at".into(), r.iso("created_at"));
    m
}

/// `_event_change(release, previous, release_count)`.
pub fn event_change(r: &Doc, previous: Option<&Doc>, count: u64) -> Value {
    let days = match (previous, r.dt("published_at"), previous.and_then(|p| p.dt("published_at"))) {
        (Some(_), Some(a), Some(b)) => {
            let secs = (a - b).num_microseconds().map_or(0.0, |us| us as f64 / 1e6);
            json!(pyfloat::round(secs / 86400.0, 2))
        }
        _ => Value::Null,
    };
    json!({
        "kind": if previous.is_none() { "initial" } else { "update" },
        "is_first_release": previous.is_none(),
        "from_tag": previous.map_or(Value::Null, |p| json!(p.str("tag"))),
        "to_tag": r.str("tag"),
        "size_delta": previous.map_or(Value::Null, |p| json!(r.int("tmod_size") - p.int("tmod_size"))),
        "days_since_previous": days,
        "release_count": count,
        "artifact_unchanged": previous.is_some_and(|p| p.str("tmod_sha") == r.str("tmod_sha")),
    })
}

/// `_previous_published_release(release)`.
async fn previous(state: &AppState, r: &Doc) -> Result<Option<Doc>, ApiError> {
    let mut q = doc! {
        "project_id": r.oid("project_id"), "branch": r.str("branch"),
        "status": "published", "_id": {"$ne": r.id},
    };
    if let Some(at) = r.dt("published_at") {
        q.insert("published_at", doc! {"$lt": dt::to_bson(at)});
    }
    let db = state.inner.db.mongo.as_ref().ok_or_else(|| internal("no mongo"))?;
    let found = db
        .collection::<Document>(access::RELEASES)
        .find_one(q)
        .sort(doc! {"published_at": -1, "created_at": -1})
        .await
        .map_err(internal)?;
    match found {
        Some(d) => Ok(Some(Doc::load(super::model::RELEASE, &d).ok_or_else(|| internal("mod_releases: invalid"))?)),
        None => Ok(None),
    }
}

fn announced(r: &mut Doc) {
    r.set("announced_at", V::Dt(dt::now()));
}

/// `_emit_release_event(project, release)`.
pub async fn emit(state: &AppState, project: &Doc, r: &mut Doc) -> Result<(), ApiError> {
    if r.str("status") != "published" {
        return Ok(());
    }
    if project.str("visibility") != "public" || project.bool("taken_down") || project.bool("is_remix") {
        return Ok(());
    }
    if project.list("hidden_release_branches").iter().any(|b| b == r.str("branch")) {
        return Ok(());
    }
    if r.dt("announced_at").is_some() {
        return Ok(());
    }
    if r.bool("silent") {
        announced(r);
        return save(state, access::RELEASES, r).await;
    }
    if let Err(e) = announce(state, project, r).await {
        tracing::warn!("mods_hub: failed to emit release event: {e}");
    }
    Ok(())
}

/// `_event_project(project)`.
pub fn event_project(u: &Urls, project: &Doc) -> Map<String, Value> {
    let mut proj: Map<String, Value> =
        public_mod_dto(u, project).into_iter().filter(|(k, _)| !OMIT.contains(&k.as_str())).collect();
    proj.insert("owner".into(), json!(project.str("owner_username")));
    proj.insert("mode".into(), json!(project.str("mode")));
    proj.insert("default_branch".into(), json!(project.str("default_branch")));
    proj.insert(
        "api_url".into(),
        json!(format!("{}/v1/mods/{}/{}", u.api, project.str("owner_handle"), project.str("slug"))),
    );
    proj
}

/// The `data` of a `mod_release` event.
pub fn event_data(u: &Urls, project: &Doc, r: &Doc, prev: Option<&Doc>, count: u64) -> Value {
    let release_out = event_release(u, r);
    let image = match release_out.get("image_url") {
        Some(v) if !v.is_null() => v.clone(),
        _ => match img_url(u, project.opt_str("banner_sha")) {
            Value::Null => project
                .list("preview_shas")
                .iter()
                .map(|s| img_url(u, Some(s)))
                .find(|v| !v.is_null())
                .unwrap_or(Value::Null),
            v => v,
        },
    };
    let mut data = Map::new();
    data.insert("project".into(), Value::Object(event_project(u, project)));
    data.insert("release".into(), Value::Object(release_out));
    data.insert("previous".into(), prev.map_or(Value::Null, |p| Value::Object(event_release(u, p))));
    data.insert("change".into(), event_change(r, prev, count));
    data.insert("image_url".into(), image);
    data.insert("download_url".into(), json!(format!("{}/v1/mods/hub/releases/{}/download", u.api, r.id.to_hex())));
    data.insert(
        "page_url".into(),
        json!(format!("{}/mods/{}/{}", u.site, project.str("owner_handle"), project.str("slug"))),
    );
    Value::Object(data)
}

async fn announce(state: &AppState, project: &Doc, r: &mut Doc) -> Result<(), ApiError> {
    let prev = previous(state, r).await?;
    let db = state.inner.db.mongo.as_ref().ok_or_else(|| internal("no mongo"))?;
    let count = db
        .collection::<Document>(access::RELEASES)
        .count_documents(doc! {"project_id": r.oid("project_id"), "branch": r.str("branch"), "status": "published"})
        .await
        .map_err(internal)?;
    let data = event_data(&Urls::of(state), project, r, prev.as_ref(), count);
    crate::events::publish(state, "mod_release", &r.id.to_hex(), data).await;
    announced(r);
    save(state, access::RELEASES, r).await
}

pub fn opt_oid(v: Option<mongodb::bson::oid::ObjectId>) -> Bson {
    v.map_or(Bson::Null, Bson::ObjectId)
}
