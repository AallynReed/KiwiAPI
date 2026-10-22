//! `app/site_auth/router.py`: token rotation, logout, profile, the Trove-name
//! claim, the claim's leaderboard stats and the session list.

use axum::{
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures::TryStreamExt;
use kiwi_core::pystr;
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use serde_json::{Value, json};

use super::{
    email,
    store::{self, coll, date, internal, iso, save_user, to_public},
};
use crate::{
    auth::{
        CurrentSiteUser, VerifiedSiteUser,
        client::device_label,
        models::{SITE_SESSIONS, SessionRow},
        sessions::{self, Audience},
        site::{REFRESH_COOKIE, clear_session_cookies, parse_cookies, set_session_cookies},
    },
    dt,
    error::{ApiError, ErrorCode},
    pyjson::{self, PydanticJson},
    routes::lb_players::{render, service, store::Live},
    state::AppState,
    validation::{DepsThenBody, Field, FromBody, FromParams, Json, Params, Path},
};

/// `SiteRefreshRequest` / `SiteLogoutRequest` (an optional body).
pub struct TokenBody(pub Option<String>);

impl FromBody for TokenBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        let token = body.model(|m| m.field("refresh_token").str().optional()).optional();
        Some(TokenBody(token.flatten()))
    }
}

pub(crate) fn user_agent(headers: &HeaderMap) -> Option<String> {
    headers.get(header::USER_AGENT).map(crate::auth::pyutil::latin1)
}

/// `(payload.refresh_token if payload else None) or request.cookies.get(REFRESH_COOKIE)`.
fn supplied_token(body: TokenBody, headers: &HeaderMap) -> Option<String> {
    body.0.filter(|t| !t.is_empty()).or_else(|| parse_cookies(headers).remove(REFRESH_COOKIE)).filter(|t| !t.is_empty())
}

/// `POST /v1/site-auth/refresh`.
pub async fn refresh(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TokenBody>,
) -> Result<Response, ApiError> {
    let rotated = match supplied_token(body, &headers) {
        Some(token) => sessions::rotate(&state, Audience::Site, &token, device_label(user_agent(&headers).as_deref()))
            .await
            .map_err(|r| *r.error)?
            .ok(),
        None => None,
    };
    let Some(tokens) = rotated else {
        return Err(ApiError::new(
            StatusCode::UNAUTHORIZED,
            ErrorCode::NotAuthenticated,
            "Invalid or expired refresh token",
        ));
    };
    let mut res = PydanticJson(serde_json::to_value(&tokens).map_err(internal)?).into_response();
    set_session_cookies(res.headers_mut(), &tokens);
    Ok(res)
}

fn cleared() -> Response {
    let mut res = pyjson::no_content();
    clear_session_cookies(res.headers_mut());
    res
}

/// `POST /v1/site-auth/logout`.
pub async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<TokenBody>,
) -> Result<Response, ApiError> {
    if let Some(token) = supplied_token(body, &headers) {
        sessions::revoke_by_refresh_token(&state, Audience::Site, &token).await.map_err(|r| *r.error)?;
    }
    Ok(cleared())
}

/// `POST /v1/site-auth/logout-all`: bump `token_version`, revoke every live session.
pub async fn logout_all(
    State(state): State<AppState>,
    CurrentSiteUser(mut user): CurrentSiteUser,
) -> Result<Response, ApiError> {
    user.token_version += 1;
    user.updated_at = dt::to_bson(dt::now());
    save_user(&state, &user).await?;
    coll(&state, SITE_SESSIONS)?
        .update_many(doc! { "site_user_id": user.id, "revoked": false }, doc! { "$set": { "revoked": true } })
        .await
        .map_err(internal)?;
    Ok(cleared())
}

/// `GET /v1/site-auth/me`.
pub async fn me(CurrentSiteUser(user): CurrentSiteUser) -> PydanticJson {
    PydanticJson(to_public(&user, None))
}

