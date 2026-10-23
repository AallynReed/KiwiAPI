//! `app/auth/account.py`: the GDPR export and account self-delete.

use axum::{
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use mongodb::bson::{Bson, Document, doc};
use serde_json::{Value, json};

use super::{
    account::email_field,
    req::{NoParams, with_body},
    store::{coll, find_all},
};
use crate::{
    auth::{
        CurrentUser, Rejection,
        models::{API_TOKENS, ApiToken, SESSIONS, SessionRow, USERS, User},
    },
    dt,
    pyjson::{self, no_content},
    state::AppState,
    usage::USAGE_COLLECTION,
    validation::{Field, FromBody},
};

fn iso(d: mongodb::bson::DateTime) -> Value {
    Value::String(dt::isoformat(&dt::from_bson(d)))
}

fn iso_opt(d: Option<mongodb::bson::DateTime>) -> Value {
    d.map_or(Value::Null, iso)
}

fn float_of(b: Option<&Bson>) -> Value {
    let f = match b {
        Some(Bson::Double(f)) => *f,
        Some(Bson::Int32(i)) => f64::from(*i),
        Some(Bson::Int64(i)) => *i as f64,
        _ => return Value::Null,
    };
    serde_json::Number::from_f64(f).map_or(Value::Null, Value::Number)
}

fn int_of(b: Option<&Bson>) -> Value {
    match b {
        Some(Bson::Int32(i)) => json!(i),
        Some(Bson::Int64(i)) => json!(i),
        _ => Value::Null,
    }
}

fn str_of(b: Option<&Bson>) -> Value {
    match b {
        Some(Bson::String(s)) => json!(s),
        _ => Value::Null,
    }
}

/// The `export_account` document, `jsonable_encoder` style (`+00:00` datetimes).
pub fn export_doc(
    exported_at: DateTime<Utc>,
    user: &User,
    tokens: &[ApiToken],
    sessions: &[SessionRow],
    usage: &[Document],
) -> Value {
    let tokens: Vec<Value> = tokens
        .iter()
        .map(|t| {
            json!({
                "id": t.id.to_hex(), "name": t.name, "prefix": t.prefix, "scopes": t.scopes,
                "allowed_ip_count": t.allowed_ip_hashes.len(),
                "revoked": t.revoked, "created_at": iso(t.created_at),
                "last_used_at": iso_opt(t.last_used_at), "expires_at": iso_opt(t.expires_at),
                "request_count": t.request_count,
            })
        })
        .collect();
    let sessions: Vec<Value> = sessions
        .iter()
        .map(|s| {
            json!({
                "id": s.id.to_hex(), "device": s.device, "revoked": s.revoked,
                "created_at": iso(s.created_at), "last_used_at": iso(s.last_used_at), "expires_at": iso(s.expires_at),
            })
        })
        .collect();
    let usage: Vec<Value> = usage
        .iter()
        .map(|e| {
            json!({
                "method": str_of(e.get("method")), "route": str_of(e.get("route")), "path": str_of(e.get("path")),
                "status_code": int_of(e.get("status_code")), "duration_ms": float_of(e.get("duration_ms")),
                "created_at": e.get_datetime("created_at").map_or(Value::Null, |d| iso(*d)),
            })
        })
        .collect();
    json!({
        "exported_at": dt::isoformat(&exported_at),
        "user": {
            "id": user.id.to_hex(), "email": user.email, "display_name": user.display_name,
            "is_verified": user.is_verified, "is_superuser": user.is_superuser,
            "roles": user.roles, "github_linked": user.github_id.is_some(),
            "created_at": iso(user.created_at), "last_login_at": iso_opt(user.last_login_at),
        },
        "api_tokens": tokens,
        "sessions": sessions,
        "usage_events": usage,
    })
}

pub async fn export(State(state): State<AppState>, CurrentUser(user): CurrentUser) -> Response {
    let run = async {
        let by_user = doc! { "user_id": user.id };
        let tokens: Vec<ApiToken> = find_all(&state, API_TOKENS, by_user.clone()).await?;
        let sessions: Vec<SessionRow> = find_all(&state, SESSIONS, by_user.clone()).await?;
        let usage: Vec<Document> = find_all(&state, USAGE_COLLECTION, by_user).await?;
        let body = pyjson::dumps(&export_doc(dt::now(), &user, &tokens, &sessions, &usage));
        let mut resp = pyjson::json_response(body);
        resp.headers_mut()
            .insert(header::CONTENT_DISPOSITION, HeaderValue::from_static("attachment; filename=\"kiwi-export.json\""));
        Ok::<_, Rejection>(resp)
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

pub struct DeleteAccount(pub String);

impl FromBody for DeleteAccount {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(DeleteAccount(email_field(m.field("confirm_email"))))).required()
    }
}

pub async fn delete_account(State(state): State<AppState>, req: Request) -> Response {
    let (CurrentUser(user), _, DeleteAccount(confirm), _) =
        match with_body::<CurrentUser, NoParams, DeleteAccount>(&state, req).await {
            Ok(v) => v,
            Err(r) => return r,
        };
    let run = async {
        if confirm.to_lowercase() != user.email.to_lowercase() {
            return Err(Rejection::new(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "The confirmation email doesn't match your account",
            ));
        }
        let by_user = doc! { "user_id": user.id };
        for name in [API_TOKENS, SESSIONS, USAGE_COLLECTION] {
            coll(&state, name)?.delete_many(by_user.clone()).await.map_err(Rejection::internal)?;
        }
        coll(&state, USERS)?.delete_one(doc! { "_id": user.id }).await.map_err(Rejection::internal)?;
        Ok(no_content())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}
