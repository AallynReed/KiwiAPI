//! The Mod Workshop (`/site/mod-workshop/*` in `app/site/router.py`, engine
//! `app/trove/mods_hub/workshop.py` + `trove_layout.py`): a stateless `.tmod`
//! compiler and unpacker. Nothing is stored.

pub mod archive;
pub mod form;
pub mod game;

#[cfg(test)]
pub(crate) mod tests;

use axum::{
    Router,
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
    routing::post,
};
use kiwi_core::form::UploadFile;
use kiwi_core::workshop::{self as ws, Fail, GameMap};
use serde_json::{Map, Value, json};

use self::archive::Archive;
use self::form::Items;
use crate::{
    auth::client::client_ip,
    blueprints::cache::{self, BuildError, Fmt},
    error::{self, ApiError, ErrorCode},
    features,
    pyjson::PyJson,
    ratelimit::check_rate_limit,
    runtime_config,
    state::AppState,
    validation::Params,
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/site/mod-workshop/inspect", post(inspect))
        .route("/site/mod-workshop/build", post(build))
        .route("/site/mod-workshop/extract", post(extract))
        .route("/site/mod-workshop/preview/blueprint", post(preview_blueprint))
        .route("/site/mod-workshop/swf/scripts", post(swf_scripts))
        .route("/site/mod-workshop/preview/binfab", post(preview_binfab))
        .route("/site/mod-workshop/extract/download", post(extract_download))
}

/// The site router's `_resolve_feature_flags`: every flag read (a failure is
/// a 500), then the bare 404 when `flag` is off.
pub async fn site_gate(state: &AppState, parts: &Parts, flag: Option<&str>) -> Result<(), Response> {
    let mut on = true;
    for f in crate::activity::SITE_FLAGS {
        let v = features::is_enabled(state, f).await.map_err(|e| {
            tracing::error!("site feature flag {f}: {e:#}");
            ApiError::internal().into_response()
        })?;
        if Some(f) == flag {
            on = v;
        }
    }
    if !on {
        return Err(error::http_exception(
            StatusCode::NOT_FOUND,
            None,
            &parts.method,
            parts.uri.path(),
            &parts.headers,
        ));
    }
    Ok(())
}

/// `_workshop_throttle`.
async fn throttle(state: &AppState, parts: &Parts) -> Result<(), Response> {
    let (max, window) = runtime_config::get_rate_limit(state, "mod_workshop_rate_limit").await.map_err(|e| {
        tracing::error!("mod_workshop_rate_limit: {e:#}");
        ApiError::internal().into_response()
    })?;
    let ip = client_ip(parts).unwrap_or_else(|| "unknown".into());
    check_rate_limit(state, &format!("modworkshop:{ip}"), max, window).await.map_err(IntoResponse::into_response)?;
    Ok(())
}

/// Form, gate, throttle: what FastAPI settles before the fields.
async fn prelude(state: &AppState, req: Request, limited: bool) -> Result<(Parts, Items), Response> {
    let (parts, body) = req.into_parts();
    let items = form::read(&parts, body).await?;
    site_gate(state, &parts, Some(features::MOD_WORKSHOP_FLAG)).await?;
    if limited {
        throttle(state, &parts).await?;
    }
    Ok((parts, items))
}

fn bad(msg: impl Into<String>) -> Response {
    ApiError::bad_request(msg).into_response()
}

fn fail(f: Fail) -> Response {
    match f {
        Fail::Bad(m) => bad(m),
        Fail::Unhandled(m) => {
            tracing::error!("mod workshop: {m}");
            ApiError::internal().into_response()
        }
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, Response> {
    tokio::task::spawn_blocking(f).await.map_err(|_| ApiError::internal().into_response())
}

macro_rules! tri {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(r) => return crate::workshop::form::Reply::reply(r),
        }
    };
}

