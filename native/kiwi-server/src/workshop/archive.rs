//! Reading what the page hands over: `workshop.read_zip` / `read_mod` /
//! `read_archive` / `describe` / `swf_bytes`, and the Mods Hub's
//! `_decode_blueprint_payload`.

use axum::http::StatusCode;
use base64::Engine as _;
use kiwi_core::workshop::{self as ws, Fail};
use kiwi_core::zipread::{self, ZipErr};
use serde_json::{Map, Value, json};

use crate::error::{ApiError, ErrorCode};

const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

pub struct Archive {
    pub kind: String,
    pub header: Map<String, Value>,
    pub files: Vec<(String, Vec<u8>)>,
}

fn zip_fail(e: ZipErr) -> Fail {
    match e {
        ZipErr::Bad(m) => Fail::Bad(format!("That .zip couldn't be opened: {m}")),
        ZipErr::Unhandled(m) => Fail::Unhandled(m),
    }
}

/// `read_zip(data)`.
pub fn read_zip(data: &[u8]) -> Result<Vec<(String, Vec<u8>)>, Fail> {
    let infos = zipread::open(data).map_err(zip_fail)?;
    let infos: Vec<_> =
        infos.into_iter().filter(|i| !i.is_dir() && !ws::is_junk(&ws::norm_path(&i.filename))).collect();
    if infos.len() > ws::MAX_FILES {
        return Err(Fail::Bad(format!("That .zip holds more than {} files - too many for one mod.", ws::MAX_FILES)));
    }
    let total: u128 = infos.iter().map(|i| u128::from(i.file_size)).sum();
    if total > u128::from(ws::MAX_UNPACKED_BYTES) {
        return Err(Fail::Bad(format!(
            "That .zip unpacks to more than {} MB.",
            ws::MAX_UNPACKED_BYTES / (1024 * 1024)
        )));
    }
    let mut out = Vec::new();
    for info in &infos {
        let path = ws::norm_path(&info.filename);
        if !path.is_empty() {
            out.push((path, zipread::read(data, info).map_err(zip_fail)?));
        }
    }
    if out.is_empty() {
        return Err(Fail::Bad("That .zip has no files in it.".into()));
    }
    Ok(out)
}

fn tmod_fail(msg: String) -> Fail {
    Fail::Bad(format!("That isn't a readable .tmod file: {msg}"))
}

fn props_of(parsed: &Value) -> Map<String, Value> {
    parsed.get("properties").and_then(Value::as_object).cloned().unwrap_or_default()
}

fn contents(parsed: &Value) -> Result<Vec<(String, Vec<u8>)>, String> {
    let files = parsed.get("files").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut out = Vec::with_capacity(files.len());
    for f in files {
        let path = f.get("path").and_then(Value::as_str).unwrap_or_default().to_owned();
        let b64 = f.get("content_base64").and_then(Value::as_str).unwrap_or_default();
        let data = B64.decode(b64).map_err(|e| format!("base64: {e}"))?;
        out.push((path, data));
    }
    Ok(out)
}

/// A `.tmod` header and its files.
pub type Unpacked = (Map<String, Value>, Vec<(String, Vec<u8>)>);

/// `read_mod(data)`: the header and the packed files (normalized paths).
pub fn read_mod(data: &[u8]) -> Result<Unpacked, Fail> {
    let parsed = crate::tools::tmod::read(data, false).map_err(tmod_fail)?;
    let files = contents(&parsed).map_err(Fail::Unhandled)?;
    let files = files.into_iter().map(|(p, b)| (ws::norm_path(&p), b)).filter(|(p, _)| !p.is_empty()).collect();
    Ok((props_of(&parsed), files))
}

fn looks_like_zip(data: &[u8], filename: &str) -> bool {
    data.starts_with(b"PK") || filename.to_lowercase().ends_with(".zip")
}

/// `read_archive(data, filename)`.
pub fn read_archive(data: &[u8], filename: &str) -> Result<Archive, Fail> {
    if data.is_empty() {
        return Err(Fail::Bad("That file is empty.".into()));
    }
    if looks_like_zip(data, filename) {
        return Ok(Archive { kind: "zip".into(), header: Map::new(), files: read_zip(data)? });
    }
    let (header, files) = read_mod(data)?;
    if files.is_empty() {
        return Err(Fail::Bad("That .tmod has no files packed in it.".into()));
    }
    Ok(Archive { kind: "tmod".into(), header, files })
}

