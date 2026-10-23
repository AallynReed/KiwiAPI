//! Store-backed flow, opt-in because it writes: point MONGO_URI / MONGO_DB /
//! REDIS_URL at throwaway stores and run
//! `cargo test -p kiwi-server site_auth_store_flow -- --ignored`.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::{
    auth::sessions::{self, Audience},
    config::Settings,
    state::AppState,
};

async fn call(app: &Router, method: &str, path: &str, bearer: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(path).header("authorization", format!("Bearer {bearer}"));
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

#[tokio::test]
#[ignore = "writes to MONGO_URI / REDIS_URL"]
async fn site_auth_store_flow() {
    let state = AppState::new(Settings::from_env().unwrap()).await.unwrap();
    let db = state.inner.db.mongo.clone().expect("MONGO_URI");
    let mut redis = state.inner.db.redis.clone().expect("REDIS_URL");
    let app = super::router().with_state(state.clone());

    let uid = ObjectId::new();
    let now = DateTime::now();
    db.collection::<Document>("site_users")
        .insert_one(doc! { "_id": uid, "username": format!("t{}", &uid.to_hex()[..10]), "is_active": true,
        "is_verified": true, "discord_id": 4194304_i64 * 7, "token_version": 0, "created_at": now, "updated_at": now })
        .await
        .unwrap();
    let pair = sessions::issue_tokens(&state, Audience::Site, uid, 0, Some("Chrome on Linux".into())).await.unwrap();
    let jwt = pair.access_token.clone();

    let (st, me) = call(
        &app,
        "PATCH",
        "/v1/site-auth/me",
        &jwt,
        Some(json!({"display_name": "  Kiwi ", "notify_email": " A@Example.COM "})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{me}");
    assert_eq!(me["display_name"], "Kiwi");
    assert_eq!(me["notify_email"], "a@example.com");
    assert_eq!(me["avatar_url"], "https://cdn.discordapp.com/embed/avatars/1.png");
    let (st, _) = call(&app, "PATCH", "/v1/site-auth/me", &jwt, Some(json!({"notify_email": "nope"}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    let (st, req) =
        call(&app, "POST", "/v1/site-auth/me/username-request", &jwt, Some(json!({"username": "Kiwi_New"}))).await;
    assert_eq!(st, StatusCode::OK, "{req}");
    assert_eq!(req["request"]["requested_username"], "kiwi_new");
    let (_, got) = call(&app, "GET", "/v1/site-auth/me/username-request", &jwt, None).await;
    assert_eq!(got["request"]["id"], req["request"]["id"]);
    let (_, ok) = call(&app, "DELETE", "/v1/site-auth/me/username-request", &jwt, None).await;
    assert_eq!(ok, json!({"ok": true}));

    let (_, list) = call(&app, "GET", "/v1/site-auth/sessions", &jwt, None).await;
    assert_eq!(list.as_array().map(Vec::len), Some(1));
    let sid = list[0]["id"].as_str().unwrap().to_owned();
    let (st, _) = call(&app, "DELETE", &format!("/v1/site-auth/sessions/{sid}"), &jwt, None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = call(&app, "DELETE", "/v1/site-auth/sessions/zz", &jwt, None).await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);

    let _: () = redis::cmd("SET")
        .arg("site_oauthx:parity-code")
        .arg(json!({"a": jwt, "r": pair.refresh_token}).to_string())
        .arg("EX")
        .arg(120)
        .query_async(&mut redis)
        .await
        .unwrap();
    let (st, tokens) =
        call(&app, "POST", "/v1/site-auth/oauth/exchange", "", Some(json!({"code": "parity-code"}))).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(tokens["access_token"], jwt.as_str());
    let (st, _) = call(&app, "POST", "/v1/site-auth/oauth/exchange", "", Some(json!({"code": "parity-code"}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    let (st, export) = call(&app, "GET", "/v1/site-auth/me/export", &jwt, None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(export["account"]["notify_email"], "a@example.com");

    let username = me["username"].as_str().unwrap().to_uppercase();
    let (st, _) = call(&app, "POST", "/v1/site-auth/me/delete", &jwt, Some(json!({"confirm_username": "x"}))).await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) =
        call(&app, "POST", "/v1/site-auth/me/delete", &jwt, Some(json!({"confirm_username": username}))).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let row = db.collection::<Document>("site_users").find_one(doc! { "_id": uid }).await.unwrap().unwrap();
    assert!(row.get_str("username").unwrap().starts_with("deleted_"));
    assert_eq!(row.get_bool("is_deleted"), Ok(true));
    assert_eq!(row.get_i32("token_version"), Ok(1));
    let (st, _) = call(&app, "GET", "/v1/site-auth/me", &jwt, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED);
}