/// `SiteUpdateProfileRequest`.
pub struct ProfileBody {
    pub display_name: Option<String>,
    pub notify_email: Option<String>,
}

impl FromBody for ProfileBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(ProfileBody {
                display_name: m.field("display_name").str().max_len(80).optional(),
                notify_email: m.field("notify_email").str().max_len(254).optional(),
            })
        })
        .required()
    }
}

/// `PATCH /v1/site-auth/me`.
pub async fn update_profile(
    State(state): State<AppState>,
    DepsThenBody(CurrentSiteUser(mut user), body): DepsThenBody<CurrentSiteUser, ProfileBody>,
) -> Result<PydanticJson, ApiError> {
    let mut dirty = false;
    if let Some(name) = &body.display_name {
        let name = pystr::strip(name);
        user.display_name = (!name.is_empty()).then(|| name.to_owned());
        dirty = true;
    }
    if let Some(raw) = &body.notify_email {
        let val = pystr::strip(raw);
        user.notify_email = if val.is_empty() {
            None
        } else {
            let addr = email::validate(val)
                .ok_or_else(|| ApiError::bad_request("That doesn't look like a valid email address."))?;
            Some(addr.to_lowercase())
        };
        dirty = true;
    }
    if dirty {
        user.updated_at = dt::to_bson(dt::now());
        save_user(&state, &user).await?;
    }
    Ok(PydanticJson(to_public(&user, None)))
}

/// `SiteClaimTroveNameRequest`.
pub struct ClaimBody {
    pub trove_name: String,
}

impl FromBody for ClaimBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(ClaimBody { trove_name: m.field("trove_name").str().min_len(1).max_len(80).required() }))
            .required()
    }
}

/// `_build_claim_baseline`: the best captured score per board uuid.
async fn claim_baseline(state: &AppState, name: &str) -> Result<Document, ApiError> {
    let rows = service::player_history(&Live(state), name, 500, None, false, None).await?;
    let mut out = Document::new();
    for r in rows {
        let key = r.row.leaderboard.to_string();
        let best = match out.get(&key) {
            Some(Bson::Double(prev)) => r.row.score > *prev,
            _ => true,
        };
        if best {
            out.insert(key, Bson::Double(r.row.score));
        }
    }
    Ok(out)
}

/// `POST /v1/site-auth/me/claim-trove-name`.
pub async fn claim(
    State(state): State<AppState>,
    DepsThenBody(VerifiedSiteUser(mut user), body): DepsThenBody<VerifiedSiteUser, ClaimBody>,
) -> Result<PydanticJson, ApiError> {
    let name = pystr::strip(&body.trove_name).to_owned();
    if name.is_empty() {
        return Err(ApiError::bad_request("Trove name can't be empty"));
    }
    let lowered = name.to_lowercase();
    let existing = store::find_user(&state, doc! { "claimed_trove_name": &lowered }).await?;
    if existing.is_some_and(|e| e.id != user.id) {
        return Err(ApiError::new(StatusCode::CONFLICT, ErrorCode::BadRequest, "That Trove name is already claimed."));
    }
    let claimed_at = dt::now();
    user.claimed_trove_name = Some(lowered);
    user.claimed_trove_display = Some(name.clone());
    user.claimed_at = Some(dt::to_bson(claimed_at));
    user.claim_verified = false;
    user.claim_verified_at = None;
    user.claim_baseline = claim_baseline(&state, &name).await?;
    user.updated_at = dt::to_bson(dt::now());
    save_user(&state, &user).await?;
    Ok(PydanticJson(to_public(&user, Some(claimed_at))))
}

/// `SiteUser.clear_claim()`.
pub fn clear_claim(user: &mut crate::auth::models::SiteUser) {
    user.claimed_trove_name = None;
    user.claimed_trove_display = None;
    user.claimed_at = None;
    user.claim_verified = false;
    user.claim_verified_at = None;
    user.claim_baseline = Document::new();
}

