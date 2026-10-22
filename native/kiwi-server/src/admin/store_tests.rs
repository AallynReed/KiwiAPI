//! Live-store checks, opt-in because they write: point MONGO_URI / MONGO_DB
//! at a throwaway mongo:7 and run
//! `cargo test -p kiwi-server admin::store_tests -- --ignored --test-threads=1`.

use axum::extract::State;
use futures::TryStreamExt;
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use serde_json::{Value, json};

use super::{
    Args, Seg, coll, config, docs, pageviews, pending, site_users,
    users::{self, UserPath},
};
use crate::{
    auth::{
        Superuser,
        models::{API_TOKENS, SESSIONS, SITE_USERS, USERS, User},
    },
    config::Settings,
    runtime_config::COLLECTION,
    state::AppState,
    validation::DepsThenBody,
};

async fn state() -> AppState {
    if std::env::var("PY_UPSTREAM").is_err() {
        // SAFETY: only these ignored tests run in their process invocation.
        unsafe { std::env::set_var("PY_UPSTREAM", "http://127.0.0.1:9") };
    }
    AppState::new(Settings::from_env().unwrap()).await.unwrap()
}

async fn all(state: &AppState, name: &str, filter: Document) -> Vec<Document> {
    coll(state, name).find(filter).await.unwrap().try_collect().await.unwrap()
}

fn admin() -> User {
    mongodb::bson::from_document(doc! {"_id": ObjectId::new(), "email": "master@aallyn.net", "is_superuser": true})
        .unwrap()
}

#[tokio::test]
#[ignore = "writes to MONGO_URI"]
async fn config_override_round_trip() {
    let state = state().await;
    let key = "embed_rate_limit_max";
    coll(&state, COLLECTION).delete_many(doc! {"key": key}).await.unwrap();
    let a = admin();
    let body = config::UpdateBody { value: json!(77) };
    let put =
        config::update_runtime_config(State(state.clone()), Seg(key.into()), DepsThenBody(Superuser(a.clone()), body))
            .await
            .unwrap();
    assert_eq!(put.0["value"], json!(77));
    assert_eq!(put.0["is_default"], json!(false));
    let stored = all(&state, COLLECTION, doc! {"key": key}).await;
    let keys: Vec<&String> = stored[0].keys().collect();
    assert_eq!(keys, ["_id", "key", "value", "updated_at", "updated_by_user_id"]);
    assert_eq!(stored[0].get_str("value").unwrap(), "77");
    assert_eq!(crate::runtime_config::get_setting(&state, key).await.unwrap(), json!(77));
    let reset = config::reset_runtime_config(State(state.clone()), Superuser(a), Seg(key.into())).await.unwrap();
    assert_eq!(reset.0["is_default"], json!(true));
    assert!(all(&state, COLLECTION, doc! {"key": key}).await.is_empty());
}

#[tokio::test]
#[ignore = "writes to MONGO_URI"]
async fn terminate_revokes_everything() {
    let state = state().await;
    let uid = ObjectId::new();
    let now = docs::now();
    coll(&state, USERS)
        .insert_one(
            doc! {"_id": uid, "email": "gone@kiwi-mail.net", "hashed_password": "x", "display_name": Bson::Null,
            "is_active": true, "is_verified": true, "is_superuser": false, "roles": [], "token_version": 3,
            "github_id": Bson::Null, "email_bounced": false, "created_at": now, "updated_at": now,
            "last_login_at": Bson::Null},
        )
        .await
        .unwrap();
    for (i, revoked) in [false, true, false].into_iter().enumerate() {
        coll(&state, API_TOKENS)
            .insert_one(doc! {"_id": ObjectId::new(), "user_id": uid, "name": format!("t{i}"), "prefix": "kiwi_x",
            "hashed_token": format!("h{uid}{i}"), "scopes": 0, "revoked": revoked, "created_at": now,
            "request_count": 0})
            .await
            .unwrap();
    }
    coll(&state, SESSIONS)
        .insert_one(doc! {"_id": ObjectId::new(), "user_id": uid, "refresh_token_hash": format!("r{uid}"),
        "created_at": now, "last_used_at": now, "expires_at": now, "revoked": false})
        .await
        .unwrap();
    let out = users::terminate_user(State(state.clone()), Superuser(admin()), Args(UserPath(uid))).await.unwrap();
    assert_eq!(out.0, json!({"id": uid.to_hex(), "is_active": false, "revoked": 2}));
    let user = &all(&state, USERS, doc! {"_id": uid}).await[0];
    assert_eq!(user.get("token_version"), Some(&Bson::Int32(4)));
    assert!(!user.get_bool("is_active").unwrap());
    assert!(all(&state, API_TOKENS, doc! {"user_id": uid}).await.iter().all(|t| t.get_bool("revoked").unwrap()));
    assert!(all(&state, SESSIONS, doc! {"user_id": uid}).await.iter().all(|t| t.get_bool("revoked").unwrap()));
}

