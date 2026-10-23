//! The release writes of `service.py`: cut from a commit or an upload,
//! attach a config to an existing build, publish/retitle, delete.

use std::borrow::Cow;

use axum::{
    body::Body,
    extract::{FromRequestParts, Request, State},
    http::{StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
};
use kiwi_core::{
    form::{self, FormError, FormValue},
    pystr::strip,
};
use mongodb::bson::{Bson, Document, doc};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::{
    access::{self, PROJECTS, RELEASES, can_edit, internal},
    artifact::{self, lower, safe_filename},
    event::{self, save},
    git,
    model::{Doc, RELEASE, V},
    read::{blob, release_dto},
    writeauth::{WriteUser, write_user},
};
use crate::{
    auth::models::SiteUser,
    dt,
    error::{ApiError, ErrorCode},
    features::{self, ModsHub},
    media,
    pyjson::PydanticJson,
    ratelimit::RateLimitInfo,
    state::AppState,
    tools::{pybytes::b64decode_strict, tmod as tools_tmod},
    validation::{
        BodyInput, Field, FromBody, FromParams, Input, Params, Path, ValidationError, Validator, read_body, validate,
    },
};

const CONFIG_MAX_BYTES: usize = 256 * 1024;
const CONTENT_LANGS: [&str; 9] = ["fr", "de", "pt-PT", "es", "ru", "ja", "ko", "zh-CN", "th"];

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError::bad_request(msg)
}

fn forbidden_edit() -> ApiError {
    ApiError::forbidden("You don't have edit access to this mod project.")
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f).await.map_err(internal)
}

async fn put_blob(state: &AppState, data: Vec<u8>) -> Result<String, ApiError> {
    let store = media::mods_store(state);
    blocking(move || store.put(&data)).await?.map(|(sha, _)| sha).map_err(internal)
}

fn require_publish_ok(actor: &SiteUser, status: &str) -> Result<(), ApiError> {
    if status == "published" && !actor.is_verified {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            ErrorCode::EmailUnverified,
            "Verify your account before publishing a release.",
        ));
    }
    Ok(())
}

/// `_clean_author(author, fallback)`.
pub(crate) fn clean_author(author: Option<&str>, fallback: &str) -> String {
    let Some(author) = author.filter(|a| !strip(a).is_empty()) else { return fallback.to_owned() };
    let names: Vec<&str> = author.split(',').map(strip).filter(|n| !n.is_empty()).collect();
    let joined = kiwi_core::pystr::prefix(&names.join(", "), 200).to_owned();
    if joined.is_empty() { fallback.to_owned() } else { joined }
}

pub(crate) type Props = Vec<(String, String)>;
pub(crate) type Files = Vec<(String, Vec<u8>)>;

fn set_prop(props: &mut Props, k: &str, v: String) {
    match props.iter_mut().find(|(n, _)| n == k) {
        Some(slot) => slot.1 = v,
        None => props.push((k.to_owned(), v)),
    }
}

/// `_release_properties(project, tag, changelog, author)`.
pub(crate) fn release_properties(project: &Doc, tag: &str, changelog: &str, author: Option<&str>) -> Props {
    let mut props = vec![
        ("title".to_owned(), project.str("title").to_owned()),
        ("author".to_owned(), clean_author(author, project.str("owner_username"))),
        ("modVersion".to_owned(), tag.to_owned()),
    ];
    if !strip(changelog).is_empty() {
        props.push(("notes".into(), strip(changelog).to_owned()));
    }
    let tags = project.list("tags");
    if !tags.is_empty() {
        props.push(("tags".into(), tags.join(",")));
        let flags = kiwi_core::mod_categories::flags_from_tags(tags.iter().map(String::as_str));
        if flags != 0 {
            props.push(("flags".into(), flags.to_string()));
        }
    }
    props
}

fn norm_lower(p: &str) -> String {
    lower(p.replace('\\', "/").trim_start_matches('/'))
}

/// `pack_preview(files, props, data, ext, stem)`.
pub(crate) fn pack_preview(files: &mut Files, props: &mut Props, data: Vec<u8>, ext: &str, stem: &str) {
    let path = lower(&format!("ui/{}.{ext}", safe_filename(stem)));
    files.retain(|(p, _)| norm_lower(p) != path);
    files.push((path.clone(), data));
    set_prop(props, "previewPath", path);
}

