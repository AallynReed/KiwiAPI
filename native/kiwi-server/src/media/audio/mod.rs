//! `app/trove/audio/service.py` and its `/site` routes: a bank indexed once
//! into a cached sound list (each `.wem` filed in the mods content store),
//! sounds decoded on demand, the playable ones zipped, and Sound Studio's
//! stateless rebuild (`POST /site/sound-studio/build`).

pub mod bank;
pub mod edit;
pub mod encode;
pub mod names;
pub mod pyval;
pub mod studio;

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::OnceLock,
};

use axum::{
    Router,
    body::Body,
    extract::{FromRequestParts, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use kiwi_core::{
    audio::{ConvertError, Converted},
    form::{self, FormError, FormValue},
};
use serde_json::{Map, Value, json};

use super::{SiteGate, bpcache, cas::ContentStore, check_branch, file_sha, latin1_header, read_blob, sanitize};
use crate::{
    admin::Args,
    error::ApiError,
    pyfloat, pyjson,
    pyjson::PyJson,
    state::AppState,
    validation::{Fail as VFail, FromParams, Params, ValidationError, Validator},
};

pub const MAX_ZIP_BYTES: usize = 512 * 1024 * 1024;
pub const CODEBOOKS_FILE: &str = "app/trove/audio/packed_codebooks_aoTuV_603.bin";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/site/updates/{branch}/file/bnk", get(site_bnk))
        .route("/site/updates/{branch}/file/bnk/audio", get(site_bnk_audio))
        .route("/site/updates/{branch}/file/bnk/zip", get(site_bnk_zip))
        .route("/site/sound-studio/build", post(site_sound_studio_build))
}

/// `wwise_vorbis.library()`: the packed codebooks, or the text of the
/// `VorbisError` a missing file raises.
pub fn codebooks() -> Result<&'static [u8], &'static str> {
    static LIB: OnceLock<Result<Vec<u8>, String>> = OnceLock::new();
    LIB.get_or_init(|| {
        let root = if cfg!(test) {
            Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."))
        } else {
            std::env::current_dir()
        };
        let path = root.map(|d| d.join(CODEBOOKS_FILE)).unwrap_or_else(|_| PathBuf::from(CODEBOOKS_FILE));
        std::fs::read(&path).map_err(|e| {
            let errno = e.raw_os_error().unwrap_or(2);
            format!("codebook library is unavailable: [Errno {errno}] {e}: '{}'", path.display())
        })
    })
    .as_ref()
    .map(Vec::as_slice)
    .map_err(String::as_str)
}

/// `wem.convert` with the shared library.
pub fn convert(raw: &[u8]) -> Result<Converted, ConvertError> {
    kiwi_core::audio::convert_detailed(raw, codebooks())
}

/// Why a manifest build failed: `BankError` (422) or anything else (500).
pub enum BuildFail {
    Bank(String),
    Crash(String),
}

/// `_build_sync(raw, sidecar)`.
pub fn build_manifest(raw: &[u8], sidecar: Option<&str>, store: &ContentStore) -> Result<Value, BuildFail> {
    let parsed = bank::parse(raw).map_err(BuildFail::Bank)?;
    let (names, events) = match sidecar.filter(|s| !s.is_empty()) {
        Some(text) => names::parse(text),
        None => (HashMap::new(), 0),
    };
    let played = parsed.sound_for_media();
    let mut sounds: Vec<Map<String, Value>> = Vec::with_capacity(parsed.media.len());
    for entry in &parsed.media {
        let data = parsed.media_bytes(entry).map_err(BuildFail::Bank)?;
        let (sha, _) = store.put(data).map_err(|e| BuildFail::Crash(e.to_string()))?;
        let named = names.get(&entry.media_id.to_string());
        let text = |f: fn(&names::SoundName) -> &String| named.map_or(String::new(), |n| f(n).clone());
        let mut r = Map::new();
        r.insert("id".into(), entry.media_id.into());
        r.insert("name".into(), named.map_or(Value::Null, |n| n.name.clone().into()));
        r.insert("group".into(), text(|n| &n.group).into());
        r.insert("path".into(), text(|n| &n.path).into());
        r.insert("source".into(), text(|n| &n.source).into());
        r.insert("notes".into(), text(|n| &n.notes).into());
        r.insert("bytes".into(), entry.size.into());
        r.insert("sha".into(), sha.into());
        let object = played.iter().find(|(k, _)| *k == entry.media_id).map(|(_, o)| o.object_id);
        r.insert("object_id".into(), object.map_or(Value::Null, Value::from));
        match kiwi_core::audio::parse(data) {
            Ok(info) => {
                r.insert("codec".into(), info.codec_name().into());
                r.insert("channels".into(), info.channels.into());
                r.insert("sample_rate".into(), info.rate.into());
                r.insert("duration".into(), json!(pyfloat::round(info.duration(), 3)));
                r.insert("error".into(), Value::Null);
            }
            Err(e) => {
                r.insert("codec".into(), Value::Null);
                r.insert("channels".into(), 0.into());
                r.insert("sample_rate".into(), 0.into());
                r.insert("duration".into(), json!(0.0));
                r.insert("error".into(), e.into());
            }
        }
        sounds.push(r);
    }
    sounds.sort_by_cached_key(|s| {
        let name = s.get("name").and_then(Value::as_str).unwrap_or("").to_lowercase();
        if name.is_empty() { format!("~{}", s.get("id").and_then(Value::as_u64).unwrap_or(0)) } else { name }
    });
    let playable = sounds.iter().filter(|s| s.get("error").is_some_and(Value::is_null)).count();
    let durations: Vec<f64> = sounds.iter().map(|s| s.get("duration").and_then(Value::as_f64).unwrap_or(0.0)).collect();
    let total =
        if durations.is_empty() { json!(0) } else { json!(pyfloat::round(kiwi_core::cheaters::py_sum(durations), 1)) };
    let count = sounds.len();
    Ok(json!({
        "bank": {
            "version": parsed.version,
            "bank_id": parsed.bank_id,
            "sections": parsed.sections.iter().map(|s| s.tag.clone()).collect::<Vec<_>>(),
            "objects": parsed.objects.len(),
            "events": events,
        },
        "sounds": sounds,
        "count": count,
        "playable": playable,
        "total_duration": total,
    }))
}

/// `service.manifest(raw, sha, sidecar, sidecar_sha)`.
pub async fn manifest(
    state: &AppState,
    raw: Vec<u8>,
    sha: &str,
    sidecar: Option<String>,
    sidecar_sha: Option<&str>,
) -> Result<Value, ApiError> {
    let store = super::mods_store(state);
    let label = sha.chars().take(12).collect::<String>();
    bpcache::get_or_build(state, &bpcache::key_for_bank(sha, sidecar_sha), || async move {
        let built = tokio::task::spawn_blocking(move || build_manifest(&raw, sidecar.as_deref(), &store)).await;
        match built {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(BuildFail::Bank(e))) => {
                tracing::info!("audio: cannot read bank {label}: {e}");
                Err(ApiError::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "bad_request",
                    "This file could not be read as a Wwise sound bank.",
                ))
            }
            Ok(Err(BuildFail::Crash(e))) => {
                tracing::error!("audio: indexing {label} failed: {e}");
                Err(ApiError::internal())
            }
            Err(_) => Err(ApiError::internal()),
        }
    })
    .await
}

