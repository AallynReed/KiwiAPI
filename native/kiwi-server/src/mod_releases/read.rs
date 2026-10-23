//! The release reads of `service.py` (DTO, download, files, inspector, cfgs,
//! blueprints, swfs, sound banks) and their `/v1/mods/hub/releases/*` and
//! `/site/mods/releases/*` routes.

use axum::{
    extract::{FromRequestParts, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use super::{
    ModsAsset, ModsPub, SiteMods,
    access::{self, internal, release_with_project},
    artifact::{self, lower},
    model::Doc,
};
use crate::{
    auth::{OptionalSiteUser, models::SiteUser},
    blueprints::cache::{self, BuildError, Fmt},
    error::{ApiError, ErrorCode},
    features::{ModsHub, RequireFeature},
    media::{self, latin1_header, swf::decompile},
    pyjson::{PyJson, PydanticJson},
    ratelimit::RateLimitInfo,
    state::AppState,
    validation::{FromParams, Params, Path},
};

// --- params ----------------------------------------------------------------

pub struct Rid(pub String);

impl FromParams for Rid {
    fn from_params(p: &mut Params<'_>) -> Self {
        Rid(p.path("release_id").str().required())
    }
}

pub struct RidPath {
    pub id: String,
    pub path: String,
}

impl FromParams for RidPath {
    fn from_params(p: &mut Params<'_>) -> Self {
        RidPath {
            id: p.path("release_id").str().required(),
            path: p.query("path").str().min_len(1).max_len(400).required(),
        }
    }
}

pub struct RidBlueprint {
    id: String,
    path: String,
    fmt: String,
}

impl FromParams for RidBlueprint {
    fn from_params(p: &mut Params<'_>) -> Self {
        RidBlueprint {
            id: p.path("release_id").str().required(),
            path: p.query("path").str().min_len(1).max_len(400).required(),
            fmt: p.query("fmt").str().one_of_pattern(&["json", "bin"]).default("json".into()),
        }
    }
}

pub struct RidSound {
    id: String,
    path: String,
    sound: i64,
    raw: bool,
}

impl FromParams for RidSound {
    fn from_params(p: &mut Params<'_>) -> Self {
        RidSound {
            id: p.path("release_id").str().required(),
            path: p.query("path").str().min_len(1).max_len(400).required(),
            sound: p.query("id").int().ge(0).le(0xFFFF_FFFFi64).required(),
            raw: p.query("raw").bool().default(false),
        }
    }
}

// --- service ---------------------------------------------------------------

pub fn not_found(msg: &str) -> ApiError {
    ApiError::not_found(msg)
}

pub fn release_props(r: &Doc) -> &[(String, String)] {
    r.map("tmod_properties")
}

/// `release_download_filename(release)`.
pub fn download_filename(r: &Doc) -> String {
    artifact::download_filename(r.str("release_format"), r.str("tmod_filename"), release_props(r))
}

pub fn is_tmod(r: &Doc) -> bool {
    r.str("release_format") == "tmod"
}

/// `_release_dto(release)`.
pub fn release_dto(r: &Doc) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("id".into(), json!(r.id.to_hex()));
    m.insert("tag".into(), json!(r.str("tag")));
    m.insert("branch".into(), json!(r.str("branch")));
    m.insert("title".into(), json!(r.str("title")));
    m.insert("title_i18n".into(), r.map_json("title_i18n"));
    m.insert("changelog".into(), json!(r.str("changelog")));
    m.insert("changelog_i18n".into(), r.map_json("changelog_i18n"));
    m.insert("status".into(), json!(r.str("status")));
    m.insert("silent".into(), json!(r.bool("silent")));
    m.insert("tmod_filename".into(), json!(download_filename(r)));
    m.insert("tmod_size".into(), json!(r.int("tmod_size")));
    m.insert("download_count".into(), json!(r.int("download_count")));
    m.insert("format".into(), json!(r.str("release_format")));
    m.insert("source_commit_sha".into(), r.opt_json("source_commit_sha"));
    m.insert("banner_sha".into(), r.opt_json("banner_sha"));
    m.insert("published_at".into(), r.iso("published_at"));
    m.insert("created_at".into(), r.iso("created_at"));
    m
}

pub async fn blob(state: &AppState, sha: &str) -> Result<Option<Vec<u8>>, ApiError> {
    media::read_blob(media::mods_store(state), sha.to_owned()).await
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f).await.map_err(internal)
}

/// The artifact of a `.tmod` release, parsed; `None` for a zip, a missing
/// blob or (with `lenient`) an unreadable file. Without `lenient` a
/// `TmodError` is the uncaught 500.
async fn parsed(state: &AppState, r: &Doc, full: bool, lenient: bool) -> Result<Option<artifact::Tmod>, ApiError> {
    if !is_tmod(r) {
        return Ok(None);
    }
    let Some(data) = blob(state, r.str("tmod_sha")).await? else { return Ok(None) };
    match blocking(move || artifact::read(&data, full)).await? {
        Ok(t) => Ok(Some(t)),
        Err(_) if lenient => Ok(None),
        Err(e) => Err(internal(format!("TmodError: {e}"))),
    }
}