/// `_inject_preview(project, preview_sha, files, props)`.
async fn inject_preview(
    state: &AppState,
    project: &Doc,
    sha: &str,
    files: &mut Files,
    props: &mut Props,
) -> Result<(), ApiError> {
    if !project.list("preview_shas").iter().any(|s| s == sha) {
        return Err(bad("Pick a preview from this mod's images."));
    }
    let asset = access::find_one(state, super::images::COLLECTION, doc! {"sha": sha}).await?;
    let ct = match asset.as_ref().map(|a| a.get("content_type")) {
        None => None,
        Some(Some(Bson::String(ct))) => Some(ct.clone()),
        Some(_) => return Err(internal("mod_images: invalid document")),
    };
    let got = match ct {
        Some(ct) => blob(state, sha).await?.map(|d| (d, ct)),
        None => None,
    };
    let (data, ct) = got.ok_or_else(|| bad("That preview image is missing."))?;
    let ext = match ct.as_str() {
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        _ => return Err(bad("The .tmod preview must be a PNG, JPEG, or GIF image (Trove can't render WebP).")),
    };
    pack_preview(files, props, data, ext, project.str("slug"));
    Ok(())
}

/// `_inject_config(files, props, data)`.
pub(crate) fn inject_config(files: &mut Files, props: &mut Props, data: Vec<u8>) -> Result<(), ApiError> {
    if data.is_empty() {
        return Err(bad("The config file is empty."));
    }
    if data.len() > CONFIG_MAX_BYTES {
        return Err(bad(format!("A config file can be at most {} KB.", CONFIG_MAX_BYTES / 1024)));
    }
    if data.contains(&0) {
        return Err(bad("That doesn't look like a text config file."));
    }
    if !files.iter().any(|(p, _)| lower(p).ends_with(".swf")) {
        return Err(bad("Only a mod with a Flash UI (.swf) can carry a config file."));
    }
    let title = props.iter().find(|(k, _)| k == "title").map_or("", |(_, v)| strip(v));
    if title.is_empty() {
        return Err(bad("This build has no title in its header to name the config after."));
    }
    let path = format!("ui/{}.cfg", safe_filename(title));
    let want = lower(&path);
    files.retain(|(p, _)| norm_lower(p) != want);
    files.push((path.clone(), data));
    set_prop(props, "configPath", path);
    Ok(())
}

/// `_repack_with_config(data, config)`: `(artifact, header properties)`.
pub(crate) fn repack_with_config(data: &[u8], config: Vec<u8>) -> Result<(Vec<u8>, Props), ApiError> {
    let parsed = artifact::read(data, true).map_err(|e| bad(format!("Not a valid .tmod file: {e}")))?;
    let mut props = parsed.props;
    let mut files: Files = parsed.files.into_iter().map(|f| (f.path, f.data)).collect();
    inject_config(&mut files, &mut props, config)?;
    let version = if parsed.version == 0 { 1 } else { i64::from(parsed.version) };
    let loader = props
        .iter()
        .find(|(k, _)| k == "modLoader")
        .map(|(_, v)| v.clone())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| tools_tmod::KIWI_MOD_LOADER.to_owned());
    let built = tools_tmod::build(version, &props, &files, &loader, false).map_err(|e| internal(format!("{e:?}")))?;
    let final_props = artifact::read(&built, false).map_err(internal)?.props;
    Ok((built, final_props))
}

fn hash_match(shas: &[&str]) -> Document {
    let want: Vec<Bson> = shas.iter().filter(|s| !s.is_empty()).map(|s| Bson::String((*s).to_owned())).collect();
    doc! {"$or": [{"tmod_sha": {"$in": want.clone()}}, {"prior_tmod_shas": {"$in": want}}]}
}

/// `_ensure_hash_unowned(project, *shas)`.
async fn ensure_hash_unowned(state: &AppState, project: &Doc, shas: &[&str]) -> Result<(), ApiError> {
    let owner = event::opt_oid(project.oid("owner_id"));
    let filter = doc! {"$and": [hash_match(shas), {"owner_id": {"$nin": [Bson::Null, owner]}}]};
    if access::find_one(state, RELEASES, filter).await?.is_some() {
        return Err(ApiError::conflict(
            "This exact mod is already published by another creator, so it can't be uploaded here. If it's genuinely yours, contact support.",
        ));
    }
    Ok(())
}

/// `_ensure_hash_globally_unique(project, *shas)`.
async fn ensure_hash_globally_unique(state: &AppState, project: &Doc, shas: &[&str]) -> Result<(), ApiError> {
    let filter = doc! {"$and": [hash_match(shas), {"project_id": {"$ne": project.id}}]};
    if access::find_one(state, RELEASES, filter).await?.is_some() {
        return Err(ApiError::conflict(
            "This mod is already on the hub - the exact same build has been uploaded before, so it can't be shared again here.",
        ));
    }
    Ok(())
}