/// `_bank_manifest(branch, path)`: the index plus the bank's sha; the
/// sidecar `.txt` (and its sha, in the cache key) when the archive has it.
async fn bank_manifest(state: &AppState, branch: &str, path: &str) -> Result<(Value, String), ApiError> {
    check_branch(branch)?;
    if !path.to_lowercase().ends_with(".bnk") {
        return Err(ApiError::http(StatusCode::BAD_REQUEST, Some("Not a .bnk file")));
    }
    let Some(sha) = file_sha(state, branch, path).await? else {
        return Err(ApiError::http(StatusCode::NOT_FOUND, Some(&format!("No file '{path}'"))));
    };
    let store = super::update_store(state);
    let Some(raw) = read_blob(store.clone(), sha.clone()).await? else {
        return Err(ApiError::http(StatusCode::NOT_FOUND, Some("Blob missing from the store")));
    };
    let cut = path.char_indices().rev().nth(3).map_or(0, |(i, _)| i);
    let side_path = format!("{}.txt", &path[..cut]);
    let (mut sidecar, mut sidecar_sha) = (None, None);
    if let Some(s) = file_sha(state, branch, &side_path).await?
        && let Some(blob) = read_blob(store, s.clone()).await?
    {
        sidecar = Some(String::from_utf8_lossy(&blob).into_owned());
        sidecar_sha = Some(s);
    }
    Ok((manifest(state, raw, &sha, sidecar, sidecar_sha.as_deref()).await?, sha))
}

