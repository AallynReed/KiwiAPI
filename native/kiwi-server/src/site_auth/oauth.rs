//! `app/site_auth/oauth.py`: "Sign in with Discord" for site accounts. The
//! callback hands the browser (or the desktop app's `btt://` link) a one-time
//! code that `POST /oauth/exchange` swaps for the session tokens.

use std::time::Duration;

use axum::{
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use kiwi_core::pystr;
use mongodb::bson::{Document, doc, oid::ObjectId};
use serde_json::{Map, Value, json};
use unicode_normalization::UnicodeNormalization;

use super::{
    routes::user_agent,
    store::{find_user, internal, save_user, token_hex, truthy, user_doc, username_taken},
};
use crate::{
    auth::{
        auth_settings,
        client::device_label,
        models::{SITE_USERS, SiteUser},
        pyutil::int_of,
        sessions::{self, Audience, TokenPair},
        site::set_session_cookies,
        tokens::token_urlsafe,
    },
    dt,
    error::ApiError,
    middleware::pyurl::{ascii_json_str, quote},
    pyjson::{self, PydanticJson},
    state::AppState,
    validation::{Field, FromBody, FromParams, Json, Params, Query},
};

const AUTHORIZE: &str = "https://discord.com/oauth2/authorize";
const TOKEN: &str = "https://discord.com/api/oauth2/token";
const API: &str = "https://discord.com/api";
pub const DISCORD_TOK_PREFIX: &str = "site_discord_tok:";
const DISCORD_TOK_MAX_TTL: i64 = 7 * 24 * 3600;
const STATE_PREFIX: &str = "site_oauthstate:";
const EXCHANGE_PREFIX: &str = "site_oauthx:";
const REDIRECT_SAFE: &str = ":/%#?=@[]!$&'()*+,;";

/// A Python `ValueError` out of `urlsplit`: an unhandled 500.
#[derive(Debug, PartialEq, Eq)]
pub struct UrlError;

fn scheme_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "+-.".contains(c)
}

/// `ipaddress.ip_address(h)`: `Some(true)` IPv4, `Some(false)` IPv6.
fn ip_kind(h: &str) -> Option<bool> {
    if h.parse::<std::net::Ipv4Addr>().is_ok() {
        return Some(true);
    }
    let addr = match h.split_once('%') {
        Some((_, scope)) if scope.is_empty() || scope.contains('%') => return None,
        Some((addr, _)) => addr,
        None => h,
    };
    addr.parse::<std::net::Ipv6Addr>().ok().map(|_| false)
}

/// `_check_bracketed_host`.
fn bracketed_host_ok(h: &str) -> bool {
    if let Some(rest) = h.strip_prefix(['v', 'V']) {
        let Some((ver, addr)) = rest.split_once('.') else { return false };
        return !ver.is_empty() && ver.chars().all(|c| c.is_ascii_hexdigit()) && !addr.is_empty();
    }
    ip_kind(h) == Some(false)
}

/// `_check_bracketed_netloc`.
fn bracketed_netloc_ok(netloc: &str) -> bool {
    let host_port = netloc.rsplit_once('@').map_or(netloc, |(_, h)| h);
    let host = match host_port.split_once('[') {
        Some((before, _)) if !before.is_empty() => return false,
        Some((_, bracketed)) => {
            let (host, port) = bracketed.split_once(']').unwrap_or((bracketed, ""));
            if !port.is_empty() && !port.starts_with(':') {
                return false;
            }
            host
        }
        None => host_port.split_once(':').map_or(host_port, |(h, _)| h),
    };
    bracketed_host_ok(host)
}

/// `_checknetloc`: NFKC must not conjure URL delimiters.
fn netloc_nfkc_ok(netloc: &str) -> bool {
    if netloc.is_ascii() {
        return true;
    }
    let n: String = netloc.chars().filter(|c| !"@:#?".contains(*c)).collect();
    let k: String = n.nfkc().collect();
    n == k || !k.contains(['/', '?', '#', '@', ':'])
}

