//! `app/auth/oauth.py`: "Sign in with GitHub". The browser always lands back
//! on the portal with the result in the URL fragment; the tokens travel
//! through a one-time Redis exchange code.

use std::time::Duration;

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
};
use kiwi_core::email_validator::{self, Email};
use mongodb::bson::{doc, oid::ObjectId};
use redis::AsyncCommands;
use serde_json::{Map, Value, json};

use super::{
    emails,
    req::{NoParams, with_body, without_body},
    store::{find_one, insert_user, save_user, user_by_email},
};
use crate::{
    auth::{
        Rejection,
        client::{device_label, user_agent},
        models::{USERS, User},
        passwords::hash_password_blocking,
        sessions::{Audience, issue_tokens},
        tokens::token_urlsafe,
    },
    config::Settings,
    dt,
    middleware::pyurl::{header_value, quote},
    pyjson::PydanticJson,
    state::AppState,
    validation::{Field, FromBody, FromParams, Params},
};

const AUTHORIZE: &str = "https://github.com/login/oauth/authorize";
const TOKEN: &str = "https://github.com/login/oauth/access_token";
const API: &str = "https://api.github.com";

/// Starlette `RedirectResponse(url, 307)`.
pub fn redirect(url: &str) -> Response {
    let mut res = Response::new(Body::empty());
    *res.status_mut() = StatusCode::TEMPORARY_REDIRECT;
    let h = res.headers_mut();
    h.insert(header::LOCATION, header_value(&quote(url, ":/%#?=@[]!$&'()*+,;")));
    h.insert(header::CONTENT_LENGTH, HeaderValue::from_static("0"));
    res
}

fn back(s: &Settings, fragment: &str) -> Response {
    redirect(&format!("{}/#{fragment}", s.dev_url))
}

fn require_enabled(s: &Settings) -> Result<(), Rejection> {
    if s.github_oauth_enabled() {
        Ok(())
    } else {
        Err(Rejection::new(StatusCode::NOT_FOUND, "not_found", "GitHub sign-in is not configured"))
    }
}

fn redirect_uri(s: &Settings) -> String {
    format!("{}/auth/oauth/github/callback", s.api_url)
}

/// `urllib.parse.urlencode` (`quote_plus` on keys and values).
pub fn urlencode(pairs: &[(&str, &str)]) -> String {
    let enc = |v: &str| quote(v, " ").replace(' ', "+");
    pairs.iter().map(|(k, v)| format!("{}={}", enc(k), enc(v))).collect::<Vec<_>>().join("&")
}

pub fn authorize_url(s: &Settings, state: &str) -> String {
    let client_id = s.github_client_id.clone().unwrap_or_default();
    let uri = redirect_uri(s);
    let q = urlencode(&[
        ("client_id", &client_id),
        ("redirect_uri", &uri),
        ("scope", "read:user user:email"),
        ("state", state),
        ("allow_signup", "true"),
    ]);
    format!("{AUTHORIZE}?{q}")
}

fn reply(r: Result<Response, Rejection>) -> Response {
    r.unwrap_or_else(IntoResponse::into_response)
}

