//! Store tests against a throwaway Mongo (`MONGO_URI`); they create and drop
//! a scratch database and a scratch mods store directory.

use std::sync::Arc;

use mongodb::bson::{doc, oid::ObjectId};

use super::{bodies::CreateProject, model, service, writes};
use crate::{
    auth::models::SiteUser,
    config::Settings,
    db::Db,
    state::{AppState, Inner},
};

async fn scratch() -> (AppState, mongodb::Database, std::path::PathBuf) {
    let uri = std::env::var("MONGO_URI").expect("MONGO_URI");
    let client = mongodb::Client::with_uri_str(&uri).await.unwrap();
    let db = client.database(&format!("kiwi_mods_test_{}", ObjectId::new().to_hex()));
    let dir = std::env::temp_dir().join(format!("kiwi-mods-store-{}", ObjectId::new().to_hex()));
    let vars = vec![("MODS_STORE_DIR".to_owned(), dir.to_string_lossy().into_owned())];
    let settings = Settings::from_vars(vars).unwrap();
    let state =
        AppState { inner: Arc::new(Inner { settings, db: Db { mongo: Some(db.clone()), redis: None, pg: None } }) };
    (state, db, dir)
}

fn user(name: &str) -> SiteUser {
    mongodb::bson::from_document(doc! {"_id": ObjectId::new(), "username": name, "display_name": format!("{name}!")})
        .unwrap()
}

#[tokio::test]
#[ignore = "needs MONGO_URI pointing at a throwaway mongo"]
async fn create_fork_star_delete() {
    let (state, db, dir) = scratch().await;
    let owner = user("owner");
    let other = user("other");
    for u in [&owner, &other] {
        db.collection::<SiteUser>("site_users").insert_one(u).await.unwrap();
    }
    let req = || CreateProject {
        title: " My Mod ".into(),
        visibility: "public".into(),
        mode: "files".into(),
        source_visibility: "public".into(),
        tags: vec!["Dragons".into(), "dragons".into()],
        ..Default::default()
    };
    let a = writes::create_project(&state, &owner, req()).await.unwrap();
    let b = writes::create_project(&state, &owner, req()).await.unwrap();
    assert_eq!((a.slug.as_str(), b.slug.as_str()), ("my-mod", "my-mod-2"));
    assert_eq!(a.tags, ["dragons"]);
    assert!(super::git::repo_dir(&state, &a.id.to_hex()).join("HEAD").is_file());
    let detail = service::project_detail(&state, &a, Some(&owner)).await.unwrap();
    assert_eq!(detail["branches"], serde_json::json!([{"name": "main", "head_commit_id": null}]));
    assert_eq!(detail["commit_count"], 0);

    let mut original = service::get_for_view(&state, "owner", "my-mod", Some(&other)).await.unwrap();
    let fork = writes::fork_project(&state, &other, &mut original).await.unwrap();
    assert!(fork.is_remix && fork.forked_from_id == Some(a.id));
    assert_eq!(original.fork_count, 1);

    let mut p = service::get_for_view(&state, "owner", "my-mod", Some(&other)).await.unwrap();
    assert_eq!(writes::star_project(&state, &other, &mut p).await.unwrap()["star_count"], 1);
    assert_eq!(writes::star_project(&state, &other, &mut p).await.unwrap()["star_count"], 1);
    assert_eq!(service::list_starred(&state, &other).await.unwrap().len(), 1);
    assert_eq!(writes::unstar_project(&state, &other, &mut p).await.unwrap()["star_count"], 0);

    let (items, total) = service::list_public(
        &state,
        &service::ListArgs {
            q: Some("my".into()),
            tag: None,
            author: None,
            sort: "popular".into(),
            limit: 30,
            offset: 0,
        },
    )
    .await
    .unwrap();
    assert_eq!((items.len(), total), (2, 2));
    writes::delete_project(&state, &p, &owner).await.unwrap();
    assert!(service::get_project(&state, "owner", "my-mod").await.unwrap().is_none());
    assert_eq!(db.collection::<mongodb::bson::Document>(model::STARS).count_documents(doc! {}).await.unwrap(), 0);
    db.drop().await.unwrap();
    let _ = std::fs::remove_dir_all(dir);
}