/// `_safe_next`: a site-relative path rebuilt from `urlsplit`, else "".
pub fn safe_next(raw: Option<&str>) -> Result<String, UrlError> {
    let Some(raw) = raw.filter(|r| !r.is_empty()) else { return Ok(String::new()) };
    let replaced = raw.replace('\\', "/");
    let mut url: String = replaced
        .trim_start_matches(|c: char| (c as u32) <= 0x20)
        .chars()
        .filter(|c| !matches!(c, '\t' | '\r' | '\n'))
        .collect();
    let mut scheme = false;
    if let Some(i) = url.find(':')
        && i > 0
        && url.starts_with(|c: char| c.is_ascii_alphabetic())
        && url[..i].chars().all(scheme_char)
    {
        scheme = true;
        url = url[i + 1..].to_owned();
    }
    let mut netloc = "";
    let mut rest = url.as_str();
    if let Some(after) = url.strip_prefix("//") {
        let end = after.find(['/', '?', '#']).unwrap_or(after.len());
        (netloc, rest) = after.split_at(end);
        if netloc.contains('[') != netloc.contains(']') {
            return Err(UrlError);
        }
        if netloc.contains('[') && !bracketed_netloc_ok(netloc) {
            return Err(UrlError);
        }
    }
    let (rest, fragment) = rest.split_once('#').unwrap_or((rest, ""));
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    if !netloc_nfkc_ok(netloc) {
        return Err(UrlError);
    }
    if scheme || !netloc.is_empty() || !path.starts_with('/') {
        return Ok(String::new());
    }
    let mut out = if path.starts_with("//") { format!("//{path}") } else { path.to_owned() };
    if !query.is_empty() {
        out.push('?');
        out.push_str(query);
    }
    if !fragment.is_empty() {
        out.push('#');
        out.push_str(fragment);
    }
    Ok(out)
}

/// Starlette `RedirectResponse(url, 307)`.
fn redirect(url: &str) -> Response {
    let location = HeaderValue::from_str(&quote(url, REDIRECT_SAFE)).unwrap_or_else(|_| HeaderValue::from_static("/"));
    (StatusCode::TEMPORARY_REDIRECT, [(header::LOCATION, location)]).into_response()
}

/// `_back`: the site's login page, the result in the fragment.
pub fn back(app_url: &str, fragment: &str, next: &str) -> Response {
    let query = if next.is_empty() { String::new() } else { format!("?next={}", quote(next, "/")) };
    redirect(&format!("{app_url}/login{query}#{fragment}"))
}