/// `describe(data)`.
pub fn describe(data: &[u8]) -> Result<Value, Fail> {
    let parsed = crate::tools::tmod::read(data, true).map_err(tmod_fail)?;
    let props = props_of(&parsed);
    let mut files: Vec<(String, u64)> = parsed
        .get("files")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|f| {
                    (
                        f.get("path").and_then(Value::as_str).unwrap_or_default().to_owned(),
                        f.get("size").and_then(Value::as_u64).unwrap_or(0),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort_by_cached_key(|(p, _)| p.to_lowercase());
    let flags = ws::flags_of(&props);
    let total: u64 = files.iter().map(|(_, s)| s).sum();
    Ok(json!({
        "version": parsed.get("version").cloned().unwrap_or(Value::Null),
        "categories": ws::categories(flags),
        "flags": flags,
        "preview_path": ws::preview_path(&props),
        "config_path": ws::declared_config_path(&props),
        "files": files.iter().map(|(p, s)| json!({"path": p, "size": s})).collect::<Vec<_>>(),
        "file_count": files.len(),
        "size": data.len(),
        "total_size": total,
    }))
    .map(|v| reorder(v, props))
}

/// `describe` keys in Python's order (`properties` second).
fn reorder(v: Value, props: Map<String, Value>) -> Value {
    let Value::Object(m) = v else { return v };
    let mut out = Map::new();
    out.insert("version".into(), m.get("version").cloned().unwrap_or(Value::Null));
    out.insert("properties".into(), Value::Object(props));
    for (k, v) in m {
        if k != "version" {
            out.insert(k, v);
        }
    }
    Value::Object(out)
}

/// `swf_bytes(data, path)`: one packed `.swf` by its full path.
pub fn swf_bytes(data: &[u8], path: &str) -> Result<Option<Vec<u8>>, Fail> {
    if data.is_empty() {
        return Err(Fail::Bad("That file is empty.".into()));
    }
    let want = kiwi_core::pystr::strip(&path.replace('\\', "/")).to_lowercase();
    let parsed = crate::tools::tmod::read(data, false).map_err(tmod_fail)?;
    if !want.ends_with(".swf") {
        return Ok(None);
    }
    let files = contents(&parsed).map_err(Fail::Unhandled)?;
    Ok(files.into_iter().find(|(p, _)| p.to_lowercase() == want).map(|(_, b)| b))
}

/// `_decode_blueprint_payload(tmod_bytes, want_path)`.
pub fn blueprint_payload(data: &[u8], want_path: &str) -> Result<Map<String, Value>, ApiError> {
    let parsed = crate::tools::tmod::read(data, false).map_err(|e| {
        tracing::error!("workshop blueprint: TmodError {e}");
        ApiError::internal()
    })?;
    let want = kiwi_core::pystr::strip(want_path).to_lowercase();
    let files = contents(&parsed).map_err(|_| ApiError::internal())?;
    let hit = files.into_iter().find(|(p, _)| p.to_lowercase() == want && want.ends_with(".blueprint"));
    let Some((path, raw)) = hit else { return Err(ApiError::not_found("No such blueprint in this release")) };
    use kiwi_core::voxel::PackError;
    match kiwi_core::voxel::pack(&raw, &path, kiwi_core::voxel::RENDER_VOXEL_CAP) {
        Ok(Value::Object(m)) => Ok(m),
        Ok(_) => Err(ApiError::internal()),
        Err(PackError::TooLarge(n)) => Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            ErrorCode::BadRequest,
            format!("This model is too large to preview ({} voxels).", kiwi_core::bpedit::py::commas(n as i64)),
        )),
        Err(PackError::Empty) => Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::BadRequest,
            "This blueprint is an empty placeholder (no voxels to show).",
        )),
        Err(PackError::Invalid(m)) => Err(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            ErrorCode::BadRequest,
            format!("Couldn't read that blueprint: {m}"),
        )),
        Err(PackError::Unhandled(m)) => {
            tracing::error!("workshop blueprint decode: {m}");
            Err(ApiError::internal())
        }
    }
}
