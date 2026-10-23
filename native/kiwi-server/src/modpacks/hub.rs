//! The Mods Hub pieces a modpack resolves against (`mods_hub.service` and
//! `mods_hub.store`): mod and release lookups, the download name, the config
//! shipped beside a packed `.tmod`, banner images and the shared CAS.

use base64::Engine;
use futures::TryStreamExt;
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use serde_json::Value;

use super::{
    model::{self, Invalid, bdt},
    text,
};
use crate::{error::ApiError, media, state::AppState};

pub const MOD_PROJECTS: &str = "mod_projects";
pub const MOD_RELEASES: &str = "mod_releases";
pub const MOD_IMAGES: &str = "mod_images";
pub const STRAY_HANDLE: &str = "stray";
const MOD_VISIBILITY: &[&str] = &["draft", "unlisted", "public", "public_website_only", "hidden"];

pub fn internal(e: impl std::fmt::Display) -> ApiError {
    tracing::error!("modpacks: {e}");
    ApiError::internal()
}

/// Beanie's filter for several `find` expressions: one stays bare, more are `$and`.
pub fn and(parts: Vec<Document>) -> Document {
    if parts.len() == 1 {
        return parts.into_iter().next().unwrap_or_default();
    }
    doc! {"$and": parts.into_iter().map(Bson::Document).collect::<Vec<_>>()}
}

/// The `ModProject` fields a pack reads.
#[derive(Clone, Debug, PartialEq)]
pub struct ModRef {
    pub id: ObjectId,
    pub slug: String,
    pub title: String,
    pub owner_username: String,
    pub owner_handle: String,
    pub visibility: String,
    pub is_beta: bool,
    pub taken_down: bool,
}

impl ModRef {
    pub fn from_doc(d: &Document) -> Result<Self, Invalid> {
        let visibility = model::s_or(d, "visibility", "draft")?;
        if !MOD_VISIBILITY.contains(&visibility.as_str()) {
            return Err(Invalid(format!("visibility: {visibility:?}")));
        }
        Ok(ModRef {
            id: model::oid(d, "_id")?,
            slug: model::s(d, "slug")?,
            title: model::s(d, "title")?,
            owner_username: model::s_or(d, "owner_username", "")?,
            owner_handle: model::s_or(d, "owner_handle", "")?,
            visibility,
            is_beta: model::boolean(d, "is_beta", false)?,
            taken_down: model::boolean(d, "taken_down", false)?,
        })
    }

    /// Hidden from a pack: taken down or a draft.
    pub fn unavailable(&self) -> bool {
        self.taken_down || self.visibility == "draft"
    }
}

/// The `ModRelease` fields a pack reads.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub project_id: ObjectId,
    pub tag: String,
    pub branch: String,
    pub release_format: String,
    pub tmod_sha: String,
    pub tmod_filename: String,
    pub title_prop: Option<String>,
}

impl Release {
    pub fn from_doc(d: &Document) -> Result<Self, Invalid> {
        let release_format = model::s_or(d, "release_format", "tmod")?;
        if !matches!(release_format.as_str(), "tmod" | "zip") {
            return Err(Invalid(format!("release_format: {release_format:?}")));
        }
        let title_prop = match d.get("tmod_properties") {
            None => None,
            Some(Bson::Document(p)) => {
                for (k, v) in p {
                    if !matches!(v, Bson::String(_)) {
                        return Err(Invalid(format!("tmod_properties.{k}: {v:?}")));
                    }
                }
                p.get_str("title").ok().map(str::to_owned)
            }
            Some(v) => return Err(Invalid(format!("tmod_properties: {v:?}"))),
        };
        Ok(Release {
            project_id: model::oid(d, "project_id")?,
            tag: model::s(d, "tag")?,
            branch: model::s_or(d, "branch", "main")?,
            release_format,
            tmod_sha: model::s(d, "tmod_sha")?,
            tmod_filename: model::s_or(d, "tmod_filename", "mod.tmod")?,
            title_prop,
        })
    }
}

async fn find_one(state: &AppState, coll: &str, filter: Document) -> Result<Option<Document>, ApiError> {
    state.mongo().collection::<Document>(coll).find_one(filter).await.map_err(internal)
}

pub async fn mod_by_id(state: &AppState, id: ObjectId) -> Result<Option<ModRef>, ApiError> {
    match find_one(state, MOD_PROJECTS, doc! {"_id": id}).await? {
        Some(d) => Ok(Some(ModRef::from_doc(&d)?)),
        None => Ok(None),
    }
}

/// The site user a URL handle names (`username == handle.strip().lower()`).
pub async fn user_by_handle(state: &AppState, handle: &str) -> Result<Option<Document>, ApiError> {
    let name = text::lower(kiwi_core::pystr::strip(handle));
    find_one(state, crate::auth::models::SITE_USERS, doc! {"username": name}).await
}