struct FileQ {
    branch: String,
    path: String,
}

impl FromParams for FileQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        FileQ { branch: p.path("branch").str().required(), path: p.query("path").str().required() }
    }
}

struct AudioQ {
    branch: String,
    path: String,
    id: i64,
    raw: bool,
}

impl FromParams for AudioQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        AudioQ {
            branch: p.path("branch").str().required(),
            path: p.query("path").str().required(),
            id: p.query("id").int().ge(0).le(0xFFFF_FFFFi64).required(),
            raw: p.query("raw").bool().default(false),
        }
    }
}

fn public_max_age(secs: u32) -> HeaderValue {
    HeaderValue::from_str(&format!("public, max-age={secs}")).unwrap_or(HeaderValue::from_static("no-store"))
}

fn sounds_of(payload: &Value) -> Vec<Value> {
    payload.get("sounds").and_then(Value::as_array).cloned().unwrap_or_default()
}

async fn site_bnk(_: SiteGate, State(state): State<AppState>, Args(q): Args<FileQ>) -> Result<Response, ApiError> {
    let (payload, sha) = bank_manifest(&state, &q.branch, &q.path).await?;
    let sounds: Vec<Value> = sounds_of(&payload)
        .into_iter()
        .map(|s| match s {
            Value::Object(m) => Value::Object(m.into_iter().filter(|(k, _)| k != "sha").collect()),
            other => other,
        })
        .collect();
    let count = sounds.len();
    let body = json!({
        "branch": q.branch,
        "path": q.path,
        "content_sha256": sha,
        "bank": payload.get("bank").cloned().unwrap_or(Value::Null),
        "sounds": sounds,
        "count": count,
        "playable": payload.get("playable").cloned().unwrap_or(Value::Null),
        "total_duration": payload.get("total_duration").cloned().unwrap_or(Value::Null),
    });
    Ok(([(header::CACHE_CONTROL, public_max_age(300))], PyJson(body)).into_response())
}

fn id_matches(s: &Value, id: i64) -> Result<bool, ApiError> {
    match s.get("id") {
        Some(Value::Bool(b)) => Ok(i64::from(*b) == id),
        Some(v) => Ok(v.as_i64() == Some(id) || v.as_f64() == Some(id as f64)),
        None => Err(ApiError::internal()),
    }
}

/// A non-empty sound name, else `sound_<id>`.
fn sound_stem(s: &Value, id: impl std::fmt::Display) -> String {
    match s.get("name") {
        Some(Value::String(n)) if !n.is_empty() => n.clone(),
        _ => format!("sound_{id}"),
    }
}