fn item(path: &str, size: u32) -> Map<String, Value> {
    let mut m = Map::new();
    m.insert("path".into(), json!(path));
    m.insert("size".into(), json!(size));
    m
}

fn sort_by_lower(items: &mut [Map<String, Value>]) {
    items.sort_by_cached_key(|m| lower(m.get("path").and_then(Value::as_str).unwrap_or("")));
}

fn items_json(items: Vec<Map<String, Value>>) -> Value {
    Value::Array(items.into_iter().map(Value::Object).collect())
}

/// `_list_blueprints_sync`: the non-empty blueprints and their basenames.
pub fn blueprint_parts(t: &artifact::Tmod) -> (Vec<Map<String, Value>>, Vec<String>) {
    let mut items = Vec::new();
    let mut fns = Vec::new();
    for f in &t.files {
        let p = lower(&f.path);
        if !p.ends_with(".blueprint") || kiwi_core::bpedit::codec::is_empty(&f.data) {
            continue;
        }
        items.push(item(&f.path, f.size));
        let last = p.rsplit('/').next().unwrap_or(&p);
        fns.push(last[..last.len() - ".blueprint".len()].to_owned());
    }
    (items, fns)
}

pub fn blueprints_value(
    mut items: Vec<Map<String, Value>>,
    fns: &[String],
    rig: Value,
    anims: Value,
    parts: &std::collections::HashSet<String>,
) -> Value {
    for (it, f) in items.iter_mut().zip(fns) {
        it.insert("assembled".into(), json!(parts.contains(f)));
    }
    json!({"items": items_json(items), "rig": rig, "animations": anims})
}

/// `list_release_blueprints(release)`.
pub async fn list_blueprints(state: &AppState, r: &Doc) -> Result<Value, ApiError> {
    let Some(t) = parsed(state, r, true, true).await? else {
        return Ok(json!({"items": [], "rig": null, "animations": []}));
    };
    let (items, fns) = blocking(move || blueprint_parts(&t)).await?;
    let (rig, anims, parts) = super::rig::resolve_release(state, &fns).await?;
    Ok(blueprints_value(items, &fns, rig, anims, &parts))
}

/// `decode_release_blueprint(release, path, fmt)`.
async fn blueprint(state: &AppState, r: &Doc, path: &str, fmt: Fmt) -> Result<cache::Cached, BuildError> {
    if !is_tmod(r) {
        return Err(not_found("This release has no blueprint models.").into());
    }
    let sha = r.str("tmod_sha").to_owned();
    let want = lower(kiwi_core::pystr::strip(path));
    let key = cache::key_for_tmod(&sha, path);
    let st = state.clone();
    let build = || async move {
        let data = blob(&st, &sha).await?.ok_or_else(|| not_found("Release artifact not found"))?;
        let packed = blocking(move || {
            let t = artifact::read(&data, true).map_err(|e| internal(format!("TmodError: {e}")))?;
            let target = t.files.into_iter().find(|f| lower(&f.path) == want && want.ends_with(".blueprint"));
            let target = target.ok_or_else(|| not_found("No such blueprint in this release"))?;
            pack_blueprint(&target.data, &target.path)
        })
        .await??;
        Ok(packed)
    };
    cache::get_or_build(state, &key, fmt, build).await
}

/// `pack_blueprint(raw, path)` with `_decode_blueprint_payload`'s mapping.
fn pack_blueprint(raw: &[u8], path: &str) -> Result<Map<String, Value>, ApiError> {
    use kiwi_core::voxel::PackError;
    let bad = |status: StatusCode, msg: String| ApiError::new(status, ErrorCode::BadRequest, msg);
    match kiwi_core::voxel::pack(raw, path, kiwi_core::voxel::RENDER_VOXEL_CAP) {
        Ok(Value::Object(m)) if m.get("count").and_then(Value::as_i64).unwrap_or(0) > 0 => Ok(m),
        Ok(_) => Err(internal("blueprint without voxels")),
        Err(PackError::Empty) => Err(bad(
            StatusCode::UNPROCESSABLE_ENTITY,
            "This blueprint is an empty placeholder (no voxels to show).".into(),
        )),
        Err(PackError::TooLarge(n)) => Err(bad(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("This model is too large to preview ({} voxels).", thousands(n)),
        )),
        Err(PackError::Invalid(m)) => {
            Err(bad(StatusCode::UNPROCESSABLE_ENTITY, format!("Couldn't read that blueprint: {m}")))
        }
        Err(PackError::Unhandled(m)) => Err(internal(m)),
    }
}

/// `f"{n:,}"`.
pub fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `list_release_files`' work.
pub fn files_value(t: &artifact::Tmod) -> Value {
    let preview = artifact::preview_path(&t.props);
    let mut items: Vec<_> =
        t.files.iter().filter(|f| lower(&f.path) != preview).map(|f| item(&f.path, f.size)).collect();
    sort_by_lower(&mut items);
    json!({"items": items_json(items)})
}

/// `list_release_files(release)`.
pub async fn list_files(state: &AppState, r: &Doc) -> Result<Value, ApiError> {
    Ok(parsed(state, r, false, true).await?.map_or_else(|| json!({"items": []}), |t| files_value(&t)))
}

