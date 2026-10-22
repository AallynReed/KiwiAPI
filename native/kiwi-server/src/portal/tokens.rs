//! `app/tokens/router.py`: create, list, edit, rotate and revoke API tokens,
//! and their usage activity. Every route is portal-session (JWT) auth.

use std::net::Ipv6Addr;

use axum::{
    extract::{MatchedPath, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::Duration;
use mongodb::bson::{doc, oid::ObjectId};
use serde_json::Value;

use super::{
    activity::aggregate_activity,
    emails,
    req::{NoParams, with_body, with_body_at, without_body, without_body_at},
    store::{TokenTimes, find_all, find_one, insert_token, save_token, token_public},
};
use crate::{
    auth::{
        CurrentUser, Rejection,
        models::{API_TOKENS, ApiToken, User},
        scopes::is_valid_mask,
        tokens::{CIDR_MESSAGE, IpError, generate_api_token, hash_ip, make_ip_salt, normalize_ip},
    },
    dt,
    pyjson::PydanticJson,
    ratelimit::check_rate_limit,
    state::AppState,
    validation::{Fail, Field, FromBody, FromParams, Input, Params},
};

fn bad_request(msg: impl Into<String>) -> Rejection {
    Rejection::new(StatusCode::BAD_REQUEST, "bad_request", msg)
}

fn reply(r: Result<Response, Rejection>) -> Response {
    r.unwrap_or_else(IntoResponse::into_response)
}

/// `ipaddress.ip_address(value.strip())` as text, with IPv6 scope ids.
pub fn normalize_ip_py(value: &str) -> Result<String, IpError> {
    let v = kiwi_core::pystr::strip(value);
    if !v.contains('/')
        && let Some((addr, scope)) = v.split_once('%')
    {
        if scope.is_empty() || scope.contains('%') {
            return Err(IpError::Invalid);
        }
        let ip: Ipv6Addr = addr.parse().map_err(|_| IpError::Invalid)?;
        return Ok(format!("{ip}%{scope}"));
    }
    normalize_ip(v)
}

/// `_hash_pinned_ips`: validated, deduped HMACs; 400 on a bad entry.
pub fn hash_pinned_ips(ips: &[String], salt: &str) -> Result<Vec<String>, Rejection> {
    let mut hashes = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for raw in ips {
        let entry = kiwi_core::pystr::strip(raw);
        if entry.is_empty() {
            continue;
        }
        let canon = match normalize_ip_py(entry) {
            Ok(c) => c,
            Err(IpError::Cidr) => return Err(bad_request(CIDR_MESSAGE)),
            Err(IpError::Invalid) if entry.contains("CIDR") => {
                let shown = kiwi_core::pystr::repr(entry);
                return Err(bad_request(format!("{shown} does not appear to be an IPv4 or IPv6 address")));
            }
            Err(IpError::Invalid) => return Err(bad_request(format!("Invalid IP: {entry}"))),
        };
        if seen.contains(&canon) {
            continue;
        }
        hashes.push(hash_ip(salt, &canon));
        seen.push(canon);
    }
    Ok(hashes)
}

// --- parameters and bodies ------------------------------------------------------------

pub struct CreateToken {
    pub name: String,
    pub scopes: i64,
    pub allowed_ips: Vec<String>,
    pub expires_in_days: Option<i64>,
}

fn literal_days(input: &Input<'_>) -> Result<i64, Fail> {
    let fail = || Fail::new("literal_error", "Input should be 30, 60 or 90").with_ctx("expected", "30, 60 or 90");
    let Input::Json(Value::Number(n)) = input else {
        return Err(fail());
    };
    let v = match (n.as_i64(), n.as_f64()) {
        (Some(i), _) => i,
        (None, Some(f)) if f.fract() == 0.0 && f.abs() < 1e3 => f as i64,
        _ => return Err(fail()),
    };
    if [30, 60, 90].contains(&v) { Ok(v) } else { Err(fail()) }
}

impl FromBody for CreateToken {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(CreateToken {
                name: m.field("name").str().min_len(1).max_len(80).required(),
                scopes: m.field("scopes").int().ge(0).saturating().default(0),
                allowed_ips: m.field("allowed_ips").list(|f| f.str().required()).default(Vec::new()),
                expires_in_days: m.field("expires_in_days").custom(literal_days).nullable_default(30),
            })
        })
        .required()
    }
}

