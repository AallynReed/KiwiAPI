//! Write auth against Python (`testdata/mods_creators_issues/golden.json`):
//! the allowlist, the scope check, and every link decision `_authorize_api`
//! makes once the token is resolved, over an in-memory store.

use axum::http::{HeaderMap, HeaderName, HeaderValue};
use mongodb::bson::oid::ObjectId;
use serde_json::{Value, json};

use super::{
    API_ROUTES, CREATE_ROUTES, Target, authorize_link,
    project::{Project, effective_visibility},
    require_write_scope,
    testkit::{arr, check_after, golden, mem, s, store},
};

#[test]
fn allowlist_matches_python() {
    let ours: serde_json::Map<String, Value> =
        API_ROUTES.iter().map(|(r, m)| ((*r).to_owned(), json!(m))).collect::<serde_json::Map<_, _>>();
    let theirs = golden()["api_routes"].as_object().unwrap();
    assert_eq!(ours.len(), theirs.len());
    for (k, v) in theirs {
        assert_eq!(ours.get(k), Some(v), "{k}");
    }
    let mut create: Vec<&str> = CREATE_ROUTES.to_vec();
    create.sort();
    assert_eq!(json!(create), golden()["create_routes"]);
    for pair in arr(golden(), "creator_router_routes") {
        let (path, method) = (s(&pair[0]), s(&pair[1]));
        let allowed = API_ROUTES.iter().find(|(r, _)| *r == path).map(|(_, m)| *m).unwrap_or(&[]);
        assert!(allowed.contains(&method), "documented creator route {method} {path} is not allowed");
    }
}

#[test]
fn scope_check_matches_python() {
    let want = &golden()["scope_denied"];
    let token = crate::auth::models::ApiToken {
        id: ObjectId::new(),
        user_id: ObjectId::new(),
        name: String::new(),
        prefix: String::new(),
        hashed_token: String::new(),
        scopes: 1,
        ip_salt: None,
        allowed_ip_hashes: Vec::new(),
        revoked: false,
        revoked_at: None,
        revoke_reason: None,
        expires_at: None,
        expiry_warned: false,
        created_at: mongodb::bson::DateTime::now(),
        last_used_at: None,
        rotated_at: None,
        request_count: 0,
    };
    let err = require_write_scope(&token).unwrap_err();
    assert_eq!(u64::from(err.status().as_u16()), want["status"].as_u64().unwrap());
    assert_eq!(err.error.code, s(&want["code"]));
    assert_eq!(err.error.message, s(&want["message"]));
    let all = crate::auth::models::ApiToken { scopes: 0, ..token.clone() };
    assert!(require_write_scope(&all).is_ok());
    let write = crate::auth::models::ApiToken { scopes: crate::auth::scopes::MODS_WRITE, ..token };
    assert!(require_write_scope(&write).is_ok());
}

#[tokio::test]
async fn link_decisions_match_python() {
    let mut bad = Vec::new();
    for case in arr(golden(), "auth") {
        let name = s(&case["name"]);
        let m = mem(case);
        let mut headers = HeaderMap::new();
        for h in arr(case, "headers") {
            headers.append(
                HeaderName::from_bytes(s(&h[0]).as_bytes()).unwrap(),
                HeaderValue::from_bytes(s(&h[1]).as_bytes()).unwrap(),
            );
        }
        let params: Vec<(String, String)> =
            case["params"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), s(v).to_owned())).collect();
        let query = s(&case["query"]);
        let target = Target::new(
            s(&case["template"]).to_owned(),
            s(&case["method"]),
            params,
            (!query.is_empty()).then_some(query),
            &headers,
        );
        let api_user = ObjectId::parse_str(s(&case["api_user"])).unwrap();
        let got = authorize_link(&store(&m), api_user, &target).await;
        let want = &case["result"];
        match got {
            Ok((user, link)) => {
                if want.get("user").and_then(Value::as_str) != Some(user.id.to_hex().as_str())
                    || want.get("link").and_then(Value::as_str) != Some(link.id.to_hex().as_str())
                {
                    bad.push(format!("{name}: ok {} / {} want {want}", user.id, link.id));
                }
            }
            Err(e) => {
                if want.get("status").and_then(Value::as_u64) != Some(u64::from(e.status.as_u16()))
                    || want.get("code").and_then(Value::as_str) != Some(e.code)
                    || want.get("message").and_then(Value::as_str) != Some(e.message.as_str())
                {
                    bad.push(format!("{name}: {} {} {:?} want {want}", e.status, e.code, e.message));
                }
            }
        }
        bad.extend(check_after(case, &m));
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn remix_visibility_follows_the_week() {
    let now = super::testkit::now();
    let mut p = Project {
        id: ObjectId::new(),
        slug: "x".into(),
        title: "x".into(),
        owner_id: None,
        owner_handle: String::new(),
        collaborators: Vec::new(),
        issues_enabled: true,
        visibility: "public".into(),
        is_remix: true,
        remix_status: "pending".into(),
        remix_requested_at: Some(now - chrono::Duration::days(7)),
        taken_down: false,
    };
    assert_eq!(effective_visibility(&p, now), "public_website_only");
    p.remix_requested_at = Some(now - chrono::Duration::days(7) + chrono::Duration::microseconds(1));
    assert_eq!(effective_visibility(&p, now), "hidden");
    p.remix_status = "denied".into();
    assert_eq!(effective_visibility(&p, now), "hidden");
    p.remix_status = "approved".into();
    assert_eq!(effective_visibility(&p, now), "public_website_only");
    p.visibility = "unlisted".into();
    assert_eq!(effective_visibility(&p, now), "unlisted");
}

/// Against a real Mongo (`MONGO_URI`): the store's queries and updates do
/// what the in-memory copy does.
#[tokio::test]
#[ignore = "needs MONGO_URI"]
async fn live_store_round_trip() {
    use mongodb::bson::doc;

    use super::store::Store;
    let settings =
        crate::config::Settings::from_vars([("MONGO_URI", std::env::var("MONGO_URI").unwrap().as_str())]).unwrap();
    let client = mongodb::Client::with_uri_str(std::env::var("MONGO_URI").unwrap()).await.unwrap();
    let db = client.database("kiwi_test_mods_social");
    let state = crate::state::AppState {
        inner: std::sync::Arc::new(crate::state::Inner {
            settings,
            db: crate::db::Db { mongo: Some(db.clone()), redis: None, pg: None },
        }),
    };
    let st = Store::live(&state);
    let id = st.insert("t", doc! {"a": 1, "tags": ["x"]}).await.unwrap();
    st.update_one(
        "t",
        doc! {"_id": id},
        doc! {"$inc": {"a": 1_i32}, "$addToSet": {"tags": "y"}, "$set": {"z": 1, "b": 2}},
    )
    .await
    .unwrap();
    let after = st.get("t", id).await.unwrap().unwrap();
    let m = super::mem::MemDb::new(Default::default(), super::testkit::now(), "");
    m.colls.lock().unwrap().insert("t".into(), vec![doc! {"_id": id, "a": 1, "tags": ["x"]}]);
    m.update(
        "t",
        &doc! {"_id": id},
        &doc! {"$inc": {"a": 1_i32}, "$addToSet": {"tags": "y"}, "$set": {"z": 1, "b": 2}},
        false,
    );
    assert_eq!(after, m.snapshot("t")[0]);
    db.drop().await.unwrap();
}