/// `_app_return`: the interstitial that opens the desktop app's deep link.
pub fn app_return(fragment: &str) -> Response {
    let target = format!("btt://auth/discord?{fragment}");
    let target_js = ascii_json_str(&target);
    let target_attr = target.replace('"', "%22");
    let html = format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Better Trove Tools — Sign in</title>
<style>
  html,body{{height:100%;margin:0}}
  body{{display:flex;align-items:center;justify-content:center;background:#0f1216;
       color:#e8edf2;font-family:'Segoe UI',system-ui,sans-serif}}
  .card{{text-align:center;max-width:420px;padding:40px 32px;background:#171b21;
        border:1px solid #232a33;border-radius:16px}}
  h1{{font-size:20px;margin:0 0 8px}}
  p{{color:#9aa7b4;margin:0 0 20px;line-height:1.5}}
  a.btn{{display:inline-block;background:#5ec6ff;color:#06202b;text-decoration:none;
        font-weight:600;padding:11px 22px;border-radius:10px}}
</style></head>
<body>
  <div class="card">
    <h1>Returning to Better Trove Tools…</h1>
    <p>You're signed in. If the app didn't come back to the front,
       use the button below. You can close this tab.</p>
    <a class="btn" href="{target_attr}">Open Better Trove Tools</a>
  </div>
  <script>location.replace({target_js});</script>
</body></html>"#
    );
    ([(header::CONTENT_TYPE, HeaderValue::from_static("text/html; charset=utf-8"))], html).into_response()
}

/// `_emit`: the btt:// interstitial for `mode == "app"`, else the site redirect.
pub fn emit(app_url: &str, app: bool, result: Result<&str, &str>, next: &str) -> Response {
    match (app, result) {
        (true, Ok(code)) => app_return(&format!("code={code}")),
        (true, Err(e)) => app_return(&format!("error={e}")),
        (false, Ok(code)) => back(app_url, &format!("discord={code}"), next),
        (false, Err(e)) => back(app_url, &format!("oauth_error={e}"), next),
    }
}

fn require_enabled(state: &AppState) -> Result<(), ApiError> {
    if state.settings().discord_oauth_enabled() {
        Ok(())
    } else {
        Err(ApiError::not_found("Discord sign-in is not configured"))
    }
}

fn redirect_uri(state: &AppState) -> String {
    format!("{}/v1/site-auth/oauth/discord/callback", state.settings().api_url)
}

/// `quote_plus(s, safe='')`.
fn quote_plus(s: &str) -> String {
    quote(s, " ").replace(' ', "+")
}

fn redis(state: &AppState) -> Option<redis::aio::ConnectionManager> {
    state.inner.db.redis.clone()
}

async fn redis_set(conn: &mut redis::aio::ConnectionManager, key: &str, value: &str, ttl: i64) -> Result<(), ApiError> {
    redis::cmd("SET").arg(key).arg(value).arg("EX").arg(ttl).query_async::<()>(conn).await.map_err(internal)
}

/// `GETDEL` as `decode_responses=True` returns it.
async fn redis_getdel(conn: &mut redis::aio::ConnectionManager, key: &str) -> Result<Option<String>, ApiError> {
    let raw: Option<Vec<u8>> = redis::cmd("GETDEL").arg(key).query_async(conn).await.map_err(internal)?;
    raw.map(|b| String::from_utf8(b).map_err(internal)).transpose()
}

/// `max(60, min(int(expires_in) or 7d, 7d))`, a failed `int()` counting as 0.
pub fn token_ttl(expires_in: &Value) -> i64 {
    let ttl = int_of(expires_in).map_or(0, |n| n.clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64);
    let ttl = if ttl == 0 { DISCORD_TOK_MAX_TTL } else { ttl };
    ttl.clamp(60, DISCORD_TOK_MAX_TTL)
}

/// `store_discord_token`.
pub async fn store_discord_token(
    state: &AppState,
    user_id: ObjectId,
    access: &str,
    expires_in: &Value,
) -> Result<(), ApiError> {
    let Some(mut conn) = redis(state) else { return Ok(()) };
    if access.is_empty() {
        return Ok(());
    }
    redis_set(&mut conn, &format!("{DISCORD_TOK_PREFIX}{}", user_id.to_hex()), access, token_ttl(expires_in)).await
}

/// `clear_discord_token`.
pub async fn clear_discord_token(state: &AppState, user_id: ObjectId) -> Result<(), ApiError> {
    let Some(mut conn) = redis(state) else { return Ok(()) };
    redis::cmd("DEL")
        .arg(format!("{DISCORD_TOK_PREFIX}{}", user_id.to_hex()))
        .query_async::<()>(&mut conn)
        .await
        .map_err(internal)
}

pub struct StartQuery {
    pub client: Option<String>,
    pub next: Option<String>,
}

impl FromParams for StartQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        StartQuery { client: p.query("client").str().optional(), next: p.query("next").str().optional() }
    }
}

/// `GET /v1/site-auth/oauth/discord/start`.
pub async fn start(State(state): State<AppState>, Query(q): Query<StartQuery>) -> Result<Response, ApiError> {
    require_enabled(&state)?;
    let app = q.client.as_deref() == Some("app");
    let next = safe_next(q.next.as_deref()).map_err(|_| ApiError::internal())?;
    let app_url = &state.settings().app_url;
    let Some(mut conn) = redis(&state) else { return Ok(emit(app_url, app, Err("unavailable"), &next)) };
    let st = token_urlsafe(24);
    let stored = pyjson::dumps_spaced(&json!({ "mode": if app { "app" } else { "web" }, "next": next }));
    redis_set(&mut conn, &format!("{STATE_PREFIX}{st}"), &stored, 600).await?;
    let client_id = state.settings().discord_client_id.clone().unwrap_or_default();
    let params = [
        ("client_id", client_id.as_str()),
        ("redirect_uri", &redirect_uri(&state)),
        ("response_type", "code"),
        ("scope", "identify guilds"),
        ("state", &st),
    ];
    let qs: Vec<String> = params.iter().map(|(k, v)| format!("{}={}", quote_plus(k), quote_plus(v))).collect();
    Ok(redirect(&format!("{AUTHORIZE}?{}", qs.join("&"))))
}

pub struct CallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
}

impl FromParams for CallbackQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        CallbackQuery { code: p.query("code").str().optional(), state: p.query("state").str().optional() }
    }
}

