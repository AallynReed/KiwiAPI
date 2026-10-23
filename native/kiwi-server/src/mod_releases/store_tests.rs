//! Store-backed checks, opt-in because they write: point MONGO_URI / MONGO_DB,
//! REDIS_URL and MODS_STORE_DIR at throwaway stores and run
//! `cargo test -p kiwi-server mod_releases::store_tests -- --ignored --test-threads 1`.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use mongodb::bson::{Document, doc, oid::ObjectId};
use tower::ServiceExt;

use super::{
    access,
    model::{Doc, PROJECT, RELEASE, V},
};
use crate::{config::Settings, state::AppState};

async fn state() -> AppState {
    AppState::new(Settings::from_env().unwrap()).await.unwrap()
}

fn keys(d: &Document) -> Vec<&str> {
    d.keys().map(String::as_str).collect()
}

async fn seed(state: &AppState, visibility: &str, status: &str, artifact: &[u8]) -> (Doc, Doc) {
    let store = crate::media::mods_store(state);
    let (sha, _) = store.put(artifact).unwrap();
    let project = Doc::build(
        PROJECT,
        vec![
            ("slug", V::Str(format!("parity-{}", ObjectId::new().to_hex()))),
            ("title", V::Str("Parity".into())),
            ("owner_id", V::Oid(ObjectId::new())),
            ("visibility", V::Str(visibility.into())),
        ],
    );
    let release = Doc::build(
        RELEASE,
        vec![
            ("project_id", V::Oid(project.id)),
            ("tag", V::Str("v1".into())),
            ("tmod_sha", V::Str(sha)),
            ("tmod_size", V::Int(artifact.len() as i64)),
            ("tmod_properties", V::Map(vec![("title".into(), "Parity Mod".into())])),
            ("status", V::Str(status.into())),
        ],
    );
    let db = state.mongo();
    db.collection::<Document>(access::PROJECTS).insert_one(project.insert_doc()).await.unwrap();
    db.collection::<Document>(access::RELEASES).insert_one(release.insert_doc()).await.unwrap();
    (project, release)
}

fn tmod() -> Vec<u8> {
    let props = vec![("title".to_owned(), "Parity Mod".to_owned())];
    let files = vec![("ui/a.cfg".to_owned(), b"x=1".to_vec()), ("ui/a.swf".to_owned(), b"FWS".to_vec())];
    crate::tools::tmod::build(1, &props, &files, "KiwiAPI", true).unwrap()
}

#[tokio::test]
#[ignore = "writes to MONGO_URI and MODS_STORE_DIR"]
async fn visibility_and_downloads() {
    let state = state().await;
    let (project, release) = seed(&state, "public", "published", &tmod()).await;
    let (r, p) = access::release_with_project(&state, &release.id.to_hex(), None).await.unwrap();
    assert_eq!((r.id, p.id), (release.id, project.id));
    assert_eq!(access::release_with_project(&state, "nope", None).await.unwrap_err().status, StatusCode::NOT_FOUND);
    let (_, draft) = seed(&state, "public", "draft", &tmod()).await;
    assert!(access::release_with_project(&state, &draft.id.to_hex(), None).await.is_err());
    let (_, hidden) = seed(&state, "draft", "published", &tmod()).await;
    assert!(access::release_with_project(&state, &hidden.id.to_hex(), None).await.is_err());

    super::read::record_download(&state, &release, &project).await;
    let db = state.mongo();
    let r = db.collection::<Document>(access::RELEASES).find_one(doc! {"_id": release.id}).await.unwrap().unwrap();
    assert_eq!(r.get_i32("download_count").unwrap(), 1);
    let ev = db
        .collection::<Document>("mod_download_events")
        .find_one(doc! {"release_id": release.id})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(keys(&ev), ["_id", "project_id", "release_id", "created_at"]);
}

#[tokio::test]
#[ignore = "writes to MONGO_URI and MODS_STORE_DIR"]
async fn routes_answer_from_the_store() {
    let state = state().await;
    let (_, release) = seed(&state, "public", "published", &tmod()).await;
    let app = crate::routes::app(state.clone());
    let id = release.id.to_hex();
    for (path, status) in [
        (format!("/site/mods/releases/{id}/files"), 200),
        (format!("/site/mods/releases/{id}/cfgs"), 200),
        (format!("/site/mods/releases/{id}/cfg?path=ui/a.cfg"), 200),
        (format!("/site/mods/releases/{id}/file?path=ui/missing"), 404),
        (format!("/site/mods/releases/{id}/download"), 200),
        ("/site/mods/releases/000000000000000000000000/inspect".to_owned(), 404),
    ] {
        let req = Request::get(&path).header("x-forwarded-for", "203.0.113.9").body(Body::empty()).unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status().as_u16(), status, "{path}");
    }
}

#[tokio::test]
#[ignore = "writes to MONGO_URI and MODS_STORE_DIR"]
async fn save_sets_every_model_field() {
    let state = state().await;
    let db = state.mongo();
    let id = ObjectId::new();
    db.collection::<Document>(access::RELEASES)
        .insert_one(doc! {"_id": id, "project_id": ObjectId::new(), "tag": "v1", "tmod_sha": "aa", "extra": 1})
        .await
        .unwrap();
    let mut r = access::release_by(&state, doc! {"_id": id}).await.unwrap().unwrap();
    r.set("title", V::Str("T".into()));
    super::event::save(&state, access::RELEASES, &r).await.unwrap();
    let back = db.collection::<Document>(access::RELEASES).find_one(doc! {"_id": id}).await.unwrap().unwrap();
    assert_eq!(back.get_str("title").unwrap(), "T");
    assert_eq!(back.get_i32("extra").unwrap(), 1);
    assert_eq!(back.get_str("release_format").unwrap(), "tmod");
}
