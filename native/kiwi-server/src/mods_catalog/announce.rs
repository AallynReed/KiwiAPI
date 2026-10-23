//! `_announce_newly_visible` / `_emit_release_event`: a published build is
//! news exactly once, gated by `announced_at`, as a `mod_release` event.

use mongodb::bson::{Bson, doc};
use serde_json::{Map, Value, json};

use super::{
    dto::{Urls, iso, public_img_url, public_mod_dto, public_release_dto},
    model::{self, Project, Release},
    service::{coll, internal, releases, save_doc},
};
use crate::{auth::pyutil::round2, dt, error::ApiError, events::bus, state::AppState};

const EVENT_PROJECT_OMIT: [&str; 4] = ["description", "description_i18n", "readme_text", "readme_i18n"];

fn blocked(p: &Project) -> bool {
    p.visibility != "public" || p.taken_down || p.is_remix
}

async fn stamp(state: &AppState, r: &mut Release) -> Result<(), ApiError> {
    r.announced_at = Some(dt::now());
    let stored = save_doc(state, model::RELEASES, r.id, r.to_doc()).await?;
    *r = Release::from_doc(&stored);
    Ok(())
}

/// `_event_project(project)`.
pub fn event_project(u: &Urls, p: &Project) -> Value {
    let mut m: Map<String, Value> =
        public_mod_dto(u, p, None).into_iter().filter(|(k, _)| !EVENT_PROJECT_OMIT.contains(&k.as_str())).collect();
    m.insert("owner".into(), p.owner_username.clone().into());
    m.insert("mode".into(), p.mode.clone().into());
    m.insert("default_branch".into(), p.default_branch.clone().into());
    m.insert("api_url".into(), format!("{}/v1/mods/{}/{}", u.api, p.owner_handle, p.slug).into());
    Value::Object(m)
}

/// `_event_release(release)`.
pub fn event_release(u: &Urls, r: &Release) -> Value {
    let mut m = public_release_dto(u, r);
    let prop =
        |k: &str| r.tmod_properties.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).filter(|v| !v.is_empty());
    m.insert("id".into(), r.id.to_hex().into());
    m.insert("status".into(), r.status.clone().into());
    m.insert("source_commit_sha".into(), json!(r.source_commit_sha));
    m.insert("properties".into(), model::map_json(&r.tmod_properties));
    m.insert("mod_version".into(), json!(prop("modVersion")));
    m.insert("mod_author".into(), json!(prop("author")));
    m.insert("image_url".into(), json!(public_img_url(u, r.banner_sha.as_deref())));
    m.insert("created_at".into(), iso(&r.created_at));
    Value::Object(m)
}

/// `_event_change(release, previous, release_count)`.
pub fn event_change(r: &Release, previous: Option<&Release>, release_count: u64) -> Value {
    let size_delta = previous.map(|p| r.tmod_size - p.tmod_size);
    let days = match (previous, r.published_at, previous.and_then(|p| p.published_at)) {
        (Some(_), Some(a), Some(b)) => {
            let secs = (a - b).num_microseconds().map_or(0.0, |us| us as f64 / 1e6);
            Some(round2(secs / 86400.0))
        }
        _ => None,
    };
    json!({
        "kind": if previous.is_none() { "initial" } else { "update" },
        "is_first_release": previous.is_none(),
        "from_tag": previous.map(|p| p.tag.clone()),
        "to_tag": r.tag,
        "size_delta": size_delta,
        "days_since_previous": days,
        "release_count": release_count,
        "artifact_unchanged": previous.is_some_and(|p| p.tmod_sha == r.tmod_sha),
    })
}

async fn previous_published(state: &AppState, r: &Release) -> Result<Option<Release>, ApiError> {
    let mut q = doc! {"project_id": r.project_id, "branch": &r.branch, "status": "published", "_id": {"$ne": r.id}};
    if let Some(p) = r.published_at {
        q.insert("published_at", doc! {"$lt": model::date(&p)});
    }
    let found = super::service::find_docs(
        &coll(state, model::RELEASES),
        q,
        Some(doc! {"published_at": -1, "created_at": -1}),
        None,
        Some(1),
    )
    .await?;
    Ok(found.first().map(Release::from_doc))
}

/// `_emit_release_event(project, release)`.
pub async fn emit_release_event(state: &AppState, p: &Project, r: &mut Release) -> Result<(), ApiError> {
    if r.status != "published"
        || blocked(p)
        || p.hidden_release_branches.contains(&r.branch)
        || r.announced_at.is_some()
    {
        return Ok(());
    }
    if r.silent {
        return stamp(state, r).await;
    }
    let built: Result<Value, ApiError> = async {
        let u = Urls::of(state.settings());
        let previous = previous_published(state, r).await?;
        let count = coll(state, model::RELEASES)
            .count_documents(doc! {"project_id": r.project_id, "branch": &r.branch, "status": "published"})
            .await
            .map_err(internal)?;
        let release_out = event_release(&u, r);
        let image = release_out["image_url"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| public_img_url(&u, p.banner_sha.as_deref()))
            .or_else(|| p.preview_shas.iter().find_map(|s| public_img_url(&u, Some(s))));
        Ok(json!({
            "project": event_project(&u, p),
            "release": release_out,
            "previous": previous.as_ref().map(|pr| event_release(&u, pr)),
            "change": event_change(r, previous.as_ref(), count),
            "image_url": image,
            "download_url": format!("{}/v1/mods/hub/releases/{}/download", u.api, r.id.to_hex()),
            "page_url": format!("{}/mods/{}/{}", u.app, p.owner_handle, p.slug),
        }))
    }
    .await;
    match built {
        Ok(data) => {
            bus::publish(state, "mod_release", &r.id.to_hex(), data).await;
            if let Err(e) = stamp(state, r).await {
                tracing::warn!("mods_hub: failed to emit release event: {e}");
            }
        }
        Err(e) => tracing::warn!("mods_hub: failed to emit release event: {e}"),
    }
    Ok(())
}

/// `_announce_newly_visible(project, branches)`.
pub async fn announce_newly_visible(
    state: &AppState,
    p: &Project,
    branches: Option<&[String]>,
) -> Result<(), ApiError> {
    if blocked(p) {
        return Ok(());
    }
    let mut q = doc! {"project_id": p.id, "status": "published", "announced_at": Bson::Null, "silent": {"$ne": true}};
    if let Some(b) = branches {
        q.insert("branch", doc! {"$in": b.to_vec()});
    } else if !p.hidden_release_branches.is_empty() {
        q.insert("branch", doc! {"$nin": p.hidden_release_branches.clone()});
    }
    let mut pending = releases(state, q, doc! {"published_at": -1, "created_at": -1}).await?;
    if pending.is_empty() {
        return Ok(());
    }
    let mut rest = pending.split_off(1);
    emit_release_event(state, p, &mut pending[0]).await?;
    for old in rest.iter_mut() {
        stamp(state, old).await?;
    }
    Ok(())
}
