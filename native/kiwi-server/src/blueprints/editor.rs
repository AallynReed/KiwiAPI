//! `/site/blueprint-editor/*` in FastAPI's order: the form is read, the site
//! gate and the per-IP throttle run, the fields are validated, then the work
//! runs off the runtime.

use axum::{
    extract::{FromRequestParts, Request, State},
    http::{HeaderValue, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
};
use kiwi_core::{
    bpedit::{Fault, editor},
    form::{FormValue, UploadFile},
};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::Value;

use super::{SiteGate, pack};
use crate::{
    auth::client::client_ip,
    error::ApiError,
    pyjson::{self, LoadsError},
    ratelimit::check_rate_limit,
    routes::misc::feedback::str_field,
    runtime_config,
    state::AppState,
    validation::{ErrorItem, Fail, Field, Input, LocItem, ValidationError, Validator},
};

pub type Items = Vec<(String, FormValue)>;

/// A route's work once the form is read and the gate and throttle passed.
pub async fn handle(route: &str, items: Items) -> Response {
    match route {
        "inspect" => inspect_form(items).await,
        "flatten" => flatten_form(items).await,
        "transform" => transform_form(items).await,
        "export-qb" => export_qb_form(items).await,
        "import-qb" => import_qb_form(items).await,
        "check" => check_form(items).await,
        _ => save_form(items).await,
    }
}

fn bad(msg: impl Into<String>) -> Response {
    ApiError::bad_request(msg).into_response()
}

/// `EditorError` is the 400; anything else escaping the engine is a 500.
fn fault(f: Fault) -> Response {
    match f {
        Fault::Editor(m) => bad(m),
        other => {
            tracing::error!("blueprint editor: {}", other.message());
            ApiError::internal().into_response()
        }
    }
}

/// Form, gate, throttle: everything FastAPI settles before validating fields.
async fn prelude(state: &AppState, req: Request) -> Result<(Parts, Items), Response> {
    let (mut parts, body) = req.into_parts();
    let ct = parts.headers.get(header::CONTENT_TYPE).map(|v| v.as_bytes().to_vec());
    let (kind, _) = kiwi_core::form::parse_options_header(ct.as_deref());
    let items = if kind == b"multipart/form-data" || kind == b"application/x-www-form-urlencoded" {
        let body = axum::body::to_bytes(body, usize::MAX).await.map_err(|_| {
            ApiError::http(StatusCode::BAD_REQUEST, Some("There was an error parsing the body")).into_response()
        })?;
        tokio::task::spawn_blocking(move || kiwi_core::form::parse_form(ct.as_deref(), &body))
            .await
            .map_err(|_| ApiError::internal().into_response())?
            .map_err(|kiwi_core::form::FormError::Http(m)| {
                ApiError::http(StatusCode::BAD_REQUEST, Some(&m)).into_response()
            })?
    } else {
        Vec::new()
    };
    SiteGate::from_request_parts(&mut parts, state).await?;
    let (max, window) = runtime_config::get_rate_limit(state, "blueprint_editor_rate_limit").await.map_err(|e| {
        tracing::error!("blueprint_editor_rate_limit: {e:#}");
        ApiError::internal().into_response()
    })?;
    let ip = client_ip(&parts).unwrap_or_else(|| "unknown".into());
    check_rate_limit(state, &format!("bpeditor:{ip}"), max, window).await.map_err(IntoResponse::into_response)?;
    Ok((parts, items))
}

fn loc(path: &[LocItem]) -> Vec<LocItem> {
    let mut l = vec![LocItem::from("body")];
    l.extend_from_slice(path);
    l
}

fn missing(name: &str) -> ErrorItem {
    ErrorItem {
        kind: "missing".into(),
        loc: loc(&[name.into()]),
        msg: "Field required".into(),
        input: Value::Null,
        ctx: None,
    }
}

fn not_upload(at: Vec<LocItem>, text: &str) -> ErrorItem {
    let fail = Fail::value_error("Expected UploadFile, received: <class 'str'>");
    ErrorItem { kind: fail.kind, loc: at, msg: fail.msg, input: Value::String(text.into()), ctx: fail.ctx }
}

/// `UploadFile = File(...)`.
fn file(items: &Items, name: &str, errors: &mut Vec<ErrorItem>) -> Option<UploadFile> {
    match items.iter().rev().find(|(k, _)| k == name).map(|(_, v)| v) {
        Some(FormValue::File(f)) => Some(f.clone()),
        Some(FormValue::Text(t)) if !t.is_empty() => {
            errors.push(not_upload(loc(&[name.into()]), t));
            None
        }
        _ => {
            errors.push(missing(name));
            None
        }
    }
}

/// `list[UploadFile] = File(...)` (`required`) or `File(default=[])`.
fn files(items: &Items, name: &str, required: bool, errors: &mut Vec<ErrorItem>) -> Vec<UploadFile> {
    let all: Vec<&FormValue> = items.iter().filter(|(k, _)| k == name).map(|(_, v)| v).collect();
    if all.is_empty() {
        if required {
            errors.push(missing(name));
        }
        return Vec::new();
    }
    let mut out = Vec::new();
    for (i, v) in all.into_iter().enumerate() {
        match v {
            FormValue::File(f) => out.push(f.clone()),
            FormValue::Text(t) => errors.push(not_upload(loc(&[name.into(), LocItem::Index(i)]), t)),
        }
    }
    out
}

/// `str = Form(default=...)`.
fn text(items: &Items, name: &str, default: &str, errors: &mut Vec<ErrorItem>) -> String {
    str_field(items, name, None, None, errors).unwrap_or_else(|| default.to_owned())
}

/// `int = Form(default=0)`.
fn int(items: &Items, name: &str, errors: &mut Vec<ErrorItem>) -> i64 {
    let mut v = Validator::default();
    let value = match items.iter().rev().find(|(k, _)| k == name).map(|(_, v)| v) {
        None => return 0,
        Some(FormValue::Text(t)) if t.is_empty() => return 0,
        Some(FormValue::Text(t)) => {
            Field::new(&mut v, loc(&[name.into()]), Input::Str(t.as_str().into())).int().required()
        }
        Some(FormValue::File(f)) => {
            Field::new(&mut v, loc(&[name.into()]), Input::Str(f.filename.as_str().into())).fail(Fail::int_type());
            0
        }
    };
    errors.extend(v.errors);
    value
}

/// `json.loads(text)` on a `str`. `Ok(None)` is the `ValueError` the route
/// catches; NaN/Infinity (which Python accepts) can't be held, so they are a 500.
fn loads(s: &str) -> Result<Option<Value>, Rej> {
    if s.starts_with('\u{feff}') {
        return Ok(None);
    }
    match pyjson::loads(s.as_bytes()) {
        Ok(v) => Ok(Some(v)),
        Err(LoadsError::NonFinite) => Err(Box::new(ApiError::internal().into_response())),
        Err(_) => Ok(None),
    }
}

/// `json.loads(...)`, its `ValueError` answered as `message`.
fn parse(s: &str, message: &str) -> Result<Value, Rej> {
    loads(s)?.ok_or_else(|| Box::new(bad(message)))
}

/// `_bp_stack(layers, stack)`: the named layer files and their placements.
fn stack(layers: &[UploadFile], stack: &str) -> Result<(Vec<Vec<u8>>, Value), Rej> {
    let data: Vec<Vec<u8>> = layers.iter().filter(|f| !f.filename.is_empty()).map(|f| f.data.clone()).collect();
    if data.is_empty() {
        return Ok((Vec::new(), Value::Array(Vec::new())));
    }
    let specs = parse(if stack.is_empty() { "[]" } else { stack }, "The layer list wasn't understood.")?;
    if !specs.is_array() {
        return Err(Box::new(bad("The layer list wasn't understood.")));
    }
    Ok((data, specs))
}

async fn run<T: Send + 'static>(f: impl FnOnce() -> Result<T, Fault> + Send + 'static) -> Result<T, Response> {
    tokio::task::spawn_blocking(f).await.map_err(|_| ApiError::internal().into_response())?.map_err(fault)
}