#[tokio::test]
#[ignore = "writes to MONGO_URI"]
async fn username_request_approval() {
    let state = state().await;
    let (sid, rid) = (ObjectId::new(), ObjectId::new());
    let now = docs::now();
    let suffix = &sid.to_hex()[16..];
    let old = format!("old{suffix}");
    let new = format!("new{suffix}");
    coll(&state, SITE_USERS)
        .insert_one(doc! {"_id": sid, "username": &old, "discord_handle": "Old", "token_version": 0,
        "claim_baseline": {"7": 3}, "created_at": now, "updated_at": now})
        .await
        .unwrap();
    coll(&state, docs::USERNAME_REQUESTS)
        .insert_one(doc! {"_id": rid, "site_user_id": sid, "current_username": &old, "requested_username": &new,
        "status": "pending", "reason": "", "resolved_by": Bson::Null, "resolved_at": Bson::Null, "created_at": now})
        .await
        .unwrap();
    coll(&state, docs::MOD_PROJECTS).insert_one(doc! {"owner_id": sid, "owner_handle": &old}).await.unwrap();
    let a = admin();
    let out = site_users::approve_username_request(State(state.clone()), Superuser(a.clone()), Seg(rid.to_hex()))
        .await
        .unwrap();
    assert_eq!(out.0["new_username"], json!(new));
    assert_eq!(out.0["status"], json!("approved"));
    let user = &all(&state, SITE_USERS, doc! {"_id": sid}).await[0];
    assert_eq!(user.get_str("username").unwrap(), new);
    assert_eq!(user.get_document("claim_baseline").unwrap(), &doc! {"7": 3.0});
    assert_eq!(user.keys().count(), 24);
    let mods = all(&state, docs::MOD_PROJECTS, doc! {"owner_id": sid}).await;
    assert_eq!(mods[0].get_str("owner_handle").unwrap(), new);
    let again = site_users::approve_username_request(State(state.clone()), Superuser(a), Seg(rid.to_hex())).await;
    assert_eq!(again.unwrap_err().message, "This request is already resolved.");
}

#[tokio::test]
#[ignore = "reads MONGO_URI (needs mongo 7 for $percentile/$median)"]
async fn pageview_pipelines_run() {
    let state = state().await;
    let now = crate::dt::now();
    let v = coll(&state, pageviews::PAGE_VIEWS);
    v.insert_one(doc! {"route": "/", "path": "/", "visitor_hash": "h1", "pid": ObjectId::new().to_hex(),
    "engaged_s": 12, "channel": "search", "device": "mobile", "perf": {"lcp": 1500, "ttfb": 200},
    "created_at": crate::dt::to_bson(now), "last_at": crate::dt::to_bson(now)})
        .await
        .unwrap();
    let (w, prev_labels, current, _) = pageviews::report_pipelines(7, now, None, &[]);
    let facet: Vec<Document> = v.aggregate(current).await.unwrap().try_collect().await.unwrap();
    let body = pageviews::build_report(7, &w, &prev_labels, None, &[], &facet[0], None).unwrap();
    assert!(body["totals"]["visits"].as_i64().unwrap() >= 1);
    let live: Vec<Document> = v.aggregate(pageviews::live_pipeline(now)).await.unwrap().try_collect().await.unwrap();
    let live = pageviews::build_live(&live[0]).unwrap();
    assert!(live["visitors"].as_i64().unwrap() >= 1);
}

#[tokio::test]
#[ignore = "reads MONGO_URI"]
async fn pending_counts_shape() {
    let state = state().await;
    let out = pending::pending_counts(State(state), Superuser(admin())).await.unwrap();
    assert!(matches!(out.0, Value::Object(ref m) if m.contains_key("counts") && m.contains_key("detail")));
}