/// `inspect_release(release)` over the stored artifact (`None`: missing).
pub fn inspect_value(r: &Doc, data: Option<&[u8]>) -> Result<Value, ApiError> {
    let mut out = Map::new();
    out.insert("format".into(), json!(r.str("release_format")));
    out.insert("tag".into(), json!(r.str("tag")));
    out.insert("branch".into(), json!(r.str("branch")));
    out.insert("filename".into(), json!(download_filename(r)));
    out.insert("sha256".into(), json!(r.str("tmod_sha")));
    out.insert("prior_sha256s".into(), r.list_json("prior_tmod_shas"));
    out.insert("size".into(), json!(r.int("tmod_size")));
    out.insert("version".into(), Value::Null);
    out.insert("properties".into(), json!({}));
    out.insert("categories".into(), json!([]));
    out.insert("flags".into(), json!(0));
    out.insert("preview_path".into(), json!(""));
    out.insert("config_path".into(), json!(""));
    out.insert("files".into(), json!([]));
    out.insert("file_count".into(), json!(0));
    out.insert("total_size".into(), json!(0));
    out.insert("readable".into(), json!(false));
    let Some(data) = data else { return Ok(Value::Object(out)) };
    let mut files: Vec<Map<String, Value>> = if r.str("release_format") == "zip" {
        let entries = match artifact::zip_entries(data) {
            Ok(e) => e,
            Err(artifact::ZipFail::Bad) => return Ok(Value::Object(out)),
            Err(artifact::ZipFail::Unhandled) => return Err(internal("zip file version")),
        };
        out.insert("readable".into(), json!(true));
        entries
            .into_iter()
            .filter(|(n, _)| !n.ends_with('/'))
            .map(|(n, s)| {
                let mut m = Map::new();
                m.insert("path".into(), json!(n));
                m.insert("size".into(), json!(s));
                m
            })
            .collect()
    } else {
        let Ok(t) = artifact::read(data, false) else { return Ok(Value::Object(out)) };
        let flags = artifact::flags_value(&t.props);
        let props: Map<String, Value> = t.props.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
        out.insert("readable".into(), json!(true));
        out.insert("version".into(), json!(t.version));
        out.insert("properties".into(), Value::Object(props));
        out.insert("categories".into(), json!(artifact::categories(flags)));
        out.insert("flags".into(), json!(flags));
        out.insert("preview_path".into(), json!(artifact::preview_path(&t.props)));
        out.insert("config_path".into(), json!(artifact::declared_config_path(&t.props)));
        t.files.iter().map(|f| item(&f.path, f.size)).collect()
    };
    sort_by_lower(&mut files);
    let total: u64 = files.iter().map(|f| f.get("size").and_then(Value::as_u64).unwrap_or(0)).sum();
    out.insert("file_count".into(), json!(files.len()));
    out.insert("total_size".into(), json!(total));
    out.insert("files".into(), items_json(files));
    Ok(Value::Object(out))
}

/// `inspect_release(release)`.
pub async fn inspect(state: &AppState, r: &Doc) -> Result<Value, ApiError> {
    let data = blob(state, r.str("tmod_sha")).await?;
    let r = r.clone();
    blocking(move || inspect_value(&r, data.as_deref())).await?
}

/// The bytes `download_release_file` serves from a parsed artifact.
pub fn file_bytes(t: artifact::Tmod, want: &str) -> Option<Vec<u8>> {
    if lower(want) == artifact::preview_path(&t.props) {
        return None;
    }
    t.files.into_iter().find(|f| f.path == want).map(|f| f.data)
}

/// `download_release_file(release, path)`: `(bytes, filename)`.
pub async fn file(state: &AppState, r: &Doc, path: &str) -> Result<(Vec<u8>, String), ApiError> {
    if !is_tmod(r) {
        return Err(not_found("This release has no individually downloadable files."));
    }
    let data = blob(state, r.str("tmod_sha")).await?.ok_or_else(|| not_found("Release artifact not found"))?;
    let want = kiwi_core::pystr::strip(path).to_owned();
    let w = want.clone();
    let raw = blocking(move || -> Result<Option<Vec<u8>>, ApiError> {
        let t = artifact::read(&data, true).map_err(|e| internal(format!("TmodError: {e}")))?;
        Ok(file_bytes(t, &w))
    })
    .await??;
    let raw = raw.ok_or_else(|| not_found("No such file in this release"))?;
    let name = want.rsplit('/').next().unwrap_or(&want).to_owned();
    Ok((raw, name))
}

/// `download_release_cfg(release, path)`.
pub async fn cfg(state: &AppState, r: &Doc, path: &str) -> Result<(Vec<u8>, String), ApiError> {
    let want = kiwi_core::pystr::strip(path).to_owned();
    if !lower(&want).ends_with(".cfg") {
        return Err(not_found("No such config file in this release"));
    }
    let (data, _) = file(state, r, &want).await?;
    Ok((data, artifact::cfg_download_name(release_props(r), &want)))
}

