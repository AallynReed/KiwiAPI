//! Live-store checks, opt-in because they write: point MONGO_URI / MONGO_DB (and
//! optionally REDIS_URL) at throwaway stores and run
//! `cargo test -p kiwi-server portal::store_tests -- --ignored`.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use tower::ServiceExt;

use super::store::{save_user, user_by_id};
use crate::{
    auth::models::{API_TOKENS, SESSIONS, USERS},
    config::Settings,
    state::AppState,
};

async fn state() -> AppState {
    // SAFETY: only these ignored tests run in their process invocation.
    unsafe { std::env::set_var("PASSWORD_BREACH_CHECK", "false") };
    AppState::new(Settings::from_env().unwrap()).await.unwrap()
}

async fn send(app: &Router, method: &str, uri: &str, auth: Option<&str>, body: Option<Value>) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri).header("user-agent", "Mozilla/5.0 (Windows) Chrome/1");
    if let Some(t) = auth {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let body = match body {
        Some(v) => {
            req = req.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let resp = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn keys(d: &Document) -> Vec<&str> {
    d.keys().map(String::as_str).collect()
}

#[tokio::test]
#[ignore = "writes to MONGO_URI"]
async fn account_and_token_flow() {
    let state = state().await;
    let db = state.mongo().clone();
    let app = super::router().with_state(state.clone());
    let email = format!("store-test-{}@example.com", ObjectId::new().to_hex());
    let pw = "Store-test-pass-1";

    let (st, user) = send(&app, "POST", "/auth/signup", None, Some(json!({"email": email, "password": pw}))).await;
    assert_eq!(st, StatusCode::CREATED, "{user}");
    let uid = ObjectId::parse_str(user["id"].as_str().unwrap()).unwrap();
    let stored = db.collection::<Document>(USERS).find_one(doc! {"_id": uid}).await.unwrap().unwrap();
    assert_eq!(
        keys(&stored),
        [
            "_id",
            "email",
            "hashed_password",
            "display_name",
            "is_active",
            "is_verified",
            "is_superuser",
            "roles",
            "token_version",
            "github_id",
            "email_bounced",
            "created_at",
            "updated_at",
            "last_login_at",
        ]
    );
    assert_eq!(stored.get("token_version"), Some(&Bson::Int32(0)));
    assert_eq!(stored.get("github_id"), Some(&Bson::Null));

    let (st, _) = send(&app, "POST", "/auth/login", None, Some(json!({"email": email, "password": pw}))).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    db.collection::<Document>(USERS).update_one(doc! {"_id": uid}, doc! {"$set": {"is_verified": true}}).await.unwrap();
    let (st, tokens) = send(&app, "POST", "/auth/login", None, Some(json!({"email": email, "password": pw}))).await;
    assert_eq!(st, StatusCode::OK, "{tokens}");
    let access = tokens["access_token"].as_str().unwrap().to_owned();
    assert_eq!(tokens["expires_in"], json!(900));

    let (st, me) = send(&app, "GET", "/auth/me", Some(&access), None).await;
    assert_eq!((st, me["email"].as_str()), (StatusCode::OK, Some(email.as_str())));

    let (st, created) = send(
        &app,
        "POST",
        "/tokens",
        Some(&access),
        Some(json!({"name": "store test", "scopes": 5, "allowed_ips": ["1.2.3.4", "::1"]})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{created}");
    assert!(created["token"].as_str().unwrap().starts_with("kiwi_"));
    let tid = created["id"].as_str().unwrap().to_owned();
    let tok = db
        .collection::<Document>(API_TOKENS)
        .find_one(doc! {"_id": ObjectId::parse_str(&tid).unwrap()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        keys(&tok),
        [
            "_id",
            "user_id",
            "name",
            "prefix",
            "hashed_token",
            "scopes",
            "ip_salt",
            "allowed_ip_hashes",
            "revoked",
            "revoked_at",
            "revoke_reason",
            "expires_at",
            "expiry_warned",
            "created_at",
            "last_used_at",
            "rotated_at",
            "request_count",
        ]
    );
    assert_eq!(tok.get_array("allowed_ip_hashes").unwrap().len(), 2);

    let (st, edited) =
        send(&app, "PATCH", &format!("/tokens/{tid}"), Some(&access), Some(json!({"allowed_ips": []}))).await;
    assert_eq!((st, edited["allowed_ip_count"].as_i64()), (StatusCode::OK, Some(0)));
    let (st, rotated) = send(&app, "POST", &format!("/tokens/{tid}/rotate"), Some(&access), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_ne!(rotated["token"], created["token"]);
    let (st, revoked) =
        send(&app, "POST", &format!("/tokens/{tid}/revoke"), Some(&access), Some(json!({"reason": "test"}))).await;
    assert_eq!((st, revoked["revoked"].as_bool()), (StatusCode::OK, Some(true)));
    let (st, summary) = send(&app, "GET", "/tokens/activity?days=3", Some(&access), None).await;
    assert_eq!((st, summary["total_requests"].as_i64()), (StatusCode::OK, Some(0)));

    let (st, sessions) = send(&app, "GET", "/auth/sessions", Some(&access), None).await;
    assert_eq!((st, sessions[0]["current"].as_bool()), (StatusCode::OK, Some(true)));
    let (st, _) = send(&app, "POST", "/auth/logout-all", Some(&access), None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = send(&app, "GET", "/auth/me", Some(&access), None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
    let live = db.collection::<Document>(SESSIONS).count_documents(doc! {"user_id": uid, "revoked": false}).await;
    assert_eq!(live.unwrap(), 0);

    let (_, tokens) = send(&app, "POST", "/auth/login", None, Some(json!({"email": email, "password": pw}))).await;
    let access = tokens["access_token"].as_str().unwrap().to_owned();
    let (st, _) =
        send(&app, "POST", "/auth/delete-account", Some(&access), Some(json!({"confirm_email": email}))).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(user_by_id(&state, uid).await.unwrap().is_none());
    let left = db.collection::<Document>(API_TOKENS).count_documents(doc! {"user_id": uid}).await.unwrap();
    assert_eq!(left, 0);
}

#[tokio::test]
#[ignore = "writes to MONGO_URI"]
async fn save_fills_legacy_fields() {
    let state = state().await;
    let users = state.mongo().collection::<Document>(USERS);
    let id = ObjectId::new();
    let email = format!("legacy-{}@example.com", id.to_hex());
    users.insert_one(doc! {"_id": id, "email": &email, "hashed_password": "x", "is_active": true}).await.unwrap();
    let user = user_by_id(&state, id).await.unwrap().unwrap();
    save_user(&state, &user).await.unwrap();
    let stored = users.find_one(doc! {"_id": id}).await.unwrap().unwrap();
    assert_eq!(
        keys(&stored),
        [
            "_id",
            "email",
            "hashed_password",
            "is_active",
            "created_at",
            "display_name",
            "email_bounced",
            "github_id",
            "is_superuser",
            "is_verified",
            "last_login_at",
            "roles",
            "token_version",
            "updated_at",
        ]
    );
    users.delete_one(doc! {"_id": id}).await.unwrap();
}