/// The mode and destination a stored state value carries (`None` = Python raises).
pub fn parse_state(stored: &str) -> Option<(bool, String)> {
    let parsed = match pyjson::loads(stored.as_bytes()) {
        Ok(Value::Object(m)) => m,
        Ok(_) => return None,
        Err(_) => {
            let mut m = Map::new();
            m.insert("mode".into(), Value::String(stored.to_owned()));
            m
        }
    };
    let app = parsed.get("mode").and_then(Value::as_str) == Some("app");
    let next = match parsed.get("next") {
        Some(Value::String(s)) => safe_next(Some(s)).ok()?,
        Some(v) if truthy(v) => return None,
        _ => String::new(),
    };
    Some((app, next))
}

/// What the two Discord calls produced, or the error the callback reports.
enum Discord {
    Profile { access: String, expires_in: Value, me: Map<String, Value> },
    Fail(&'static str),
}

async fn get_json(req: reqwest::RequestBuilder) -> Result<Result<Value, &'static str>, ApiError> {
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("Discord OAuth request failed: {e}");
            return Ok(Err("discord"));
        }
    };
    let body = match resp.bytes().await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("Discord OAuth request failed: {e}");
            return Ok(Err("discord"));
        }
    };
    pyjson::loads(&body).map(Ok).map_err(|_| internal("Discord answered with something that is not JSON"))
}

async fn discord_profile(state: &AppState, code: &str) -> Result<Discord, ApiError> {
    // httpx.AsyncClient(timeout=10.0): per-phase timeouts, no redirects, its default UA.
    let client = crate::http::client_builder()
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(10))
        .user_agent("python-httpx/0.28.1")
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(internal)?;
    let s = state.settings();
    let form = [
        ("client_id", s.discord_client_id.clone().unwrap_or_default()),
        ("client_secret", s.discord_client_secret.clone().unwrap_or_default()),
        ("grant_type", "authorization_code".into()),
        ("code", code.to_owned()),
        ("redirect_uri", redirect_uri(state)),
    ];
    let body = form_urlencoded::Serializer::new(String::new()).extend_pairs(&form).finish();
    let req = client.post(TOKEN).header(header::CONTENT_TYPE, "application/x-www-form-urlencoded").body(body);
    let tok = match get_json(req).await? {
        Ok(Value::Object(m)) => m,
        Ok(_) => return Err(internal("Discord token reply is not an object")),
        Err(e) => return Ok(Discord::Fail(e)),
    };
    let access = match tok.get("access_token") {
        Some(v) if !truthy(v) => return Ok(Discord::Fail("exchange")),
        None => return Ok(Discord::Fail("exchange")),
        Some(Value::String(a)) => a.clone(),
        Some(_) => return Err(internal("Discord access_token is not a string")),
    };
    let expires_in = tok.get("expires_in").cloned().unwrap_or(Value::Null);
    let me = match get_json(client.get(format!("{API}/users/@me")).bearer_auth(&access)).await? {
        Ok(Value::Object(m)) => m,
        Ok(_) => return Err(internal("Discord /users/@me reply is not an object")),
        Err(e) => return Ok(Discord::Fail(e)),
    };
    Ok(Discord::Profile { access, expires_in, me })
}