fn header_value(s: &str) -> HeaderValue {
    HeaderValue::from_str(s).unwrap_or_else(|_| HeaderValue::from_static(""))
}

fn octet(body: Vec<u8>, extra: Vec<(&'static str, String)>) -> Response {
    let mut res = Response::new(axum::body::Body::from(body));
    let h = res.headers_mut();
    for (k, v) in extra {
        h.insert(k, header_value(&v));
    }
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    res
}

fn json_no_store(v: &Value) -> Response {
    let mut res = pyjson::PyJson(v.clone()).into_response();
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    res
}

fn no_errors(errors: Vec<ErrorItem>) -> Result<(), Rej> {
    if errors.is_empty() { Ok(()) } else { Err(Box::new(ValidationError(errors).into_response())) }
}

/// `name.rsplit("/", 1)[-1]`.
fn tail(name: &str) -> &str {
    name.rsplit('/').next().unwrap_or(name)
}

fn edits_or_default(s: &str) -> &str {
    if s.is_empty() { "[]" } else { s }
}

/// An early reply from a sync helper (boxed: a `Response` is large).
type Rej = Box<Response>;

trait Reply {
    fn reply(self) -> Response;
}

impl Reply for Response {
    fn reply(self) -> Response {
        self
    }
}

impl Reply for Rej {
    fn reply(self) -> Response {
        *self
    }
}

macro_rules! tri {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(r) => return Reply::reply(r),
        }
    };
}

