//! Store-backed flow, opt-in because it writes: point MONGO_URI / MONGO_DB and
//! MODS_STORE_DIR at throwaway stores and run
//! `cargo test -p kiwi-server modpacks::store_tests -- --ignored --test-threads 1`.

use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode},
};
use mongodb::bson::{Document, doc, oid::ObjectId};
use serde_json::Value;
use tower::ServiceExt;

use super::model::{PACKS, STARS};
use crate::{
    auth::jwt,
    config::Settings,
    middleware::{self, EmbedSource, Mw, MwConfig, forwarded::TrustedHosts},
    state::AppState,
    tools::tmod,
};

const BOUNDARY: &str = "storeTestBoundary";

async fn state() -> AppState {
    AppState::new(Settings::from_env().unwrap()).await.unwrap()
}

fn app(state: AppState) -> Router {
    let mw =
        Mw::new(MwConfig::from_lookup(&|_| None), None, EmbedSource::Fixed(vec![]), TrustedHosts::parse("127.0.0.1"));
    let inner = middleware::wrap(super::router(), Arc::new(mw), None)
        .fallback(|| async { StatusCode::IM_A_TEAPOT })
        .with_state(state.clone());
    middleware::outer(inner)
}

fn token(state: &AppState, id: ObjectId) -> String {
    let now = chrono::Utc::now();
    let t = jwt::create_site_access_token_at(
        &state.settings().secret_key,
        &id.to_hex(),
        0,
        None,
        chrono::Duration::minutes(15),
        now,
    );
    format!("Bearer {t}")
}

fn build(title: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
    let props = vec![("title".to_owned(), title.to_owned()), ("author".to_owned(), "Store".to_owned())];
    let files: Vec<(String, Vec<u8>)> = files.iter().map(|(p, d)| ((*p).to_owned(), d.to_vec())).collect();
    tmod::build(1, &props, &files, tmod::KIWI_MOD_LOADER, false).unwrap()
}

fn multipart(data: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"m.tmod\"\r\n\
         Content-Type: application/octet-stream\r\n\r\n"
    )
    .into_bytes();
    out.extend_from_slice(data);
    out.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    out
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    auth: Option<&str>,
    body: Option<(&str, Vec<u8>)>,
) -> (u16, Vec<u8>) {
    let mut req = Request::builder().method(method).uri(uri);
    if let Some(a) = auth {
        req = req.header("authorization", a);
    }
    let req = match body {
        Some((ct, b)) => req.header("content-type", ct).body(Body::from(b)),
        None => req.body(Body::empty()),
    }
    .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status().as_u16();
    (status, axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap().to_vec())
}

fn json(b: &[u8]) -> Value {
    serde_json::from_slice(b).unwrap_or_else(|_| panic!("json: {}", String::from_utf8_lossy(b)))
}

fn site_user(id: ObjectId, username: &str) -> Document {
    let now = mongodb::bson::DateTime::now();
    doc! {
        "_id": id, "username": username, "discord_handle": username, "notify_email": null, "display_name": null,
        "is_active": true, "is_verified": true, "is_deleted": false, "discord_id": null, "discord_avatar": null,
        "token_version": 0, "created_at": now, "updated_at": now,
    }
}