/// `_unique_username`'s seed cleanup: `[a-z0-9_.]`, single dots, no leading
/// dot, at most 20 characters, padded with "kiwi" when shorter than 3.
pub fn username_base(seed: &str) -> String {
    let kept: String = seed
        .to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '_' || *c == '.')
        .collect();
    let mut collapsed = String::with_capacity(kept.len());
    for c in kept.chars() {
        if !(c == '.' && collapsed.ends_with('.')) {
            collapsed.push(c);
        }
    }
    let mut base: String = collapsed.trim_start_matches('.').chars().take(20).collect();
    if base.len() < 3 {
        base = format!("{base}kiwi").chars().take(20).collect();
    }
    base
}

/// `_unique_username`.
async fn unique_username(state: &AppState, seed: &str) -> Result<String, ApiError> {
    let base = username_base(seed);
    let mut candidate = base.clone();
    for _ in 0..12 {
        if username_taken(state, &candidate).await?.is_none() {
            return Ok(candidate);
        }
        candidate = format!("{}{}", pystr::prefix(&base, 18), token_hex(2));
    }
    Ok(format!("kiwi{}", token_hex(8)))
}

/// A `str | None` profile field; `Err` where Python's validation or `.strip()` raises.
fn opt_string(v: Option<&Value>) -> Result<Option<String>, ApiError> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(other) => Err(internal(format!("unexpected Discord profile value {other}"))),
    }
}

/// `a or b` over two JSON values.
fn first_truthy<'a>(a: Option<&'a Value>, b: Option<&'a Value>) -> Option<&'a Value> {
    match a {
        Some(v) if truthy(v) => Some(v),
        _ => b,
    }
}

/// `_find_or_create`.
async fn find_or_create(state: &AppState, discord_id: i64, me: &Map<String, Value>) -> Result<SiteUser, ApiError> {
    let avatar = opt_string(me.get("avatar"))?;
    let handle = match first_truthy(me.get("username"), me.get("global_name")).filter(|v| truthy(v)) {
        Some(Value::String(s)) => pystr::strip(s).to_owned(),
        Some(_) => return Err(internal("Discord username is not a string")),
        None => String::new(),
    };
    if let Some(mut user) = find_user(state, doc! { "discord_id": discord_id }).await? {
        let mut dirty = false;
        if user.discord_avatar != avatar {
            user.discord_avatar = avatar;
            dirty = true;
        }
        if !handle.is_empty() && user.discord_handle != handle {
            user.discord_handle = handle.clone();
            dirty = true;
        }
        if user.discord_handle.is_empty() {
            user.discord_handle = if handle.is_empty() { user.username.clone() } else { handle };
            dirty = true;
        }
        if dirty {
            save_user(state, &user).await?;
        }
        return Ok(user);
    }
    let display_name = opt_string(first_truthy(me.get("global_name"), me.get("username")))?;
    let now = dt::to_bson(dt::now());
    let user = SiteUser {
        id: ObjectId::new(),
        username: unique_username(state, &handle).await?,
        discord_handle: handle,
        notify_email: None,
        display_name,
        is_active: true,
        is_verified: true,
        is_deleted: false,
        discord_id: Some(discord_id),
        discord_avatar: avatar,
        token_version: 0,
        claimed_trove_name: None,
        claimed_trove_display: None,
        claimed_at: None,
        claim_verified: false,
        claim_baseline: Document::new(),
        claim_verified_at: None,
        creator_token_hash: None,
        creator_token_prefix: None,
        creator_token_at: None,
        mod_issues_seen_at: None,
        created_at: now,
        updated_at: now,
        last_login_at: None,
    };
    super::store::coll(state, SITE_USERS)?.insert_one(user_doc(&user)).await.map_err(internal)?;
    Ok(user)
}