pub struct EditToken {
    pub name: Option<String>,
    pub allowed_ips: Option<Vec<String>>,
}

impl FromBody for EditToken {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(EditToken {
                name: m.field("name").str().min_len(1).max_len(80).optional(),
                allowed_ips: m.field("allowed_ips").list(|f| f.str().required()).optional(),
            })
        })
        .required()
    }
}

pub struct RevokeToken(pub String);

impl FromBody for RevokeToken {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(RevokeToken(m.field("reason").str().min_len(1).max_len(200).required()))).required()
    }
}

pub struct TokenId(pub ObjectId);

impl FromParams for TokenId {
    fn from_params(p: &mut Params<'_>) -> Self {
        TokenId(p.path("token_id").object_id().required())
    }
}

pub struct Days(pub i64);

impl FromParams for Days {
    fn from_params(p: &mut Params<'_>) -> Self {
        Days(p.query("days").int().ge(1).le(365).default(7))
    }
}

pub struct TokenDays(pub ObjectId, pub i64);

impl FromParams for TokenDays {
    fn from_params(p: &mut Params<'_>) -> Self {
        let TokenId(id) = TokenId::from_params(p);
        let Days(days) = Days::from_params(p);
        TokenDays(id, days)
    }
}

/// The `{token_id}` Starlette captured when a literal route stands in for it.
fn literal_id(req: &Request) -> Option<Vec<(String, String)>> {
    let matched = req.extensions().get::<MatchedPath>()?.as_str();
    (!matched.contains("{token_id}")).then(|| vec![("token_id".to_owned(), "activity".to_owned())])
}

// --- routes ---------------------------------------------------------------------------

async fn owned_token(state: &AppState, id: ObjectId, user: &User) -> Result<ApiToken, Rejection> {
    let token: Option<ApiToken> = find_one(state, API_TOKENS, doc! { "_id": id }).await?;
    token
        .filter(|t| t.user_id == user.id)
        .ok_or_else(|| Rejection::new(StatusCode::NOT_FOUND, "not_found", "Token not found"))
}

fn created_response(public: Value, full: String, status: StatusCode) -> Response {
    let mut v = public;
    if let Value::Object(m) = &mut v {
        m.insert("token".into(), Value::String(full));
    }
    (status, PydanticJson(v)).into_response()
}

pub async fn create(State(state): State<AppState>, req: Request) -> Response {
    let (CurrentUser(user), _, body, _) = match with_body::<CurrentUser, NoParams, CreateToken>(&state, req).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let run = async {
        let s = state.settings();
        if s.require_verified_for_tokens && !user.is_verified {
            return Err(Rejection::forbidden(
                "email_unverified",
                "Verify your email address before creating API tokens.",
            ));
        }
        check_rate_limit(&state, &format!("tokencreate:{}", user.id), s.token_creation_daily_limit, 86400).await?;
        if !is_valid_mask(body.scopes) {
            return Err(bad_request("Invalid scope bitmask - it sets bits that aren't real scopes"));
        }
        let salt = make_ip_salt();
        let allowed_ip_hashes = hash_pinned_ips(&body.allowed_ips, &salt)?;
        let (full, hashed, prefix) = generate_api_token(&s.api_token_prefix);
        let expires = body.expires_in_days.map(|d| dt::now() + Duration::days(d));
        let created = dt::now();
        let token = ApiToken {
            id: ObjectId::new(),
            user_id: user.id,
            name: body.name,
            prefix,
            hashed_token: hashed,
            scopes: body.scopes,
            ip_salt: Some(salt),
            allowed_ip_hashes,
            revoked: false,
            revoked_at: None,
            revoke_reason: None,
            expires_at: expires.map(dt::to_bson),
            expiry_warned: false,
            created_at: dt::to_bson(created),
            last_used_at: None,
            rotated_at: None,
            request_count: 0,
        };
        insert_token(&state, &token).await?;
        let times = TokenTimes {
            created_at: dt::pydantic(&created),
            expires_at: expires.map_or(Value::Null, |e| Value::String(dt::pydantic(&e))),
        };
        let public = token_public(&token, times);
        if s.security_email_notifications {
            let (name, prefix) = (token.name.clone(), token.prefix.clone());
            emails::background(&state, move |st| {
                Some(emails::token_created_email(st.settings(), &user, &name, &prefix))
            });
        }
        Ok(created_response(public, full, StatusCode::CREATED))
    };
    reply(run.await)
}

