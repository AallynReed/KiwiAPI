//! Live-store checks, opt-in because they write: point MONGO_URI / MONGO_DB at
//! a throwaway mongo:7 and run
//! `cargo test -p kiwi-server mods_moderation::store_tests -- --ignored --test-threads=1`.

use futures::TryStreamExt;
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};

use super::{service as svc, strayimport};
use crate::{config::Settings, state::AppState};

async fn state() -> AppState {
    AppState::new(Settings::from_env().unwrap()).await.unwrap()
}

async fn all(state: &AppState, name: &str, filter: Document) -> Vec<Document> {
    svc::coll(state, name).find(filter).await.unwrap().try_collect().await.unwrap()
}

fn now() -> mongodb::bson::DateTime {
    mongodb::bson::DateTime::now()
}

#[tokio::test]
#[ignore = "writes to MONGO_URI"]
async fn takedown_restore_and_purge() {
    let st = state().await;
    let (pid, rid, rep) = (ObjectId::new(), ObjectId::new(), ObjectId::new());
    let sha = format!("{:064x}", u128::from_be_bytes(*b"store-tests-sha1"));
    svc::coll(&st, svc::MOD_PROJECTS)
        .insert_one(doc! {"_id": pid, "slug": "store-test", "title": "Store Test", "owner_id": Bson::Null,
        "owner_handle": "stray", "visibility": "public", "created_at": now(), "updated_at": now()})
        .await
        .unwrap();
    svc::coll(&st, svc::MOD_RELEASES)
        .insert_one(doc! {"_id": rid, "project_id": pid, "tag": "v1", "tmod_sha": &sha, "prior_tmod_shas": [""]})
        .await
        .unwrap();
    svc::coll(&st, svc::REPORTS)
        .insert_one(doc! {"_id": rep, "target_type": "mod", "target_id": pid, "reason": "x", "resolved": false,
        "created_at": now()})
        .await
        .unwrap();

    let reply = svc::take_down_project(&st, &pid.to_hex(), "  why  ").await.unwrap();
    assert_eq!(reply["takedown_reason"], "why");
    let stored = &all(&st, svc::MOD_PROJECTS, doc! {"_id": pid}).await[0];
    assert_eq!(stored.get_str("takedown_reason").unwrap(), "why");
    assert!(stored.contains_key("popularity_score"), "save() fills every model field");
    let art = &all(&st, svc::TAKEDOWNS, doc! {"sha256": &sha}).await[0];
    let mut keys: Vec<&String> = art.keys().collect();
    keys.sort();
    assert_eq!(keys, ["_id", "created_at", "md5", "owner_handle", "project_id", "sha256", "title"]);
    assert_eq!(art.get_str("md5").unwrap(), "");
    assert!(all(&st, svc::REPORTS, doc! {"_id": rep}).await[0].get_bool("resolved").unwrap());

    svc::restore_project(&st, &pid.to_hex()).await.unwrap();
    assert!(all(&st, svc::TAKEDOWNS, doc! {"project_id": pid}).await.is_empty());

    svc::take_down_project(&st, &pid.to_hex(), "").await.unwrap();
    svc::master_delete_project(&st, &pid.to_hex()).await.unwrap();
    assert!(all(&st, svc::MOD_PROJECTS, doc! {"_id": pid}).await.is_empty());
    assert!(all(&st, svc::MOD_RELEASES, doc! {"project_id": pid}).await.is_empty());
    assert!(all(&st, svc::REPORTS, doc! {"_id": rep}).await.is_empty());
    assert_eq!(all(&st, svc::TAKEDOWNS, doc! {"sha256": &sha}).await.len(), 1);
    svc::coll(&st, svc::TAKEDOWNS).delete_many(doc! {"sha256": &sha}).await.unwrap();
}

#[tokio::test]
#[ignore = "writes to MONGO_URI"]
async fn import_state_singleton() {
    let st = state().await;
    let first = strayimport::load_state(&st).await.unwrap();
    let again = strayimport::load_state(&st).await.unwrap();
    assert_eq!(first.id, again.id);
    let rows = all(&st, "stray_import_state", doc! {"key": strayimport::SOURCE}).await;
    assert_eq!(rows.len(), 1);
}
