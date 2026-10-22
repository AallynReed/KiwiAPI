//! `app/main.py`'s own routes: `/health`, `/config`, `/api-info`, and the
//! Swagger UI / ReDoc pages FastAPI registers (`/openapi.json` stays in Python).

use axum::{
    Router,
    extract::State,
    response::{Html, Response},
    routing::get,
};
use serde_json::{Value, json};

use super::docs_html::{OAUTH2_REDIRECT, REDOC_HEAD, REDOC_TAIL, SWAGGER_HEAD, SWAGGER_TAIL};
use crate::{auth::scopes, config::Settings, error, pyjson::PydanticJson, state::AppState};

/// `app.tokens.schemas.REVOKE_REASONS`.
pub const REVOKE_REASONS: [&str; 4] =
    ["No longer using it", "Rotating to a new token", "Possibly compromised", "Created by mistake"];

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/config", get(public_config))
        .route("/api-info", get(api_info))
        .route("/docs", get(swagger_ui).fallback(starlette_405))
        .route("/docs/oauth2-redirect", get(oauth2_redirect).fallback(starlette_405))
        .route("/redoc", get(redoc).fallback(starlette_405))
}

async fn health() -> PydanticJson {
    PydanticJson(json!({"status": "ok"}))
}

/// `PublicConfig`, in model field order.
pub fn config_body(s: &Settings) -> Value {
    let scopes: Vec<Value> = scopes::catalog()
        .iter()
        .map(|sc| json!({"key": sc.key, "bit": sc.bit, "resource": sc.resource, "description": sc.description}))
        .collect();
    json!({
        "app_name": s.app_name,
        "api_url": s.api_url,
        "captcha_provider": s.captcha_provider,
        "captcha_sitekey": s.captcha_sitekey,
        "require_verified_for_tokens": s.require_verified_for_tokens,
        "scopes": scopes,
        "token_creation_daily_limit": s.token_creation_daily_limit,
        "revoke_reasons": REVOKE_REASONS,
        "github_oauth_enabled": s.github_oauth_enabled(),
        "discord_oauth_enabled": s.discord_oauth_enabled(),
    })
}

async fn public_config(State(state): State<AppState>) -> PydanticJson {
    PydanticJson(config_body(state.settings()))
}

pub fn api_info_html(s: &Settings) -> String {
    let name = &s.app_name;
    format!(
        r#"<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>{name}</title>
<style>
  body {{ margin:0; min-height:100vh; display:flex; align-items:center; justify-content:center;
    font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,sans-serif;
    background:#0d1117; color:#e6edf3; }}
  .card {{ text-align:center; padding:40px; max-width:440px; }}
  h1 {{ font-size:2rem; margin:0 0 10px; }}
  h1 .mark {{ color:#58a6ff; }}
  p {{ color:#9aa4b2; margin:0 0 24px; }}
  a {{ color:#58a6ff; text-decoration:none; margin:0 10px; font-weight:600; }}
  a:hover {{ text-decoration:underline; }}
</style></head><body>
  <div class="card">
    <h1><span class="mark">◆</span> {name}</h1>
    <p>Programmatic API access. Authenticate requests with an API token.</p>
    <a href="{docs}">Documentation</a>·<a href="{dev}">Developer Portal</a>
  </div>
</body></html>"#,
        docs = s.docs_url,
        dev = s.dev_url,
    )
}

async fn api_info(State(state): State<AppState>) -> Html<String> {
    Html(api_info_html(state.settings()))
}

pub fn swagger_html(app_name: &str) -> String {
    format!("{SWAGGER_HEAD}{app_name}{SWAGGER_TAIL}")
}

pub fn redoc_html(app_name: &str) -> String {
    format!("{REDOC_HEAD}{app_name}{REDOC_TAIL}")
}

async fn swagger_ui(State(state): State<AppState>) -> Html<String> {
    Html(swagger_html(&state.settings().app_name))
}

async fn redoc(State(state): State<AppState>) -> Html<String> {
    Html(redoc_html(&state.settings().app_name))
}

async fn oauth2_redirect() -> Html<&'static str> {
    Html(OAUTH2_REDIRECT)
}

/// These are plain Starlette routes (`{"GET", "HEAD"}`), whose 405 `Allow`
/// is that set joined in hash order; this is one of its two spellings.
async fn starlette_405() -> Response {
    error::method_not_allowed("GET, HEAD")
}