fn no_store(v: Value) -> Response {
    let mut r = PyJson(v).into_response();
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

/// `_workshop_download(data, filename, media_type, plan)`.
pub fn download(data: Vec<u8>, filename: &str, media_type: &'static str, plan: Option<&ws::Plan>) -> Response {
    let mut r = Response::new(axum::body::Body::from(data));
    let h = r.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&crate::blueprints::editor::disposition(filename, "mod")) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if let Some(p) = plan {
        h.insert("x-kiwi-packed", HeaderValue::from(p.packed()));
        h.insert("x-kiwi-moved", HeaderValue::from(p.count("moved")));
        h.insert("x-kiwi-skipped", HeaderValue::from(p.count("skipped")));
    }
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(media_type));
    r
}

/// `trove_layout.game_file_map()` as `plan` reads it: unavailable is `{}`.
async fn game_map(state: &AppState) -> GameMap {
    match game::index(state, ws::LIVE_BRANCH).await {
        Ok(idx) => idx.map.clone(),
        Err(e) => {
            tracing::warn!("mod workshop: game file index unavailable: {e:#}");
            GameMap::new()
        }
    }
}

/// `str(p)` of each path in a page-sent JSON list.
fn str_list(v: &[Value]) -> Vec<String> {
    v.iter().map(kiwi_core::bpedit::py::str_of).collect()
}

const TOO_MANY: &str = "too many for one mod.";

/// `_workshop_source(paths, archive)`: `(kind, header, files, paths)`.
async fn source(
    paths: &str,
    upload: Option<UploadFile>,
) -> Result<(String, Map<String, Value>, Option<Archive>, Vec<String>), Response> {
    if let Some(f) = upload.filter(|f| !f.filename.is_empty()) {
        let name = f.filename.clone();
        let a = blocking(move || archive::read_archive(&f.data, &name)).await?.map_err(fail)?;
        let names = a.files.iter().map(|(p, _)| p.clone()).collect();
        return Ok((a.kind.clone(), a.header.clone(), Some(a), names));
    }
    let parsed = form::loads(if paths.is_empty() { "[]" } else { paths }).map_err(|r| *r)?;
    let Some(parsed) = parsed else { return Err(bad("The file list wasn't understood.")) };
    let names = match parsed {
        Value::Array(a) if !a.is_empty() => a,
        _ => return Err(bad("No files were selected.")),
    };
    if names.len() > ws::MAX_FILES {
        return Err(bad(format!("That's more than {} files - {TOO_MANY}", ws::MAX_FILES)));
    }
    Ok(("files".into(), Map::new(), None, str_list(&names)))
}

fn put_sizes(entries: &mut Value, sizes: &[usize]) {
    if let Some(list) = entries.as_array_mut() {
        for e in list {
            let i = e.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            if let Some(m) = e.as_object_mut() {
                m.insert("size".into(), json!(sizes.get(i).copied().unwrap_or(0)));
            }
        }
    }
}

// --- /inspect ------------------------------------------------------------------

pub async fn inspect(State(state): State<AppState>, req: Request) -> Response {
    let (_, items) = tri!(prelude(&state, req, true).await);
    inspect_form(&state, items).await
}

pub async fn inspect_form(state: &AppState, items: Items) -> Response {
    let mut errors = Vec::new();
    let paths = form::text(&items, "paths", "[]", &mut errors);
    let upload = form::file_opt(&items, "archive", &mut errors);
    tri!(form::check(errors));
    let (kind, header, files, names) = tri!(source(&paths, upload).await);
    let gm = game_map(state).await;
    let plan = ws::preview(&names, &gm);
    let mut payload = plan.to_map();
    payload.insert("source".into(), kind.into());
    payload.insert("properties".into(), Value::Object(header));
    payload.insert("config_candidates".into(), json!(ws::config_candidates(&names)));
    payload.insert("preview_candidates".into(), json!(ws::preview_candidates(&names)));
    if let Some(a) = files {
        let sizes: Vec<usize> = a.files.iter().map(|(_, d)| d.len()).collect();
        if let Some(e) = payload.get_mut("entries") {
            put_sizes(e, &sizes);
        }
    }
    no_store(Value::Object(payload))
}

// --- /build ----------------------------------------------------------------------

pub async fn build(State(state): State<AppState>, req: Request) -> Response {
    let (_, items) = tri!(prelude(&state, req, true).await);
    build_form(&state, items).await
}

