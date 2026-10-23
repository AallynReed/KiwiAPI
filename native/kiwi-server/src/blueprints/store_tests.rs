//! Store-backed checks, opt-in because they write: point MONGO_URI / MONGO_DB,
//! REDIS_URL, MODS_STORE_DIR and TROVE_UPDATE_STORE_DIR at throwaway stores and run
//! `cargo test -p kiwi-server blueprints::store_tests -- --ignored --test-threads 1`.

use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue, Request, StatusCode},
};
use mongodb::bson::{Bson, Document, doc};
use serde_json::{Map, Value, json};
use tower::ServiceExt;

use super::cache::{self, BuildError, Fmt};
use crate::{config::Settings, error::ApiError, state::AppState};

async fn state() -> AppState {
    AppState::new(Settings::from_env().unwrap()).await.unwrap()
}

fn sample() -> Vec<u8> {
    std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/blueprints/bp/s_mixed.blueprint"))
        .unwrap()
}

fn payload() -> Map<String, Value> {
    match kiwi_core::voxel::pack(&sample(), "blueprints/s_mixed.blueprint", kiwi_core::voxel::RENDER_VOXEL_CAP).unwrap()
    {
        Value::Object(m) => m,
        _ => unreachable!(),
    }
}

fn keys(d: &Document) -> Vec<&str> {
    d.keys().map(String::as_str).collect()
}

#[tokio::test]
#[ignore = "writes to MONGO_URI and MODS_STORE_DIR"]
async fn bp_cache_rows_blobs_and_errors() {
    let state = state().await;
    let rows = state.mongo().collection::<Document>("blueprint_cache");
    let key = format!("v3:file:parity{}:x.blueprint", std::process::id());
    rows.delete_many(doc! {"key": {"$regex": format!("^{key}")}}).await.unwrap();

    let first = cache::get_or_build(&state, &key, Fmt::Bin, || async { Ok(payload()) }).await.unwrap();
    assert_eq!((first.hit, first.stored, first.state()), (false, true, "miss"));
    let row = rows.find_one(doc! {"key": format!("{key}:bin")}).await.unwrap().unwrap();
    assert_eq!(
        keys(&row),
        [
            "_id",
            "key",
            "blob_sha",
            "byte_len",
            "voxel_count",
            "size",
            "err_status",
            "err_code",
            "err_msg",
            "created_at"
        ]
    );
    assert_eq!(row.get("voxel_count"), Some(&Bson::Int32(first.count as i32)));
    assert_eq!(row.get("err_status"), Some(&Bson::Null));
    let raw = super::pack::gunzip(&first.gz).unwrap();
    assert_eq!(super::pack::kvx_decode(&raw).unwrap(), Value::Object(payload()));

    let second = cache::get_or_build(&state, &key, Fmt::Bin, || async { panic!("must not rebuild") }).await.unwrap();
    assert_eq!((second.hit, second.etag.as_str()), (true, first.etag.as_str()));

    let mut headers = HeaderMap::new();
    headers.insert("if-none-match", HeaderValue::from_str(&format!("W/\"x\", \"{}\"", first.etag)).unwrap());
    let r = cache::respond(&headers, &second, 300);
    assert_eq!(r.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(r.headers()["etag"], format!("\"{}\"", first.etag).as_str());
    headers.clear();
    headers.insert("accept-encoding", HeaderValue::from_static("br, GZip"));
    let r = cache::respond(&headers, &second, 300);
    assert_eq!(r.headers()["content-encoding"], "gzip");
    assert_eq!(r.headers()["content-type"], "application/x-kiwi-voxel");
    assert_eq!(r.headers()["x-kiwi-cache"], "hit");

    // A pruned blob drops the row and rebuilds.
    std::fs::remove_file(cache::blob_path(&state.settings().mods_store_dir, &first.etag)).unwrap();
    let third = cache::get_or_build(&state, &key, Fmt::Bin, || async { Ok(payload()) }).await.unwrap();
    assert!(!third.hit && third.stored);

    // 413/422 are remembered, a 404 is not.
    let bad = format!("{key}:bad");
    let err = || async {
        Err::<Map<String, Value>, _>(BuildError::Api(ApiError::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            crate::error::ErrorCode::BadRequest,
            "This blueprint is empty or could not be decoded.",
        )))
    };
    assert!(cache::get_or_build(&state, &bad, Fmt::Json, err).await.is_err());
    let row = rows.find_one(doc! {"key": &bad}).await.unwrap().unwrap();
    assert_eq!(row.get("err_status"), Some(&Bson::Int32(422)));
    assert_eq!(row.get_str("err_code").unwrap(), "bad_request");
    match cache::get_or_build(&state, &bad, Fmt::Json, || async { panic!("remembered") }).await {
        Err(BuildError::Api(e)) => assert_eq!((e.status.as_u16(), e.code), (422, "bad_request")),
        other => panic!("{other:?}"),
    }
    let missing = format!("{key}:404");
    let r = cache::get_or_build(&state, &missing, Fmt::Json, || async {
        Err(BuildError::Api(ApiError::not_found("Blob missing from the store")))
    })
    .await;
    assert!(r.is_err());
    assert!(rows.find_one(doc! {"key": &missing}).await.unwrap().is_none());
    rows.delete_many(doc! {"key": {"$regex": format!("^{key}")}}).await.unwrap();
}

#[tokio::test]
#[ignore = "writes to MONGO_URI, REDIS_URL and TROVE_UPDATE_STORE_DIR"]
async fn render_routes_cache_in_redis() {
    let state = state().await;
    let data = sample();
    let (sha, _) = cache::store_put(&state.settings().trove_update_store_dir, &data).unwrap();
    let name = format!("parity/p{}.blueprint", std::process::id());
    let coll = state.mongo().collection::<Document>("update_state");
    coll.delete_many(doc! {"branch": "pts", "path": format!("blueprints/{name}")}).await.unwrap();
    coll.insert_one(doc! {"branch": "pts", "path": format!("blueprints/{name}"), "content_sha256": &sha, "size": 1})
        .await
        .unwrap();
    let key = super::render::blueprint_key("pts", &name.to_uppercase(), 64);
    let mut con = state.redis();
    let _: () = redis::cmd("DEL").arg(&key).query_async(&mut con).await.unwrap();

    let app = crate::routes::app(state.clone());
    let uri = format!("/site/codexes/render?branch=pts&dim=64&blueprint={}", name.to_uppercase());
    let req = Request::get(&uri).header("x-forwarded-for", "10.9.8.7").body(Body::empty()).unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()["cache-control"], "public, max-age=86400");
    let png = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let cached: String = redis::cmd("GET").arg(&key).query_async(&mut con).await.unwrap();
    use base64::Engine;
    assert_eq!(base64::engine::general_purpose::STANDARD.decode(cached).unwrap(), png.to_vec());
    let ttl: i64 = redis::cmd("TTL").arg(&key).query_async(&mut con).await.unwrap();
    assert!(ttl > 0 && ttl <= state.settings().trove_render_cache_ttl);

    let req = Request::get("/site/updates/pts/file/blueprint?fmt=bin&path=blueprints/".to_owned() + &name)
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()["content-type"], "application/x-kiwi-voxel");

    let req = Request::get("/v1/codexes/render?branch=pts&blueprint=nope/never").body(Body::empty()).unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let body: Value =
        serde_json::from_slice(&axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body["error"]["message"], json!("No blueprint 'nope/never' on branch 'pts'"));
    coll.delete_many(doc! {"branch": "pts", "path": format!("blueprints/{name}")}).await.unwrap();
}
