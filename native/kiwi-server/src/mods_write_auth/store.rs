//! The Mongo calls the write-auth, creator-link and issue code makes (Beanie's
//! queries spelled out), behind one type so the services also run against an
//! in-memory copy in tests.

use std::fmt::Display;

use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use mongodb::{
    bson::{Document, doc, oid::ObjectId},
    options::ReturnDocument,
};

use crate::{auth::Rejection, error::ApiError, outbound::store::with_id, state::AppState};

pub type Res<T> = Result<T, ApiError>;

#[derive(Clone)]
pub enum Store {
    Live(AppState),
    #[cfg(test)]
    Mem(std::sync::Arc<super::mem::MemDb>),
}

fn internal(e: impl Display) -> ApiError {
    tracing::error!("mods store: {e}");
    ApiError::internal()
}

impl Store {
    pub fn live(state: &AppState) -> Self {
        Store::Live(state.clone())
    }

    fn coll(state: &AppState, name: &str) -> Res<mongodb::Collection<Document>> {
        let db = state.inner.db.mongo.as_ref().ok_or_else(|| internal("MONGO_URI not configured"))?;
        Ok(db.collection::<Document>(name))
    }

    /// `utcnow()`.
    pub fn now(&self) -> DateTime<Utc> {
        match self {
            Store::Live(_) => crate::dt::now(),
            #[cfg(test)]
            Store::Mem(m) => m.now,
        }
    }

    /// `secrets.token_urlsafe(24)`.
    pub fn token_urlsafe(&self) -> String {
        match self {
            Store::Live(_) => crate::auth::tokens::token_urlsafe(24),
            #[cfg(test)]
            Store::Mem(m) => m.minted.clone(),
        }
    }

    pub fn site_master_handle(&self) -> String {
        match self {
            Store::Live(s) => s.settings().site_master_handle.clone(),
            #[cfg(test)]
            Store::Mem(m) => m.master.clone(),
        }
    }

    /// `find(filter).sort(sort).skip(skip).limit(limit)`; a zero limit is none.
    pub async fn find(
        &self,
        coll: &str,
        filter: Document,
        sort: Option<Document>,
        skip: u64,
        limit: i64,
    ) -> Res<Vec<Document>> {
        match self {
            Store::Live(s) => {
                let c = Self::coll(s, coll)?;
                let mut q = c.find(filter);
                if let Some(sort) = sort {
                    q = q.sort(sort);
                }
                if skip > 0 {
                    q = q.skip(skip);
                }
                if limit > 0 {
                    q = q.limit(limit);
                }
                q.await.map_err(internal)?.try_collect().await.map_err(internal)
            }
            #[cfg(test)]
            Store::Mem(m) => Ok(m.find(coll, &filter, sort.as_ref(), skip, limit)),
        }
    }

    pub async fn find_one(&self, coll: &str, filter: Document) -> Res<Option<Document>> {
        match self {
            Store::Live(s) => Self::coll(s, coll)?.find_one(filter).await.map_err(internal),
            #[cfg(test)]
            Store::Mem(m) => Ok(m.find(coll, &filter, None, 0, 1).into_iter().next()),
        }
    }

    pub async fn get(&self, coll: &str, id: ObjectId) -> Res<Option<Document>> {
        self.find_one(coll, doc! {"_id": id}).await
    }

    pub async fn count(&self, coll: &str, filter: Document) -> Res<u64> {
        match self {
            Store::Live(s) => Self::coll(s, coll)?.count_documents(filter).await.map_err(internal),
            #[cfg(test)]
            Store::Mem(m) => Ok(m.find(coll, &filter, None, 0, 0).len() as u64),
        }
    }

    /// `insert()`: a client-side id, written first.
    pub async fn insert(&self, coll: &str, doc: Document) -> Res<ObjectId> {
        match self {
            Store::Live(s) => {
                let id = ObjectId::new();
                Self::coll(s, coll)?.insert_one(with_id(id, doc)).await.map_err(internal)?;
                Ok(id)
            }
            #[cfg(test)]
            Store::Mem(m) => Ok(m.insert(coll, doc)),
        }
    }

    pub async fn update_one(&self, coll: &str, filter: Document, update: Document) -> Res<()> {
        match self {
            Store::Live(s) => Self::coll(s, coll)?.update_one(filter, update).await.map(drop).map_err(internal),
            #[cfg(test)]
            Store::Mem(m) => {
                m.update(coll, &filter, &update, false);
                Ok(())
            }
        }
    }

    pub async fn update_many(&self, coll: &str, filter: Document, update: Document) -> Res<()> {
        match self {
            Store::Live(s) => Self::coll(s, coll)?.update_many(filter, update).await.map(drop).map_err(internal),
            #[cfg(test)]
            Store::Mem(m) => {
                m.update(coll, &filter, &update, true);
                Ok(())
            }
        }
    }

    /// `find_one_and_update(..., return_document=AFTER)`.
    pub async fn find_one_and_update(&self, coll: &str, filter: Document, update: Document) -> Res<Option<Document>> {
        match self {
            Store::Live(s) => Self::coll(s, coll)?
                .find_one_and_update(filter, update)
                .return_document(ReturnDocument::After)
                .await
                .map_err(internal),
            #[cfg(test)]
            Store::Mem(m) => Ok(m.update(coll, &filter, &update, false).into_iter().next()),
        }
    }

    /// Beanie `save()` of a loaded document: `$set` of every model field.
    pub async fn save(&self, coll: &str, set: Document) -> Res<()> {
        let id = set.get("_id").cloned().unwrap_or(mongodb::bson::Bson::Null);
        match self {
            Store::Live(s) => Self::coll(s, coll)?
                .update_one(doc! {"_id": id}, doc! {"$set": set})
                .upsert(true)
                .await
                .map(drop)
                .map_err(internal),
            #[cfg(test)]
            Store::Mem(m) => {
                m.update(coll, &doc! {"_id": id}, &doc! {"$set": set}, false);
                Ok(())
            }
        }
    }

    pub async fn delete_one(&self, coll: &str, filter: Document) -> Res<()> {
        match self {
            Store::Live(s) => Self::coll(s, coll)?.delete_one(filter).await.map(drop).map_err(internal),
            #[cfg(test)]
            Store::Mem(m) => {
                m.delete(coll, &filter, false);
                Ok(())
            }
        }
    }

    pub async fn delete_many(&self, coll: &str, filter: Document) -> Res<()> {
        match self {
            Store::Live(s) => Self::coll(s, coll)?.delete_many(filter).await.map(drop).map_err(internal),
            #[cfg(test)]
            Store::Mem(m) => {
                m.delete(coll, &filter, true);
                Ok(())
            }
        }
    }

    /// `max_, window = runtime_config.get_rate_limit(prefix)` then
    /// `check_rate_limit(key, max_, window)`.
    pub async fn rate_limit(&self, prefix: &str, key: &str) -> Result<(), Rejection> {
        match self {
            Store::Live(s) => {
                let (max, window) =
                    crate::runtime_config::get_rate_limit(s, prefix).await.map_err(Rejection::internal)?;
                crate::ratelimit::check_rate_limit(s, key, max, window).await.map(drop)
            }
            #[cfg(test)]
            Store::Mem(m) => m.limit(prefix, key),
        }
    }
}
