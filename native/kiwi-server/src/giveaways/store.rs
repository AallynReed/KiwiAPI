//! The Mongo calls Beanie makes for the giveaways service, behind [`Store`]
//! so tests can replay the service against an in-memory collection set.

use std::future::Future;

use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use mongodb::{
    Database,
    bson::{Bson, Document, doc, oid::ObjectId},
    error::{ErrorKind, WriteFailure},
    options::FindOptions,
};

pub type DbResult<T> = anyhow::Result<T>;

/// A unique index rejected an insert (`DuplicateKeyError`).
#[derive(Debug)]
pub struct Duplicate;

pub trait Store: Sync {
    /// `utcnow()`.
    fn now(&self) -> DateTime<Utc>;
    /// `settings.email_enabled`.
    fn email_enabled(&self) -> bool;
    /// `secrets.choice` over `n` items: the index picked.
    fn pick(&self, n: usize) -> usize;

    fn find(
        &self,
        coll: &'static str,
        filter: Document,
        sort: Option<Document>,
        limit: Option<i64>,
    ) -> impl Future<Output = DbResult<Vec<Document>>> + Send;
    fn find_one(&self, coll: &'static str, filter: Document)
    -> impl Future<Output = DbResult<Option<Document>>> + Send;
    fn count(&self, coll: &'static str, filter: Document) -> impl Future<Output = DbResult<u64>> + Send;
    /// `insert_one`; the store assigns `_id` (first, as pymongo does).
    fn insert(
        &self,
        coll: &'static str,
        fields: Document,
    ) -> impl Future<Output = DbResult<Result<ObjectId, Duplicate>>> + Send;
    fn insert_many(
        &self,
        coll: &'static str,
        docs: Vec<Document>,
    ) -> impl Future<Output = DbResult<Vec<ObjectId>>> + Send;
    /// Beanie `save()`: `$set` of `_id` and every field, upserting.
    fn save(&self, coll: &'static str, id: ObjectId, fields: Document) -> impl Future<Output = DbResult<()>> + Send;
    fn delete_one(&self, coll: &'static str, id: ObjectId) -> impl Future<Output = DbResult<()>> + Send;
    fn delete_many(&self, coll: &'static str, filter: Document) -> impl Future<Output = DbResult<()>> + Send;
}

/// The live database.
pub struct Mongo<'a> {
    pub db: &'a Database,
    pub email_enabled: bool,
}

fn with_id(id: ObjectId, fields: Document) -> Document {
    let mut d = doc! { "_id": id };
    d.extend(fields);
    d
}

fn is_duplicate(e: &mongodb::error::Error) -> bool {
    match e.kind.as_ref() {
        ErrorKind::Write(WriteFailure::WriteError(w)) => w.code == 11000,
        ErrorKind::InsertMany(m) => m.write_errors.as_ref().is_some_and(|ws| ws.iter().any(|w| w.code == 11000)),
        _ => false,
    }
}

/// A uniform index in `0..n` from the OS RNG (`secrets.choice`).
pub fn secure_index(n: usize) -> usize {
    if n <= 1 {
        return 0;
    }
    let n = n as u64;
    let zone = u64::MAX - u64::MAX % n;
    loop {
        let mut b = [0u8; 8];
        if getrandom::fill(&mut b).is_err() {
            return (chrono::Utc::now().timestamp_subsec_nanos() as u64 % n) as usize;
        }
        let v = u64::from_le_bytes(b);
        if v < zone {
            return (v % n) as usize;
        }
    }
}

impl Store for Mongo<'_> {
    fn now(&self) -> DateTime<Utc> {
        crate::dt::now()
    }

    fn email_enabled(&self) -> bool {
        self.email_enabled
    }

    fn pick(&self, n: usize) -> usize {
        secure_index(n)
    }

    async fn find(
        &self,
        coll: &'static str,
        filter: Document,
        sort: Option<Document>,
        limit: Option<i64>,
    ) -> DbResult<Vec<Document>> {
        let opts = FindOptions::builder().sort(sort).limit(limit).build();
        Ok(self.db.collection::<Document>(coll).find(filter).with_options(opts).await?.try_collect().await?)
    }

    async fn find_one(&self, coll: &'static str, filter: Document) -> DbResult<Option<Document>> {
        Ok(self.db.collection::<Document>(coll).find_one(filter).await?)
    }

    async fn count(&self, coll: &'static str, filter: Document) -> DbResult<u64> {
        Ok(self.db.collection::<Document>(coll).count_documents(filter).await?)
    }

    async fn insert(&self, coll: &'static str, fields: Document) -> DbResult<Result<ObjectId, Duplicate>> {
        let id = ObjectId::new();
        match self.db.collection::<Document>(coll).insert_one(with_id(id, fields)).await {
            Ok(_) => Ok(Ok(id)),
            Err(e) if is_duplicate(&e) => Ok(Err(Duplicate)),
            Err(e) => Err(e.into()),
        }
    }

    async fn insert_many(&self, coll: &'static str, docs: Vec<Document>) -> DbResult<Vec<ObjectId>> {
        let ids: Vec<ObjectId> = docs.iter().map(|_| ObjectId::new()).collect();
        let docs: Vec<Document> = ids.iter().zip(docs).map(|(id, d)| with_id(*id, d)).collect();
        self.db.collection::<Document>(coll).insert_many(docs).await?;
        Ok(ids)
    }

    async fn save(&self, coll: &'static str, id: ObjectId, fields: Document) -> DbResult<()> {
        self.db
            .collection::<Document>(coll)
            .update_one(doc! { "_id": id }, doc! { "$set": with_id(id, fields) })
            .upsert(true)
            .await?;
        Ok(())
    }

    async fn delete_one(&self, coll: &'static str, id: ObjectId) -> DbResult<()> {
        self.db.collection::<Document>(coll).delete_one(doc! { "_id": id }).await?;
        Ok(())
    }

    async fn delete_many(&self, coll: &'static str, filter: Document) -> DbResult<()> {
        self.db.collection::<Document>(coll).delete_many(filter).await?;
        Ok(())
    }
}

/// `{"_id": id}`.
pub fn by_id(id: ObjectId) -> Document {
    doc! { "_id": id }
}

/// `{"field": {"$in": [...]}}`.
pub fn is_in(field: &str, values: impl IntoIterator<Item = impl Into<Bson>>) -> Document {
    let values: Vec<Bson> = values.into_iter().map(Into::into).collect();
    doc! { field: { "$in": values } }
}