pub async fn start(State(state): State<AppState>) -> Response {
    let run = async {
        let s = state.settings();
        require_enabled(s)?;
        let Some(mut r) = state.inner.db.redis.clone() else {
            return Ok(back(s, "oauth_error=unavailable"));
        };
        let st = token_urlsafe(24);
        let _: () = r.set_ex(format!("oauthstate:{st}"), "1", 600).await.map_err(Rejection::internal)?;
        Ok(redirect(&authorize_url(s, &st)))
    };
    reply(run.await)
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

/// `_pick_email`: primary+verified, else any verified; a non-object entry is
/// an unhandled error, as `.get` on it raises.
pub fn pick_email(emails: &Value) -> Result<Option<Value>, Rejection> {
    let Value::Array(items) = emails else {
        return Ok(None);
    };
    let obj = |e: &Value| -> Result<Map<String, Value>, Rejection> {
        e.as_object().cloned().ok_or_else(|| Rejection::internal("GitHub email entry is not an object"))
    };
    let truthy = crate::features::py_truthy;
    for e in items {
        let e = obj(e)?;
        if e.get("primary").is_some_and(truthy) && e.get("verified").is_some_and(truthy) {
            return Ok(Some(e.get("email").cloned().unwrap_or(Value::Null)));
        }
    }
    for e in items {
        let e = obj(e)?;
        if e.get("verified").is_some_and(truthy) {
            return Ok(Some(e.get("email").cloned().unwrap_or(Value::Null)));
        }
    }
    Ok(None)
}

enum GitHub {
    Ok { user: Value, emails: Value },
    Exchange,
    Failed,
}

async fn github(s: &Settings, code: &str) -> Result<GitHub, Rejection> {
    let client = crate::http::client_builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(crate::http::KIWI_UA)
        .build()
        .map_err(Rejection::internal)?;
    let json_of = |b: &[u8]| crate::pyjson::loads(b).map_err(|_| Rejection::internal("GitHub reply is not JSON"));
    let (id, secret, uri) = (
        s.github_client_id.clone().unwrap_or_default(),
        s.github_client_secret.clone().unwrap_or_default(),
        redirect_uri(s),
    );
    let form = urlencode(&[("client_id", &id), ("client_secret", &secret), ("code", code), ("redirect_uri", &uri)]);
    let sent = client
        .post(TOKEN)
        .header(header::ACCEPT, "application/json")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(form)
        .send()
        .await;
    let Ok(resp) = sent else {
        return Ok(GitHub::Failed);
    };
    let Ok(body) = resp.bytes().await else {
        return Ok(GitHub::Failed);
    };
    let tok = json_of(&body)?;
    let tok = tok.as_object().ok_or_else(|| Rejection::internal("GitHub token reply is not an object"))?;
    let access = match tok.get("access_token") {
        Some(v) if crate::features::py_truthy(v) => v.clone(),
        _ => return Ok(GitHub::Exchange),
    };
    let access = match access {
        Value::String(s) => s,
        other => other.to_string(),
    };
    let mut out = Vec::new();
    for path in ["/user", "/user/emails"] {
        let sent = client
            .get(format!("{API}{path}"))
            .header(header::AUTHORIZATION, format!("Bearer {access}"))
            .header(header::ACCEPT, "application/vnd.github+json")
            .send()
            .await;
        let Ok(resp) = sent else {
            return Ok(GitHub::Failed);
        };
        let Ok(body) = resp.bytes().await else {
            return Ok(GitHub::Failed);
        };
        out.push(json_of(&body)?);
    }
    let emails = out.pop().unwrap_or(Value::Null);
    let user = out.pop().unwrap_or(Value::Null);
    Ok(GitHub::Ok { user, emails })
}

fn gh_get<'a>(v: &'a Value, key: &str) -> Result<Option<&'a Value>, Rejection> {
    let obj = v.as_object().ok_or_else(|| Rejection::internal("GitHub user is not an object"))?;
    Ok(obj.get(key).filter(|v| !v.is_null()))
}

/// The address pydantic's `EmailStr` stores for a GitHub-verified email.
fn normalize(email: &str) -> Result<String, Rejection> {
    match email_validator::validate(email) {
        Email::Valid(e) => Ok(e),
        Email::Unsupported => Ok(email.to_owned()),
        Email::Invalid(reason) => Err(Rejection::internal(format!("GitHub email rejected: {reason}"))),
    }
}

async fn find_or_create(state: &AppState, github_id: i64, email: &str, gh_user: &Value) -> Result<User, Rejection> {
    if let Some(u) = find_one::<User>(state, USERS, doc! { "github_id": github_id }).await? {
        return Ok(u);
    }
    if let Some(mut u) = user_by_email(state, email).await? {
        u.github_id = Some(github_id);
        u.is_verified = true;
        u.email_bounced = false;
        save_user(state, &u).await?;
        if state.settings().security_email_notifications {
            emails::queue_email(state, &emails::github_linked_email(state.settings(), &u)).await?;
        }
        return Ok(u);
    }
    let name = gh_get(gh_user, "name")?.filter(|v| crate::features::py_truthy(v));
    let picked = match name {
        Some(v) => Some(v.clone()),
        None => gh_get(gh_user, "login")?.cloned(),
    };
    let display_name = match picked {
        None => None,
        Some(Value::String(s)) => Some(s),
        Some(_) => return Err(Rejection::internal("GitHub name is not a string")),
    };
    let created = dt::now();
    let updated = dt::now();
    let user = User {
        id: ObjectId::new(),
        email: normalize(email)?,
        hashed_password: hash_password_blocking(token_urlsafe(32)).await,
        display_name,
        is_active: true,
        is_verified: true,
        is_superuser: false,
        roles: Vec::new(),
        token_version: 0,
        github_id: Some(github_id),
        email_bounced: false,
        created_at: dt::to_bson(created),
        updated_at: dt::to_bson(updated),
        last_login_at: None,
    };
    insert_user(state, &user).await?;
    Ok(user)
}