/// `mods_hub.service.get_project`.
pub async fn get_project(state: &AppState, handle: &str, slug: &str) -> Result<Option<ModRef>, ApiError> {
    let filter = if text::lower(kiwi_core::pystr::strip(handle)) == STRAY_HANDLE {
        and(vec![doc! {"is_stray": true}, doc! {"slug": slug}])
    } else {
        let Some(user) = user_by_handle(state, handle).await? else { return Ok(None) };
        let uid = model::oid(&user, "_id")?;
        and(vec![doc! {"owner_id": uid}, doc! {"slug": slug}])
    };
    match find_one(state, MOD_PROJECTS, filter).await? {
        Some(d) => Ok(Some(ModRef::from_doc(&d)?)),
        None => Ok(None),
    }
}

/// A locked entry's release: that tag on the branch, any published format.
pub async fn release_locked(
    state: &AppState,
    project: ObjectId,
    branch: &str,
    tag: &str,
) -> Result<Option<Release>, ApiError> {
    let filter = and(vec![
        doc! {"project_id": project},
        doc! {"branch": branch},
        doc! {"tag": tag},
        doc! {"status": "published"},
    ]);
    match find_one(state, MOD_RELEASES, filter).await? {
        Some(d) => Ok(Some(Release::from_doc(&d)?)),
        None => Ok(None),
    }
}

/// An unlocked entry's release: the newest published `.tmod` on the branch.
pub async fn release_latest(state: &AppState, project: ObjectId, branch: &str) -> Result<Option<Release>, ApiError> {
    let filter = and(vec![
        doc! {"project_id": project},
        doc! {"branch": branch},
        doc! {"status": "published"},
        doc! {"release_format": "tmod"},
    ]);
    let docs: Vec<Document> = state
        .mongo()
        .collection::<Document>(MOD_RELEASES)
        .find(filter)
        .sort(doc! {"published_at": -1})
        .limit(1)
        .await
        .map_err(internal)?
        .try_collect()
        .await
        .map_err(internal)?;
    match docs.first() {
        Some(d) => Ok(Some(Release::from_doc(d)?)),
        None => Ok(None),
    }
}

/// `release_by_artifact_hash`: current or prior artifact hash.
pub async fn release_by_artifact_hash(state: &AppState, sha: &str) -> Result<Option<Release>, ApiError> {
    let filter = doc! {"$or": [{"tmod_sha": {"$in": [sha]}}, {"prior_tmod_shas": {"$in": [sha]}}]};
    match find_one(state, MOD_RELEASES, filter).await? {
        Some(d) => Ok(Some(Release::from_doc(&d)?)),
        None => Ok(None),
    }
}

/// `release_download_filename`: a `.tmod` is named after its header title.
pub fn release_download_filename(r: &Release) -> String {
    if r.release_format == "zip" {
        return r.tmod_filename.clone();
    }
    let title = kiwi_core::pystr::strip(r.title_prop.as_deref().unwrap_or(""));
    if title.is_empty() { r.tmod_filename.clone() } else { format!("{}.tmod", text::safe_filename(title, "mod")) }
}

// --- the CAS ---------------------------------------------------------------

