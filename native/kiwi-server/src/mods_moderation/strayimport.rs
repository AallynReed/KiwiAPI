//! `mods_hub/strayimport.py`: mirror the external mod catalog into the hub as
//! stray mods. A run is started by a master, is resumable and idempotent by
//! `source_id`, and keeps its progress in the `stray_import_state` singleton.

use std::time::Duration;

use chrono::{DateTime, Utc};
use kiwi_core::pystr;
use mongodb::bson::{Bson, Document, doc};
use serde_json::{Value, json};

use super::{
    model::{IMAGE_ASSET, MOD_PROJECT, MOD_RELEASE, Model, STRAY_STATE, internal},
    service::{self, MOD_IMAGES, MOD_PROJECTS, MOD_RELEASES, Res, STRAY_HANDLE, coll, find_model, now},
};
use crate::{auth::models::SiteUser, auth::pyutil::bson_int, dt, media::cas::ContentStore, pyfloat, state::AppState};

macro_rules! origin {
    () => {
        concat!("trove", "saurus")
    };
}

pub const SOURCE: &str = origin!();
pub const CATALOG_URL: &str = concat!("https://", origin!(), ".com/api/mods-all");
const FILE_URL: &str = concat!("https://", origin!(), ".com/client/downloadfile.php?fileid=");
const MOD_PAGE_URL: &str = concat!("https://", origin!(), ".com/mods/");
const USER_AGENT: &str = "BetterTroveTools-API/1.0 (+https://trove.aallyn.net; mod mirror)";
const STATE: &str = "stray_import_state";

const MAX_FILE_BYTES: usize = 80 * 1024 * 1024;
const PROGRESS_EVERY: i64 = 25;
const STALE_RUN_SECONDS: f64 = 3.0 * 3600.0;

// --- state ---------------------------------------------------------------------------

/// `_state()`: the singleton, created on first read.
pub async fn load_state(state: &AppState) -> Res<Model> {
    let c = coll(state, STATE);
    if let Some(st) = find_model(&c, STRAY_STATE, doc! {"key": SOURCE}).await? {
        return Ok(st);
    }
    let st = Model::build(STRAY_STATE, doc! {"key": SOURCE})?;
    st.insert(&c).await?;
    Ok(st)
}

pub fn state_dto(st: &Model) -> Value {
    json!({
        "running": st.bool("running"), "phase": st.str("phase"), "total": st.int("total"),
        "processed": st.int("processed"), "imported": st.int("imported"), "updated": st.int("updated"),
        "pending_added": st.int("pending_added"), "failed": st.int("failed"),
        "last_error": st.json_str_opt("last_error"),
        "started_at": st.iso("started_at"),
        "finished_at": st.iso("finished_at"),
    })
}

/// `start(...)`: a refusal while a fresh run holds the flag, else the job in the background.
pub async fn start(
    state: &AppState,
    resync: bool,
    force: bool,
    author_id: Option<String>,
    owner: Option<SiteUser>,
) -> Res<Value> {
    let st = load_state(state).await?;
    if still_running(&st, force, now()) {
        let mut out = json!({"started": false, "reason": "An import is already running."});
        if let (Value::Object(o), Value::Object(s)) = (&mut out, state_dto(&st)) {
            o.extend(s);
        }
        return Ok(out);
    }
    let resync = resync && author_id.is_none();
    let (st2, author) = (state.clone(), author_id.clone());
    tokio::spawn(async move {
        if let Err(e) = run(&st2, resync, author.as_deref(), owner.as_ref()).await {
            tracing::error!("stray import state write failed: {e}");
        }
    });
    Ok(json!({"started": true, "resync": resync, "author_id": author_id}))
}

/// A run holds the flag unless forced or older than the stale limit.
pub fn still_running(st: &Model, force: bool, at: DateTime<Utc>) -> bool {
    if !st.bool("running") || force {
        return false;
    }
    let age = st.dt("started_at").map_or(1e9, |s| (at - s).num_microseconds().unwrap_or(i64::MAX) as f64 / 1e6);
    age < STALE_RUN_SECONDS
}

// --- Python value semantics ---------------------------------------------------------------

pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_f64() => "float",
        Value::Number(_) => "int",
        Value::String(_) => "str",
        Value::Array(_) => "list",
        Value::Object(_) => "dict",
    }
}

