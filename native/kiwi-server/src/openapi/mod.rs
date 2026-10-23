//! `GET /openapi.json` as `app/main.py` serves it: FastAPI's generated spec
//! (`base.json`), `custom_openapi`'s 🔓 / ⭐ markers, and the operations of
//! every feature whose toggle is off pruned at request time. `base.json` and
//! `ops.json` come from native/port/parity/gen_openapi.py.

use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

use axum::{
    Router,
    extract::State,
    response::{IntoResponse, Response},
    routing::get,
};
use bytes::Bytes;
use kiwi_core::pystr;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{config::Settings, error::ApiError, features, pyjson, state::AppState};

#[cfg(test)]
mod tests;

const BASE: &str = include_str!("base.json");
const OPS: &str = include_str!("ops.json");

const TOKENLESS_NOTE: &str = "\n\n> 🔓 **No token required.** This endpoint works without an API token - \
    anonymous callers are allowed but share a tighter per-IP rate limit; send a token carrying this scope for \
    the higher per-token limit (and usage accounting).";
const TOKENLESS_MARKER: &str = "🔓 **No token required";
const MASTER_NOTE: &str = "\n\n> ⭐ **Master only.** Requires a superuser-owned API token (or a master \
    session in the dev portal); a normal token gets 403.";
const MASTER_MARKER: &str = "⭐ **Master only";
const SCHEME_NOTE: &str = " Endpoints marked 🔓 are tokenless (callable without this token at a tighter \
    per-IP rate limit); endpoints marked ⭐ require a superuser token.";
const HTTP_METHODS: [&str; 8] = ["get", "post", "put", "patch", "delete", "options", "head", "trace"];

#[derive(Deserialize)]
struct Ops {
    tokenless: Vec<(String, String)>,
    master: Vec<(String, String)>,
    gated: Vec<(String, String, String)>,
}

/// The marked spec for one `(app_name, api_url)`, rendered once.
pub struct Spec {
    title: String,
    server: String,
    schema: Value,
    full: Bytes,
    gated: Vec<(String, String, String)>,
    flags: Vec<String>,
    pruned: Mutex<Option<(Vec<String>, Bytes)>>,
}

fn text(v: Option<&Value>) -> &str {
    v.and_then(Value::as_str).unwrap_or("")
}

fn rstrip(s: &str) -> &str {
    s.trim_end_matches(pystr::is_space)
}

/// `custom_openapi`'s pass over the generated schema.
fn mark(schema: &mut Value, ops: &Ops) {
    let tokenless: HashSet<(&str, &str)> = ops.tokenless.iter().map(|(p, m)| (p.as_str(), m.as_str())).collect();
    let master: HashSet<(&str, &str)> = ops.master.iter().map(|(p, m)| (p.as_str(), m.as_str())).collect();
    if let Some(paths) = schema.get_mut("paths").and_then(Value::as_object_mut) {
        for (path, item) in paths.iter_mut() {
            let Some(item) = item.as_object_mut() else { continue };
            for (method, op) in item.iter_mut() {
                if !HTTP_METHODS.contains(&method.as_str()) {
                    continue;
                }
                let Some(op) = op.as_object_mut() else { continue };
                let key = (path.as_str(), method.as_str());
                if tokenless.contains(&key) || op.get("x-tokenless") == Some(&Value::Bool(true)) {
                    flag_op(op, "🔓", TOKENLESS_MARKER, TOKENLESS_NOTE);
                    if let Some(Value::Array(sec)) = op.get_mut("security")
                        && !sec.iter().any(|v| v.as_object().is_some_and(Map::is_empty))
                    {
                        sec.insert(0, json!({}));
                    }
                } else if master.contains(&key) {
                    flag_op(op, "⭐", MASTER_MARKER, MASTER_NOTE);
                }
            }
        }
    }
    if let Some(scheme) = schema.pointer_mut("/components/securitySchemes/APIToken").and_then(Value::as_object_mut)
        && !scheme.is_empty()
        && !text(scheme.get("description")).contains('🔓')
    {
        let desc = format!("{}{SCHEME_NOTE}", rstrip(text(scheme.get("description"))));
        scheme.insert("description".into(), Value::String(pystr::strip(&desc).to_owned()));
    }
}

