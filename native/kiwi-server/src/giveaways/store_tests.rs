//! The golden scenario against a real Mongo, opt-in: it creates and drops its
//! own `kiwi_giveaways_test_*` database on MONGO_URI. Run
//! `cargo test -p kiwi-server giveaways_mongo_replay -- --ignored`.

use std::sync::Mutex;

use chrono::{DateTime, Utc};
use mongodb::{
    IndexModel,
    bson::{self, Document, doc, oid::ObjectId},
    options::IndexOptions,
};
use serde_json::Value;

use super::{
    model::ENTRIES,
    store::{DbResult, Duplicate, Mongo, Store},
    tests::{Harness, replay, seq_id},
};
use crate::auth::models::{SITE_USERS, SiteUser};

/// [`Mongo`] with the scenario's frozen clock, first-entrant draw and
/// sequential ids.
struct SeqMongo<'a> {
    inner: Mongo<'a>,
    next: Mutex<u64>,
    clock: Mutex<DateTime<Utc>>,
}

impl SeqMongo<'_> {
    fn id(&self) -> ObjectId {
        let mut n = self.next.lock().expect("lock");
        *n += 1;
        seq_id(*n)
    }

    async fn put(&self, coll: &'static str, fields: Document) -> DbResult<Result<ObjectId, Duplicate>> {
        let id = self.id();
        let mut d = doc! {"_id": id};
        d.extend(fields);
        match self.inner.db.collection::<Document>(coll).insert_one(d).await {
            Ok(_) => Ok(Ok(id)),
            Err(e) if e.to_string().contains("E11000") => Ok(Err(Duplicate)),
            Err(e) => Err(e.into()),
        }
    }
}

impl Store for SeqMongo<'_> {
    fn now(&self) -> DateTime<Utc> {
        *self.clock.lock().expect("lock")
    }

    fn email_enabled(&self) -> bool {
        true
    }

    fn pick(&self, _n: usize) -> usize {
        0
    }

    async fn find(
        &self,
        coll: &'static str,
        filter: Document,
        sort: Option<Document>,
        limit: Option<i64>,
    ) -> DbResult<Vec<Document>> {
        self.inner.find(coll, filter, sort, limit).await
    }

    async fn find_one(&self, coll: &'static str, filter: Document) -> DbResult<Option<Document>> {
        self.inner.find_one(coll, filter).await
    }

    async fn count(&self, coll: &'static str, filter: Document) -> DbResult<u64> {
        self.inner.count(coll, filter).await
    }

    async fn insert(&self, coll: &'static str, fields: Document) -> DbResult<Result<ObjectId, Duplicate>> {
        self.put(coll, fields).await
    }

    async fn insert_many(&self, coll: &'static str, docs: Vec<Document>) -> DbResult<Vec<ObjectId>> {
        let mut ids = Vec::new();
        for d in docs {
            ids.push(self.put(coll, d).await?.map_err(|_| anyhow::anyhow!("duplicate"))?);
        }
        Ok(ids)
    }

    async fn save(&self, coll: &'static str, id: ObjectId, fields: Document) -> DbResult<()> {
        self.inner.save(coll, id, fields).await
    }

    async fn delete_one(&self, coll: &'static str, id: ObjectId) -> DbResult<()> {
        self.inner.delete_one(coll, id).await
    }

    async fn delete_many(&self, coll: &'static str, filter: Document) -> DbResult<()> {
        self.inner.delete_many(coll, filter).await
    }
}

impl Harness for SeqMongo<'_> {
    fn set_now(&self, t: DateTime<Utc>) {
        *self.clock.lock().expect("lock") = t;
    }

    async fn site_user(&self, name: &str) -> SiteUser {
        let d = self.find_one(SITE_USERS, doc! {"username": name}).await.expect("find").expect("user");
        bson::from_document(d).expect("site user")
    }
}

#[tokio::test]
#[ignore = "creates and drops a database on MONGO_URI"]
async fn giveaways_mongo_replay() {
    let uri = std::env::var("MONGO_URI").expect("MONGO_URI");
    let client = mongodb::Client::with_uri_str(&uri).await.expect("client");
    let db = client.database(&format!("kiwi_giveaways_test_{}", ObjectId::new().to_hex()));
    let unique = IndexModel::builder()
        .keys(doc! {"giveaway_id": 1, "site_user_id": 1})
        .options(IndexOptions::builder().unique(true).build())
        .build();
    db.collection::<Document>(ENTRIES).create_index(unique).await.expect("index");
    let scenario: Value = serde_json::from_str(include_str!("../../testdata/giveaways/golden.json")).expect("fixture");
    let mut seeded = 0;
    for (coll, docs) in scenario["scenario"]["seed"].as_object().expect("seed") {
        for d in docs.as_array().expect("docs") {
            let bson::Bson::Document(d) = bson::Bson::try_from(d.clone()).expect("extjson") else { panic!("doc") };
            db.collection::<Document>(coll).insert_one(d).await.expect("seed");
            seeded += 1;
        }
    }
    let st = SeqMongo {
        inner: Mongo { db: &db, email_enabled: true },
        next: Mutex::new(seeded),
        clock: Mutex::new(Utc::now()),
    };
    let bad = replay(&st).await;
    db.drop().await.expect("drop");
    assert!(bad.is_empty(), "{} step(s) differ:\n{}", bad.len(), bad.join("\n"));
}