pub async fn inspect(State(state): State<AppState>, req: Request) -> Response {
    match prelude(&state, req).await {
        Ok((_, items)) => inspect_form(items).await,
        Err(r) => r,
    }
}

async fn inspect_form(items: Items) -> Response {
    let mut errors = Vec::new();
    let f = file(&items, "file", &mut errors);
    tri!(no_errors(errors));
    let f = f.unwrap_or_else(|| unreachable!());
    if f.data.is_empty() {
        return bad("That file is empty.");
    }
    let name = tail(if f.filename.is_empty() { "blueprint" } else { &f.filename }).to_owned();
    let payload = tri!(run(move || editor::inspect(&f.data, &name)).await);
    json_no_store(&payload)
}

struct StackForm {
    file: UploadFile,
    layers: Vec<UploadFile>,
    edits: String,
    stack: String,
    anchor_at: i64,
    kind: String,
}

fn stack_form(items: &Items, with_kind: bool) -> Result<StackForm, Rej> {
    let mut errors = Vec::new();
    let file = file(items, "file", &mut errors);
    let layers = files(items, "layers", false, &mut errors);
    let edits = text(items, "edits", "[]", &mut errors);
    let stack = text(items, "stack", "[]", &mut errors);
    let anchor_at = int(items, "anchor_at", &mut errors);
    let kind = if with_kind { text(items, "kind", "other", &mut errors) } else { String::new() };
    no_errors(errors)?;
    Ok(StackForm { file: file.unwrap_or_else(|| unreachable!()), layers, edits, stack, anchor_at, kind })
}

fn summary_header(summary: &serde_json::Map<String, Value>) -> String {
    pyjson::dumps_spaced(&Value::Object(summary.clone()))
}

pub async fn flatten(State(state): State<AppState>, req: Request) -> Response {
    match prelude(&state, req).await {
        Ok((_, items)) => flatten_form(items).await,
        Err(r) => r,
    }
}

async fn flatten_form(items: Items) -> Response {
    let f = tri!(stack_form(&items, false));
    if f.file.data.is_empty() {
        return bad("That file is empty.");
    }
    let (layers, specs) = tri!(stack(&f.layers, &f.stack));
    if layers.is_empty() {
        return bad("There are no layers to flatten.");
    }
    let edits = tri!(parse(edits_or_default(&f.edits), "The edit list wasn't understood."));
    let (data, anchor) = (f.file.data, f.anchor_at);
    let (out, summary) = tri!(
        run(move || {
            let refs: Vec<&[u8]> = layers.iter().map(Vec::as_slice).collect();
            editor::composite(&data, &edits, &refs, &specs, anchor)
        })
        .await
    );
    octet(out, vec![("cache-control", "no-store".into()), ("x-kiwi-summary", summary_header(&summary))])
}