async fn ensure_hash(state: &AppState, project: &Doc, shas: &[&str]) -> Result<(), ApiError> {
    if project.bool("uploaded_on_behalf") {
        ensure_hash_globally_unique(state, project, shas).await
    } else {
        ensure_hash_unowned(state, project, shas).await
    }
}

/// `_check_release_tag(project, branch, tag)`.
async fn check_tag(state: &AppState, project: &Doc, branch: &str, tag: &str) -> Result<(), ApiError> {
    let filter = doc! {"project_id": project.id, "branch": branch, "tag": tag};
    if access::find_one(state, RELEASES, filter).await?.is_some() {
        return Err(ApiError::conflict(format!("Release '{tag}' already exists on branch '{branch}'.")));
    }
    Ok(())
}

struct NewRelease {
    tag: String,
    branch: String,
    title: String,
    changelog: String,
    status: String,
    sha: String,
    size: usize,
    props: Props,
    source_commit_sha: Option<String>,
    format: &'static str,
    prior: Vec<String>,
    silent: bool,
}

/// `_insert_release(...)`.
async fn insert_release(state: &AppState, project: &mut Doc, n: NewRelease) -> Result<Value, ApiError> {
    let filename = if n.format == "zip" {
        format!("{}-{}.zip", project.str("slug"), n.tag)
    } else {
        let title = n.props.iter().find(|(k, _)| k == "title").map_or("", |(_, v)| strip(v));
        if title.is_empty() {
            format!("{}-{}.tmod", project.str("slug"), n.tag)
        } else {
            format!("{}.tmod", safe_filename(title))
        }
    };
    let published = n.status == "published";
    let published_at = published.then(dt::now);
    let mut release = Doc::build(
        RELEASE,
        vec![
            ("project_id", V::Oid(project.id)),
            ("owner_id", project.oid("owner_id").map_or(V::Null, V::Oid)),
            ("tag", V::Str(n.tag)),
            ("branch", V::Str(n.branch)),
            ("title", V::Str(strip(&n.title).to_owned())),
            ("changelog", V::Str(n.changelog)),
            ("source_commit_sha", n.source_commit_sha.map_or(V::Null, V::Str)),
            ("release_format", V::Str(n.format.to_owned())),
            ("tmod_sha", V::Str(n.sha)),
            ("prior_tmod_shas", V::List(n.prior)),
            ("tmod_size", V::Int(n.size as i64)),
            ("tmod_filename", V::Str(filename)),
            ("tmod_properties", V::Map(n.props)),
            ("banner_sha", project.opt_str("banner_sha").map_or(V::Null, |s| V::Str(s.to_owned()))),
            ("status", V::Str(n.status)),
            ("published_at", published_at.map_or(V::Null, V::Dt)),
            ("silent", V::Bool(n.silent)),
        ],
    );
    let db = state.inner.db.mongo.as_ref().ok_or_else(|| internal("no mongo"))?;
    db.collection::<Document>(RELEASES).insert_one(release.insert_doc()).await.map_err(internal)?;
    project.set("updated_at", V::Dt(dt::now()));
    if published && !n.silent {
        project.set("last_release_at", V::Dt(published_at.unwrap_or_else(dt::now)));
    }
    save(state, PROJECTS, project).await?;
    event::emit(state, project, &mut release).await?;
    Ok(Value::Object(release_dto(&release)))
}

// --- request shapes --------------------------------------------------------

struct Hs {
    handle: String,
    slug: String,
}

impl FromParams for Hs {
    fn from_params(p: &mut Params<'_>) -> Self {
        Hs { handle: p.path("handle").str().required(), slug: p.path("slug").str().required() }
    }
}

struct Rid(String);

impl FromParams for Rid {
    fn from_params(p: &mut Params<'_>) -> Self {
        Rid(p.path("release_id").str().required())
    }
}

type I18n = Option<Vec<(String, String)>>;

#[derive(Default)]
struct CreateReq {
    tag: String,
    title: String,
    changelog: String,
    reference: String,
    format: String,
    status: String,
    author: Option<String>,
    preview_sha: Option<String>,
    config_base64: Option<String>,
    silent: bool,
}

impl FromBody for CreateReq {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            let tag = m.field("tag").str().min_len(1).max_len(60).required();
            let title = m.field("title").str().max_len(160).default(String::new());
            let _: I18n = m.field("title_i18n").dict(|f| f.str().required()).optional();
            let changelog = m.field("changelog").str().max_len(20_000).default(String::new());
            let _: I18n = m.field("changelog_i18n").dict(|f| f.str().required()).optional();
            let reference = m.field("ref").str().default(String::new());
            let format = m.field("format").literal(&["tmod", "zip"]).default("tmod".into());
            let status = m.field("status").literal(&["draft", "published"]).default("published".into());
            let author = m.field("author").str().max_len(200).optional();
            let preview_sha = m.field("preview_sha").str().optional();
            let config_base64 = m.field("config_base64").str().optional();
            let silent = m.field("silent").bool().default(false);
            Some(CreateReq {
                tag,
                title,
                changelog,
                reference,
                format,
                status,
                author,
                preview_sha,
                config_base64,
                silent,
            })
        })
        .required()
    }
}