async fn site_bnk_audio(
    _: SiteGate,
    State(state): State<AppState>,
    headers: HeaderMap,
    Args(q): Args<AudioQ>,
) -> Result<Response, ApiError> {
    let (payload, _) = bank_manifest(&state, &q.branch, &q.path).await?;
    let sounds = sounds_of(&payload);
    let mut found = None;
    for s in &sounds {
        if id_matches(s, q.id)? {
            found = Some(s);
            break;
        }
    }
    let Some(sound) = found else {
        return Err(ApiError::http(StatusCode::NOT_FOUND, Some(&format!("No sound {} in '{}'", q.id, q.path))));
    };
    let sha = sound.get("sha").and_then(Value::as_str).ok_or_else(ApiError::internal)?.to_owned();
    let etag = latin1_header(&format!("\"{sha}{}\"", if q.raw { "-raw" } else { "" }))?;
    if headers.get(header::IF_NONE_MATCH).is_some_and(|v| v.as_bytes() == etag.as_bytes()) {
        return Ok((StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response());
    }
    let (data, media, ext) = if q.raw {
        let Some(data) = read_blob(super::mods_store(&state), sha).await? else {
            return Err(ApiError::http(StatusCode::NOT_FOUND, Some("Sound blob missing from the store")));
        };
        (data, "audio/vnd.wave", "wem")
    } else {
        if let Some(err) = sound.get("error").filter(|e| super::swf::truthy(Some(e))) {
            let detail = match err {
                Value::String(s) => s.clone(),
                other => pyjson::dumps(other),
            };
            return Err(ApiError::http(StatusCode::UNPROCESSABLE_ENTITY, Some(&detail)));
        }
        let Some(blob) = read_blob(super::mods_store(&state), sha).await? else {
            return Err(ApiError::http(StatusCode::NOT_FOUND, Some("Sound blob missing from the store")));
        };
        match tokio::task::spawn_blocking(move || convert(&blob)).await.map_err(|_| ApiError::internal())? {
            Ok(c) => (c.bytes, c.mime, c.ext),
            Err(ConvertError::Wem(e)) => {
                return Err(ApiError::new(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "bad_request",
                    format!("This sound could not be decoded: {e}"),
                ));
            }
            Err(ConvertError::Crash(e)) => {
                tracing::error!("audio: decode crashed: {e}");
                return Err(ApiError::internal());
            }
        }
    };
    let stem = sanitize(&sound_stem(sound, q.id), "._-");
    let stem = if stem.is_empty() { "sound".to_owned() } else { stem };
    Ok((
        [
            (header::CONTENT_TYPE, HeaderValue::from_static(media)),
            (header::ETAG, etag),
            (header::CACHE_CONTROL, public_max_age(86400)),
            (header::CONTENT_DISPOSITION, latin1_header(&format!("inline; filename=\"{stem}.{ext}\""))?),
        ],
        data,
    )
        .into_response())
}

/// `_zip_name(sound, extension, taken)`.
fn zip_name(sound: &Value, ext: &str, taken: &mut HashSet<String>) -> String {
    let id = sound.get("id").map_or_else(String::new, |v| match v {
        Value::String(s) => s.clone(),
        other => pyjson::dumps(other),
    });
    let clean = |s: &str| kiwi_core::pystr::strip(&sanitize(s, "._- ")).to_owned();
    let stem = clean(&sound_stem(sound, id));
    let stem = if stem.is_empty() { "sound".to_owned() } else { stem };
    let group = match sound.get("group") {
        Some(Value::String(g)) => clean(g),
        _ => String::new(),
    };
    let base = if group.is_empty() { stem } else { format!("{group}/{stem}") };
    let mut name = format!("{base}.{ext}");
    let mut n = 2;
    while taken.contains(&name) {
        name = format!("{base}_{n}.{ext}");
        n += 1;
    }
    taken.insert(name.clone());
    name
}

/// `build_zip(sounds)`.
pub fn build_zip(sounds: &[Value], store: &ContentStore) -> Result<Vec<u8>, ApiError> {
    let mut zip = super::zipfile::ZipWriter::new();
    let mut taken = HashSet::new();
    let mut total = 0usize;
    for sound in sounds {
        if super::swf::truthy(sound.get("error")) {
            continue;
        }
        let sha = sound.get("sha").and_then(Value::as_str).ok_or_else(ApiError::internal)?;
        let Some(data) = store.get(sha).map_err(|_| ApiError::internal())? else { continue };
        let c = match convert(&data) {
            Ok(c) => c,
            Err(ConvertError::Wem(_)) => continue,
            Err(ConvertError::Crash(_)) => return Err(ApiError::internal()),
        };
        total += c.bytes.len();
        if total > MAX_ZIP_BYTES {
            return Err(ApiError::new(
                StatusCode::PAYLOAD_TOO_LARGE,
                "bad_request",
                "These sounds are too large to download as one archive.",
            ));
        }
        let name = zip_name(sound, c.ext, &mut taken);
        zip.writestr(&name, &c.bytes);
    }
    Ok(zip.finish())
}

async fn site_bnk_zip(_: SiteGate, State(state): State<AppState>, Args(q): Args<FileQ>) -> Result<Response, ApiError> {
    let (payload, sha) = bank_manifest(&state, &q.branch, &q.path).await?;
    let sounds = sounds_of(&payload);
    if !sounds.iter().any(|s| s.get("error").is_none_or(Value::is_null)) {
        return Err(ApiError::http(StatusCode::NOT_FOUND, Some("This bank has no playable sounds")));
    }
    let store = super::mods_store(&state);
    let blob =
        tokio::task::spawn_blocking(move || build_zip(&sounds, &store)).await.map_err(|_| ApiError::internal())??;
    let stem = super::file_stem(&q.path, "sounds");
    Ok((
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("application/zip")),
            (header::CONTENT_DISPOSITION, latin1_header(&format!("attachment; filename=\"{stem}-sounds.zip\""))?),
            (header::ETAG, latin1_header(&format!("\"{sha}-zip\""))?),
            (header::CACHE_CONTROL, public_max_age(3600)),
        ],
        blob,
    )
        .into_response())
}

