//! The embeddable viewer's own surface (`app/embed`): the `/embed/viewer`
//! page, the framing allowlist the website reads, and the partner `.tmod`
//! upload API. All ride `feature_embed_viewer_enabled`.
//!
//! The `/site/embed/*` data endpoints (manifest, blueprint, assembled, VFX,
//! backdrop, audio) are shells over the Mods Hub, rig assembly, the payload
//! cache and the audio indexer, and stay in Python until those are ported.

pub mod tmod;

#[cfg(test)]
mod tests;

use std::sync::{LazyLock, Mutex};

use axum::{
    Router,
    extract::{FromRequestParts, Request, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::Engine;
use kiwi_core::form::{self, FormError};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    auth::{Public, RequireScope, scopes},
    error::{ApiError, ErrorCode, method_not_allowed},
    features::{self, Embed, RequireFeature},
    middleware::security::parse_origins,
    pyjson::{PyJson, PydanticJson},
    state::AppState,
    validation::{FromParams, Params, Query},
};

const KEY: &str = "embed:tmod:";

pub fn router() -> Router<AppState> {
    let get_only = || async { method_not_allowed("GET") };
    Router::new()
        .route("/embed/viewer", get(viewer).fallback(get_only))
        .route("/site/embed/allowed-origins", get(allowed_origins).fallback(get_only))
        .route("/v1/embed/tmod", post(upload).fallback(|| async { method_not_allowed("POST") }))
        .route("/v1/embed/tmod/{token}", get(upload_status).fallback(get_only))
}

type Memo = Option<(std::time::Instant, Vec<String>)>;

static ORIGINS: LazyLock<Mutex<Memo>> = LazyLock::new(Default::default);

/// `service.allowed_origins`: memoised 30s, never raises (an unreadable
/// setting is nobody).
pub async fn origins(state: &AppState) -> Vec<String> {
    if let Ok(cache) = ORIGINS.lock()
        && let Some((at, v)) = cache.as_ref()
        && at.elapsed().as_secs_f64() <= 30.0
    {
        return v.clone();
    }
    let raw = match crate::runtime_config::get_setting(state, "embed.allowed_origins").await {
        Ok(Value::String(s)) => s,
        Ok(v) if !crate::features::py_truthy(&v) => String::new(),
        Ok(other) => crate::feeds::pyval::py_str(&other),
        Err(e) => {
            tracing::warn!("embed: allowlist unreadable ({e}) - framing denied until it returns");
            String::new()
        }
    };
    let parsed = parse_origins(&raw);
    if let Ok(mut cache) = ORIGINS.lock() {
        *cache = Some((std::time::Instant::now(), parsed.clone()));
    }
    parsed
}

fn short_cache(mut res: Response) -> Response {
    res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=60"));
    res
}

async fn allowed_origins(State(state): State<AppState>, _: RequireFeature<Embed>) -> Response {
    short_cache(PyJson(json!({"origins": origins(&state).await})).into_response())
}

// --- the page ---------------------------------------------------------------

pub struct ViewerQuery {
    pub fields: Vec<(&'static str, String)>,
}

const BACKDROPS: &[&str] = &["melee", "pistol", "staff", "spear", "fist", "hat"];

impl FromParams for ViewerQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        let mut s =
            |name: &'static str, max: usize| (name, p.query(name).str().max_len(max).optional().unwrap_or_default());
        let mut fields = vec![
            s("release", 64),
            s("tmod", 64),
            s("game", 400),
            s("prefab", 400),
            s("dress", 700),
            s("path", 400),
            s("sound", 200),
        ];
        fields.push(("backdrop", p.query("backdrop").str().one_of_pattern(BACKDROPS).optional().unwrap_or_default()));
        fields.push((
            "mode",
            p.query("mode")
                .str()
                .one_of_pattern(&["auto", "blueprint", "assembled", "vfx", "audio"])
                .default("auto".into()),
        ));
        fields.push(("theme", p.query("theme").str().one_of_pattern(&["dark", "light"]).default("dark".into())));
        ViewerQuery { fields }
    }
}

/// markupsafe's `escape`.
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&#34;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Jinja over `embed_viewer.html`: newlines normalised, `{# #}` comments
/// dropped, `{{ name }}` autoescaped, one trailing newline removed.
pub fn render_viewer(template: &str, vars: &[(&str, String)]) -> Option<String> {
    let text = template.replace("\r\n", "\n").replace('\r', "\n");
    let mut out = String::with_capacity(text.len() + 256);
    let mut rest = text.as_str();
    loop {
        let next = [rest.find("{#"), rest.find("{{"), rest.find("{%")].into_iter().flatten().min();
        let Some(i) = next else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..i]);
        let tag = &rest[i..i + 2];
        let close = match tag {
            "{#" => "#}",
            "{{" => "}}",
            _ => return None,
        };
        let end = rest[i + 2..].find(close)? + i + 2;
        if tag == "{{" {
            let name = rest[i + 2..end].trim();
            let (_, v) = vars.iter().find(|(k, _)| *k == name)?;
            out.push_str(&escape(v));
        }
        rest = &rest[end + 2..];
    }
    if out.ends_with('\n') {
        out.pop();
    }
    Some(out)
}