#[tokio::test]
#[ignore = "writes to MONGO_URI and MODS_STORE_DIR"]
async fn pack_lifecycle() {
    let state = state().await;
    let db = state.mongo();
    let app = app(state.clone());
    let tag = ObjectId::new().to_hex();
    let (owner, other, modder) = (ObjectId::new(), ObjectId::new(), ObjectId::new());
    let (oname, xname, mname) = (format!("own{tag}"), format!("oth{tag}"), format!("mod{tag}"));
    let users = db.collection::<Document>("site_users");
    users.insert_many([site_user(owner, &oname), site_user(other, &xname), site_user(modder, &mname)]).await.unwrap();

    let hub_build = build(&format!("Hub {tag}"), &[("blocks/x.blueprint", b"\x01")]);
    let hub_sha = {
        use sha2::Digest;
        hex::encode(sha2::Sha256::digest(&hub_build))
    };
    let mod_id = ObjectId::new();
    db.collection::<Document>("mod_projects")
        .insert_one(doc! {"_id": mod_id, "slug": "hub", "title": "Hub", "owner_id": modder, "owner_username": "M",
        "owner_handle": &mname, "visibility": "public"})
        .await
        .unwrap();
    db.collection::<Document>("mod_releases")
        .insert_one(doc! {"_id": ObjectId::new(), "project_id": mod_id, "tag": "v1", "branch": "main",
        "tmod_sha": &hub_sha, "tmod_properties": {"title": format!("Hub {tag}")},
        "status": "published", "release_format": "tmod",
        "published_at": mongodb::bson::DateTime::now()})
        .await
        .unwrap();

    let auth = token(&state, owner);
    let (st, body) = call(
        &app,
        "POST",
        "/v1/modpacks/hub/projects",
        Some(&auth),
        Some(("application/json", br#"{"title":" Store Pack ","visibility":"public"}"#.to_vec())),
    )
    .await;
    assert_eq!(st, 201, "{}", String::from_utf8_lossy(&body));
    assert_eq!(json(&body)["slug"], "store-pack");
    let stored = db.collection::<Document>(PACKS).find_one(doc! {"owner_id": owner}).await.unwrap().unwrap();
    let keys: Vec<&str> = stored.keys().map(String::as_str).collect();
    assert_eq!(keys[..4], ["_id", "slug", "title", "summary"]);
    assert_eq!(keys.last(), Some(&"updated_at"));

    let base = format!("/v1/modpacks/hub/projects/{oname}/store-pack");
    let ct = format!("multipart/form-data; boundary={BOUNDARY}");
    let custom = build(&format!("Custom {tag}"), &[("ui/a.swf", b"FWS\x0a"), ("ModCfgs/c.cfg", b"[a.swf]\n")]);
    let (st, body) =
        call(&app, "POST", &format!("{base}/variants/default/upload"), Some(&auth), Some((&ct, multipart(&custom))))
            .await;
    assert_eq!(st, 201, "{}", String::from_utf8_lossy(&body));
    assert_eq!(json(&body)["matched_existing"], false);
    let (st, body) =
        call(&app, "POST", &format!("{base}/variants/default/upload"), Some(&auth), Some((&ct, multipart(&hub_build))))
            .await;
    assert_eq!(st, 201);
    let v = json(&body);
    assert_eq!((v["matched_existing"].clone(), v["variants"][0]["available_count"].clone()), (true.into(), 2.into()));

    let (st, blob) = call(&app, "GET", &format!("/v1/modpacks/{oname}/store-pack/download"), None, None).await;
    assert_eq!(st, 200);
    let read = tmod::read(&blob, true).unwrap();
    let paths: Vec<&str> = read["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap()).collect();
    assert_eq!(paths, [format!("Custom {tag}.tmod"), format!("Hub {tag}.tmod"), format!("ModCfgs/Custom {tag}.cfg")]);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let stored = db.collection::<Document>(PACKS).find_one(doc! {"owner_id": owner}).await.unwrap().unwrap();
    assert_eq!(stored.get_i32("download_count").unwrap(), 1);

    let xauth = token(&state, other);
    let (st, body) = call(&app, "POST", &format!("{base}/star"), Some(&xauth), None).await;
    assert_eq!((st, json(&body)), (200, serde_json::json!({"starred": true, "star_count": 1})));
    let (st, body) = call(&app, "DELETE", &format!("{base}/star"), Some(&xauth), None).await;
    assert_eq!((st, json(&body)), (200, serde_json::json!({"starred": false, "star_count": 0})));
    let (st, _) = call(&app, "DELETE", &base, Some(&xauth), None).await;
    assert_eq!(st, 404);
    call(&app, "POST", &format!("{base}/star"), Some(&xauth), None).await;
    let (st, body) = call(&app, "DELETE", &base, Some(&auth), None).await;
    assert_eq!((st, body.len()), (204, 0));
    let pid = stored.get_object_id("_id").unwrap();
    assert_eq!(db.collection::<Document>(STARS).count_documents(doc! {"modpack_id": pid}).await.unwrap(), 0);

    users.delete_many(doc! {"_id": {"$in": [owner, other, modder]}}).await.unwrap();
    db.collection::<Document>("mod_projects").delete_one(doc! {"_id": mod_id}).await.unwrap();
    db.collection::<Document>("mod_releases").delete_many(doc! {"project_id": mod_id}).await.unwrap();
}