fn flag_op(op: &mut Map<String, Value>, badge: &str, marker: &str, note: &str) {
    let summary = text(op.get("summary"));
    if !summary.starts_with(badge) {
        let marked = format!("{badge} {summary}");
        op.insert("summary".into(), Value::String(rstrip(&marked).to_owned()));
    }
    let desc = text(op.get("description"));
    if !desc.contains(marker) {
        op.insert("description".into(), Value::String(format!("{desc}{note}")));
    }
}

/// Drop every operation gated on a `disabled` flag, and a path left empty.
fn prune(schema: &mut Value, gated: &[(String, String, String)], disabled: &[String]) {
    let Some(paths) = schema.get_mut("paths").and_then(Value::as_object_mut) else { return };
    for (path, method, flag) in gated {
        if !disabled.contains(flag) {
            continue;
        }
        let Some(item) = paths.get_mut(path).and_then(Value::as_object_mut) else { continue };
        item.shift_remove(method);
        if item.is_empty() {
            paths.shift_remove(path);
        }
    }
}

impl Spec {
    pub fn build(title: &str, server: &str) -> Spec {
        let mut schema: Value = serde_json::from_str(BASE).expect("openapi base.json");
        let ops: Ops = serde_json::from_str(OPS).expect("openapi ops.json");
        mark(&mut schema, &ops);
        if let Some(info) = schema.get_mut("info").and_then(Value::as_object_mut) {
            info.insert("title".into(), Value::String(title.to_owned()));
        }
        if let Some(first) = schema.pointer_mut("/servers/0").and_then(Value::as_object_mut) {
            first.insert("url".into(), Value::String(server.to_owned()));
        }
        let mut flags: Vec<String> = ops.gated.iter().map(|(_, _, f)| f.clone()).collect();
        flags.sort();
        flags.dedup();
        let full = Bytes::from(pyjson::dumps(&schema));
        Spec {
            title: title.to_owned(),
            server: server.to_owned(),
            schema,
            full,
            gated: ops.gated,
            flags,
            pruned: Mutex::new(None),
        }
    }

    /// Every feature flag that gates an operation.
    pub fn flags(&self) -> &[String] {
        &self.flags
    }

    /// The body with the operations of `disabled` (sorted) removed.
    pub fn render(&self, disabled: &[String]) -> Bytes {
        if disabled.is_empty() {
            return self.full.clone();
        }
        let mut last = self.pruned.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((key, body)) = last.as_ref()
            && key == disabled
        {
            return body.clone();
        }
        let mut schema = self.schema.clone();
        prune(&mut schema, &self.gated, disabled);
        let body = Bytes::from(pyjson::dumps(&schema));
        *last = Some((disabled.to_vec(), body.clone()));
        body
    }
}

pub fn spec(settings: &Settings) -> Arc<Spec> {
    static CACHE: Mutex<Option<Arc<Spec>>> = Mutex::new(None);
    let mut cached = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = cached.as_ref()
        && s.title == settings.app_name
        && s.server == settings.api_url
    {
        return s.clone();
    }
    let s = Arc::new(Spec::build(&settings.app_name, &settings.api_url));
    *cached = Some(s.clone());
    s
}

pub fn router() -> Router<AppState> {
    Router::new().route("/openapi.json", get(openapi_json))
}

async fn openapi_json(State(state): State<AppState>) -> Response {
    let spec = spec(state.settings());
    let mut disabled = Vec::new();
    for flag in spec.flags() {
        match features::is_enabled(&state, flag).await {
            Ok(true) => {}
            Ok(false) => disabled.push(flag.clone()),
            Err(e) => {
                tracing::error!("openapi: feature flag {flag} lookup failed: {e:#}");
                return ApiError::internal().into_response();
            }
        }
    }
    pyjson::json_response(spec.render(&disabled))
}