#[derive(Default)]
struct UpdateReq {
    title: Option<String>,
    title_i18n: I18n,
    changelog: Option<String>,
    changelog_i18n: I18n,
    status: Option<String>,
    silent: Option<bool>,
}

impl FromBody for UpdateReq {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(UpdateReq {
                title: m.field("title").str().max_len(160).optional(),
                title_i18n: m.field("title_i18n").dict(|f| f.str().required()).optional(),
                changelog: m.field("changelog").str().max_len(20_000).optional(),
                changelog_i18n: m.field("changelog_i18n").dict(|f| f.str().required()).optional(),
                status: m.field("status").literal(&["draft", "published"]).optional(),
                silent: m.field("silent").bool().optional(),
            })
        })
        .required()
    }
}

// --- shared route plumbing -------------------------------------------------

/// The dependencies of a write route, in FastAPI's order: the Mods Hub gate,
/// then `get_mod_write_user`.
async fn deps(state: &AppState, parts: &Parts) -> Result<WriteUser, Response> {
    features::check::<ModsHub>(state).await.map_err(IntoResponse::into_response)?;
    write_user(state, parts).await.map_err(IntoResponse::into_response)
}

fn reply(status: StatusCode, rl: Option<RateLimitInfo>, r: Result<Value, ApiError>) -> Response {
    match r {
        Ok(v) => (status, rl, PydanticJson(v)).into_response(),
        Err(e) => e.into_response(),
    }
}

/// `_require_owned(handle, slug, user)`.
async fn require_owned(state: &AppState, hs: &Hs, user: &SiteUser) -> Result<Doc, ApiError> {
    let project = access::get_project(state, &hs.handle, &hs.slug).await?;
    project.filter(|p| can_edit(p, Some(user))).ok_or_else(|| ApiError::not_found("Mod project not found"))
}

fn parse_error(msg: &str) -> Response {
    ApiError::http(StatusCode::BAD_REQUEST, Some(msg)).into_response()
}

async fn read_form(parts: &Parts, body: Body) -> Result<Vec<(String, FormValue)>, Response> {
    let bytes =
        axum::body::to_bytes(body, usize::MAX).await.map_err(|_| parse_error("There was an error parsing the body"))?;
    let ct = parts.headers.get(header::CONTENT_TYPE).map(|v| v.as_bytes().to_vec());
    match blocking(move || form::parse_form(ct.as_deref(), &bytes)).await {
        Ok(Ok(items)) => Ok(items),
        Ok(Err(FormError::Http(msg))) => Err(parse_error(&msg)),
        Err(e) => Err(e.into_response()),
    }
}

/// A `str = Form(default)` field (`None` default = required).
fn form_str(v: &mut Validator, items: &[(String, FormValue)], name: &str, default: Option<&str>) -> String {
    let loc = vec!["body".into(), name.into()];
    match items.iter().rev().find(|(k, _)| k == name).map(|(_, v)| v) {
        Some(FormValue::Text(t)) if !t.is_empty() => t.clone(),
        Some(FormValue::File(f)) => {
            v.push(loc, crate::routes::misc::feedback::upload_json(f), crate::validation::Fail::string_type());
            String::new()
        }
        _ => match default {
            Some(d) => d.to_owned(),
            None => {
                v.push(loc, Value::Null, crate::validation::Fail::missing());
                String::new()
            }
        },
    }
}

fn form_bool(v: &mut Validator, items: &[(String, FormValue)], name: &str) -> bool {
    match items.iter().rev().find(|(k, _)| k == name).map(|(_, v)| v) {
        Some(FormValue::Text(t)) if !t.is_empty() => {
            Field::new(v, vec!["body".into(), name.into()], Input::Str(Cow::Owned(t.clone()))).bool().default(false)
        }
        Some(FormValue::File(f)) => {
            let input = crate::routes::misc::feedback::upload_json(f);
            v.push(vec!["body".into(), name.into()], input, crate::validation::Fail::bool_type());
            false
        }
        _ => false,
    }
}

