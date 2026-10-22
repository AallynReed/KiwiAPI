//! `POST /v1/misc/feedback` (`app/trove/router.py` `post_feedback` and
//! `app/trove/misc.py` `insert_feedback`): FastAPI's form validation, the two
//! rate-limit buckets, a `feedback_entries` insert, then the Discord webhook
//! after the response.

use std::{sync::LazyLock, time::Duration};

use axum::{
    extract::{Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use kiwi_core::{
    form::{self, FormError, FormValue, UploadFile},
    pystr,
};
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use regex::Regex;
use serde_json::{Map, Value, json};

use crate::{
    auth::client::client_ip,
    dt,
    error::ApiError,
    pyjson::{self, PydanticJson},
    ratelimit::check_rate_limit,
    runtime_config::get_setting,
    state::AppState,
    validation::{ErrorItem, LocItem, ValidationError},
};

pub const COLLECTION: &str = "feedback_entries";
const MAX_ATTACHMENTS: usize = 4;
const MAX_FILE_BYTES: usize = 5 * 1024 * 1024;
const ALLOWED_MIME: [&str; 4] = ["image/png", "image/jpeg", "image/webp", "image/gif"];
const SPOOL_MAX: usize = 1024 * 1024;

/// The validated form fields.
#[derive(Debug, PartialEq, Eq)]
pub struct FeedbackForm {
    pub message: String,
    pub contact: Option<String>,
    pub category: String,
    pub app_version: Option<String>,
    pub attachments: Vec<UploadFile>,
}

/// `jsonable_encoder` of the `SpooledTemporaryFile` behind an `UploadFile`:
/// `dict(file)` over its lines, else `vars(file)` with `_file` iterated on.
fn spooled_json(data: &[u8]) -> Value {
    let lines: Vec<&[u8]> = data.split_inclusive(|&b| b == b'\n').collect();
    let pairs = |lines: &[&[u8]]| -> Option<Map<String, Value>> {
        let mut m = Map::new();
        for l in lines {
            if l.len() != 2 {
                return None;
            }
            m.insert(l[0].to_string(), Value::from(l[1]));
        }
        Some(m)
    };
    if let Some(m) = pairs(&lines) {
        return Value::Object(m);
    }
    let failed = lines.iter().position(|l| l.len() != 2).unwrap_or(0);
    let rest = pairs(&lines[failed + 1..]).unwrap_or_default();
    let rolled = data.len() > SPOOL_MAX;
    let mut v = json!({"_file": rest, "_max_size": SPOOL_MAX, "_rolled": rolled});
    if !rolled {
        v["_TemporaryFileArgs"] = json!({
            "mode": "w+b", "buffering": -1, "suffix": null, "prefix": null,
            "encoding": null, "newline": null, "dir": null, "errors": null,
        });
    }
    v
}

/// `jsonable_encoder(upload_file)`, the `input` of a type error on a str field.
fn upload_json(f: &UploadFile) -> Value {
    let mut headers = Map::new();
    for (k, v) in &f.headers {
        headers.entry(form::latin1(k)).or_insert_with(|| Value::String(form::latin1(v)));
    }
    json!({
        "filename": f.filename,
        "file": spooled_json(&f.data),
        "size": f.data.len(),
        "headers": headers,
        "_max_mem_size": SPOOL_MAX,
    })
}

fn item(kind: &'static str, loc: Vec<LocItem>, msg: String, input: Value, ctx: Option<Value>) -> ErrorItem {
    ErrorItem {
        kind: kind.into(),
        loc,
        msg,
        input,
        ctx: ctx.and_then(|c| match c {
            Value::Object(m) => Some(m),
            _ => None,
        }),
    }
}

fn loc(name: &str) -> Vec<LocItem> {
    vec!["body".into(), name.into()]
}

/// One `str` form field (the last value of the name): `None` when absent,
/// empty or invalid.
fn str_field(
    items: &[(String, FormValue)],
    name: &str,
    min: Option<usize>,
    max: Option<usize>,
    errors: &mut Vec<ErrorItem>,
) -> Option<String> {
    let value = items.iter().rev().find(|(k, _)| k == name).map(|(_, v)| v)?;
    match value {
        FormValue::Text(t) if t.is_empty() => None,
        FormValue::Text(t) => {
            let n = t.chars().count();
            if let Some(min) = min.filter(|&m| n < m) {
                let s = if min == 1 { "" } else { "s" };
                errors.push(item(
                    "string_too_short",
                    loc(name),
                    format!("String should have at least {min} character{s}"),
                    Value::String(t.clone()),
                    Some(json!({"min_length": min})),
                ));
                return None;
            }
            if let Some(max) = max.filter(|&m| n > m) {
                let s = if max == 1 { "" } else { "s" };
                errors.push(item(
                    "string_too_long",
                    loc(name),
                    format!("String should have at most {max} character{s}"),
                    Value::String(t.clone()),
                    Some(json!({"max_length": max})),
                ));
                return None;
            }
            Some(t.clone())
        }
        FormValue::File(f) => {
            errors.push(item("string_type", loc(name), "Input should be a valid string".into(), upload_json(f), None));
            None
        }
    }
}

/// FastAPI's form-field extraction and validation for the five parameters.
pub fn validate(items: &[(String, FormValue)]) -> Result<FeedbackForm, Vec<ErrorItem>> {
    let mut errors = Vec::new();
    let present = |name: &str| {
        items
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .is_some_and(|(_, v)| !matches!(v, FormValue::Text(t) if t.is_empty()))
    };
    if !present("message") {
        errors.push(item("missing", loc("message"), "Field required".into(), Value::Null, None));
    }
    let message = str_field(items, "message", Some(5), Some(2000), &mut errors);
    let contact = str_field(items, "contact", None, Some(200), &mut errors);
    let category = str_field(items, "category", None, None, &mut errors);
    let app_version = str_field(items, "app_version", None, Some(64), &mut errors);
    let mut attachments = Vec::new();
    for (i, (_, v)) in items.iter().filter(|(k, _)| k == "attachments").enumerate() {
        match v {
            FormValue::File(f) => attachments.push(f.clone()),
            FormValue::Text(t) => errors.push(item(
                "value_error",
                vec!["body".into(), "attachments".into(), i.into()],
                "Value error, Expected UploadFile, received: <class 'str'>".into(),
                Value::String(t.clone()),
                Some(json!({"error": {}})),
            )),
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }
    Ok(FeedbackForm {
        message: message.unwrap_or_default(),
        contact,
        category: category.unwrap_or_else(|| "general".into()),
        app_version,
        attachments,
    })
}

/// `_sanitise_filename`.
pub fn sanitise_filename(name: Option<&str>) -> String {
    let Some(name) = name.filter(|n| !n.is_empty()) else { return "image".into() };
    let base = name.rsplit('/').next().unwrap_or("");
    let base = base.rsplit('\\').next().unwrap_or("");
    let cleaned: String =
        base.chars().map(|c| if pystr::is_word(c) || c == '.' || c == '-' { c } else { '_' }).collect();
    let cut = pystr::prefix(&cleaned, 80);
    if cut.is_empty() { "image".into() } else { cut.to_owned() }
}

struct Rule {
    name: &'static str,
    re: Regex,
}

fn rules(list: &[(&'static str, &str)]) -> Vec<Rule> {
    list.iter().map(|(name, re)| Rule { name, re: Regex::new(&format!("(?i){re}")).expect("UA regex") }).collect()
}

static OS_RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    rules(&[
        ("iOS", r"\b(?:iPhone|iPad|iPod)(?:.*?OS\s+(\d+(?:[._]\d+)?))?"),
        ("Android", r"\bAndroid(?:\s+(\d+(?:\.\d+)?))?"),
        ("Windows", r"\bWindows NT\s+(\d+(?:\.\d+)?)"),
        ("macOS", r"\bMac OS X\s+(\d+(?:[._]\d+)?)"),
        ("Linux", r"\bLinux\b"),
    ])
});

static CLIENT_RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    rules(&[
        ("BetterTroveTools", r"\bBetterTroveTools/(\d+(?:\.\d+){1,3})"),
        ("Edge", r"\bEdg(?:e|A|iOS)?/(\d+(?:\.\d+)?)"),
        ("Opera", r"\bOPR/(\d+(?:\.\d+)?)"),
        ("Brave", r"\bBrave/(\d+(?:\.\d+)?)"),
        ("Firefox", r"\bFirefox/(\d+(?:\.\d+)?)"),
        ("Chrome", r"\bChrome/(\d+(?:\.\d+)?)"),
        ("Safari", r"\bVersion/(\d+(?:\.\d+)?).*?Safari"),
    ])
});

/// `parse_user_agent`. The UA is Latin-1 text; the two places Python's `re`
/// classes differ from Rust's there (`\w` takes ²³¹¼½¾, `\s` takes
/// U+001C..U+001F) are mapped to same-length stand-ins first.
pub fn parse_user_agent(ua: Option<&str>) -> (Option<String>, Option<String>) {
    let Some(ua) = ua.filter(|u| !u.is_empty()) else { return (None, None) };
    let s = pystr::prefix(ua, 500);
    let mapped: String = s
        .chars()
        .map(|c| match c {
            '²' | '³' | '¹' | '¼' | '½' | '¾' => 'ª',
            '\x1c'..='\x1f' => '\x0b',
            c => c,
        })
        .collect();
    let mut os = None;
    for rule in OS_RULES.iter() {
        let Some(m) = rule.re.captures(&mapped) else { continue };
        let ver = m.get(1).map(|g| &s[g.range()]);
        os = Some(match (rule.name, ver) {
            ("Windows", Some(v)) => {
                let friendly = match v {
                    "10.0" => "10/11",
                    "6.3" => "8.1",
                    "6.2" => "8",
                    "6.1" => "7",
                    v => v,
                };
                format!("Windows {friendly}")
            }
            ("macOS" | "iOS", Some(v)) => format!("{} {}", rule.name, v.replace('_', ".")),
            ("Android", Some(v)) => format!("Android {v}"),
            (name, _) => name.to_owned(),
        });
        break;
    }
    let client = CLIENT_RULES.iter().find_map(|rule| {
        rule.re.captures(&mapped).map(|m| format!("{} {}", rule.name, &s[m.get(1).map_or(0..0, |g| g.range())]))
    });
    (os, client)
}

fn bad(message: impl Into<String>) -> Response {
    ApiError::bad_request(message).into_response()
}

fn opt(v: &Option<String>) -> Bson {
    v.as_ref().map_or(Bson::Null, |s| Bson::String(s.clone()))
}

fn int(n: usize) -> Bson {
    i32::try_from(n).map_or(Bson::Int64(n as i64), Bson::Int32)
}

async fn setting(state: &AppState, key: &str) -> Result<i64, Response> {
    match get_setting(state, key).await {
        Ok(v) => v.as_i64().ok_or_else(|| ApiError::internal().into_response()),
        Err(err) => {
            tracing::error!("feedback: {key}: {err:#}");
            Err(ApiError::internal().into_response())
        }
    }
}

pub async fn post_feedback(State(state): State<AppState>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, usize::MAX).await else {
        return ApiError::http(StatusCode::BAD_REQUEST, Some("There was an error parsing the body")).into_response();
    };
    let items = match form::parse_form(parts.headers.get(header::CONTENT_TYPE).map(|v| v.as_bytes()), &bytes) {
        Ok(items) => items,
        Err(FormError::Http(msg)) => return ApiError::http(StatusCode::BAD_REQUEST, Some(&msg)).into_response(),
    };
    let f = match validate(&items) {
        Ok(f) => f,
        Err(errors) => return ValidationError(errors).into_response(),
    };
    drop(items);

    if !matches!(f.category.as_str(), "bug" | "feature" | "general") {
        return bad("category must be one of 'bug', 'feature', 'general'.");
    }
    let mut limits = [0i64; 4];
    for (slot, key) in limits.iter_mut().zip([
        "feedback.per_ip_max",
        "feedback.per_ip_window_seconds",
        "feedback.global_max",
        "feedback.global_window_seconds",
    ]) {
        *slot = match setting(&state, key).await {
            Ok(v) => v,
            Err(resp) => return resp,
        };
    }
    let ip = client_ip(&parts).unwrap_or_else(|| "unknown".into());
    let per_ip = match check_rate_limit(&state, &format!("feedback_ip:{ip}"), limits[0], limits[1]).await {
        Ok(info) => info,
        Err(rej) => return rej.into_response(),
    };
    if let Err(rej) = check_rate_limit(&state, "feedback_global", limits[2], limits[3]).await {
        return rej.into_response();
    }

    if f.attachments.len() > MAX_ATTACHMENTS {
        return bad(format!("At most {MAX_ATTACHMENTS} attachments allowed."));
    }
    let mut files: Vec<(String, Vec<u8>, String)> = Vec::new();
    for a in f.attachments {
        let ctype = a.content_type().unwrap_or_default().to_lowercase();
        if !ALLOWED_MIME.contains(&ctype.as_str()) {
            let shown = if ctype.is_empty() { "?" } else { &ctype };
            return bad(format!(
                "Attachment '{}' has unsupported type '{shown}'. Allowed: PNG, JPEG, WebP, GIF.",
                a.filename
            ));
        }
        if a.data.is_empty() {
            continue;
        }
        if a.data.len() > MAX_FILE_BYTES {
            return bad(format!("Attachment '{}' exceeds 5 MB.", a.filename));
        }
        files.push((sanitise_filename(Some(&a.filename)), a.data, ctype));
    }

    let message = pystr::strip(&f.message).to_owned();
    if message.chars().count() < 5 {
        return bad("Message must be at least 5 non-whitespace characters.");
    }
    let clean = |v: &Option<String>| v.as_deref().map(pystr::strip).filter(|s| !s.is_empty()).map(str::to_owned);
    let contact = clean(&f.contact);
    let app_version = clean(&f.app_version);
    let ua = parts.headers.get(header::USER_AGENT).map(|v| crate::request_id::latin1(v.as_bytes()));
    let ua = ua.as_deref().map(|u| pystr::prefix(u, 300).to_owned()).filter(|u| !u.is_empty());
    let (os, client) = parse_user_agent(ua.as_deref());
    let created_at = dt::now();
    let id = ObjectId::new();
    let attachments: Vec<Bson> = files
        .iter()
        .map(|(name, data, ctype)| {
            Bson::Document(doc! {"filename": name, "content_type": ctype, "size": int(data.len())})
        })
        .collect();
    let entry: Document = doc! {
        "_id": id,
        "message": &message,
        "contact": opt(&contact),
        "category": &f.category,
        "app_version": opt(&app_version),
        "os": opt(&os),
        "client": opt(&client),
        "user_agent": opt(&ua),
        "attachments": attachments,
        "created_at": dt::to_bson(created_at),
        "read": false,
    };
    if let Err(err) = state.mongo().collection::<Document>(COLLECTION).insert_one(entry).await {
        tracing::error!("feedback insert failed: {err}");
        return ApiError::internal().into_response();
    }

    let hook = Webhook {
        doc_id: id.to_hex(),
        category: f.category,
        message,
        contact,
        app_version,
        os_label: os,
        client_label: client,
        files,
    };
    let state2 = state.clone();
    tokio::spawn(async move { hook.post(&state2).await });

    let body = json!({"ok": true, "received_at": dt::pydantic(&created_at)});
    (per_ip, PydanticJson(body)).into_response()
}