/// A key the store could have written (64 lowercase hex). Anything else is
/// treated as absent, so a client-sent sha can't name a file outside the store.
fn is_sha(sha: &str) -> bool {
    sha.len() == 64 && sha.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

pub async fn has_blob(state: &AppState, sha: &str) -> Result<bool, ApiError> {
    if !is_sha(sha) {
        return Ok(false);
    }
    let store = media::mods_store(state);
    let sha = sha.to_owned();
    tokio::task::spawn_blocking(move || store.has(&sha)).await.map_err(internal)
}

pub async fn get_blob(state: &AppState, sha: &str) -> Result<Option<Vec<u8>>, ApiError> {
    if !is_sha(sha) {
        return Ok(None);
    }
    media::read_blob(media::mods_store(state), sha.to_owned()).await
}

pub async fn put_blob(state: &AppState, data: Vec<u8>) -> Result<String, ApiError> {
    let store = media::mods_store(state);
    let (sha, _) = tokio::task::spawn_blocking(move || store.put(&data)).await.map_err(internal)?.map_err(internal)?;
    Ok(sha)
}

// --- banner images ---------------------------------------------------------

pub async fn image_exists(state: &AppState, sha: &str) -> Result<bool, ApiError> {
    Ok(find_one(state, MOD_IMAGES, doc! {"sha": sha}).await?.is_some())
}

/// `store_image`: the stored (or already known) image's sha.
pub async fn store_image(state: &AppState, owner: ObjectId, data: Vec<u8>) -> Result<String, ApiError> {
    let max = state.settings().mods_image_max_bytes;
    if data.len() as i64 > max {
        return Err(ApiError::new(
            axum::http::StatusCode::PAYLOAD_TOO_LARGE,
            "bad_request",
            format!("Image exceeds the {max}-byte limit."),
        ));
    }
    let Some((content_type, w, h)) = crate::images::service::sniff_image(&data) else {
        return Err(ApiError::bad_request("Unsupported image - use PNG, JPEG, WebP or GIF."));
    };
    let size = data.len() as i64;
    let sha = put_blob(state, data).await?;
    if find_one(state, MOD_IMAGES, doc! {"sha": &sha}).await?.is_some() {
        return Ok(sha);
    }
    let dim = |v: Option<u32>| v.map_or(Bson::Null, |n| crate::auth::pyutil::bson_int(i64::from(n)));
    let asset = doc! {
        "_id": ObjectId::new(),
        "sha": &sha,
        "content_type": content_type,
        "byte_size": crate::auth::pyutil::bson_int(size),
        "owner_id": owner,
        "width": dim(w),
        "height": dim(h),
        "variants": {},
        "created_at": bdt(crate::dt::now()),
    };
    state.mongo().collection::<Document>(MOD_IMAGES).insert_one(asset).await.map_err(internal)?;
    Ok(sha)
}

// --- the packed .tmod ------------------------------------------------------

/// What a pack reads of a `.tmod`: properties as `dict(pairs)` keeps them.
pub struct Header {
    pub properties: Vec<(String, String)>,
    pub files: Vec<(String, Option<Vec<u8>>)>,
}

impl Header {
    pub fn prop(&self, key: &str) -> Option<&str> {
        self.properties.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
}

/// `tmod.read_tmod(data, metadata_only)`; `None` where it raises.
pub fn read_tmod(data: &[u8], metadata_only: bool) -> Option<Header> {
    let v = crate::tools::tmod::read(data, metadata_only).ok()?;
    let mut properties = Vec::new();
    if let Some(Value::Object(p)) = v.get("properties") {
        for (k, val) in p {
            properties.push((k.clone(), val.as_str().unwrap_or_default().to_owned()));
        }
    }
    let mut files = Vec::new();
    for f in v.get("files").and_then(Value::as_array).into_iter().flatten() {
        let path = f.get("path").and_then(Value::as_str).unwrap_or_default().to_owned();
        let content = f
            .get("content_base64")
            .and_then(Value::as_str)
            .map(|b| base64::engine::general_purpose::STANDARD.decode(b).unwrap_or_default());
        files.push((path, content));
    }
    Some(Header { properties, files })
}

/// `_declared_config_path`.
fn declared_config_path(h: &Header) -> String {
    let declared = h.properties.iter().find(|(k, _)| text::lower(k) == "configpath").map_or("", |(_, v)| v.as_str());
    let norm = declared.replace('\\', "/");
    text::lower(kiwi_core::pystr::strip(norm.trim_start_matches('/')))
}

/// `_generated_config`: one empty `[name.swf]` section per packed `.swf`.
fn generated_config(swfs: &[&str]) -> Vec<u8> {
    let mut names: Vec<String> =
        swfs.iter().map(|p| text::lower(p.replace('\\', "/").rsplit('/').next().unwrap_or(""))).collect();
    names.sort();
    names.dedup();
    names.iter().map(|n| format!("[{n}]\n")).collect::<String>().into_bytes()
}

/// `pack_config_for_artifact`: `<title>.cfg` and its bytes, or `None` when the
/// build has nothing that reads a config.
pub fn pack_config_for_artifact(data: &[u8]) -> Option<(String, Vec<u8>)> {
    let h = read_tmod(data, false)?;
    let title = kiwi_core::pystr::strip(h.prop("title").unwrap_or("")).to_owned();
    if title.is_empty() {
        return None;
    }
    let name = format!("{}.cfg", text::safe_filename(&title, "mod"));
    let declared = declared_config_path(&h);
    let lowered: Vec<String> = h.files.iter().map(|(p, _)| text::lower(p)).collect();
    let cfgs: Vec<usize> = (0..h.files.len()).filter(|&i| lowered[i].ends_with(".cfg")).collect();
    let chosen =
        cfgs.iter().copied().find(|&i| lowered[i] == declared).or(if cfgs.len() == 1 { Some(cfgs[0]) } else { None });
    if let Some(i) = chosen
        && let Some(content) = &h.files[i].1
    {
        return Some((name, content.clone()));
    }
    let swfs: Vec<&str> =
        (0..h.files.len()).filter(|&i| lowered[i].ends_with(".swf")).map(|i| h.files[i].0.as_str()).collect();
    if swfs.is_empty() {
        return None;
    }
    Some((name, generated_config(&swfs)))
}