/// A `UploadFile = File(...)` (`required`) or `UploadFile | None = File(None)`.
fn form_file(v: &mut Validator, items: &[(String, FormValue)], name: &str, required: bool) -> Option<form::UploadFile> {
    let loc = || vec!["body".into(), name.into()];
    match items.iter().rev().find(|(k, _)| k == name).map(|(_, v)| v) {
        Some(FormValue::File(f)) => Some(f.clone()),
        Some(FormValue::Text(t)) if !t.is_empty() => {
            v.push(
                loc(),
                Value::String(t.clone()),
                crate::validation::Fail::value_error("Expected UploadFile, received: <class 'str'>"),
            );
            None
        }
        _ => {
            if required {
                v.push(loc(), Value::Null, crate::validation::Fail::missing());
            }
            None
        }
    }
}

// --- create from a commit --------------------------------------------------

/// `create_release_from_commit(...)`.
async fn create_from_commit(
    state: &AppState,
    project: &mut Doc,
    actor: &SiteUser,
    req: CreateReq,
    config: Option<Vec<u8>>,
) -> Result<Value, ApiError> {
    if project.str("mode") != "files" {
        return Err(bad("This mod is in releases-only mode - it has no file versioning."));
    }
    require_publish_ok(actor, &req.status)?;
    let tag = strip(&req.tag).to_owned();
    let dir = git::repo_dir(&state.settings().mods_store_dir, &project.id.to_hex());
    let target =
        if req.reference.is_empty() { project.str("default_branch").to_owned() } else { req.reference.clone() };
    let (d, t) = (dir.clone(), target.clone());
    let commit = blocking(move || git::resolve(&d, &t)).await?.map_err(internal)?;
    let commit =
        commit.ok_or_else(|| ApiError::not_found(format!("No branch or commit '{target}', or no commits yet.")))?;
    let d = dir.clone();
    let names = blocking(move || git::branches(&d)).await?.map_err(internal)?;
    let branch = if !req.reference.is_empty() && names.iter().any(|(n, _)| n.as_slice() == req.reference.as_bytes()) {
        req.reference.clone()
    } else {
        project.str("default_branch").to_owned()
    };
    check_tag(state, project, &branch, &tag).await?;
    let (d, c) = (dir.clone(), commit.clone());
    let entries = blocking(move || git::tree(&d, &c)).await?.map_err(internal)?;
    if entries.is_empty() {
        return Err(bad("That commit has no files to compile."));
    }
    if entries.iter().any(|e| !e.is_blob) {
        return Err(internal("tree entry is not a blob"));
    }
    let compilable: Vec<&git::Entry> = entries.iter().filter(|e| artifact::is_compilable(&e.path)).collect();
    if compilable.is_empty() {
        return Err(bad(
            "No files inside a Trove folder to compile. Root files and non-Trove folders (like 'bin') are ignored - put files under blueprints/, ui/, prefabs/, etc.",
        ));
    }
    let shas: Vec<String> = compilable.iter().filter(|e| e.utf8).map(|e| e.sha.clone()).collect();
    let d = dir.clone();
    let mut contents = blocking(move || git::blobs(&d, &shas)).await?.map_err(internal)?;
    let mut files: Files = Vec::with_capacity(compilable.len());
    for e in &compilable {
        let data = if e.utf8 { contents.get(&e.sha).cloned() } else { None };
        let data = data.ok_or_else(|| {
            ApiError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorCode::InternalError,
                format!("Missing blob for {}", e.path),
            )
        })?;
        files.push((e.path.clone(), data));
    }
    contents.clear();
    let (artifact, props, format): (Vec<u8>, Props, &'static str) = if req.format == "zip" {
        if config.is_some() {
            return Err(bad("A config file can only be packed into a .tmod build."));
        }
        (blocking(move || artifact::build_zip(&files)).await?.ok_or_else(|| internal("zip"))?, Vec::new(), "zip")
    } else {
        let mut props = release_properties(project, &tag, &req.changelog, req.author.as_deref());
        if let Some(sha) = req.preview_sha.as_deref().filter(|s| !s.is_empty()) {
            inject_preview(state, project, sha, &mut files, &mut props).await?;
        }
        if let Some(cfg) = config {
            inject_config(&mut files, &mut props, cfg)?;
        }
        let p = props.clone();
        let built = blocking(move || tools_tmod::build(1, &p, &files, tools_tmod::KIWI_MOD_LOADER, true)).await?;
        (built.map_err(|e| internal(format!("{e:?}")))?, props, "tmod")
    };
    let size = artifact.len();
    let sha = put_blob(state, artifact).await?;
    ensure_hash_unowned(state, project, &[&sha]).await?;
    let n = NewRelease {
        tag,
        branch,
        title: req.title,
        changelog: req.changelog,
        status: req.status,
        sha,
        size,
        props,
        source_commit_sha: Some(commit),
        format,
        prior: Vec::new(),
        silent: req.silent,
    };
    insert_release(state, project, n).await
}