/// `DELETE /v1/site-auth/me/claim-trove-name`.
pub async fn unclaim(
    State(state): State<AppState>,
    CurrentSiteUser(mut user): CurrentSiteUser,
) -> Result<PydanticJson, ApiError> {
    clear_claim(&mut user);
    user.updated_at = dt::to_bson(dt::now());
    save_user(&state, &user).await?;
    Ok(PydanticJson(to_public(&user, None)))
}

/// `POST /v1/site-auth/me/verify-trove-claim`: always pending until a master approves.
pub async fn verify_claim(VerifiedSiteUser(user): VerifiedSiteUser) -> Result<PydanticJson, ApiError> {
    if user.claimed_trove_name.as_deref().is_none_or(str::is_empty) {
        return Err(ApiError::bad_request("No Trove name claimed - claim one first."));
    }
    let (verified, detail) = if user.claim_verified {
        (true, "Already verified.")
    } else {
        (false, "Your claim is pending manual review by an admin.")
    };
    Ok(PydanticJson(json!({ "verified": verified, "detail": detail, "user": to_public(&user, None) })))
}

/// `GET /v1/site-auth/me/trove-stats`.
pub async fn trove_stats(
    State(state): State<AppState>,
    CurrentSiteUser(user): CurrentSiteUser,
) -> Result<PydanticJson, ApiError> {
    let Some(claimed) = user.claimed_trove_name.clone().filter(|s| !s.is_empty()) else {
        return Ok(PydanticJson(json!({ "claimed": false, "items": [], "series": null })));
    };
    let name = user.claimed_trove_display.clone().filter(|s| !s.is_empty()).unwrap_or(claimed);
    let live = Live(&state);
    let history = service::player_history(&live, &name, 50, None, false, None).await?;
    let series = service::player_history_series(&live, &name, 7).await?;
    let items: Vec<Value> = history.iter().map(|r| Value::Object(render::site_row(r))).collect();
    Ok(PydanticJson(json!({
        "claimed": true,
        "player_name": name,
        "items": items,
        "series": render::site_series_value(&series),
    })))
}

/// `GET /v1/site-auth/sessions`: live sessions in natural order.
pub async fn list_sessions(
    State(state): State<AppState>,
    CurrentSiteUser(user): CurrentSiteUser,
) -> Result<PydanticJson, ApiError> {
    let filter = doc! { "site_user_id": user.id, "revoked": false, "expires_at": { "$gt": date(dt::now()) } };
    let rows: Vec<SessionRow> = store::db(&state)?
        .collection::<SessionRow>(SITE_SESSIONS)
        .find(filter)
        .await
        .map_err(internal)?
        .try_collect()
        .await
        .map_err(internal)?;
    let out: Vec<Value> = rows
        .iter()
        .map(|s| {
            json!({
                "id": s.id.to_hex(),
                "device": s.device,
                "created_at": iso(Some(s.created_at)),
                "last_used_at": iso(Some(s.last_used_at)),
                "expires_at": iso(Some(s.expires_at)),
            })
        })
        .collect();
    Ok(PydanticJson(Value::Array(out)))
}

pub struct SessionId(pub ObjectId);

impl FromParams for SessionId {
    fn from_params(p: &mut Params<'_>) -> Self {
        SessionId(p.path("session_id").object_id().required())
    }
}

/// `DELETE /v1/site-auth/sessions/{session_id}`.
pub async fn revoke_session(
    State(state): State<AppState>,
    CurrentSiteUser(user): CurrentSiteUser,
    Path(SessionId(id)): Path<SessionId>,
) -> Result<Response, ApiError> {
    let found = store::db(&state)?
        .collection::<SessionRow>(SITE_SESSIONS)
        .find_one(doc! { "_id": id })
        .await
        .map_err(internal)?;
    let Some(mut session) = found.filter(|s| s.owner_id == user.id) else {
        return Err(ApiError::not_found("Session not found"));
    };
    if !session.revoked {
        session.revoked = true;
        store::save_session(&state, &session).await?;
    }
    Ok(pyjson::no_content())
}
