//! The golden HTTP cases against a real Mongo, opt-in: it creates and drops
//! its own `kiwi_store_test_*` database on MONGO_URI. Run
//! `cargo test -p kiwi-server store_mongo_replay -- --ignored`.

use std::panic::AssertUnwindSafe;

use axum::response::IntoResponse;
use futures::FutureExt;
use mongodb::{
    IndexModel,
    bson::{Bson, Document, doc, oid::ObjectId},
    options::IndexOptions,
};
use serde_json::Value;

use super::{
    service::{self, Mongo},
    tests::{arr, dispatch, golden, header, parts, rid, s, to_doc},
};
use crate::{error::ApiError, request_id};

async fn seed(db: &mongodb::Database, dataset: &Value) {
    db.drop().await.unwrap();
    let unique =
        |keys: Document| IndexModel::builder().keys(keys).options(IndexOptions::builder().unique(true).build()).build();
    let products = db.collection::<Document>(service::PRODUCTS);
    products.create_index(unique(doc! {"code": 1})).await.unwrap();
    products.create_index(IndexModel::builder().keys(doc! {"last_seen": -1}).build()).await.unwrap();
    products.create_index(IndexModel::builder().keys(doc! {"categories": 1}).build()).await.unwrap();
    db.collection::<Document>(service::CATEGORIES).create_index(unique(doc! {"index": 1})).await.unwrap();
    db.collection::<Document>(service::UPDATE_STATE).create_index(unique(doc! {"branch": 1, "path": 1})).await.unwrap();
    for (coll, docs) in dataset.as_object().unwrap() {
        let docs: Vec<Document> = docs.as_array().unwrap().iter().map(to_doc).collect();
        if !docs.is_empty() {
            db.collection::<Document>(coll).insert_many(docs).await.unwrap();
        }
    }
}

/// Timeline ties on `last_seen` have no defined order in Mongo.
fn normalize(name: &str, body: &[u8]) -> Vec<u8> {
    if !name.contains("timeline") {
        return body.to_vec();
    }
    let Ok(mut v) = serde_json::from_slice::<Value>(body) else { return body.to_vec() };
    if let Some(items) = v.get_mut("items").and_then(Value::as_array_mut) {
        items.sort_by_key(|i| (-i["last_seen"].as_i64().unwrap_or(0), s(&i["code"]).to_owned()));
    }
    serde_json::to_vec(&v).unwrap()
}

#[tokio::test]
#[ignore = "creates and drops a database on MONGO_URI"]
async fn store_mongo_replay() {
    let uri = std::env::var("MONGO_URI").expect("MONGO_URI");
    let client = mongodb::Client::with_uri_str(&uri).await.unwrap();
    let db = client.database(&format!("kiwi_store_test_{}", ObjectId::new().to_hex()));
    let blobs = std::env::temp_dir().join(format!("kiwi_store_blobs_{}", ObjectId::new().to_hex()));
    for (sha, data) in golden()["blobs"].as_object().unwrap() {
        use base64::Engine;
        let dir = blobs.join("objects").join(&sha[..2]);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(sha), base64::engine::general_purpose::STANDARD.decode(s(data)).unwrap()).unwrap();
    }
    let f = Mongo { db: &db, blob_root: blobs.clone() };
    let now = golden()["now"].as_i64().unwrap();
    let mut bad = Vec::new();
    for case in arr(golden(), "http") {
        let name = s(&case["name"]);
        seed(&db, &golden()["datasets"][s(&case["dataset"])]).await;
        let run = AssertUnwindSafe(request_id::scope(rid(case), dispatch(case, &f, now))).catch_unwind();
        let resp = match run.await {
            Ok(resp) => resp,
            Err(_) => request_id::scope(rid(case), async { ApiError::internal().into_response() }).await,
        };
        let (status, headers, body) = parts(resp).await;
        if u64::from(status) != case["status"].as_u64().unwrap() {
            bad.push(format!("{name}: status {status} want {}", case["status"]));
        }
        if header(&headers, "content-type").as_deref() != case["resp_headers"]["content-type"].as_str() {
            bad.push(format!("{name}: content-type {:?}", header(&headers, "content-type")));
        }
        let want = if name.starts_with("texture") {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.decode(s(&case["resp"])).unwrap()
        } else {
            s(&case["resp"]).as_bytes().to_vec()
        };
        if normalize(name, &body) != normalize(name, &want) {
            bad.push(format!(
                "{name}:\n  got  {}\n  want {}",
                String::from_utf8_lossy(&body),
                String::from_utf8_lossy(&want)
            ));
        }
        if name == "reset" {
            let rows: Vec<Document> = {
                use futures::TryStreamExt;
                db.collection::<Document>(service::INGEST_LOG).find(doc! {}).await.unwrap().try_collect().await.unwrap()
            };
            let call = arr(case, "calls").iter().find(|c| c["op"] == "insert_one").unwrap();
            let want = to_doc(&call["doc"]);
            let keys = |d: &Document| d.keys().cloned().collect::<Vec<_>>();
            if rows.len() != 1
                || keys(&rows[0]) != keys(&want)
                || !matches!(rows[0].get("timestamp"), Some(Bson::DateTime(_)))
            {
                bad.push(format!("reset: ingest_log rows {rows:?}"));
            }
            for coll in [service::PRODUCTS, service::CATEGORIES, service::STATE] {
                if db.collection::<Document>(coll).count_documents(doc! {}).await.unwrap() != 0 {
                    bad.push(format!("reset left documents in {coll}"));
                }
            }
        }
    }
    db.drop().await.unwrap();
    let _ = std::fs::remove_dir_all(&blobs);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