/// `POST /v1/mods/hub/projects/{handle}/{slug}/releases`; not routed while
/// the path's GET is still the catalog's.
pub async fn create(State(s): State<AppState>, req: Request) -> Response {
    let (mut parts, body) = req.into_parts();
    let body = match read_body(&parts.headers, body).await {
        Ok(b) => b,
        Err(e) => return e.into_response(),
    };
    let wu = match deps(&s, &parts).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    let hs = match Path::<Hs>::from_request_parts(&mut parts, &s).await {
        Ok(Path(h)) => h,
        Err(e) => return e.into_response(),
    };
    let req: CreateReq = match validate(&body) {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    let r = async {
        let mut project = require_owned(&s, &hs, &wu.user).await?;
        let config = match req.config_base64.as_deref().filter(|c| !c.is_empty()) {
            Some(enc) => Some(b64decode_strict(enc).map_err(|_| bad("config_base64 is not valid base64."))?),
            None => None,
        };
        create_from_commit(&s, &mut project, &wu.user, req, config).await
    };
    reply(StatusCode::CREATED, wu.rate_limit, r.await)
}

// --- upload ----------------------------------------------------------------

struct UploadForm {
    tag: String,
    title: String,
    changelog: String,
    status: String,
    branch: String,
    silent: bool,
    file: form::UploadFile,
    config: Option<form::UploadFile>,
}

fn upload_form(items: &[(String, FormValue)]) -> Result<UploadForm, ValidationError> {
    let mut v = Validator::default();
    let tag = form_str(&mut v, items, "tag", None);
    let title = form_str(&mut v, items, "title", Some(""));
    let changelog = form_str(&mut v, items, "changelog", Some(""));
    let status = form_str(&mut v, items, "status", Some("published"));
    let branch = form_str(&mut v, items, "branch", Some(""));
    let silent = form_bool(&mut v, items, "silent");
    let file = form_file(&mut v, items, "file", true);
    let config = form_file(&mut v, items, "config", false);
    v.finish(())?;
    Ok(UploadForm { tag, title, changelog, status, branch, silent, file: file.unwrap_or_else(empty_file), config })
}

fn empty_file() -> form::UploadFile {
    form::UploadFile { filename: String::new(), headers: Vec::new(), data: Vec::new() }
}

/// `create_release_from_upload(...)`.
async fn create_from_upload(
    state: &AppState,
    project: &mut Doc,
    actor: &SiteUser,
    f: UploadForm,
) -> Result<Value, ApiError> {
    require_publish_ok(actor, &f.status)?;
    let tag = strip(&f.tag).to_owned();
    let branch = match strip(&f.branch) {
        "" => project.str("default_branch").to_owned(),
        b => b.to_owned(),
    };
    check_tag(state, project, &branch, &tag).await?;
    let filename = if f.file.filename.is_empty() { "mod.tmod".to_owned() } else { f.file.filename.clone() };
    let mut data = f.file.data;
    let config = f.config.map(|c| c.data).filter(|c| !c.is_empty());
    let is_zip = data.starts_with(b"PK") || lower(&filename).ends_with(".zip");
    let mut props: Props = Vec::new();
    let mut base_sha: Option<String> = None;
    let format = if is_zip {
        if config.is_some() {
            return Err(bad("A config file can only be packed into a .tmod build."));
        }
        let (ok, d) = blocking(move || (artifact::is_zipfile(&data), data)).await?;
        data = d;
        if !ok.map_err(|_| internal("BadZipFile"))? {
            return Err(bad("Not a valid .zip file."));
        }
        "zip"
    } else {
        let (parsed, d) = blocking(move || (artifact::read(&data, false).map(|t| t.props), data)).await?;
        data = d;
        props = parsed.map_err(|e| bad(format!("Not a valid .tmod file: {e}")))?;
        if let Some(cfg) = config {
            base_sha = Some(hex::encode(Sha256::digest(&data)));
            let (built, p) = blocking(move || repack_with_config(&data, cfg)).await??;
            data = built;
            props = p;
        }
        "tmod"
    };
    let size = data.len();
    let sha = put_blob(state, data).await?;
    if base_sha.as_deref() == Some(sha.as_str()) {
        base_sha = None;
    }
    ensure_hash(state, project, &[&sha, base_sha.as_deref().unwrap_or("")]).await?;
    let n = NewRelease {
        tag,
        branch,
        title: f.title,
        changelog: f.changelog,
        status: f.status,
        sha,
        size,
        props,
        source_commit_sha: None,
        format,
        prior: base_sha.into_iter().collect(),
        silent: f.silent,
    };
    insert_release(state, project, n).await
}

/// `POST /v1/mods/hub/projects/{handle}/{slug}/releases/upload`.
pub async fn upload(State(s): State<AppState>, req: Request) -> Response {
    let (mut parts, body) = req.into_parts();
    let items = match read_form(&parts, body).await {
        Ok(i) => i,
        Err(r) => return r,
    };
    let wu = match deps(&s, &parts).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    let hs = match Path::<Hs>::from_request_parts(&mut parts, &s).await {
        Ok(Path(h)) => h,
        Err(e) => return e.into_response(),
    };
    let f = match upload_form(&items) {
        Ok(f) => f,
        Err(e) => return ApiError::from(e).into_response(),
    };
    drop(items);
    let r = async {
        let mut project = require_owned(&s, &hs, &wu.user).await?;
        if !matches!(f.status.as_str(), "draft" | "published") {
            return Err(bad("status must be draft or published"));
        }
        create_from_upload(&s, &mut project, &wu.user, f).await
    };
    reply(StatusCode::CREATED, wu.rate_limit, r.await)
}

// --- the release itself ----------------------------------------------------

async fn release_and_project(state: &AppState, id: &str) -> Result<(Doc, Doc), ApiError> {
    let release = access::get_release(state, id).await?.ok_or_else(|| ApiError::not_found("Release not found"))?;
    let project = access::project_by_id(state, release.oid("project_id")).await?;
    let project = project.ok_or_else(|| ApiError::not_found("Release not found"))?;
    Ok((release, project))
}

fn with_changed(r: &Doc, changed: bool) -> Value {
    let mut m: Map<String, Value> = release_dto(r);
    m.insert("changed".into(), json!(changed));
    Value::Object(m)
}

/// `attach_config_to_release(release, project, actor, data)`.
async fn attach(
    state: &AppState,
    mut release: Doc,
    project: &Doc,
    actor: &SiteUser,
    data: Vec<u8>,
) -> Result<Value, ApiError> {
    if !can_edit(project, Some(actor)) {
        return Err(forbidden_edit());
    }
    if release.str("release_format") != "tmod" {
        return Err(bad("A config file can only be packed into a .tmod build."));
    }
    let current =
        blob(state, release.str("tmod_sha")).await?.ok_or_else(|| ApiError::not_found("Release artifact not found"))?;
    let (artifact, props) = blocking(move || repack_with_config(&current, data)).await??;
    let size = artifact.len();
    let sha = put_blob(state, artifact).await?;
    if sha == release.str("tmod_sha") {
        return Ok(with_changed(&release, false));
    }
    ensure_hash(state, project, &[&sha]).await?;
    let mut prior = release.list("prior_tmod_shas").to_vec();
    let old = release.str("tmod_sha").to_owned();
    if !prior.contains(&old) {
        prior.push(old);
    }
    release.set("prior_tmod_shas", V::List(prior));
    release.set("tmod_sha", V::Str(sha));
    release.set("tmod_size", V::Int(size as i64));
    release.set("tmod_properties", V::Map(props));
    let filename = super::read::download_filename(&release);
    release.set("tmod_filename", V::Str(filename));
    release.set("updated_at", V::Dt(dt::now()));
    save(state, RELEASES, &release).await?;
    Ok(with_changed(&release, true))
}

/// `POST /v1/mods/hub/releases/{release_id}/config`.
pub async fn attach_config(State(s): State<AppState>, req: Request) -> Response {
    let (mut parts, body) = req.into_parts();
    let items = match read_form(&parts, body).await {
        Ok(i) => i,
        Err(r) => return r,
    };
    let wu = match deps(&s, &parts).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    let Rid(id) = match Path::<Rid>::from_request_parts(&mut parts, &s).await {
        Ok(Path(r)) => r,
        Err(e) => return e.into_response(),
    };
    let mut v = Validator::default();
    let file = form_file(&mut v, &items, "file", true);
    if let Err(e) = v.finish(()) {
        return ApiError::from(e).into_response();
    }
    drop(items);
    let data = file.map(|f| f.data).unwrap_or_default();
    let r = async {
        let (release, project) = release_and_project(&s, &id).await?;
        attach(&s, release, &project, &wu.user, data).await
    };
    reply(StatusCode::OK, wu.rate_limit, r.await)
}

/// `_clean_i18n_map(translations, base=, max_len=)`.
pub(crate) fn clean_i18n(
    translations: &[(String, String)],
    base: &str,
    max_len: usize,
) -> Result<Vec<(String, String)>, ApiError> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (lang, text) in translations {
        let lang = strip(lang);
        if lang == "en" {
            return Err(bad(format!("The English {base} is `{base}`, not a translation.")));
        }
        if !CONTENT_LANGS.contains(&lang) {
            return Err(bad(format!("'{lang}' isn't a language the site supports.")));
        }
        if text.chars().count() > max_len {
            return Err(bad(format!(
                "The {lang} {base} is too long (max {} characters).",
                super::read::thousands(max_len)
            )));
        }
        let text = strip(text);
        if !text.is_empty() {
            match out.iter_mut().find(|(k, _)| k == lang) {
                Some(slot) => slot.1 = text.to_owned(),
                None => out.push((lang.to_owned(), text.to_owned())),
            }
        }
    }
    Ok(out)
}