/// `_workshop_spec(spec)`.
fn spec_of(raw: &str) -> Result<Map<String, Value>, form::Rej> {
    match form::loads(if raw.is_empty() { "{}" } else { raw })? {
        Some(Value::Object(m)) => Ok(m),
        _ => Err(Box::new(bad("The request wasn't understood."))),
    }
}

pub async fn build_form(state: &AppState, items: Items) -> Response {
    let mut errors = Vec::new();
    let spec = form::text_required(&items, "spec", None, None, &mut errors);
    let paths = form::text(&items, "paths", "[]", &mut errors);
    let uploads = form::files(&items, "files", &mut errors);
    let upload = form::file_opt(&items, "archive", &mut errors);
    let config = form::file_opt(&items, "config", &mut errors);
    let preview = form::file_opt(&items, "preview", &mut errors);
    tri!(form::check(errors));
    let parsed = tri!(spec_of(&spec.unwrap_or_default()));
    let (source, properties): (Vec<(String, Vec<u8>)>, Value) =
        if let Some(f) = upload.filter(|f| !f.filename.is_empty()) {
            let name = f.filename.clone();
            let a = tri!(tri!(blocking(move || archive::read_archive(&f.data, &name)).await).map_err(fail));
            let page = match parsed.get("properties") {
                Some(v) if kiwi_core::bpedit::py::truthy(v) => match v.as_object() {
                    Some(m) => m.clone(),
                    None => return fail(Fail::Unhandled("AttributeError: no .items".into())),
                },
                _ => Map::new(),
            };
            let mut merged = a.header.clone();
            for (k, v) in page {
                if kiwi_core::bpedit::py::truthy(&v) || k == "tags" {
                    merged.insert(k, v);
                }
            }
            (a.files, Value::Object(merged))
        } else {
            let names = match tri!(form::loads(if paths.is_empty() { "[]" } else { &paths })) {
                Some(v) => v,
                None => return bad("The file list wasn't understood."),
            };
            let names = match names {
                Value::Array(a) if a.len() == uploads.len() => a,
                _ => return bad("The file list didn't match the files that arrived."),
            };
            if uploads.is_empty() {
                return bad("No files were selected.");
            }
            if uploads.len() > ws::MAX_FILES {
                return bad(format!("That's more than {} files - {TOO_MANY}", ws::MAX_FILES));
            }
            let src = str_list(&names).into_iter().zip(uploads).map(|(n, f)| (n, f.data)).collect();
            (src, parsed.get("properties").cloned().unwrap_or(Value::Null))
        };
    let mut config_path = ws::py_str(parsed.get("config_path"));
    let mut preview_path = ws::py_str(parsed.get("preview_path"));
    let mut attached = Vec::new();
    for (part, is_config) in [(config, true), (preview, false)] {
        let Some(part) = part.filter(|p| !p.filename.is_empty()) else { continue };
        let name = ws::norm_path(&part.filename);
        attached.push((name.clone(), part.data));
        if is_config {
            config_path = name;
        } else {
            preview_path = name;
        }
    }
    let keep: Vec<String> = match parsed.get("keep") {
        Some(Value::Array(a)) => a.iter().map(|k| ws::py_str(Some(k))).collect(),
        _ => Vec::new(),
    };
    let fix = parsed.get("fix").is_none_or(kiwi_core::bpedit::py::truthy);
    let gm = game_map(state).await;
    let title = ws::safe_title(&match properties.as_object() {
        Some(m) => ws::py_str(m.get("title")),
        None => String::new(),
    });
    let built = tri!(
        blocking(move || {
            let args = ws::BuildArgs {
                properties: Some(&properties),
                fix,
                keep: &keep,
                config_path: &config_path,
                preview_path: &preview_path,
                attached,
            };
            let built = ws::build_mod(source, args, &gm)?;
            let data =
                crate::tools::tmod::build(1, &built.props, &built.packed, crate::tools::tmod::KIWI_MOD_LOADER, true)
                    .map_err(|e| Fail::Unhandled(format!("build_tmod: {e:?}")))?;
            Ok::<_, Fail>((data, built.plan))
        })
        .await
    );
    let (data, plan) = tri!(built.map_err(fail));
    download(data, &format!("{title}.tmod"), "application/octet-stream", Some(&plan))
}