/// `jsonable_encoder(upload)` as a 422 echoes it: `vars(UploadFile)`, with the
/// spooled file as `dict()` of its lines (each line must be a byte pair; any
/// other content takes FastAPI down its `vars()` path, approximated here).
fn upload_input(f: &kiwi_core::form::UploadFile) -> Value {
    let mut lines = Vec::new();
    let mut start = 0;
    for (i, b) in f.data.iter().enumerate() {
        if *b == b'\n' {
            lines.push(&f.data[start..=i]);
            start = i + 1;
        }
    }
    if start < f.data.len() {
        lines.push(&f.data[start..]);
    }
    let file = if lines.iter().all(|l| l.len() == 2) {
        let mut m = Map::new();
        for l in lines {
            m.insert(l[0].to_string(), Value::from(l[1]));
        }
        Value::Object(m)
    } else {
        json!({"_file": {}, "_max_size": 1048576, "_rolled": false})
    };
    let mut headers = Map::new();
    for (k, v) in &f.headers {
        headers.entry(form::latin1(k)).or_insert_with(|| Value::String(form::latin1(v)));
    }
    json!({"filename": f.filename, "file": file, "size": f.data.len(), "headers": headers, "_max_mem_size": 1048576})
}

/// Uploaded clip parts as (filename, bytes).
pub type Clips = Vec<(String, Vec<u8>)>;

/// The form of `site_sound_studio_build(spec: str = Form(...), clips:
/// list[UploadFile] = File(default=[]))` as FastAPI validates it.
pub fn studio_form(items: Vec<(String, FormValue)>) -> Result<(String, Clips), ValidationError> {
    let mut v = Validator::default();
    let spec = match items.iter().rev().find(|(k, _)| k == "spec").map(|(_, v)| v) {
        Some(FormValue::Text(t)) if !t.is_empty() => t.clone(),
        Some(FormValue::File(f)) => {
            v.push(vec!["body".into(), "spec".into()], upload_input(f), VFail::string_type());
            String::new()
        }
        _ => {
            v.push(vec!["body".into(), "spec".into()], Value::Null, VFail::missing());
            String::new()
        }
    };
    let mut clips = Vec::new();
    for (i, (_, value)) in items.into_iter().filter(|(k, _)| k == "clips").enumerate() {
        match value {
            FormValue::File(f) => clips.push((f.filename, f.data)),
            FormValue::Text(t) => v.push(
                vec!["body".into(), "clips".into(), i.into()],
                Value::String(t),
                VFail::value_error("Expected UploadFile, received: <class 'str'>"),
            ),
        }
    }
    v.finish((spec, clips))
}