/// `_post_feedback_webhook`.
pub struct Webhook {
    pub doc_id: String,
    pub category: String,
    pub message: String,
    pub contact: Option<String>,
    pub app_version: Option<String>,
    pub os_label: Option<String>,
    pub client_label: Option<String>,
    pub files: Vec<(String, Vec<u8>, String)>,
}

/// `httpx.AsyncClient(timeout=20)` over rustls (ring, webpki roots).
pub fn http_client() -> Option<&'static reqwest::Client> {
    static HTTP: LazyLock<Option<reqwest::Client>> = LazyLock::new(|| {
        let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
        let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
        let tls = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| tracing::error!("feedback webhook TLS: {e}"))
            .ok()?
            .with_root_certificates(roots)
            .with_no_client_auth();
        let client = reqwest::Client::builder().timeout(Duration::from_secs(20)).tls_backend_preconfigured(tls).build();
        client.map_err(|e| tracing::error!("feedback webhook client: {e}")).ok()
    });
    HTTP.as_ref()
}

impl Webhook {
    /// The embeds (and renamed files) Python sends.
    pub fn payload(&self, now: &str) -> (Value, Vec<String>) {
        let msg_display = if self.message.chars().count() <= 1500 {
            self.message.clone()
        } else {
            format!("{} …", pystr::prefix(&self.message, 1500))
        };
        let color = match self.category.as_str() {
            "bug" => 0xf8_5149,
            "feature" => 0xd2_9922,
            _ => 0x58_a6ff,
        };
        let mut fields = Vec::new();
        if let Some(os) = &self.os_label {
            fields.push(json!({"name": "OS", "value": os, "inline": true}));
        }
        if let Some(client) = &self.client_label {
            fields.push(json!({"name": "Client", "value": client, "inline": true}));
        }
        if let Some(v) = self.app_version.as_deref().filter(|v| !v.is_empty()) {
            fields.push(json!({"name": "App version", "value": pystr::prefix(v, 256), "inline": true}));
        }
        if let Some(c) = self.contact.as_deref().filter(|c| !c.is_empty()) {
            fields.push(json!({"name": "Contact", "value": pystr::prefix(c, 1024), "inline": false}));
        }
        let names: Vec<String> = self.files.iter().enumerate().map(|(i, (n, _, _))| format!("{i}_{n}")).collect();
        let gallery = format!("https://api.aallyn.net/feedback/{}", self.doc_id);
        let mut main = json!({
            "title": format!("New {} feedback", self.category),
            "description": msg_display,
            "color": color,
            "fields": fields,
            "footer": {"text": format!("id {}", self.doc_id)},
            "timestamp": now,
        });
        if let Some(first) = names.first() {
            main["url"] = json!(gallery);
            main["image"] = json!({"url": format!("attachment://{first}")});
        }
        let mut embeds = vec![main];
        for name in names.iter().skip(1) {
            embeds.push(json!({"url": gallery, "image": {"url": format!("attachment://{name}")}}));
        }
        (json!({"embeds": embeds}), names)
    }