fn repr(v: &Value) -> String {
    match v {
        Value::String(s) => pystr::repr(s),
        other => py_str(other),
    }
}

/// `str(v)`.
pub fn py_str(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => match (n.as_i64(), n.as_u64()) {
            (Some(i), _) => i.to_string(),
            (_, Some(u)) => u.to_string(),
            _ => pyfloat::repr(n.as_f64().unwrap_or(f64::NAN)),
        },
        Value::String(s) => s.clone(),
        Value::Array(a) => format!("[{}]", a.iter().map(repr).collect::<Vec<_>>().join(", ")),
        Value::Object(m) => format!(
            "{{{}}}",
            m.iter().map(|(k, v)| format!("{}: {}", pystr::repr(k), repr(v))).collect::<Vec<_>>().join(", ")
        ),
    }
}

/// `int(v)`; `Err` where Python raises.
pub fn py_int(v: &Value) -> Result<i64, String> {
    match v {
        Value::Bool(b) => Ok(i64::from(*b)),
        Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => Ok(i),
            (_, Some(f)) if f.is_finite() => Ok(f.trunc() as i64),
            _ => Err("cannot convert float to integer".into()),
        },
        Value::String(s) => kiwi_core::pynum::int(s)
            .ok_or_else(|| format!("invalid literal for int() with base 10: {}", pystr::repr(s))),
        other => Err(format!(
            "int() argument must be a string, a bytes-like object or a real number, not '{}'",
            type_name(other)
        )),
    }
}

/// `(v or default).strip()` for a field expected to be text.
fn text_or(v: Option<&Value>, default: &str) -> Result<String, String> {
    match v {
        Some(v) if truthy(v) => match v {
            Value::String(s) => Ok(pystr::strip(s).to_owned()),
            other => Err(format!("'{}' object has no attribute 'strip'", type_name(other))),
        },
        _ => Ok(pystr::strip(default).to_owned()),
    }
}

/// `int(v or 0)`.
fn int_or_zero(v: Option<&Value>) -> Result<i64, String> {
    match v {
        Some(v) if truthy(v) => py_int(v),
        _ => Ok(0),
    }
}

/// `str(v or "")`.
fn str_or_empty(v: Option<&Value>) -> String {
    v.filter(|v| truthy(v)).map(py_str).unwrap_or_default()
}

pub fn json_to_bson(v: &Value) -> Bson {
    match v {
        Value::Null => Bson::Null,
        Value::Bool(b) => Bson::Boolean(*b),
        Value::Number(n) => match n.as_i64() {
            Some(i) => bson_int(i),
            None => Bson::Double(n.as_f64().unwrap_or(f64::NAN)),
        },
        Value::String(s) => Bson::String(s.clone()),
        Value::Array(a) => Bson::Array(a.iter().map(json_to_bson).collect()),
        Value::Object(m) => {
            let mut d = Document::new();
            for (k, x) in m {
                d.insert(k.clone(), json_to_bson(x));
            }
            Bson::Document(d)
        }
    }
}

// --- helpers ----------------------------------------------------------------------------

/// `_latest_download`: the highest `fileid`, skipping `extra` files.
pub fn latest_download(downloads: &Value) -> Result<Option<&serde_json::Map<String, Value>>, String> {
    let items: Vec<&Value> = match downloads {
        Value::Array(a) => a.iter().collect(),
        Value::Object(_) | Value::String(_) => Vec::new(),
        v if !truthy(v) => Vec::new(),
        other => return Err(format!("'{}' object is not iterable", type_name(other))),
    };
    let mut valid = Vec::new();
    for d in items {
        let Value::Object(m) = d else { continue };
        let extra = m.get("extra").filter(|v| truthy(v));
        if let Some(e) = extra
            && let Ok(n) = py_int(e)
            && n != 0
        {
            continue;
        }
        valid.push(m);
    }
    let mut keyed = Vec::with_capacity(valid.len());
    for m in valid {
        keyed.push((-int_or_zero(m.get("fileid"))?, m));
    }
    keyed.sort_by_key(|(k, _)| *k);
    Ok(keyed.into_iter().next().map(|(_, m)| m))
}