/// `update_release(release, project, actor, **fields)`.
async fn update_release(
    state: &AppState,
    mut release: Doc,
    mut project: Doc,
    actor: &SiteUser,
    req: UpdateReq,
) -> Result<Value, ApiError> {
    if !can_edit(&project, Some(actor)) {
        return Err(forbidden_edit());
    }
    if let Some(silent) = req.silent {
        release.set("silent", V::Bool(silent));
    }
    if let Some(title) = &req.title {
        release.set("title", V::Str(strip(title).to_owned()));
    }
    if let Some(changelog) = req.changelog {
        release.set("changelog", V::Str(changelog));
    }
    if let Some(t) = &req.title_i18n {
        release.set("title_i18n", V::Map(clean_i18n(t, "title", 160)?));
    }
    if let Some(t) = &req.changelog_i18n {
        release.set("changelog_i18n", V::Map(clean_i18n(t, "changelog", 20_000)?));
    }
    let mut became_published = false;
    if let Some(status) = req.status.filter(|s| s != release.str("status")) {
        require_publish_ok(actor, &status)?;
        if status == "published" {
            became_published = true;
            if release.dt("published_at").is_none() {
                release.set("published_at", V::Dt(dt::now()));
            }
        }
        release.set("status", V::Str(status));
    }
    release.set("updated_at", V::Dt(dt::now()));
    save(state, RELEASES, &release).await?;
    if became_published {
        if !release.bool("silent") {
            let at = release.dt("published_at").unwrap_or_else(dt::now);
            project.set("last_release_at", V::Dt(at));
            save(state, PROJECTS, &project).await?;
        }
        event::emit(state, &project, &mut release).await?;
    }
    Ok(Value::Object(release_dto(&release)))
}

