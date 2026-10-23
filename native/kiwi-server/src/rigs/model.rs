//! `/site/blueprint-editor/{model,game-model,model-save}`: a whole creature
//! open in the Blueprint Editor, placed from the game's prefab bindings.

use axum::{
    Router,
    extract::{FromRequestParts, Request, State},
    http::{HeaderValue, header, request::Parts},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use kiwi_core::bpedit::Fault;
use kiwi_core::bpmodel;
use kiwi_core::form::UploadFile;
use kiwi_core::rig::prefab_stem;
use kiwi_core::workshop::{self as ws, Fail};
use serde_json::{Map, Value};

use crate::{
    auth::client::client_ip,
    blueprints::SiteGate,
    error::ApiError,
    pyjson::{self, PyJson},
    ratelimit::check_rate_limit,
    runtime_config,
    state::AppState,
    validation::Params,
    workshop::{archive, form, form::Items},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/site/blueprint-editor/model", post(model))
        .route("/site/blueprint-editor/game-model", get(game_model))
        .route("/site/blueprint-editor/model-save", post(model_save))
}

macro_rules! tri {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(r) => return crate::workshop::form::Reply::reply(r),
        }
    };
}

/// Gate and `_blueprint_editor_throttle`.
async fn gate(state: &AppState, parts: &mut Parts) -> Result<(), Response> {
    SiteGate::from_request_parts(parts, state).await?;
    let (max, window) = runtime_config::get_rate_limit(state, "blueprint_editor_rate_limit").await.map_err(|e| {
        tracing::error!("blueprint_editor_rate_limit: {e:#}");
        ApiError::internal().into_response()
    })?;
    let ip = client_ip(parts).unwrap_or_else(|| "unknown".into());
    check_rate_limit(state, &format!("bpeditor:{ip}"), max, window).await.map_err(IntoResponse::into_response)?;
    Ok(())
}

async fn prelude(state: &AppState, req: Request) -> Result<(Parts, Items), Response> {
    let (mut parts, body) = req.into_parts();
    let items = form::read(&parts, body).await?;
    gate(state, &mut parts).await?;
    Ok((parts, items))
}

fn bad(msg: impl Into<String>) -> Response {
    ApiError::bad_request(msg).into_response()
}

fn fault(f: Fault) -> Response {
    match f {
        Fault::Editor(m) => bad(m),
        other => {
            tracing::error!("blueprint model: {}", other.message());
            ApiError::internal().into_response()
        }
    }
}