pub async fn callback(State(state): State<AppState>, req: Request) -> Response {
    let ((), q, parts) = match without_body::<(), CallbackQuery>(&state, req).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    reply(callback_run(&state, &parts, q).await)
}

async fn callback_run(state: &AppState, parts: &Parts, q: CallbackQuery) -> Result<Response, Rejection> {
    let s = state.settings();
    require_enabled(s)?;
    let (Some(code), Some(st)) = (q.code.filter(|c| !c.is_empty()), q.state.filter(|c| !c.is_empty())) else {
        return Ok(back(s, "oauth_error=missing"));
    };
    let Some(mut r) = state.inner.db.redis.clone() else {
        return Ok(back(s, "oauth_error=unavailable"));
    };
    let deleted: i64 = r.del(format!("oauthstate:{st}")).await.map_err(Rejection::internal)?;
    if deleted == 0 {
        return Ok(back(s, "oauth_error=state"));
    }
    let (gh_user, gh_emails) = match github(s, &code).await? {
        GitHub::Ok { user, emails } => (user, emails),
        GitHub::Exchange => return Ok(back(s, "oauth_error=exchange")),
        GitHub::Failed => {
            tracing::error!("GitHub OAuth request failed");
            return Ok(back(s, "oauth_error=github"));
        }
    };
    let github_id = gh_get(&gh_user, "id")?.cloned();
    let email = pick_email(&gh_emails)?.filter(crate::features::py_truthy);
    let (Some(github_id), Some(email)) = (github_id, email) else {
        return Ok(back(s, "oauth_error=noemail"));
    };
    let github_id = github_id.as_i64().ok_or_else(|| Rejection::internal("GitHub id is not an int"))?;
    let email = email.as_str().ok_or_else(|| Rejection::internal("GitHub email is not a string"))?.to_lowercase();
    let mut user = find_or_create(state, github_id, &email, &gh_user).await?;
    if !user.is_active {
        return Ok(back(s, "oauth_error=inactive"));
    }
    user.last_login_at = Some(dt::to_bson(dt::now()));
    save_user(state, &user).await?;
    let device = device_label(user_agent(parts).as_deref());
    let tokens = issue_tokens(state, Audience::Portal, user.id, user.token_version, device).await?;
    let xcode = token_urlsafe(24);
    let payload = crate::pyjson::dumps_spaced(&json!({ "a": tokens.access_token, "r": tokens.refresh_token }));
    let _: () = r.set_ex(format!("oauthx:{xcode}"), payload, 120).await.map_err(Rejection::internal)?;
    Ok(back(s, &format!("oauth={xcode}")))
}

pub struct ExchangeBody(pub String);

impl FromBody for ExchangeBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(ExchangeBody(m.field("code").str().required()))).required()
    }
}

pub async fn exchange(State(state): State<AppState>, req: Request) -> Response {
    let (_, _, ExchangeBody(code), _) = match with_body::<(), NoParams, ExchangeBody>(&state, req).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let run = async {
        let invalid = || Rejection::new(StatusCode::BAD_REQUEST, "bad_request", "Invalid or expired exchange code");
        let raw: Option<String> = match state.inner.db.redis.clone() {
            Some(mut r) => r.get_del(format!("oauthx:{code}")).await.map_err(Rejection::internal)?,
            None => None,
        };
        let raw = raw.ok_or_else(invalid)?;
        let d: Value = serde_json::from_str(&raw).map_err(Rejection::internal)?;
        let pick = |k: &str| d.get(k).cloned().ok_or_else(|| Rejection::internal("exchange payload"));
        let expires_in = crate::auth::auth_settings().access_token_expire_minutes * 60;
        let body = json!({
            "access_token": pick("a")?, "refresh_token": pick("r")?, "token_type": "bearer", "expires_in": expires_in,
        });
        Ok(PydanticJson(body).into_response())
    };
    reply(run.await)
}