async fn owned_release(state: &AppState, id: &str, user: &SiteUser) -> Result<(Doc, Doc), ApiError> {
    let (release, project) = release_and_project(state, id).await?;
    if project.oid("owner_id") != Some(user.id) {
        return Err(ApiError::not_found("Release not found"));
    }
    Ok((release, project))
}

/// `PATCH /v1/mods/hub/releases/{release_id}`.
pub async fn update(State(s): State<AppState>, req: Request) -> Response {
    let (mut parts, body) = req.into_parts();
    let body: BodyInput = match read_body(&parts.headers, body).await {
        Ok(b) => b,
        Err(e) => return e.into_response(),
    };
    let wu = match deps(&s, &parts).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    let Rid(id) = match Path::<Rid>::from_request_parts(&mut parts, &s).await {
        Ok(Path(r)) => r,
        Err(e) => return e.into_response(),
    };
    let req: UpdateReq = match validate(&body) {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    let r = async {
        let (release, project) = owned_release(&s, &id, &wu.user).await?;
        update_release(&s, release, project, &wu.user, req).await
    };
    reply(StatusCode::OK, wu.rate_limit, r.await)
}

/// `DELETE /v1/mods/hub/releases/{release_id}`.
pub async fn delete(State(s): State<AppState>, req: Request) -> Response {
    let (mut parts, _) = req.into_parts();
    let wu = match deps(&s, &parts).await {
        Ok(w) => w,
        Err(r) => return r,
    };
    let Rid(id) = match Path::<Rid>::from_request_parts(&mut parts, &s).await {
        Ok(Path(r)) => r,
        Err(e) => return e.into_response(),
    };
    let r = async {
        let (release, project) = owned_release(&s, &id, &wu.user).await?;
        if !can_edit(&project, Some(&wu.user)) {
            return Err(forbidden_edit());
        }
        let db = s.inner.db.mongo.as_ref().ok_or_else(|| internal("no mongo"))?;
        db.collection::<Document>(RELEASES).delete_one(doc! {"_id": release.id}).await.map_err(internal)?;
        Ok(())
    };
    match r.await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => e.into_response(),
    }
}