/// `_upload_time`: `Err` where `fromtimestamp` raises.
pub fn upload_time(dl: &serde_json::Map<String, Value>) -> Result<Option<DateTime<Utc>>, String> {
    let Ok(ts) = int_or_zero(dl.get("date")) else { return Ok(None) };
    if ts <= 0 {
        return Ok(None);
    }
    DateTime::from_timestamp(ts, 0)
        .filter(|d| chrono::Datelike::year(d) <= 9999)
        .map(Some)
        .ok_or_else(|| "year is out of range".to_owned())
}

/// `_clean_tags`.
pub fn clean_tags(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in tags {
        let low = pystr::strip(t).to_lowercase();
        let mut collapsed = String::with_capacity(low.len());
        let mut in_space = false;
        for c in low.chars() {
            if pystr::is_space(c) {
                if !in_space {
                    collapsed.push(' ');
                }
                in_space = true;
            } else {
                collapsed.push(c);
                in_space = false;
            }
        }
        let t = pystr::prefix(&collapsed, 30).to_owned();
        if !t.is_empty() && !out.contains(&t) {
            out.push(t);
        }
    }
    out.truncate(12);
    out
}

/// A downloaded catalog file, parsed.
pub struct Parsed {
    pub fileid: String,
    pub fmt: String,
    pub data: Vec<u8>,
    pub tag: String,
    pub changelog: Value,
    pub published_at: Option<DateTime<Utc>>,
    pub props: serde_json::Map<String, Value>,
    pub title_prop: String,
    pub tmod_author: Option<String>,
}

/// The `.tmod` header properties (`{}` for anything unreadable).
pub fn tmod_props(data: &[u8]) -> serde_json::Map<String, Value> {
    match crate::tools::tmod::read(data, true) {
        Ok(Value::Object(mut o)) => match o.remove("properties") {
            Some(Value::Object(p)) => p,
            _ => serde_json::Map::new(),
        },
        _ => serde_json::Map::new(),
    }
}

/// `_fetch_release` once the file is in hand.
pub fn parse_release(
    dl: &serde_json::Map<String, Value>,
    fileid: String,
    fmt: String,
    data: Vec<u8>,
) -> Result<Parsed, String> {
    let props = if fmt == "tmod" { tmod_props(&data) } else { serde_json::Map::new() };
    let prop = |k: &str| props.get(k).and_then(Value::as_str).map(|s| pystr::strip(s).to_owned()).unwrap_or_default();
    let title_prop = prop("title");
    let author = prop("author");
    let tag = match dl.get("version").filter(|v| truthy(v)) {
        Some(v) => py_str(v),
        None => fileid.clone(),
    };
    Ok(Parsed {
        tag,
        changelog: dl.get("changes").filter(|v| truthy(v)).cloned().unwrap_or_else(|| Value::String(String::new())),
        published_at: upload_time(dl)?,
        title_prop,
        tmod_author: (!author.is_empty()).then_some(author),
        props,
        fileid,
        fmt,
        data,
    })
}

/// `_fetch_release`'s format: `tmod` unless the catalog says `zip`.
pub fn file_format(dl: &serde_json::Map<String, Value>) -> Result<String, String> {
    let fmt = match dl.get("format").filter(|v| truthy(v)) {
        Some(Value::String(s)) => s.to_lowercase(),
        Some(other) => return Err(format!("'{}' object has no attribute 'lower'", type_name(other))),
        None => "tmod".into(),
    };
    Ok(if fmt == "tmod" || fmt == "zip" { fmt } else { "tmod".into() })
}

/// `_resolve_author`.
pub fn resolve_author(parsed: Option<&Parsed>, fallback: &str) -> String {
    match parsed {
        Some(p) if p.fmt == "tmod" => p.tmod_author.clone().unwrap_or_else(|| fallback.to_owned()),
        _ => fallback.to_owned(),
    }
}

/// The release download name: the `.tmod` title, else the slug.
pub fn release_filename(parsed: &Parsed, project_title: &str) -> String {
    if parsed.fmt == "tmod" && !parsed.title_prop.is_empty() {
        format!("{}.tmod", parsed.title_prop)
    } else {
        format!("{}.{}", service::slugify(project_title), parsed.fmt)
    }
}