fn body_error(msg: &str) -> Response {
    ApiError::http(StatusCode::BAD_REQUEST, Some(msg)).into_response()
}

fn api_err(code: StatusCode, msg: &str) -> ApiError {
    ApiError::new(code, "bad_request", msg)
}

/// `POST /site/sound-studio/build`.
async fn site_sound_studio_build(State(state): State<AppState>, req: Request<Body>) -> Response {
    let (mut parts, body) = req.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, usize::MAX).await else {
        return body_error("There was an error parsing the body");
    };
    let items = match form::parse_form(parts.headers.get(header::CONTENT_TYPE).map(|v| v.as_bytes()), &bytes) {
        Ok(items) => items,
        Err(FormError::Http(msg)) => return body_error(&msg),
    };
    drop(bytes);
    if let Err(r) = SiteGate::from_request_parts(&mut parts, &state).await {
        return r;
    }
    let (spec, clips) = match studio_form(items) {
        Ok(x) => x,
        Err(e) => return e.into_response(),
    };
    match studio_build(&state, &spec, clips).await {
        Ok(r) => r,
        Err(e) => e.into_response(),
    }
}

async fn studio_build(state: &AppState, spec: &str, clips: Clips) -> Result<Response, ApiError> {
    let parsed = match pyval::loads(spec) {
        Ok(v) => v,
        Err(pyval::LoadErr::Invalid) => {
            return Err(api_err(StatusCode::BAD_REQUEST, "The change list was not valid JSON."));
        }
        Err(pyval::LoadErr::TooDeep) => return Err(ApiError::internal()),
    };
    if !matches!(parsed, pyval::PyVal::Dict(_)) {
        return Err(api_err(StatusCode::BAD_REQUEST, "The change list was not understood."));
    }
    let branch = pyval::str_or(parsed.get("branch"), "");
    let path = pyval::str_or(parsed.get("path"), "");
    check_branch(&branch)?;
    if !path.to_lowercase().ends_with(".bnk") {
        return Err(api_err(StatusCode::BAD_REQUEST, "Only a .bnk file can be edited."));
    }
    let Some(sha) = file_sha(state, &branch, &path).await? else {
        return Err(ApiError::http(StatusCode::NOT_FOUND, Some(&format!("No file '{path}'"))));
    };
    let Some(raw) = read_blob(super::update_store(state), sha).await? else {
        return Err(ApiError::http(StatusCode::NOT_FOUND, Some("Blob missing from the store")));
    };
    let uploads: HashMap<String, Vec<u8>> = clips.into_iter().filter(|(name, _)| !name.is_empty()).collect();
    let (blob, filename, replaced, added) = tokio::task::spawn_blocking(move || {
        let result = studio::apply_edits(&raw, &parsed, &uploads)?;
        let (replaced, added): (usize, Vec<String>) =
            (result.replaced, result.added.iter().map(|a| a.event.clone()).collect());
        let (blob, filename) = studio::package(result, &path, &parsed)?;
        Ok::<_, ApiError>((blob, filename, replaced, added))
    })
    .await
    .map_err(|_| ApiError::internal())??;
    let added = pyjson::dumps_spaced(&Value::from(added));
    Ok((
        [
            (header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream")),
            (header::CONTENT_DISPOSITION, latin1_header(&format!("attachment; filename=\"{filename}\""))?),
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
        ],
        [
            (header::HeaderName::from_static("x-kiwi-sounds-replaced"), HeaderValue::from(replaced)),
            (header::HeaderName::from_static("x-kiwi-sounds-added"), latin1_header(&added)?),
        ],
        blob,
    )
        .into_response())
}
