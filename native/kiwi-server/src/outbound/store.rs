//! The Beanie calls both features make on their one collection, behind a
//! trait so the service logic runs against an in-memory copy in tests.

use std::future::Future;

use futures::TryStreamExt;
use mongodb::{
    Collection,
    bson::{Bson, Document, doc, oid::ObjectId},
};

pub trait Coll: Send + Sync {
    /// `find(owner_id == owner).sort("-created_at")`.
    fn list_owned(&self, owner: ObjectId) -> impl Future<Output = anyhow::Result<Vec<Document>>> + Send;
    fn count_owned(&self, owner: ObjectId) -> impl Future<Output = anyhow::Result<u64>> + Send;
    /// `Model.get(id)`.
    fn get(&self, id: ObjectId) -> impl Future<Output = anyhow::Result<Option<Document>>> + Send;
    /// `insert()`: the store assigns `_id`, encoded first.
    fn insert(&self, doc: Document) -> impl Future<Output = anyhow::Result<ObjectId>> + Send;
    /// `save()`: `$set` of every model field (upsert).
    fn save(&self, set: Document) -> impl Future<Output = anyhow::Result<()>> + Send;
    fn delete(&self, id: ObjectId) -> impl Future<Output = anyhow::Result<()>> + Send;
    /// `find(active == True, {"events": event})`.
    fn active_for(&self, event: &str) -> impl Future<Output = anyhow::Result<Vec<Document>>> + Send;
}

pub struct MongoColl(pub Collection<Document>);

impl MongoColl {
    pub fn new(db: &mongodb::Database, name: &str) -> Self {
        MongoColl(db.collection(name))
    }
}

/// `doc` with `_id` moved to the front, as pymongo encodes an insert.
pub fn with_id(id: ObjectId, doc: Document) -> Document {
    let mut out = Document::new();
    out.insert("_id", id);
    for (k, v) in doc {
        if k != "_id" {
            out.insert(k, v);
        }
    }
    out
}

impl Coll for MongoColl {
    async fn list_owned(&self, owner: ObjectId) -> anyhow::Result<Vec<Document>> {
        Ok(self.0.find(doc! {"owner_id": owner}).sort(doc! {"created_at": -1}).await?.try_collect().await?)
    }

    async fn count_owned(&self, owner: ObjectId) -> anyhow::Result<u64> {
        Ok(self.0.count_documents(doc! {"owner_id": owner}).await?)
    }

    async fn get(&self, id: ObjectId) -> anyhow::Result<Option<Document>> {
        Ok(self.0.find_one(doc! {"_id": id}).await?)
    }

    async fn insert(&self, doc: Document) -> anyhow::Result<ObjectId> {
        let id = ObjectId::new();
        self.0.insert_one(with_id(id, doc)).await?;
        Ok(id)
    }

    async fn save(&self, set: Document) -> anyhow::Result<()> {
        let id = set.get("_id").cloned().unwrap_or(Bson::Null);
        self.0.update_one(doc! {"_id": id}, doc! {"$set": set}).upsert(true).await?;
        Ok(())
    }

    async fn delete(&self, id: ObjectId) -> anyhow::Result<()> {
        self.0.delete_one(doc! {"_id": id}).await?;
        Ok(())
    }

    async fn active_for(&self, event: &str) -> anyhow::Result<Vec<Document>> {
        Ok(self.0.find(doc! {"$and": [{"active": true}, {"events": event}]}).await?.try_collect().await?)
    }
}