// --- /extract ----------------------------------------------------------------------

pub async fn extract(State(state): State<AppState>, req: Request) -> Response {
    let (_, items) = tri!(prelude(&state, req, true).await);
    extract_form(&state, items).await
}

pub async fn extract_form(state: &AppState, items: Items) -> Response {
    let mut errors = Vec::new();
    let f = form::file(&items, "file", &mut errors);
    tri!(form::check(errors));
    let data = f.map(|f| f.data).unwrap_or_default();
    let info = tri!(tri!(blocking(move || archive::describe(&data)).await).map_err(fail));
    let files = info.get("files").and_then(Value::as_array).cloned().unwrap_or_default();
    if files.is_empty() {
        return bad("That .tmod has no files packed in it.");
    }
    let paths: Vec<String> =
        files.iter().map(|f| f.get("path").and_then(Value::as_str).unwrap_or_default().to_owned()).collect();
    let sizes: Vec<usize> = files.iter().map(|f| f.get("size").and_then(Value::as_u64).unwrap_or(0) as usize).collect();
    let gm = game_map(state).await;
    let mut payload = ws::plan(&paths, false, &[], &gm).to_map();
    if let Some(e) = payload.get_mut("entries") {
        put_sizes(e, &sizes);
    }
    if let Value::Object(info) = info {
        for (k, v) in info {
            payload.insert(k, v);
        }
    }
    payload.insert("decompiler".into(), crate::media::swf::decompile::available(state.settings()).into());
    no_store(Value::Object(payload))
}

// --- /preview/blueprint ----------------------------------------------------------

pub async fn preview_blueprint(State(state): State<AppState>, req: Request) -> Response {
    let (parts, items) = tri!(prelude(&state, req, true).await);
    preview_blueprint_form(&parts, items).await
}

pub async fn preview_blueprint_form(parts: &Parts, items: Items) -> Response {
    let mut p = Params::new(Vec::new(), parts.uri.query(), &parts.headers);
    let fmt = p.query("fmt").str().one_of_pattern(&["json", "bin"]).default("json".into());
    let mut errors = p.v.errors;
    let f = form::file(&items, "file", &mut errors);
    let path = form::text_required(&items, "path", None, None, &mut errors);
    tri!(form::check(errors));
    let (data, path) = (f.map(|f| f.data).unwrap_or_default(), path.unwrap_or_default());
    if data.is_empty() {
        return bad("That file is empty.");
    }
    let fmt = if fmt == "bin" { Fmt::Bin } else { Fmt::Json };
    let built = cache::build_uncached(fmt, async move {
        tokio::task::spawn_blocking(move || archive::blueprint_payload(&data, &path))
            .await
            .map_err(|_| BuildError::Api(ApiError::internal()))?
            .map_err(BuildError::Api)
    })
    .await;
    match built {
        Ok(cached) => {
            let mut r = cache::respond(&parts.headers, &cached, 300);
            r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            r
        }
        Err(e) => e.into_response(),
    }
}

// --- /swf/scripts --------------------------------------------------------------------

pub async fn swf_scripts(State(state): State<AppState>, req: Request) -> Response {
    let (parts, items) = tri!(prelude(&state, req, false).await);
    swf_scripts_form(&state, &parts, items, true).await
}

/// After the gate; `throttled` is false only in tests.
pub async fn swf_scripts_form(state: &AppState, parts: &Parts, items: Items, throttled: bool) -> Response {
    let mut errors = Vec::new();
    let f = form::file(&items, "file", &mut errors);
    let path = form::text_required(&items, "path", Some(1), Some(400), &mut errors);
    tri!(form::check(errors));
    if throttled && let Err(r) = crate::media::swf::decompile::throttle(state, parts).await {
        return r.into_response();
    }
    let (data, path) = (f.map(|f| f.data).unwrap_or_default(), path.unwrap_or_default());
    let p2 = path.clone();
    let raw = tri!(tri!(blocking(move || archive::swf_bytes(&data, &p2)).await).map_err(fail));
    let Some(raw) = raw else { return ApiError::not_found("No such Flash movie in that mod.").into_response() };
    let size = raw.len();
    match crate::media::swf::decompile::scripts_uncached(state, raw).await {
        Ok(Value::Object(payload)) => {
            let mut out = Map::new();
            out.insert("path".into(), path.into());
            out.insert("size".into(), size.into());
            out.extend(payload);
            no_store(Value::Object(out))
        }
        Ok(_) => ApiError::internal().into_response(),
        Err(e) => e.into_response(),
    }
}