/// `list_release_cfgs`' work (`props` = the release's stored header).
pub fn cfgs_value(props: &[(String, String)], t: &artifact::Tmod) -> Value {
    let declared = artifact::declared_config_path(&t.props);
    let mut items: Vec<(bool, String, Map<String, Value>)> = t
        .files
        .iter()
        .filter(|f| lower(&f.path).ends_with(".cfg"))
        .map(|f| {
            let is_declared = lower(&f.path) == declared;
            let mut m = item(&f.path, f.size);
            m.insert("filename".into(), json!(artifact::cfg_download_name(props, &f.path)));
            m.insert("declared".into(), json!(is_declared));
            (!is_declared, lower(&f.path), m)
        })
        .collect();
    items.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    let flash = t.files.iter().any(|f| lower(&f.path).ends_with(".swf"));
    json!({"items": items_json(items.into_iter().map(|i| i.2).collect()), "has_flash_ui": flash})
}

/// `list_release_cfgs(release)`.
pub async fn list_cfgs(state: &AppState, r: &Doc) -> Result<Value, ApiError> {
    match parsed(state, r, false, true).await? {
        Some(t) => Ok(cfgs_value(release_props(r), &t)),
        None => Ok(json!({"items": [], "has_flash_ui": false})),
    }
}

pub fn by_ext(t: &artifact::Tmod, ext: &str) -> Value {
    let mut items: Vec<_> =
        t.files.iter().filter(|f| lower(&f.path).ends_with(ext)).map(|f| item(&f.path, f.size)).collect();
    sort_by_lower(&mut items);
    items_json(items)
}

/// `list_release_swfs(release)`.
pub async fn list_swfs(state: &AppState, r: &Doc) -> Result<Value, ApiError> {
    let decompiler = decompile::available(state.settings());
    let items = match parsed(state, r, false, true).await? {
        Some(t) => by_ext(&t, ".swf"),
        None => json!([]),
    };
    Ok(json!({"items": items, "decompiler": decompiler}))
}

/// `release_swf_scripts(release, path)`.
pub async fn swf_scripts(state: &AppState, r: &Doc, path: &str) -> Result<Value, ApiError> {
    if !is_tmod(r) {
        return Err(not_found("That release has no Flash movies."));
    }
    let data = blob(state, r.str("tmod_sha")).await?.ok_or_else(|| not_found("Release artifact not found."))?;
    let want = lower(kiwi_core::pystr::strip(&path.replace('\\', "/")));
    let raw = blocking(move || -> Result<Option<Vec<u8>>, ApiError> {
        let t = artifact::read(&data, true).map_err(|_| ApiError::bad_request("That file isn't a readable .tmod."))?;
        Ok(t.files.into_iter().find(|f| lower(&f.path) == want && want.ends_with(".swf")).map(|f| f.data))
    })
    .await??;
    let raw = raw.ok_or_else(|| not_found("No such Flash movie in this mod."))?;
    let sha = hex::encode(Sha256::digest(&raw));
    let size = raw.len();
    let payload = decompile::scripts(state, raw, &sha).await?;
    let mut out = Map::new();
    out.insert("path".into(), json!(path));
    out.insert("size".into(), json!(size));
    if let Value::Object(m) = payload {
        out.extend(m);
    }
    Ok(Value::Object(out))
}

/// `list_release_banks(release)`.
pub async fn list_banks(state: &AppState, r: &Doc) -> Result<Value, ApiError> {
    let items = match parsed(state, r, false, true).await? {
        Some(t) => by_ext(&t, ".bnk"),
        None => json!([]),
    };
    Ok(json!({"items": items}))
}

async fn tmod_or_404(state: &AppState, r: &Doc) -> Result<Vec<u8>, ApiError> {
    if !is_tmod(r) {
        return Err(not_found("That release has no sounds to play."));
    }
    blob(state, r.str("tmod_sha")).await?.ok_or_else(|| not_found("Release artifact not found."))
}

/// A bank's bytes and its sidecar text.
type BankFiles = (Vec<u8>, Option<String>);

/// `_tmod_bank_bytes_sync(tmod_bytes, path)`.
pub fn bank_files(t: &artifact::Tmod, path: &str) -> Option<BankFiles> {
    let want = lower(&artifact::basename(path));
    let hit =
        t.files.iter().find(|f| lower(&f.path).ends_with(".bnk") && lower(&artifact::basename(&f.path)) == want)?;
    let n = hit.path.chars().count();
    let stem: String = lower(&hit.path.chars().take(n.saturating_sub(4)).collect::<String>());
    let side = t.files.iter().find(|f| lower(&f.path) == format!("{stem}.txt"));
    let text = side.map(|s| String::from_utf8_lossy(&s.data).into_owned());
    Some((hit.data.clone(), text))
}

