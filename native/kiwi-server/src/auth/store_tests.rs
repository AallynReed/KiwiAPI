//! Store-backed round trip, opt-in because it writes: point MONGO_URI /
//! MONGO_DB / REDIS_URL at throwaway stores and run
//! `cargo test -p kiwi-server store_roundtrip -- --ignored`.

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
    middleware::from_fn_with_state,
    routing::get,
};
use mongodb::bson::{DateTime, Document, doc, oid::ObjectId};
use tower::ServiceExt;

use super::{
    CurrentSiteUser, CurrentUser, MasterIngest, Public, RequireScope, lockout, scopes,
    sessions::{self, Audience, RotateError},
    tokens,
};
use crate::{config::Settings, ratelimit, state::AppState, usage};

async fn get_status(app: &Router, path: &str, bearer: &str) -> (StatusCode, axum::http::HeaderMap) {
    let req = Request::get(path).header("authorization", format!("Bearer {bearer}")).body(Body::empty()).unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    (resp.status(), resp.headers().clone())
}

#[tokio::test]
#[ignore = "writes to MONGO_URI / REDIS_URL"]
async fn store_roundtrip() {
    let state = AppState::new(Settings::from_env().unwrap()).await.unwrap();
    let db = state.inner.db.mongo.clone().expect("MONGO_URI");
    let mut redis = state.inner.db.redis.clone().expect("REDIS_URL");

    let user_id = ObjectId::new();
    let site_id = ObjectId::new();
    let now = DateTime::now();
    db.collection::<Document>("users")
        .insert_one(doc! { "_id": user_id, "email": format!("{user_id}@example.test"), "hashed_password": "x",
        "is_active": true, "is_superuser": true, "token_version": 0, "created_at": now, "updated_at": now })
        .await
        .unwrap();
    db.collection::<Document>("site_users")
        .insert_one(doc! { "_id": site_id, "username": format!("t{site_id}"), "is_active": true,
        "is_verified": true, "token_version": 0, "created_at": now, "updated_at": now })
        .await
        .unwrap();
    let (api_token, api_hash, prefix) = tokens::generate_api_token("kiwi");
    let token_id = ObjectId::new();
    db.collection::<Document>("api_tokens")
        .insert_one(doc! { "_id": token_id, "user_id": user_id, "name": "t", "prefix": prefix,
        "hashed_token": api_hash, "scopes": scopes::STATS_READ as i32, "allowed_ip_hashes": [],
        "revoked": false, "created_at": now, "request_count": 0 })
        .await
        .unwrap();

    let app = Router::new()
        .route("/scoped", get(|c: RequireScope<{ scopes::STATS_READ }>| async move { (c.0.rate_limit, "ok") }))
        .route("/misc", get(|_: RequireScope<{ scopes::MISC_READ }>| async { "ok" }))
        .route(
            "/public",
            get(|p: Public<{ scopes::BTT_READ }>| async move { if p.0.authenticated() { "tok" } else { "anon" } }),
        )
        .route("/portal", get(|_: CurrentUser| async { "ok" }))
        .route("/site", get(|_: CurrentSiteUser| async { "ok" }))
        .route("/v1/ingest/{*rest}", get(|_: MasterIngest| async { "ok" }))
        .route_layer(from_fn_with_state(state.clone(), usage::record))
        .with_state(state.clone());

    let (status, headers) = get_status(&app, "/scoped", &api_token).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers["x-ratelimit-limit"], "120");
    assert_eq!(headers["x-ratelimit-remaining"], "119");
    assert_eq!(get_status(&app, "/misc", &api_token).await.0, StatusCode::FORBIDDEN);
    assert_eq!(get_status(&app, "/public", &api_token).await.0, StatusCode::OK);
    assert_eq!(get_status(&app, "/v1/ingest/a/b", &api_token).await.0, StatusCode::OK);
    assert_eq!(get_status(&app, "/v1/ingest/a/b", &api_token).await.0, StatusCode::TOO_MANY_REQUESTS);

    let tok = db.collection::<Document>("api_tokens").find_one(doc! { "_id": token_id }).await.unwrap().unwrap();
    assert_eq!(tok.get_i32("request_count").unwrap(), 1, "first use in the interval flushes delta 1");
    assert!(tok.get_datetime("last_used_at").is_ok());

    usage::shutdown().await;
    let events: Vec<Document> = {
        use futures::TryStreamExt;
        db.collection::<Document>("usage_events")
            .find(doc! { "token_id": token_id })
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap()
    };
    let seen: Vec<(String, i32)> =
        events.iter().map(|e| (e.get_str("route").unwrap().to_owned(), e.get_i32("status_code").unwrap())).collect();
    assert!(seen.contains(&("/scoped".into(), 200)) && seen.contains(&("/misc".into(), 403)), "{seen:?}");
    assert!(seen.contains(&("/v1/ingest/{rest:path}".into(), 429)), "{seen:?}");
    let keys: Vec<&str> = events[0].keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        ["_id", "user_id", "token_id", "method", "route", "path", "status_code", "duration_ms", "created_at"]
    );

    let pair =
        sessions::issue_tokens(&state, Audience::Portal, user_id, 0, Some("Chrome on Windows".into())).await.unwrap();
    assert_eq!(get_status(&app, "/portal", &pair.access_token).await.0, StatusCode::OK);
    let rotated = sessions::rotate(&state, Audience::Portal, &pair.refresh_token, None).await.unwrap().unwrap();
    assert_eq!(
        sessions::rotate(&state, Audience::Portal, &pair.refresh_token, None).await.unwrap(),
        Err(RotateError::InvalidToken)
    );
    sessions::revoke_all_sessions(&state, Audience::Portal, user_id, 0).await.unwrap();
    assert_eq!(get_status(&app, "/portal", &rotated.access_token).await.0, StatusCode::UNAUTHORIZED);

    let site = sessions::issue_tokens(&state, Audience::Site, site_id, 0, None).await.unwrap();
    assert_eq!(get_status(&app, "/site", &site.access_token).await.0, StatusCode::OK);
    assert_eq!(get_status(&app, "/portal", &site.access_token).await.0, StatusCode::UNAUTHORIZED);

    let key = format!("test:{token_id}");
    for _ in 0..2 {
        ratelimit::check_rate_limit(&state, &key, 2, 60).await.unwrap();
    }
    let over = ratelimit::check_rate_limit(&state, &key, 2, 60).await.unwrap_err();
    assert_eq!(over.status(), StatusCode::TOO_MANY_REQUESTS);

    let email = format!("{user_id}@example.test");
    for _ in 0..5 {
        lockout::record_failure(&state, &email).await.unwrap();
    }
    assert!(lockout::lock_ttl(&state, &email).await.unwrap() > 0);
    lockout::clear(&state, &email).await.unwrap();
    assert_eq!(lockout::lock_ttl(&state, &email).await.unwrap(), 0);

    for (coll, field, id) in [
        ("users", "_id", user_id),
        ("site_users", "_id", site_id),
        ("api_tokens", "_id", token_id),
        ("usage_events", "token_id", token_id),
        ("sessions", "user_id", user_id),
        ("site_sessions", "site_user_id", site_id),
    ] {
        db.collection::<Document>(coll).delete_many(doc! { field: id }).await.unwrap();
    }
    let _: () = redis::cmd("DEL")
        .arg(format!("rl:{key}"))
        .arg(format!("rl:token:{token_id}"))
        .arg(format!("rl:ingest:/v1/ingest/{{rest:path}}:{token_id}"))
        .arg(format!("tok:pending:{token_id}"))
        .arg(format!("tok:gate:{token_id}"))
        .arg("rl:public:unknown")
        .query_async(&mut redis)
        .await
        .unwrap();
}