/// `store.sniff_image`: `(content_type, width, height)`.
pub fn sniff_image(data: &[u8]) -> Option<(&'static str, Option<i64>, Option<i64>)> {
    if data.len() < 24 {
        return None;
    }
    let be16 = |i: usize| i64::from(u16::from_be_bytes([data[i], data[i + 1]]));
    if data[..8] == *b"\x89PNG\r\n\x1a\n" {
        let be32 = |i: usize| i64::from(u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]));
        return Some(("image/png", Some(be32(16)), Some(be32(20))));
    }
    if &data[..6] == b"GIF87a" || &data[..6] == b"GIF89a" {
        let le16 = |i: usize| i64::from(u16::from_le_bytes([data[i], data[i + 1]]));
        return Some(("image/gif", Some(le16(6)), Some(le16(8))));
    }
    if &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        let (w, h) = webp_dims(data);
        return Some(("image/webp", w, h));
    }
    if data[..2] == [0xFF, 0xD8] {
        let n = data.len();
        let mut i = 2;
        while i + 9 < n {
            if data[i] != 0xFF {
                i += 1;
                continue;
            }
            let marker = data[i + 1];
            if (0xC0..=0xCF).contains(&marker) && ![0xC4, 0xC8, 0xCC].contains(&marker) {
                return Some(("image/jpeg", Some(be16(i + 7)), Some(be16(i + 5))));
            }
            if marker == 0xD8 || marker == 0xD9 || (0xD0..=0xD7).contains(&marker) {
                i += 2;
                continue;
            }
            i += 2 + be16(i + 2) as usize;
        }
        return Some(("image/jpeg", None, None));
    }
    None
}

fn webp_dims(data: &[u8]) -> (Option<i64>, Option<i64>) {
    let get = |i: usize| data.get(i).map(|b| i64::from(*b));
    let le16 = |i: usize| Some(get(i)? | (get(i + 1)? << 8));
    match &data[12..16] {
        b"VP8 " => match (le16(26), le16(28)) {
            (Some(w), Some(h)) => (Some(w & 0x3FFF), Some(h & 0x3FFF)),
            _ => (None, None),
        },
        b"VP8L" => match (get(21), get(22), get(23), get(24)) {
            (Some(a), Some(b), Some(c), Some(d)) => {
                let bits = a | (b << 8) | (c << 16) | (d << 24);
                (Some((bits & 0x3FFF) + 1), Some(((bits >> 14) & 0x3FFF) + 1))
            }
            _ => (None, None),
        },
        b"VP8X" => match (get(24), get(25), get(26), get(27), get(28), get(29)) {
            (Some(a), Some(b), Some(c), Some(d), Some(e), Some(f)) => {
                (Some((a | (b << 8) | (c << 16)) + 1), Some((d | (e << 8) | (f << 16)) + 1))
            }
            _ => (None, None),
        },
        _ => (None, None),
    }
}

/// What `_upsert_mod` reads off a catalog entry before touching the store.
pub struct Prep<'a> {
    pub sid: String,
    pub name: String,
    pub ts_author: String,
    pub author_id: String,
    pub latest: Option<&'a serde_json::Map<String, Value>>,
    pub tags: Vec<String>,
    pub total_dl: i64,
    pub likes: i64,
}

/// `None` for an entry without an id (counted as failed).
pub fn prepare(m: &serde_json::Map<String, Value>) -> Result<Option<Prep<'_>>, String> {
    let sid = pystr::strip(&str_or_empty(m.get("id"))).to_owned();
    if sid.is_empty() {
        return Ok(None);
    }
    let name = match text_or(m.get("name"), "Unnamed mod")? {
        s if s.is_empty() => "Unnamed mod".to_owned(),
        s => s,
    };
    let (ts_author, author_id) = match m.get("author") {
        Some(Value::Object(a)) => (text_or(a.get("Username"), "Unknown")?, str_or_empty(a.get("ID"))),
        _ => ("Unknown".to_owned(), String::new()),
    };
    let ts_author = if ts_author.is_empty() { "Unknown".to_owned() } else { ts_author };
    let latest = match m.get("downloads").filter(|v| truthy(v)) {
        Some(d) => latest_download(d)?,
        None => None,
    };
    let mut raw_tags = Vec::new();
    for key in ["type", "subtype"] {
        let t = text_or(m.get(key), "")?.to_lowercase();
        if !t.is_empty() {
            raw_tags.push(t);
        }
    }
    let total_dl = int_or_zero(m.get("totaldownloads"))?;
    let likes = int_or_zero(m.get("likes"))?;
    Ok(Some(Prep { sid, name, ts_author, author_id, latest, tags: clean_tags(&raw_tags), total_dl, likes }))
}