/// `tmod_bank_index(tmod_bytes, path)`.
async fn bank_index(state: &AppState, data: Vec<u8>, path: &str) -> Result<Value, ApiError> {
    let p = path.to_owned();
    let got = blocking(move || -> Result<Option<BankFiles>, ApiError> {
        let t = artifact::read(&data, true).map_err(|_| ApiError::bad_request("That file isn't a readable .tmod."))?;
        Ok(bank_files(&t, &p))
    })
    .await??;
    let (raw, sidecar) = got.ok_or_else(|| not_found("No such sound bank in this mod."))?;
    let sidecar_sha = sidecar.as_ref().filter(|s| !s.is_empty()).map(|s| hex::encode(Sha256::digest(s.as_bytes())));
    let sha = hex::encode(Sha256::digest(&raw));
    media::audio::manifest(state, raw, &sha, sidecar, sidecar_sha.as_deref()).await
}

/// `release_bank_index(release, path)`.
pub async fn bank(state: &AppState, r: &Doc, path: &str) -> Result<Value, ApiError> {
    let data = tmod_or_404(state, r).await?;
    let payload = bank_index(state, data, path).await?;
    let get = |k: &str| payload.get(k).cloned().unwrap_or(Value::Null);
    let sounds: Vec<Value> = payload
        .get("sounds")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|s| match s {
                    Value::Object(m) => Value::Object(
                        m.iter().filter(|(k, _)| *k != "sha").map(|(k, v)| (k.clone(), v.clone())).collect(),
                    ),
                    other => other.clone(),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(json!({
        "path": path, "bank": get("bank"), "sounds": sounds, "count": get("count"),
        "playable": get("playable"), "total_duration": get("total_duration"),
    }))
}

/// `release_sound(release, path, id, raw)`: `(bytes, media type, filename, etag)`.
async fn sound(
    state: &AppState,
    r: &Doc,
    path: &str,
    id: i64,
    raw: bool,
) -> Result<(Vec<u8>, String, String, String), ApiError> {
    let data = tmod_or_404(state, r).await?;
    let payload = bank_index(state, data, path).await?;
    let sounds = payload.get("sounds").and_then(Value::as_array).cloned().unwrap_or_default();
    let found = sounds.iter().find(|s| match s.get("id") {
        Some(Value::Bool(b)) => i64::from(*b) == id,
        Some(v) => v.as_i64() == Some(id) || v.as_f64() == Some(id as f64),
        None => false,
    });
    let s = found.ok_or_else(|| not_found("No such sound in this bank."))?;
    let sha = s.get("sha").and_then(Value::as_str).ok_or_else(|| internal("sound without sha"))?.to_owned();
    let store = media::mods_store(state);
    let (data, media_type, ext) = if raw {
        (media::read_blob(store, sha.clone()).await?, "audio/vnd.wave".to_owned(), "wem".to_owned())
    } else {
        if let Some(err) = s.get("error").filter(|e| media::swf::truthy(Some(e))) {
            let msg = match err {
                Value::String(t) => t.clone(),
                other => crate::pyjson::dumps(other),
            };
            return Err(ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::BadRequest, msg));
        }
        match media::read_blob(store, sha.clone()).await? {
            None => (None, String::new(), String::new()),
            Some(blob) => match blocking(move || media::audio::convert(&blob)).await? {
                Ok(c) => (Some(c.bytes), c.mime.to_owned(), c.ext.to_owned()),
                Err(kiwi_core::audio::ConvertError::Wem(e)) => {
                    return Err(ApiError::new(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        ErrorCode::BadRequest,
                        format!("This sound could not be decoded: {e}"),
                    ));
                }
                Err(kiwi_core::audio::ConvertError::Crash(e)) => return Err(internal(e)),
            },
        }
    };
    let data = data.ok_or_else(|| not_found("Sound blob missing from the store."))?;
    let stem = match s.get("name") {
        Some(Value::String(n)) if !n.is_empty() => n.clone(),
        _ => format!("sound_{id}"),
    };
    let stem = media::sanitize(&stem, "._-");
    let stem = if stem.is_empty() { "sound".to_owned() } else { stem };
    let etag = format!("\"{sha}{}\"", if raw { "-raw" } else { "" });
    Ok((data, media_type, format!("{stem}.{ext}"), etag))
}

// --- responses -------------------------------------------------------------

fn v1(rl: &RateLimitInfo, r: Result<Value, ApiError>) -> Response {
    match r {
        Ok(v) => (*rl, PydanticJson(v)).into_response(),
        Err(e) => e.into_response(),
    }
}

fn site(max_age: Option<u32>, r: Result<Value, ApiError>) -> Response {
    match r {
        Ok(v) => {
            let mut res = PyJson(v).into_response();
            if let Some(age) = max_age
                && let Ok(h) = HeaderValue::from_str(&format!("public, max-age={age}"))
            {
                res.headers_mut().insert(header::CACHE_CONTROL, h);
            }
            res
        }
        Err(e) => e.into_response(),
    }
}

fn attachment(data: Vec<u8>, media_type: &'static str, filename: &str, cache: Option<&'static str>) -> Response {
    let safe: String = filename.chars().filter(|c| !matches!(c, '"' | '\r' | '\n')).collect();
    let disposition = match latin1_header(&format!("attachment; filename=\"{safe}\"")) {
        Ok(h) => h,
        Err(e) => return e.into_response(),
    };
    let mut res = (StatusCode::OK, data).into_response();
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(media_type));
    h.insert(header::CONTENT_DISPOSITION, disposition);
    if let Some(c) = cache {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static(c));
    }
    res
}