async fn viewer(State(state): State<AppState>, _: RequireFeature<Embed>, Query(q): Query<ViewerQuery>) -> Response {
    let path = std::path::Path::new(&state.settings().site_root).join("templates").join("embed_viewer.html");
    let Ok(template) = tokio::fs::read_to_string(&path).await else {
        tracing::error!("embed viewer template missing at {}", path.display());
        return ApiError::internal().into_response();
    };
    let mut vars = q.fields;
    vars.push(("api_base", String::new()));
    vars.push(("app_url", state.settings().app_url.trim_end_matches('/').to_owned()));
    match render_viewer(&template, &vars) {
        Some(html) => {
            ([(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"))], html).into_response()
        }
        None => ApiError::internal().into_response(),
    }
}

// --- the partner upload API -----------------------------------------------

fn token_valid(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

async fn ttl_seconds(state: &AppState) -> anyhow::Result<i64> {
    let v = crate::runtime_config::get_setting(state, "embed.upload_ttl_minutes").await?;
    let m = crate::feeds::pyval::py_int(&v).ok_or_else(|| anyhow::anyhow!("int() of {v}"))?;
    Ok((m.max(1) as i64).saturating_mul(60))
}

fn parse_error(msg: &str) -> Response {
    ApiError::http(StatusCode::BAD_REQUEST, Some(msg)).into_response()
}

async fn upload(State(state): State<AppState>, req: Request) -> Response {
    let (mut parts, body) = req.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, usize::MAX).await else {
        return parse_error("There was an error parsing the body");
    };
    let items = match form::parse_form(parts.headers.get(header::CONTENT_TYPE).map(|v| v.as_bytes()), &bytes) {
        Ok(items) => items,
        Err(FormError::Http(msg)) => return parse_error(&msg),
    };
    drop(bytes);
    if let Err(e) = features::check::<Embed>(&state).await {
        return e.into_response();
    }
    let ctx = match RequireScope::<{ scopes::MODS_READ }>::from_request_parts(&mut parts, &state).await {
        Ok(RequireScope(ctx)) => ctx,
        Err(r) => return r.into_response(),
    };
    let data = match crate::images::upload_file(items) {
        Ok(d) => d,
        Err(e) => return e.into_response(),
    };
    let rl = ctx.rate_limit;
    match store_upload(&state, &data).await {
        Ok(v) => (rl, PydanticJson(v)).into_response(),
        Err(e) => (rl, e).into_response(),
    }
}

async fn store_upload(state: &AppState, data: &[u8]) -> Result<Value, ApiError> {
    if data.is_empty() {
        return Err(ApiError::bad_request("Empty upload."));
    }
    let max = state.settings().embed_upload_max_bytes;
    if data.len() as i64 > max {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "bad_request",
            format!("The .tmod exceeds the {max}-byte limit."),
        ));
    }
    if let Err(e) = tmod::validate(data) {
        return Err(ApiError::bad_request(format!("Not a readable .tmod: {e}")));
    }
    let sha = hex::encode(Sha256::digest(data));
    let Some(mut conn) = state.inner.db.redis.clone() else {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::ServiceUnavailable,
            "Mod previews are temporarily unavailable. Try again shortly.",
        ));
    };
    let internal = |e: &dyn std::fmt::Display| {
        tracing::error!("embed upload: {e}");
        ApiError::internal()
    };
    let ttl = ttl_seconds(state).await.map_err(|e| internal(&e))?;
    let key = format!("{KEY}{sha}");
    let exists: i64 = redis::cmd("EXISTS").arg(&key).query_async(&mut conn).await.map_err(|e| internal(&e))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(data);
    let _: () = redis::cmd("SET")
        .arg(&key)
        .arg(b64)
        .arg("EX")
        .arg(ttl)
        .query_async(&mut conn)
        .await
        .map_err(|e| internal(&e))?;
    let app = state.settings().app_url.trim_end_matches('/').to_owned();
    Ok(json!({
        "token": sha,
        "size": data.len(),
        "expires_in": ttl,
        "reused": exists > 0,
        "viewer_url": format!("{app}/embed/viewer?tmod={sha}"),
    }))
}

/// `uploads.load`: the held bytes, without extending the expiry.
pub async fn load(state: &AppState, token: &str) -> Option<Vec<u8>> {
    if !token_valid(token) {
        return None;
    }
    let mut conn = state.inner.db.redis.clone()?;
    let raw: Option<String> = redis::cmd("GET").arg(format!("{KEY}{token}")).query_async(&mut conn).await.ok()?;
    base64::engine::general_purpose::STANDARD.decode(raw?.trim()).ok()
}

/// `{token}` as Starlette decodes it.
struct Token(String);

impl<S: Send + Sync> FromRequestParts<S> for Token {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut axum::http::request::Parts, _: &S) -> Result<Self, Self::Rejection> {
        let path = crate::middleware::pyurl::scope_path(&parts.uri);
        Ok(Token(path.strip_prefix("/v1/embed/tmod/").unwrap_or_default().to_owned()))
    }
}

async fn upload_status(
    State(state): State<AppState>,
    Token(token): Token,
    _: RequireFeature<Embed>,
    Public(ctx): Public<{ scopes::MODS_READ }>,
) -> Response {
    let body = match load(&state, &token).await {
        Some(data) => json!({"token": token, "live": true, "size": data.len()}),
        None => json!({"token": token, "live": false}),
    };
    (ctx.rate_limit, PydanticJson(body)).into_response()
}