/// The fields of a newly mirrored stray mod, as `ModProject(...)` takes them.
pub fn new_project_fields(
    prep: &Prep<'_>,
    m: &serde_json::Map<String, Value>,
    slug: &str,
    author: &str,
    pending: bool,
    banner_sha: Option<&str>,
    fileid: &str,
) -> Document {
    let description = m.get("description").filter(|v| truthy(v)).map_or(Bson::String(String::new()), json_to_bson);
    let sid = &prep.sid;
    doc! {
        "slug": slug, "title": &prep.name, "summary": "", "description": description,
        "tags": prep.tags.clone(), "owner_id": Bson::Null, "owner_username": author, "owner_handle": STRAY_HANDLE,
        "visibility": if pending { "draft" } else { "public" }, "mode": "releases", "source_visibility": "public",
        "banner_sha": banner_sha.map_or(Bson::Null, |s| Bson::String(s.into())),
        "download_count": bson_int(prep.total_dl),
        "is_stray": true, "stray_status": if pending { "pending" } else { "approved" }, "author": author,
        "source": SOURCE, "source_id": sid, "source_url": format!("{MOD_PAGE_URL}{sid}"),
        "source_author_id": &prep.author_id, "source_likes": bson_int(prep.likes), "source_file_id": fileid,
    }
}

/// The catalog's fields copied onto an existing stray mod.
pub fn refresh_fields(ex: &mut Model, m: &serde_json::Map<String, Value>, prep: &Prep<'_>) {
    ex.set("title", prep.name.clone());
    if let Some(d) = m.get("description").filter(|v| truthy(v)) {
        ex.set("description", json_to_bson(d));
    }
    ex.set("tags", prep.tags.clone());
    ex.set("download_count", bson_int(prep.total_dl));
    ex.set("source_likes", bson_int(prep.likes));
}

/// The newer catalog file an existing stray mod should mirror, if any.
pub fn changed_file_id(ex: &Model, prep: &Prep<'_>) -> Option<String> {
    let fid = prep.latest?.get("fileid").map(py_str)?;
    (!fid.is_empty() && ex.opt_str("source_file_id") != Some(fid.as_str())).then_some(fid)
}

/// After a refresh mirrored a new file: the author follows it.
pub fn adopt_file(ex: &mut Model, fid: String, parsed: &Parsed, ts_author: &str) {
    ex.set("source_file_id", fid);
    ex.set("author", resolve_author(Some(parsed), ts_author));
}

pub fn sync_display(ex: &mut Model, ts_author: &str) {
    let display = if ex.str("author").is_empty() { ts_author.to_owned() } else { ex.str("author").to_owned() };
    ex.set("owner_username", display);
}

/// An existing release replaced by a newer file.
pub fn replace_release(
    rel: &mut Model,
    parsed: &Parsed,
    sha: &str,
    filename: &str,
    published_at: DateTime<Utc>,
    at: DateTime<Utc>,
) {
    rel.set("tag", parsed.tag.clone());
    rel.set("release_format", parsed.fmt.clone());
    rel.set("tmod_sha", sha.to_owned());
    rel.set("tmod_size", bson_int(parsed.data.len() as i64));
    rel.set("tmod_filename", filename.to_owned());
    rel.set("tmod_properties", json_to_bson(&Value::Object(parsed.props.clone())));
    rel.set_dt("published_at", published_at);
    if truthy(&parsed.changelog) {
        rel.set("changelog", json_to_bson(&parsed.changelog));
    }
    rel.set_dt("updated_at", at);
}