/// The download routes: the artifact, then `record_download` after the reply.
async fn download(state: &AppState, viewer: Option<&SiteUser>, id: &str, cache: Option<&'static str>) -> Response {
    let (release, project) = match release_with_project(state, id, viewer).await {
        Ok(rp) => rp,
        Err(e) => return e.into_response(),
    };
    let data = match blob(state, release.str("tmod_sha")).await {
        Ok(Some(d)) => d,
        Ok(None) => return not_found("Release file is missing from storage").into_response(),
        Err(e) => return e.into_response(),
    };
    let media_type =
        if release.str("release_format") == "zip" { "application/zip" } else { "application/octet-stream" };
    let disposition = match latin1_header(&format!("attachment; filename=\"{}\"", download_filename(&release))) {
        Ok(h) => h,
        Err(e) => return e.into_response(),
    };
    let st = state.clone();
    tokio::spawn(async move { record_download(&st, &release, &project).await });
    let mut res = (StatusCode::OK, data).into_response();
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(media_type));
    h.insert(header::CONTENT_DISPOSITION, disposition);
    if let Some(c) = cache {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static(c));
    }
    res
}

/// `record_download(release, project)`.
pub async fn record_download(state: &AppState, release: &Doc, project: &Doc) {
    use mongodb::bson::doc;
    let Some(db) = state.inner.db.mongo.as_ref() else { return };
    let run = async {
        db.collection::<mongodb::bson::Document>(access::RELEASES)
            .update_many(doc! {"_id": release.id}, doc! {"$inc": {"download_count": 1}})
            .await?;
        db.collection::<mongodb::bson::Document>(access::PROJECTS)
            .update_many(doc! {"_id": project.id}, doc! {"$inc": {"download_count": 1}})
            .await?;
        let event = doc! {
            "_id": mongodb::bson::oid::ObjectId::new(),
            "project_id": project.id,
            "release_id": release.id,
            "created_at": crate::dt::to_bson(crate::dt::now()),
        };
        db.collection::<mongodb::bson::Document>("mod_download_events").insert_one(event).await?;
        Ok::<_, mongodb::error::Error>(())
    };
    if let Err(e) = run.await {
        tracing::warn!("mods_hub: failed to record download: {e}");
    }
}

fn blueprint_reply(headers: &HeaderMap, r: Result<cache::Cached, BuildError>) -> Response {
    match r {
        Ok(c) => cache::respond(headers, &c, 300),
        Err(e) => e.into_response(),
    }
}

fn fmt_of(s: &str) -> Fmt {
    if s == "bin" { Fmt::Bin } else { Fmt::Json }
}