    pub async fn post(self, state: &AppState) {
        let url = match get_setting(state, "feedback.discord_webhook").await {
            Ok(Value::String(s)) => s,
            Ok(_) => String::new(),
            Err(err) => {
                tracing::warn!("feedback webhook POST failed: {err:#}");
                return;
            }
        };
        if pystr::strip(&url).is_empty() {
            return;
        }
        let Some(http) = http_client() else { return };
        let (payload, names) = self.payload(&dt::isoformat(&dt::now()));
        let req = if self.files.is_empty() {
            http.post(&url).header(header::CONTENT_TYPE, "application/json").body(pyjson::dumps(&payload))
        } else {
            let mut form = reqwest::multipart::Form::new().text("payload_json", pyjson::dumps_spaced(&payload));
            for (i, ((_, data, ctype), name)) in self.files.into_iter().zip(names).enumerate() {
                let part = reqwest::multipart::Part::bytes(data).file_name(name);
                match part.mime_str(&ctype) {
                    Ok(part) => form = form.part(format!("files[{i}]"), part),
                    Err(err) => tracing::warn!("feedback webhook: {err}"),
                }
            }
            http.post(&url).multipart(form)
        };
        if let Err(err) = req.send().await.and_then(|r| r.error_for_status()) {
            tracing::warn!("feedback webhook POST failed: {err}");
        }
    }
}