/// The fields of a stray mod's first release, as `ModRelease(...)` takes them.
pub fn new_release_fields(
    project: &Model,
    parsed: &Parsed,
    sha: &str,
    filename: &str,
    published_at: DateTime<Utc>,
) -> Document {
    doc! {
        "project_id": project.id, "owner_id": Bson::Null, "tag": &parsed.tag, "branch": "main", "title": "",
        "changelog": json_to_bson(&parsed.changelog), "release_format": &parsed.fmt, "tmod_sha": sha,
        "tmod_size": bson_int(parsed.data.len() as i64), "tmod_filename": filename,
        "tmod_properties": json_to_bson(&Value::Object(parsed.props.clone())), "status": "published",
        "published_at": dt::to_bson(published_at),
    }
}

// --- the job --------------------------------------------------------------------------------

fn client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::USER_AGENT, reqwest::header::HeaderValue::from_static(USER_AGENT));
        crate::http::client_builder()
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(30))
            .read_timeout(Duration::from_secs(300))
            .redirect(reqwest::redirect::Policy::limited(20))
            .no_proxy()
            .build()
            .expect("reqwest client")
    })
}

/// `_download`: the body of a 200, else `None`.
async fn download(url: &str) -> Option<Vec<u8>> {
    let resp = match client().get(url).send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("stray import: download failed: {url}: {e}");
            return None;
        }
    };
    if resp.status().as_u16() != 200 {
        return None;
    }
    match resp.bytes().await {
        Ok(b) if !b.is_empty() && b.len() <= MAX_FILE_BYTES => Some(b.to_vec()),
        Ok(_) => None,
        Err(e) => {
            tracing::warn!("stray import: download failed: {url}: {e}");
            None
        }
    }
}

async fn put_blob(state: &AppState, data: Vec<u8>) -> Result<String, String> {
    let store = ContentStore::new(&state.settings().mods_store_dir);
    tokio::task::spawn_blocking(move || store.put(&data))
        .await
        .map_err(|e| e.to_string())?
        .map(|(sha, _)| sha)
        .map_err(|e| e.to_string())
}

/// `_store_image`: mirror a mod's image into the CAS.
async fn store_image(state: &AppState, url: Option<&Value>) -> Result<Option<String>, String> {
    let Some(url) = url.filter(|v| truthy(v)) else { return Ok(None) };
    let Value::String(url) = url else { return Ok(None) };
    let Some(data) = download(url).await else { return Ok(None) };
    let Some((content_type, w, h)) = sniff_image(&data) else { return Ok(None) };
    let size = data.len() as i64;
    let sha = put_blob(state, data).await?;
    let c = coll(state, MOD_IMAGES);
    if find_model(&c, IMAGE_ASSET, doc! {"sha": &sha}).await.map_err(|e| e.to_string())?.is_none() {
        let asset = Model::build(
            IMAGE_ASSET,
            doc! {"sha": &sha, "content_type": content_type, "byte_size": bson_int(size), "owner_id": Bson::Null,
            "width": w.map_or(Bson::Null, bson_int), "height": h.map_or(Bson::Null, bson_int)},
        )
        .map_err(|e| e.to_string())?;
        asset.insert(&c).await.map_err(|e| e.to_string())?;
    }
    Ok(Some(sha))
}

async fn fetch_release(dl: &serde_json::Map<String, Value>) -> Result<Option<Parsed>, String> {
    let fileid = str_or_empty(dl.get("fileid"));
    if fileid.is_empty() {
        return Ok(None);
    }
    let fmt = file_format(dl)?;
    let Some(data) = download(&format!("{FILE_URL}{fileid}")).await else { return Ok(None) };
    parse_release(dl, fileid, fmt, data).map(Some)
}