pub async fn list(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> Response {
    match find_all::<ApiToken>(&state, API_TOKENS, doc! { "user_id": user.id }).await {
        Ok(tokens) => {
            let out: Vec<Value> = tokens.iter().map(|t| token_public(t, TokenTimes::stored(t))).collect();
            PydanticJson(Value::Array(out)).into_response()
        }
        Err(e) => e.into_response(),
    }
}

pub async fn my_activity(State(state): State<AppState>, req: Request) -> Response {
    let (CurrentUser(user), Days(days), _) = match without_body::<CurrentUser, Days>(&state, req).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    match aggregate_activity(&state, doc! { "user_id": user.id }, days).await {
        Ok(v) => PydanticJson(v).into_response(),
        Err(e) => e.into_response(),
    }
}

pub async fn token_activity(State(state): State<AppState>, req: Request) -> Response {
    let path = literal_id(&req);
    let (CurrentUser(user), TokenDays(id, days), _) =
        match without_body_at::<CurrentUser, TokenDays>(&state, req, path).await {
            Ok(v) => v,
            Err(r) => return r,
        };
    let run = async {
        let token = owned_token(&state, id, &user).await?;
        Ok(PydanticJson(aggregate_activity(&state, doc! { "token_id": token.id }, days).await?).into_response())
    };
    reply(run.await)
}

pub async fn edit(State(state): State<AppState>, req: Request) -> Response {
    let path = literal_id(&req);
    let (CurrentUser(user), TokenId(id), body, _) =
        match with_body_at::<CurrentUser, TokenId, EditToken>(&state, req, path).await {
            Ok(v) => v,
            Err(r) => return r,
        };
    let run = async {
        let mut token = owned_token(&state, id, &user).await?;
        if token.revoked {
            return Err(bad_request("Token is revoked"));
        }
        if let Some(name) = body.name {
            token.name = name;
        }
        if let Some(ips) = body.allowed_ips {
            if token.ip_salt.as_deref().is_none_or(str::is_empty) {
                token.ip_salt = Some(make_ip_salt());
            }
            token.allowed_ip_hashes = hash_pinned_ips(&ips, token.ip_salt.as_deref().unwrap_or_default())?;
        }
        save_token(&state, &token).await?;
        Ok(PydanticJson(token_public(&token, TokenTimes::stored(&token))).into_response())
    };
    reply(run.await)
}

pub async fn rotate(State(state): State<AppState>, req: Request) -> Response {
    let path = literal_id(&req);
    let (CurrentUser(user), TokenId(id), _) = match without_body_at::<CurrentUser, TokenId>(&state, req, path).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let run = async {
        let mut token = owned_token(&state, id, &user).await?;
        if token.revoked {
            return Err(bad_request("Token is revoked"));
        }
        let s = state.settings();
        let (full, hashed, prefix) = generate_api_token(&s.api_token_prefix);
        token.hashed_token = hashed;
        token.prefix = prefix;
        token.rotated_at = Some(dt::to_bson(dt::now()));
        token.last_used_at = None;
        token.expiry_warned = false;
        save_token(&state, &token).await?;
        if s.security_email_notifications {
            let (name, prefix) = (token.name.clone(), token.prefix.clone());
            emails::background(&state, move |st| {
                Some(emails::token_created_email(st.settings(), &user, &name, &prefix))
            });
        }
        Ok(created_response(token_public(&token, TokenTimes::stored(&token)), full, StatusCode::OK))
    };
    reply(run.await)
}

pub async fn revoke(State(state): State<AppState>, req: Request) -> Response {
    let path = literal_id(&req);
    let (CurrentUser(user), TokenId(id), RevokeToken(reason), _) =
        match with_body_at::<CurrentUser, TokenId, RevokeToken>(&state, req, path).await {
            Ok(v) => v,
            Err(r) => return r,
        };
    let run = async {
        let mut token = owned_token(&state, id, &user).await?;
        if !token.revoked {
            token.revoked = true;
            token.revoked_at = Some(dt::to_bson(dt::now()));
            token.revoke_reason = Some(reason);
            save_token(&state, &token).await?;
        }
        Ok(PydanticJson(token_public(&token, TokenTimes::stored(&token))).into_response())
    };
    reply(run.await)
}