/// `GET /v1/site-auth/oauth/discord/callback`.
pub async fn callback(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<CallbackQuery>,
) -> Result<Response, ApiError> {
    require_enabled(&state)?;
    let app_url = state.settings().app_url.clone();
    let Some(mut conn) = redis(&state) else { return Ok(back(&app_url, "oauth_error=unavailable", "")) };
    let (mut app, mut next) = (false, String::new());
    let st = q.state.as_deref().filter(|s| !s.is_empty());
    if let Some(st) = st {
        let stored = redis_getdel(&mut conn, &format!("{STATE_PREFIX}{st}")).await?.filter(|s| !s.is_empty());
        let Some(stored) = stored else { return Ok(back(&app_url, "oauth_error=state", "")) };
        (app, next) = parse_state(&stored).ok_or_else(ApiError::internal)?;
    }
    let (Some(code), Some(_)) = (q.code.as_deref().filter(|c| !c.is_empty()), st) else {
        return Ok(emit(&app_url, app, Err("missing"), &next));
    };
    let (access, expires_in, me) = match discord_profile(&state, code).await? {
        Discord::Profile { access, expires_in, me } => (access, expires_in, me),
        Discord::Fail(e) => return Ok(emit(&app_url, app, Err(e), &next)),
    };
    let raw_id = me.get("id").filter(|v| truthy(v));
    let Some(discord_id) = raw_id.and_then(int_of) else {
        return Ok(emit(&app_url, app, Err("discord"), &next));
    };
    let discord_id = i64::try_from(discord_id).map_err(internal)?;
    let mut user = find_or_create(&state, discord_id, &me).await?;
    if !user.is_active {
        return Ok(emit(&app_url, app, Err("inactive"), &next));
    }
    user.last_login_at = Some(dt::to_bson(dt::now()));
    save_user(&state, &user).await?;
    store_discord_token(&state, user.id, &access, &expires_in).await?;
    let device = device_label(user_agent(&headers).as_deref());
    let tokens = sessions::issue_tokens(&state, Audience::Site, user.id, user.token_version, device)
        .await
        .map_err(|r| *r.error)?;
    let xcode = token_urlsafe(24);
    let payload = pyjson::dumps_spaced(&json!({ "a": tokens.access_token, "r": tokens.refresh_token }));
    redis_set(&mut conn, &format!("{EXCHANGE_PREFIX}{xcode}"), &payload, 120).await?;
    Ok(emit(&app_url, app, Ok(&xcode), &next))
}

pub struct ExchangeBody {
    pub code: String,
}

impl FromBody for ExchangeBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(ExchangeBody { code: m.field("code").str().required() })).required()
    }
}

/// `POST /v1/site-auth/oauth/exchange`: the one-time code for the tokens, as cookies too.
pub async fn exchange(State(state): State<AppState>, Json(body): Json<ExchangeBody>) -> Result<Response, ApiError> {
    let raw = match redis(&state) {
        Some(mut conn) => redis_getdel(&mut conn, &format!("{EXCHANGE_PREFIX}{}", body.code)).await?,
        None => None,
    };
    let Some(raw) = raw else { return Err(ApiError::bad_request("Invalid or expired exchange code")) };
    let d = pyjson::loads(raw.as_bytes()).map_err(|_| internal("exchange payload is not JSON"))?;
    let field =
        |k: &str| d.get(k).and_then(Value::as_str).map(str::to_owned).ok_or_else(|| internal("bad exchange payload"));
    let tokens = TokenPair {
        access_token: field("a")?,
        refresh_token: field("r")?,
        token_type: "bearer",
        expires_in: auth_settings().access_token_expire_minutes * 60,
    };
    let mut res = PydanticJson(serde_json::to_value(&tokens).map_err(internal)?).into_response();
    set_session_cookies(res.headers_mut(), &tokens);
    Ok(res)
}