fn internal(e: impl std::fmt::Display) -> Response {
    tracing::error!("blueprint model: {e}");
    ApiError::internal().into_response()
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, Response> {
    tokio::task::spawn_blocking(f).await.map_err(|_| ApiError::internal().into_response())
}

fn no_store(v: Value) -> Response {
    let mut r = PyJson(v).into_response();
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

struct Source {
    kind: String,
    props: Map<String, Value>,
    files: Vec<(String, Vec<u8>)>,
    name: String,
}

/// `_bp_model_files(file, files)`.
async fn model_files(file: Option<UploadFile>, files: Vec<UploadFile>) -> Result<Source, Response> {
    if let Some(f) = file.filter(|f| !f.filename.is_empty()) {
        if f.data.is_empty() {
            return Err(bad("That file is empty."));
        }
        let name = f.filename.rsplit('/').next().unwrap_or(&f.filename).to_owned();
        let n2 = name.clone();
        let a = blocking(move || archive::read_archive(&f.data, &n2)).await?.map_err(|e| match e {
            Fail::Bad(m) => bad(m),
            Fail::Unhandled(m) => internal(m),
        })?;
        return Ok(Source { kind: a.kind, props: a.header, files: a.files, name });
    }
    let mut loose = Vec::new();
    for part in files {
        if part.filename.is_empty() {
            continue;
        }
        let p = part.filename.replace('\\', "/");
        let path = p.rsplit('/').next().unwrap_or(&p).to_owned();
        if !path.to_lowercase().ends_with(".blueprint") {
            return Err(bad(format!("'{path}' isn't a .blueprint.")));
        }
        loose.push((path, part.data));
    }
    if loose.is_empty() {
        return Err(bad("Send a .tmod or .zip, or the .blueprint files themselves."));
    }
    Ok(Source { kind: "files".into(), props: Map::new(), files: loose, name: "model".into() })
}

fn branch(state: &AppState) -> String {
    state.settings().trove_render_branch.clone()
}

// --- /model ------------------------------------------------------------------------

pub async fn model(State(state): State<AppState>, req: Request) -> Response {
    let (_, items) = tri!(prelude(&state, req).await);
    model_form(&state, items).await
}

pub async fn model_form(state: &AppState, items: Items) -> Response {
    let mut errors = Vec::new();
    let file = form::file_opt(&items, "file", &mut errors);
    let files = form::files(&items, "files", &mut errors);
    tri!(form::check(errors));
    let src = tri!(model_files(file, files).await);
    let blueprints = bpmodel::parts_of(&src.files);
    if blueprints.is_empty() {
        return bad("There are no .blueprint files in there.");
    }
    let basenames: Vec<String> = blueprints.iter().map(|(p, _)| bpmodel::basename_of(p)).collect();
    let rm = tri!(super::rig_map(state, &branch(state)).await.map_err(internal));
    let (skeleton, attach) = rm.resolve(&basenames);
    let skeleton = skeleton.filter(|s| super::has_baked_rig(s));
    let rig = skeleton.as_deref().and_then(super::rig);
    let scales = rm.scales_for(&attach, rig);
    let (files, name, kind) = (src.files, src.name, src.kind);
    let opened = tri!(
        blocking(move || {
            let rig = skeleton.as_deref().and_then(|s| super::rig(s).map(|r| (s, r)));
            bpmodel::open_project(&files, rig, &attach, &name, scales.head, &scales.parts)
        })
        .await
    );
    let mut payload = tri!(opened.map_err(fault));
    if let Some(m) = payload.as_object_mut() {
        m.insert("source".into(), kind.into());
    }
    no_store(payload)
}

// --- /game-model ----------------------------------------------------------------------

#[cfg(test)]
pub static TEST_FILES: std::sync::OnceLock<std::collections::HashMap<String, Vec<u8>>> = std::sync::OnceLock::new();

/// `embed.service._read_game_file(path)`.
async fn read_game_file(state: &AppState, path: &str) -> Result<Option<Vec<u8>>, ApiError> {
    #[cfg(test)]
    if let Some(files) = TEST_FILES.get() {
        return Ok(files.get(path).cloned());
    }
    let Some(sha) = crate::media::file_sha(state, ws::LIVE_BRANCH, path).await? else { return Ok(None) };
    crate::media::read_blob(crate::media::update_store(state), sha).await
}

pub async fn game_model(State(state): State<AppState>, req: Request) -> Response {
    let (mut parts, _) = req.into_parts();
    tri!(gate(&state, &mut parts).await);
    game_model_reply(&state, &parts).await
}

pub async fn game_model_reply(state: &AppState, parts: &Parts) -> Response {
    let mut p = Params::new(Vec::new(), parts.uri.query(), &parts.headers);
    let prefab = p.query("prefab").str().min_len(1).required();
    if let Err(e) = p.v.finish(()) {
        return e.into_response();
    }
    let rm = tri!(super::rig_map(state, &branch(state)).await.map_err(internal));
    let (canonical, candidates) = rm.prefab_path(&prefab);
    let Some(canonical) = canonical else {
        if let Some(first) = candidates.first() {
            return bad(format!(
                "'{prefab}' names {} different creatures. Use the full path: {first}",
                candidates.len()
            ));
        }
        return ApiError::not_found(format!("The game data has no creature at '{prefab}'.")).into_response();
    };
    let (skeleton, bound) = rm.creature_by_prefab(&canonical);
    let Some(skeleton) = skeleton.filter(|s| !s.is_empty() && !bound.is_empty()) else {
        return ApiError::not_found("That prefab binds no blueprint parts.").into_response();
    };
    let idx = tri!(crate::workshop::game::index(state, ws::LIVE_BRANCH).await.map_err(internal));
    let mut files = Vec::new();
    for (basename, _) in &bound {
        let candidates = idx.all.get(&format!("{basename}.blueprint")).cloned().unwrap_or_default();
        let Some(path) = ws::nearest_path(&candidates, &canonical) else { continue };
        match read_game_file(state, path).await {
            Ok(Some(raw)) if !raw.is_empty() => files.push((format!("{basename}.blueprint"), raw)),
            Ok(_) => {}
            Err(e) => return e.into_response(),
        }
    }
    if files.is_empty() {
        return ApiError::not_found("None of that creature's parts are in the game archive.").into_response();
    }
    let rig_name = Some(skeleton.clone()).filter(|s| super::has_baked_rig(s));
    let scales = rm.creature_scales(&canonical, super::rig);
    let name = format!("{}.zip", prefab_stem(&canonical));
    let opened = tri!(
        blocking(move || {
            let rig = rig_name.as_deref().and_then(|s| super::rig(s).map(|r| (s, r)));
            bpmodel::open_project(&files, rig, &bound, &name, scales.head, &scales.parts)
        })
        .await
    );
    let mut payload = tri!(opened.map_err(fault));
    if let Some(m) = payload.as_object_mut() {
        m.insert("source".into(), "game".into());
        m.insert("prefab".into(), canonical.into());
    }
    no_store(payload)
}

// --- /model-save ------------------------------------------------------------------------

pub async fn model_save(State(state): State<AppState>, req: Request) -> Response {
    let (_, items) = tri!(prelude(&state, req).await);
    model_save_form(items).await
}

/// `re.sub(r"\.(tmod|zip)$", "", name, flags=re.I)`.
fn drop_archive_ext(name: &str) -> String {
    let (body, nl) = name.strip_suffix('\n').map_or((name, ""), |b| (b, "\n"));
    let low = body.to_ascii_lowercase();
    for ext in [".tmod", ".zip"] {
        if low.ends_with(ext) {
            return format!("{}{nl}", &body[..body.len() - ext.len()]);
        }
    }
    name.to_owned()
}

/// The rebuilt archive, its extension and the summary.
type Saved = (Vec<u8>, &'static str, Map<String, Value>);

fn props_vec(m: &Map<String, Value>) -> ws::Props {
    m.iter()
        .map(|(k, v)| (k.clone(), v.as_str().map_or_else(|| kiwi_core::bpedit::py::str_of(v), str::to_owned)))
        .collect()
}

pub async fn model_save_form(items: Items) -> Response {
    let mut errors = Vec::new();
    let file = form::file_opt(&items, "file", &mut errors);
    let files = form::files(&items, "files", &mut errors);
    let paths = form::text(&items, "paths", "[]", &mut errors);
    let edits = form::text(&items, "edits", "{}", &mut errors);
    let moves = form::text(&items, "moves", "{}", &mut errors);
    let name = form::text(&items, "name", "", &mut errors);
    tri!(form::check(errors));
    let has_file = file.is_some();
    let src = tri!(model_files(file, if has_file { Vec::new() } else { files.clone() }).await);
    let tail = name.rsplit('/').next().unwrap_or(&name).to_owned();
    let name = if tail.is_empty() { src.name.clone() } else { tail };
    let parse = |s: &str, default: &'static str| form::loads(if s.is_empty() { default } else { s });
    let not_understood = || bad("The edit list wasn't understood.");
    let parsed = tri!(parse(&edits, "{}"));
    let parsed_moves = tri!(parse(&moves, "{}"));
    let extra_paths = tri!(parse(&paths, "[]"));
    let (Some(Value::Object(parsed)), Some(Value::Object(parsed_moves)), Some(Value::Array(extra_paths))) =
        (parsed, parsed_moves, extra_paths)
    else {
        return not_understood();
    };
    let mut extra = Vec::new();
    if has_file {
        if extra_paths.len() != files.len() {
            return bad("The added parts didn't match the paths that arrived.");
        }
        for (part, want) in files.into_iter().zip(&extra_paths) {
            extra.push((bpmodel::pack_path(&bpmodel::wanted_path(want, &part.filename)), part.data));
        }
    }
    let (kind, props, unpacked) = (src.kind, props_vec(&src.props), src.files);
    let done = tri!(
        blocking(move || -> Result<Saved, Fault> {
            let (edited, summary) = bpmodel::apply_project(&unpacked, &parsed, extra, &parsed_moves)?;
            if kind == "tmod" {
                let out = crate::tools::tmod::build(1, &props, &edited, crate::tools::tmod::KIWI_MOD_LOADER, true)
                    .map_err(|e| match e {
                        crate::tools::tmod::BuildError::Bad(m) => {
                            Fault::Editor(format!("That mod couldn't be rebuilt: {m}"))
                        }
                        crate::tools::tmod::BuildError::Overflow => Fault::Unhandled("OverflowError".into()),
                    })?;
                return Ok((out, "tmod", summary));
            }
            let out = crate::blueprints::pack::zip(&edited, crate::blueprints::pack::now_date_time())
                .ok_or_else(|| Fault::Unhandled("zip".into()))?;
            Ok((out, "zip", summary))
        })
        .await
    );
    let (out, ext, summary) = tri!(done.map_err(fault));
    let stem = drop_archive_ext(&name);
    let stem = if stem.is_empty() { "model".to_owned() } else { stem };
    let filename = format!("{stem}.{ext}");
    let mut r = Response::new(axum::body::Body::from(out));
    let h = r.headers_mut();
    let set = |h: &mut axum::http::HeaderMap, k: &'static str, v: String| {
        if let Ok(v) = HeaderValue::from_str(&v) {
            h.insert(k, v);
        }
    };
    set(h, "content-disposition", crate::blueprints::editor::disposition(&filename, ""));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    set(h, "x-kiwi-summary", pyjson::dumps_spaced(&Value::Object(summary)));
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    r
}