// --- /preview/binfab -----------------------------------------------------------------

pub async fn preview_binfab(State(state): State<AppState>, req: Request) -> Response {
    let (_, items) = tri!(prelude(&state, req, true).await);
    preview_binfab_form(items).await
}

pub async fn preview_binfab_form(items: Items) -> Response {
    let mut errors = Vec::new();
    let f = form::file(&items, "file", &mut errors);
    let path = form::text_required(&items, "path", Some(1), Some(400), &mut errors);
    tri!(form::check(errors));
    let (data, path) = (f.map(|f| f.data).unwrap_or_default(), path.unwrap_or_default());
    let (_, files) = tri!(tri!(blocking(move || archive::read_mod(&data)).await).map_err(fail));
    let wanted = ws::norm_path(&path).to_lowercase();
    let raw = files.into_iter().find(|(p, _)| p.to_lowercase() == wanted).map(|(_, b)| b);
    let Some(raw) = raw.filter(|_| wanted.ends_with(".binfab")) else {
        return ApiError::not_found("No such prefab in that mod.").into_response();
    };
    match tri!(blocking(move || crate::decode::view::describe_upload(&raw)).await) {
        Ok(Value::Object(payload)) => {
            let mut out = Map::new();
            out.insert("path".into(), path.into());
            out.extend(payload);
            no_store(Value::Object(out))
        }
        Ok(_) => ApiError::internal().into_response(),
        Err(msg) => ApiError::new(StatusCode::UNPROCESSABLE_ENTITY, ErrorCode::BadRequest, msg).into_response(),
    }
}

// --- /extract/download -----------------------------------------------------------------

pub async fn extract_download(State(state): State<AppState>, req: Request) -> Response {
    let (parts, items) = tri!(prelude(&state, req, true).await);
    extract_download_form(&parts, items).await
}

pub async fn extract_download_form(parts: &Parts, items: Items) -> Response {
    let mut errors = Vec::new();
    let f = form::file(&items, "file", &mut errors);
    let path = form::text(&items, "path", "", &mut errors);
    tri!(form::check(errors));
    let f = f.unwrap_or_else(|| UploadFile { filename: String::new(), headers: Vec::new(), data: Vec::new() });
    let data = f.data;
    let (_, files) = tri!(tri!(blocking(move || archive::read_mod(&data)).await).map_err(fail));
    if files.is_empty() {
        return bad("That .tmod has no files packed in it.");
    }
    let wanted = ws::norm_path(&path);
    if !wanted.is_empty() {
        let Some((_, data)) = files.into_iter().find(|(p, _)| *p == wanted) else {
            let detail = format!("No file '{wanted}' in that mod.");
            return error::http_exception(
                StatusCode::NOT_FOUND,
                Some(&detail),
                &parts.method,
                parts.uri.path(),
                &parts.headers,
            );
        };
        let name = wanted.rsplit('/').next().unwrap_or(&wanted).to_owned();
        return download(data, &name, "application/octet-stream", None);
    }
    let base = if f.filename.is_empty() { "mod" } else { &f.filename };
    let mut stem = base.rsplit('/').next().unwrap_or(base).to_owned();
    if stem.to_lowercase().ends_with(".tmod") {
        let chars: Vec<char> = stem.chars().collect();
        stem = chars[..chars.len() - 5].iter().collect();
    }
    let zip =
        tri!(blocking(move || crate::blueprints::pack::zip(&files, crate::blueprints::pack::now_date_time())).await);
    let Some(zip) = zip else { return ApiError::internal().into_response() };
    download(zip, &format!("{}.zip", ws::safe_title(&stem)), "application/zip", None)
}