pub async fn transform(State(state): State<AppState>, req: Request) -> Response {
    match prelude(&state, req).await {
        Ok((_, items)) => transform_form(items).await,
        Err(r) => r,
    }
}

async fn transform_form(items: Items) -> Response {
    let mut errors = Vec::new();
    let file = file(&items, "file", &mut errors);
    let edits = text(&items, "edits", "[]", &mut errors);
    let ops = crate::drops::upload::required_str(&items, "ops", &mut errors);
    tri!(no_errors(errors));
    let data = file.unwrap_or_else(|| unreachable!()).data;
    if data.is_empty() {
        return bad("That file is empty.");
    }
    let message = "The request wasn't understood.";
    let edits = tri!(parse(edits_or_default(&edits), message));
    let ops = tri!(parse(if ops.is_empty() { "[]" } else { &ops }, message));
    let (out, summary) = tri!(run(move || editor::transform(&data, &edits, &ops)).await);
    octet(out, vec![("cache-control", "no-store".into()), ("x-kiwi-summary", pyjson::dumps_spaced(&summary))])
}

/// `re.sub(r"[^A-Za-z0-9._-]", "_", stem) or "model"` after the extension is cut.
fn qb_stem(filename: &str) -> String {
    let stem = tail(if filename.is_empty() { "model" } else { filename });
    let stem = if stem.to_lowercase().ends_with(".blueprint") {
        let keep = stem.chars().count().saturating_sub(".blueprint".len());
        stem.chars().take(keep).collect()
    } else {
        stem.to_owned()
    };
    let clean: String =
        stem.chars().map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' }).collect();
    if clean.is_empty() { "model".into() } else { clean }
}

pub async fn export_qb(State(state): State<AppState>, req: Request) -> Response {
    match prelude(&state, req).await {
        Ok((_, items)) => export_qb_form(items).await,
        Err(r) => r,
    }
}

async fn export_qb_form(items: Items) -> Response {
    let f = tri!(stack_form(&items, false));
    if f.file.data.is_empty() {
        return bad("That file is empty.");
    }
    let edits = tri!(parse(edits_or_default(&f.edits), "The edit list wasn't understood."));
    let stem = qb_stem(&f.file.filename);
    let (layers, specs) = tri!(stack(&f.layers, &f.stack));
    let (data, anchor, s) = (f.file.data, f.anchor_at, stem.clone());
    let (entries, summary) = tri!(
        run(move || {
            let refs: Vec<&[u8]> = layers.iter().map(Vec::as_slice).collect();
            editor::export_qb(&data, &edits, &refs, &specs, anchor, &s)
        })
        .await
    );
    let Some(archive) = pack::zip(&entries, pack::now_date_time()) else { return ApiError::internal().into_response() };
    let mut res = octet(
        archive,
        vec![
            ("content-disposition", format!("attachment; filename=\"{stem}_qb.zip\"")),
            ("cache-control", "no-store".into()),
            ("x-kiwi-notes", pyjson::dumps_spaced(&summary["notes"])),
        ],
    );
    res.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/zip"));
    res
}

pub async fn import_qb(State(state): State<AppState>, req: Request) -> Response {
    match prelude(&state, req).await {
        Ok((_, items)) => import_qb_form(items).await,
        Err(r) => r,
    }
}