fn sound_reply(headers: &HeaderMap, r: Result<(Vec<u8>, String, String, String), ApiError>) -> Response {
    let (data, media_type, filename, etag) = match r {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let etag_h = match latin1_header(&etag) {
        Ok(h) => h,
        Err(e) => return e.into_response(),
    };
    let inm = headers.get(header::IF_NONE_MATCH).map(|v| crate::request_id::latin1(v.as_bytes()));
    if inm.as_deref() == Some(etag.as_str()) {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag_h)]).into_response();
    }
    let disposition = match latin1_header(&format!("inline; filename=\"{filename}\"")) {
        Ok(h) => h,
        Err(e) => return e.into_response(),
    };
    let ct = HeaderValue::from_str(&media_type).unwrap_or(HeaderValue::from_static("application/octet-stream"));
    (
        [
            (header::CONTENT_TYPE, ct),
            (header::ETAG, etag_h),
            (header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=86400")),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        data,
    )
        .into_response()
}

// --- /v1 routes ------------------------------------------------------------

type Gate = RequireFeature<ModsHub>;

pub async fn v1_release(
    State(s): State<AppState>,
    _: Gate,
    ModsPub(ctx): ModsPub,
    Path(Rid(id)): Path<Rid>,
) -> Response {
    let r = async {
        let (release, project) = release_with_project(&s, &id, None).await?;
        let mut m = release_dto(&release);
        m.insert("project_slug".into(), json!(project.str("slug")));
        m.insert("project_handle".into(), json!(project.str("owner_handle")));
        m.insert("project_title".into(), json!(project.str("title")));
        Ok(Value::Object(m))
    };
    v1(&ctx.rate_limit, r.await)
}

pub async fn v1_download(State(s): State<AppState>, _: Gate, _: ModsAsset, Path(Rid(id)): Path<Rid>) -> Response {
    download(&s, None, &id, None).await
}

pub async fn v1_blueprints(
    State(s): State<AppState>,
    _: Gate,
    ModsPub(ctx): ModsPub,
    Path(Rid(id)): Path<Rid>,
) -> Response {
    let r = async { list_blueprints(&s, &release_with_project(&s, &id, None).await?.0).await };
    v1(&ctx.rate_limit, r.await)
}

pub async fn v1_blueprint(
    State(s): State<AppState>,
    _: Gate,
    _: ModsAsset,
    headers: HeaderMap,
    Path(q): Path<RidBlueprint>,
) -> Response {
    let r = async {
        let (release, _) = release_with_project(&s, &q.id, None).await?;
        blueprint(&s, &release, &q.path, fmt_of(&q.fmt)).await
    };
    blueprint_reply(&headers, r.await)
}

pub async fn v1_files(State(s): State<AppState>, _: Gate, ModsPub(ctx): ModsPub, Path(Rid(id)): Path<Rid>) -> Response {
    let r = async { list_files(&s, &release_with_project(&s, &id, None).await?.0).await };
    v1(&ctx.rate_limit, r.await)
}

pub async fn v1_file(State(s): State<AppState>, _: Gate, _: ModsAsset, Path(q): Path<RidPath>) -> Response {
    let r = async { file(&s, &release_with_project(&s, &q.id, None).await?.0, &q.path).await };
    match r.await {
        Ok((data, name)) => attachment(data, "application/octet-stream", &name, None),
        Err(e) => e.into_response(),
    }
}

pub async fn v1_swfs(State(s): State<AppState>, _: Gate, ModsPub(ctx): ModsPub, Path(Rid(id)): Path<Rid>) -> Response {
    let r = async { list_swfs(&s, &release_with_project(&s, &id, None).await?.0).await };
    v1(&ctx.rate_limit, r.await)
}

pub async fn v1_swf_scripts(State(s): State<AppState>, req: Request) -> Response {
    let (mut parts, _) = req.into_parts();
    if let Err(e) = crate::features::check::<ModsHub>(&s).await {
        return e.into_response();
    }
    let ctx = match ModsPub::from_request_parts(&mut parts, &s).await {
        Ok(ModsPub(ctx)) => ctx,
        Err(r) => return r.into_response(),
    };
    let q = match Path::<RidPath>::from_request_parts(&mut parts, &s).await {
        Ok(Path(q)) => q,
        Err(e) => return e.into_response(),
    };
    if let Err(r) = decompile::throttle(&s, &parts).await {
        return r.into_response();
    }
    let r = async { swf_scripts(&s, &release_with_project(&s, &q.id, None).await?.0, &q.path).await };
    v1(&ctx.rate_limit, r.await)
}

pub async fn v1_inspect(
    State(s): State<AppState>,
    _: Gate,
    ModsPub(ctx): ModsPub,
    Path(Rid(id)): Path<Rid>,
) -> Response {
    let r = async { inspect(&s, &release_with_project(&s, &id, None).await?.0).await };
    v1(&ctx.rate_limit, r.await)
}

pub async fn v1_cfgs(State(s): State<AppState>, _: Gate, ModsPub(ctx): ModsPub, Path(Rid(id)): Path<Rid>) -> Response {
    let r = async { list_cfgs(&s, &release_with_project(&s, &id, None).await?.0).await };
    v1(&ctx.rate_limit, r.await)
}

pub async fn v1_cfg(State(s): State<AppState>, _: Gate, _: ModsAsset, Path(q): Path<RidPath>) -> Response {
    let r = async { cfg(&s, &release_with_project(&s, &q.id, None).await?.0, &q.path).await };
    match r.await {
        Ok((data, name)) => attachment(data, "text/plain; charset=utf-8", &name, None),
        Err(e) => e.into_response(),
    }
}

// --- /site routes ----------------------------------------------------------

type Viewer = OptionalSiteUser;

pub async fn site_download(
    State(s): State<AppState>,
    _: SiteMods,
    OptionalSiteUser(v): Viewer,
    Path(Rid(id)): Path<Rid>,
) -> Response {
    download(&s, v.as_ref(), &id, Some("no-cache")).await
}

pub async fn site_blueprints(
    State(s): State<AppState>,
    _: SiteMods,
    OptionalSiteUser(v): Viewer,
    Path(Rid(id)): Path<Rid>,
) -> Response {
    let r = async { list_blueprints(&s, &release_with_project(&s, &id, v.as_ref()).await?.0).await };
    site(Some(60), r.await)
}

pub async fn site_files(
    State(s): State<AppState>,
    _: SiteMods,
    OptionalSiteUser(v): Viewer,
    Path(Rid(id)): Path<Rid>,
) -> Response {
    let r = async { list_files(&s, &release_with_project(&s, &id, v.as_ref()).await?.0).await };
    site(None, r.await)
}

pub async fn site_file(
    State(s): State<AppState>,
    _: SiteMods,
    OptionalSiteUser(v): Viewer,
    Path(q): Path<RidPath>,
) -> Response {
    let r = async { file(&s, &release_with_project(&s, &q.id, v.as_ref()).await?.0, &q.path).await };
    match r.await {
        Ok((data, name)) => attachment(data, "application/octet-stream", &name, Some("no-cache")),
        Err(e) => e.into_response(),
    }
}

pub async fn site_inspect(
    State(s): State<AppState>,
    _: SiteMods,
    OptionalSiteUser(v): Viewer,
    Path(Rid(id)): Path<Rid>,
) -> Response {
    let r = async { inspect(&s, &release_with_project(&s, &id, v.as_ref()).await?.0).await };
    site(None, r.await)
}

pub async fn site_swfs(
    State(s): State<AppState>,
    _: SiteMods,
    OptionalSiteUser(v): Viewer,
    Path(Rid(id)): Path<Rid>,
) -> Response {
    let r = async { list_swfs(&s, &release_with_project(&s, &id, v.as_ref()).await?.0).await };
    site(Some(60), r.await)
}

pub async fn site_swf_scripts(State(s): State<AppState>, req: Request) -> Response {
    let (mut parts, _) = req.into_parts();
    let (v, q) = match site_prelude::<RidPath>(&s, &mut parts).await {
        Ok(x) => x,
        Err(r) => return r,
    };
    if let Err(r) = decompile::throttle(&s, &parts).await {
        return r.into_response();
    }
    let r = async { swf_scripts(&s, &release_with_project(&s, &q.id, v.as_ref()).await?.0, &q.path).await };
    site(Some(300), r.await)
}

async fn site_prelude<T: FromParams>(s: &AppState, parts: &mut Parts) -> Result<(Option<SiteUser>, T), Response> {
    SiteMods::from_request_parts(parts, s).await?;
    let OptionalSiteUser(v) =
        OptionalSiteUser::from_request_parts(parts, s).await.map_err(IntoResponse::into_response)?;
    let Path(q) = Path::<T>::from_request_parts(parts, s).await.map_err(IntoResponse::into_response)?;
    Ok((v, q))
}

pub async fn site_cfgs(
    State(s): State<AppState>,
    _: SiteMods,
    OptionalSiteUser(v): Viewer,
    Path(Rid(id)): Path<Rid>,
) -> Response {
    let r = async { list_cfgs(&s, &release_with_project(&s, &id, v.as_ref()).await?.0).await };
    site(None, r.await)
}

pub async fn site_cfg(
    State(s): State<AppState>,
    _: SiteMods,
    OptionalSiteUser(v): Viewer,
    Path(q): Path<RidPath>,
) -> Response {
    let r = async { cfg(&s, &release_with_project(&s, &q.id, v.as_ref()).await?.0, &q.path).await };
    match r.await {
        Ok((data, name)) => attachment(data, "text/plain; charset=utf-8", &name, Some("no-cache")),
        Err(e) => e.into_response(),
    }
}

pub async fn site_blueprint(
    State(s): State<AppState>,
    _: SiteMods,
    OptionalSiteUser(v): Viewer,
    headers: HeaderMap,
    Path(q): Path<RidBlueprint>,
) -> Response {
    let r = async {
        let (release, _) = release_with_project(&s, &q.id, v.as_ref()).await?;
        blueprint(&s, &release, &q.path, fmt_of(&q.fmt)).await
    };
    blueprint_reply(&headers, r.await)
}

pub async fn site_audio(
    State(s): State<AppState>,
    _: SiteMods,
    OptionalSiteUser(v): Viewer,
    Path(Rid(id)): Path<Rid>,
) -> Response {
    let r = async { list_banks(&s, &release_with_project(&s, &id, v.as_ref()).await?.0).await };
    site(Some(60), r.await)
}

pub async fn site_audio_bank(
    State(s): State<AppState>,
    _: SiteMods,
    OptionalSiteUser(v): Viewer,
    Path(q): Path<RidPath>,
) -> Response {
    let r = async { bank(&s, &release_with_project(&s, &q.id, v.as_ref()).await?.0, &q.path).await };
    site(Some(300), r.await)
}

pub async fn site_audio_sound(
    State(s): State<AppState>,
    _: SiteMods,
    OptionalSiteUser(v): Viewer,
    headers: HeaderMap,
    Path(q): Path<RidSound>,
) -> Response {
    let r = async { sound(&s, &release_with_project(&s, &q.id, v.as_ref()).await?.0, &q.path, q.sound, q.raw).await };
    sound_reply(&headers, r.await)
}

pub async fn site_vfx(
    State(s): State<AppState>,
    _: SiteMods,
    OptionalSiteUser(v): Viewer,
    Path(Rid(id)): Path<Rid>,
) -> Response {
    let r = async { super::vfx::list(&s, &release_with_project(&s, &id, v.as_ref()).await?.0).await };
    site(Some(60), r.await)
}

pub async fn site_vfx_manifest(
    State(s): State<AppState>,
    _: SiteMods,
    OptionalSiteUser(v): Viewer,
    Path(q): Path<RidPath>,
) -> Response {
    let r = async { super::vfx::manifest(&s, &release_with_project(&s, &q.id, v.as_ref()).await?.0, &q.path).await };
    site(Some(120), r.await)
}

pub async fn site_vfx_asset(
    State(s): State<AppState>,
    _: SiteMods,
    OptionalSiteUser(v): Viewer,
    Path(q): Path<RidPath>,
) -> Response {
    let r = async { super::vfx::asset(&s, &release_with_project(&s, &q.id, v.as_ref()).await?.0, &q.path).await };
    match r.await {
        Ok((data, media_type)) => (
            [
                (header::CONTENT_TYPE, HeaderValue::from_static(media_type)),
                (header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=3600")),
            ],
            data,
        )
            .into_response(),
        Err(e) => e.into_response(),
    }
}
