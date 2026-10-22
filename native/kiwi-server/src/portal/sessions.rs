//! `app/auth/sessions.py`: refresh rotation, logout, logout-all and the
//! active-session list.

use axum::{
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use mongodb::bson::{DateTime, doc, oid::ObjectId};
use serde_json::{Value, json};

use super::{
    req::{NoParams, with_body, without_body},
    store::{find_all, find_one, pyd, revoke_all_sessions, save_session},
};
use crate::{
    auth::{
        AuthContext, CurrentUser, Rejection,
        client::{device_label, user_agent},
        models::{SESSIONS, SessionRow},
        sessions::{Audience, RotateError, revoke_by_refresh_token, rotate},
    },
    pyjson::{PydanticJson, no_content},
    state::AppState,
    validation::{Field, FromBody, FromParams, Params},
};

pub struct RefreshBody(pub String);

impl FromBody for RefreshBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(RefreshBody(m.field("refresh_token").str().required()))).required()
    }
}

pub struct LogoutBody(pub Option<String>);

impl FromBody for LogoutBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(LogoutBody(m.field("refresh_token").str().optional()))).required()
    }
}

pub struct SessionId(pub ObjectId);

impl FromParams for SessionId {
    fn from_params(p: &mut Params<'_>) -> Self {
        SessionId(p.path("session_id").object_id().required())
    }
}

pub async fn refresh(State(state): State<AppState>, req: Request) -> Response {
    let (_, _, RefreshBody(token), parts) = match with_body::<(), NoParams, RefreshBody>(&state, req).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let device = device_label(user_agent(&parts).as_deref());
    let run = async {
        match rotate(&state, Audience::Portal, &token, device).await? {
            Ok(pair) => Ok(PydanticJson(serde_json::to_value(pair).map_err(Rejection::internal)?).into_response()),
            Err(RotateError::InvalidToken) => {
                Err(Rejection::new(StatusCode::UNAUTHORIZED, "not_authenticated", "Invalid or expired refresh token"))
            }
            Err(RotateError::InactiveUser) => {
                Err(Rejection::new(StatusCode::UNAUTHORIZED, "not_authenticated", "Account is inactive"))
            }
        }
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

pub async fn logout(State(state): State<AppState>, req: Request) -> Response {
    let (_, _, LogoutBody(token), _) = match with_body::<(), NoParams, LogoutBody>(&state, req).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    match revoke_by_refresh_token(&state, Audience::Portal, token.as_deref().unwrap_or_default()).await {
        Ok(()) => no_content(),
        Err(e) => e.into_response(),
    }
}

pub async fn logout_all(State(state): State<AppState>, CurrentUser(mut user): CurrentUser) -> Response {
    match revoke_all_sessions(&state, &mut user).await {
        Ok(()) => no_content(),
        Err(e) => e.into_response(),
    }
}

/// `SessionPublic`.
pub fn session_public(s: &SessionRow, current: Option<&str>) -> Value {
    let id = s.id.to_hex();
    json!({
        "id": id,
        "device": s.device,
        "created_at": pyd(s.created_at),
        "last_used_at": pyd(s.last_used_at),
        "expires_at": pyd(s.expires_at),
        "current": Some(id.as_str()) == current,
    })
}

pub async fn list(State(state): State<AppState>, ctx: AuthContext) -> Response {
    let filter = doc! { "user_id": ctx.user.id, "revoked": false, "expires_at": { "$gt": DateTime::now() } };
    match find_all::<SessionRow>(&state, SESSIONS, filter).await {
        Ok(rows) => {
            let out: Vec<Value> = rows.iter().map(|s| session_public(s, ctx.session_id.as_deref())).collect();
            PydanticJson(Value::Array(out)).into_response()
        }
        Err(e) => e.into_response(),
    }
}

pub async fn revoke(State(state): State<AppState>, req: Request) -> Response {
    let (CurrentUser(user), SessionId(id), _) = match without_body::<CurrentUser, SessionId>(&state, req).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    let run = async {
        let session: Option<SessionRow> = find_one(&state, SESSIONS, doc! { "_id": id }).await?;
        let Some(mut session) = session.filter(|s| s.owner_id == user.id) else {
            return Err(Rejection::new(StatusCode::NOT_FOUND, "not_found", "Session not found"));
        };
        if !session.revoked {
            session.revoked = true;
            save_session(&state, &session).await?;
        }
        Ok(no_content())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}