async fn import_qb_form(items: Items) -> Response {
    let mut errors = Vec::new();
    let uploads = files(&items, "files", true, &mut errors);
    tri!(no_errors(errors));
    let mut parts: Vec<(String, Vec<u8>)> = Vec::new();
    for up in uploads {
        if up.filename.is_empty() {
            continue;
        }
        let name = tail(&up.filename).to_owned();
        if !name.to_lowercase().ends_with(".qb") {
            return bad(format!("'{name}' isn't a .qb file."));
        }
        match parts.iter_mut().find(|(n, _)| *n == name) {
            Some(slot) => slot.1 = up.data,
            None => parts.push((name, up.data)),
        }
    }
    if parts.is_empty() {
        return bad("No .qb files were sent.");
    }
    if parts.len() > 4 {
        return bad("Send the model and up to three material maps.");
    }
    let (data, summary) = tri!(run(move || editor::import_qb(&parts)).await);
    octet(data, vec![("cache-control", "no-store".into()), ("x-kiwi-summary", pyjson::dumps_spaced(&summary))])
}

pub async fn check(State(state): State<AppState>, req: Request) -> Response {
    match prelude(&state, req).await {
        Ok((_, items)) => check_form(items).await,
        Err(r) => r,
    }
}

async fn check_form(items: Items) -> Response {
    let f = tri!(stack_form(&items, true));
    if f.file.data.is_empty() {
        return bad("That file is empty.");
    }
    let edits = tri!(parse(edits_or_default(&f.edits), "The edit list wasn't understood."));
    let (layers, specs) = tri!(stack(&f.layers, &f.stack));
    let (data, anchor, kind) = (f.file.data, f.anchor_at, f.kind);
    let report = tri!(
        run(move || {
            let refs: Vec<&[u8]> = layers.iter().map(Vec::as_slice).collect();
            editor::check(&data, &edits, &kind, &refs, &specs, anchor)
        })
        .await
    );
    json_no_store(&report)
}

/// `urllib.parse.quote(name)`: everything but `A-Za-z0-9_.-~/` escaped.
const QUOTE: &AsciiSet = &NON_ALPHANUMERIC.remove(b'_').remove(b'.').remove(b'-').remove(b'~').remove(b'/');

/// `attachment; filename="<ASCII fallback>"; filename*=UTF-8''<quoted>`.
pub fn disposition(name: &str, empty_fallback: &str) -> String {
    let fallback: String =
        name.chars().filter(|&c| c != '"').map(|c| if (' '..='~').contains(&c) { c } else { '_' }).collect();
    let fallback = if fallback.is_empty() { empty_fallback.to_owned() } else { fallback };
    format!("attachment; filename=\"{fallback}\"; filename*=UTF-8''{}", utf8_percent_encode(name, QUOTE))
}

pub async fn save(State(state): State<AppState>, req: Request) -> Response {
    match prelude(&state, req).await {
        Ok((_, items)) => save_form(items).await,
        Err(r) => r,
    }
}

async fn save_form(items: Items) -> Response {
    let f = tri!(stack_form(&items, false));
    if f.file.data.is_empty() {
        return bad("That file is empty.");
    }
    let edits = tri!(parse(edits_or_default(&f.edits), "The edit list wasn't understood."));
    let (layers, specs) = tri!(stack(&f.layers, &f.stack));
    let (data, anchor) = (f.file.data, f.anchor_at);
    let (out, summary) = tri!(
        run(move || {
            let refs: Vec<&[u8]> = layers.iter().map(Vec::as_slice).collect();
            editor::composite(&data, &edits, &refs, &specs, anchor)
        })
        .await
    );
    let mut name = tail(if f.file.filename.is_empty() { "blueprint.blueprint" } else { &f.file.filename }).to_owned();
    if !name.to_lowercase().ends_with(".blueprint") {
        name.push_str(".blueprint");
    }
    let n = |k: &str| summary.get(k).map(Value::to_string).unwrap_or_default();
    octet(
        out,
        vec![
            ("content-disposition", disposition(&name, "edited.blueprint")),
            ("cache-control", "no-store".into()),
            ("x-kiwi-recoloured", n("recoloured")),
            ("x-kiwi-rematerialised", n("rematerialised")),
            ("x-kiwi-ignored", n("ignored")),
        ],
    )
}