/// `_write_release`: the stray mod's single release, created or replaced.
async fn write_release(state: &AppState, proj: &mut Model, parsed: &Parsed) -> Result<(), String> {
    let sha = put_blob(state, parsed.data.clone()).await?;
    let filename = release_filename(parsed, proj.str("title"));
    let c = coll(state, MOD_RELEASES);
    let rel = find_model(&c, MOD_RELEASE, doc! {"project_id": proj.id}).await.map_err(|e| e.to_string())?;
    let published_at = parsed.published_at.unwrap_or_else(now);
    proj.set_dt("last_release_at", published_at);
    match rel {
        None => {
            let release = Model::build(MOD_RELEASE, new_release_fields(proj, parsed, &sha, &filename, published_at))
                .map_err(|e| e.to_string())?;
            release.insert(&c).await.map_err(|e| e.to_string())?;
        }
        Some(mut rel) => {
            replace_release(&mut rel, parsed, &sha, &filename, published_at, now());
            rel.save(&c).await.map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// `_upsert_mod`: create or refresh one stray mod; the outcome label.
async fn upsert_mod(
    state: &AppState,
    m: &serde_json::Map<String, Value>,
    resync: bool,
) -> Result<&'static str, String> {
    let Some(prep) = prepare(m)? else { return Ok("failed") };
    let c = coll(state, MOD_PROJECTS);
    let existing = find_model(&c, MOD_PROJECT, doc! {"$and": [{"source": SOURCE}, {"source_id": &prep.sid}]})
        .await
        .map_err(|e| e.to_string())?;

    if let Some(mut ex) = existing {
        if ex.opt_str("stray_status") == Some("rejected") || !ex.bool("is_stray") {
            return Ok("skipped");
        }
        refresh_fields(&mut ex, m, &prep);
        if let (Some(fid), Some(dl)) = (changed_file_id(&ex, &prep), prep.latest)
            && let Some(parsed) = fetch_release(dl).await?
        {
            write_release(state, &mut ex, &parsed).await?;
            adopt_file(&mut ex, fid, &parsed, &prep.ts_author);
        }
        sync_display(&mut ex, &prep.ts_author);
        if matches!(ex.get("banner_sha"), Bson::Null) {
            let banner = store_image(state, m.get("image")).await?;
            ex.set("banner_sha", banner.map_or(Bson::Null, Bson::String));
        }
        ex.set_dt("updated_at", now());
        ex.save(&c).await.map_err(|e| e.to_string())?;
        return Ok("updated");
    }

    let Some(dl) = prep.latest else { return Ok("skipped") };
    let Some(parsed) = fetch_release(dl).await? else { return Ok("skipped") };
    let author = resolve_author(Some(&parsed), &prep.ts_author);
    let slug = service::unique_slug(state, None, &prep.name).await.map_err(|e| e.to_string())?;
    let banner = store_image(state, m.get("image")).await?;
    let fields = new_project_fields(&prep, m, &slug, &author, resync, banner.as_deref(), &parsed.fileid);
    let mut proj = Model::build(MOD_PROJECT, fields).map_err(|e| e.to_string())?;
    proj.insert(&c).await.map_err(|e| e.to_string())?;
    write_release(state, &mut proj, &parsed).await?;
    proj.save(&c).await.map_err(|e| e.to_string())?;
    Ok(if resync { "pending" } else { "imported" })
}

async fn hand_to(state: &AppState, owner: &SiteUser, source_id: &str) -> Result<(), String> {
    let c = coll(state, MOD_PROJECTS);
    let proj = find_model(&c, MOD_PROJECT, doc! {"$and": [{"source": SOURCE}, {"source_id": source_id}]})
        .await
        .map_err(|e| e.to_string())?;
    if let Some(mut p) = proj
        && p.bool("is_stray")
        && p.opt_str("stray_status") != Some("rejected")
    {
        service::handover_stray(state, &mut p, owner).await.map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// `str(JSONDecodeError)`.
pub fn json_error(body: &[u8], e: crate::pyjson::LoadsError) -> String {
    match e {
        crate::pyjson::LoadsError::Syntax { msg, pos } => {
            let text = String::from_utf8_lossy(body);
            let before: String = text.chars().take(pos).collect();
            let line = before.matches('\n').count() + 1;
            let col = match before.rfind('\n') {
                Some(i) => before[i + 1..].chars().count() + 1,
                None => pos + 1,
            };
            format!("{msg}: line {line} column {col} (char {pos})")
        }
        _ => "Expecting value: line 1 column 1 (char 0)".into(),
    }
}

/// The catalog's mods: `list(data.values())` or the list itself, dicts with an id.
pub fn catalog_mods(data: Value, author_id: Option<&str>) -> Result<Vec<serde_json::Map<String, Value>>, String> {
    let items: Vec<Value> = match data {
        Value::Object(m) => m.into_iter().map(|(_, v)| v).collect(),
        Value::Array(a) => a,
        Value::String(_) => Vec::new(),
        other => return Err(format!("'{}' object is not iterable", type_name(&other))),
    };
    let mut mods: Vec<serde_json::Map<String, Value>> = items
        .into_iter()
        .filter_map(|v| match v {
            Value::Object(m) if m.get("id").is_some_and(truthy) => Some(m),
            _ => None,
        })
        .collect();
    if let Some(aid) = author_id {
        let mut kept = Vec::new();
        for m in mods {
            let author = m.get("author").filter(|v| truthy(v));
            let id = match author {
                None => String::new(),
                Some(Value::Object(a)) => str_or_empty(a.get("ID")),
                Some(other) => return Err(format!("'{}' object has no attribute 'get'", type_name(other))),
            };
            if id == aid {
                kept.push(m);
            }
        }
        mods = kept;
    }
    Ok(mods)
}

async fn fetch_catalog(author_id: Option<&str>) -> Result<Vec<serde_json::Map<String, Value>>, String> {
    let resp = client().get(CATALOG_URL).send().await.map_err(|e| crate::feeds::image::transport_message(&e))?;
    let status = resp.status().as_u16();
    let reason = crate::error::status_phrase(status).unwrap_or("");
    if let Some(msg) = crate::feeds::image::status_message(status, reason, None, resp.url().as_str()) {
        return Err(msg);
    }
    let body = resp.bytes().await.map_err(|e| crate::feeds::image::transport_message(&e))?;
    let data = crate::pyjson::loads(&body).map_err(|e| json_error(&body, e))?;
    catalog_mods(data, author_id)
}

pub fn reset_for_run(st: &mut Model, resync: bool, at: DateTime<Utc>) {
    st.set("running", true);
    st.set("phase", if resync { "resyncing" } else { "importing" });
    for k in ["total", "processed", "imported", "updated", "pending_added", "failed"] {
        st.set(k, 0);
    }
    st.set_dt("started_at", at);
    st.set("finished_at", Bson::Null);
    st.set("last_error", Bson::Null);
}

fn bump(st: &mut Model, k: &'static str) {
    st.set(k, bson_int(st.int(k) + 1));
}

/// `run(resync, author_id, owner)`. `Err` only when the state itself can't be written.
pub async fn run(state: &AppState, resync: bool, author_id: Option<&str>, owner: Option<&SiteUser>) -> Res<()> {
    let c = coll(state, STATE);
    let mut st = load_state(state).await?;
    reset_for_run(&mut st, resync, now());
    st.save(&c).await?;
    let outcome: Result<(), String> = async {
        let mods = fetch_catalog(author_id).await?;
        st.set("total", bson_int(mods.len() as i64));
        st.save(&c).await.map_err(|e| e.to_string())?;
        for (i, m) in mods.iter().enumerate() {
            let id = py_str(m.get("id").unwrap_or(&Value::Null));
            let mut step = upsert_mod(state, m, resync).await;
            if let Ok(outcome) = step {
                match outcome {
                    "imported" => bump(&mut st, "imported"),
                    "updated" => bump(&mut st, "updated"),
                    "pending" => bump(&mut st, "pending_added"),
                    "failed" => bump(&mut st, "failed"),
                    _ => {}
                }
                if let Some(owner) = owner {
                    step = hand_to(state, owner, &id).await.map(|()| outcome);
                }
            }
            if let Err(e) = step {
                bump(&mut st, "failed");
                tracing::warn!("stray import: mod {id} failed: {e}");
            }
            st.set("processed", bson_int(i as i64 + 1));
            if st.int("processed") % PROGRESS_EVERY == 0 {
                st.set_dt("updated_at", now());
                st.save(&c).await.map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
    .await;
    if let Err(e) = &outcome {
        tracing::error!("stray import failed: {e}");
    }
    finish(&mut st, &outcome, now());
    st.save(&c).await.map_err(internal)
}

/// The run's last state write: done, or the error's first 500 characters.
pub fn finish(st: &mut Model, outcome: &Result<(), String>, at: DateTime<Utc>) {
    match outcome {
        Ok(()) => st.set("phase", "done"),
        Err(e) => {
            st.set("phase", "error");
            st.set("last_error", pystr::prefix(e, 500).to_owned());
        }
    }
    st.set("running", false);
    st.set_dt("finished_at", at);
    st.set_dt("updated_at", at);
}
