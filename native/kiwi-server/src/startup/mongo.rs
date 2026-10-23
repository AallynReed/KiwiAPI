//! `app.core.database.init_db`: Beanie's index creation, the extra TTL
//! indexes, the legacy index drops and the one-time backfills and purges.

use futures::TryStreamExt;
use mongodb::{
    Database,
    bson::{Bson, Document, doc},
    error::ErrorKind,
    options::UpdateModifications,
};

use super::indexes;
use crate::{auth::pyutil::bson_int, config::Settings};

pub const RATE_LIMIT_COLLECTION: &str = "rate_limit_buckets";
pub const ISSUES_OPTED_IN_HANDLE: &str = "aallyn";
const SOURCE_INDEX: &str = "source_1_source_id_1";

#[derive(Debug)]
pub struct OpError {
    pub code: Option<i32>,
    pub message: String,
}

impl std::fmt::Display for OpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.code {
            Some(code) => write!(f, "{} (code {code})", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl From<mongodb::error::Error> for OpError {
    fn from(e: mongodb::error::Error) -> Self {
        let code = match &*e.kind {
            ErrorKind::Command(c) => Some(c.code),
            _ => None,
        };
        OpError { code, message: e.to_string() }
    }
}

pub type OpResult<T> = Result<T, OpError>;

/// The handful of Mongo calls `init_db` makes.
#[allow(async_fn_in_trait)]
pub trait Mongo {
    async fn create_indexes(&self, coll: &str, indexes: Vec<Document>) -> OpResult<()>;
    async fn list_indexes(&self, coll: &str) -> OpResult<Vec<Document>>;
    async fn drop_index(&self, coll: &str, name: &str) -> OpResult<()>;
    /// `count_documents(filter, limit=1)`.
    async fn count_one(&self, coll: &str, filter: Document) -> OpResult<u64>;
    async fn find(&self, coll: &str, filter: Document, projection: Document) -> OpResult<Vec<Document>>;
    async fn find_one(
        &self,
        coll: &str,
        filter: Document,
        projection: Document,
        sort: Document,
    ) -> OpResult<Option<Document>>;
    async fn update_many(&self, coll: &str, filter: Document, update: UpdateModifications) -> OpResult<u64>;
    async fn update_one(&self, coll: &str, filter: Document, update: Document) -> OpResult<()>;
}

pub struct Live<'a>(pub &'a Database);

impl Mongo for Live<'_> {
    async fn create_indexes(&self, coll: &str, indexes: Vec<Document>) -> OpResult<()> {
        self.0.run_command(doc! {"createIndexes": coll, "indexes": indexes}).await?;
        Ok(())
    }

    async fn list_indexes(&self, coll: &str) -> OpResult<Vec<Document>> {
        let reply = match self.0.run_command(doc! {"listIndexes": coll}).await {
            Ok(r) => r,
            Err(e) => {
                let e = OpError::from(e);
                return if e.code == Some(26) { Ok(Vec::new()) } else { Err(e) };
            }
        };
        let batch = reply
            .get_document("cursor")
            .and_then(|c| c.get_array("firstBatch"))
            .map_err(|e| OpError { code: None, message: e.to_string() })?;
        Ok(batch.iter().filter_map(|b| b.as_document().cloned()).collect())
    }

    async fn drop_index(&self, coll: &str, name: &str) -> OpResult<()> {
        self.0.run_command(doc! {"dropIndexes": coll, "index": name}).await?;
        Ok(())
    }

    async fn count_one(&self, coll: &str, filter: Document) -> OpResult<u64> {
        Ok(self.0.collection::<Document>(coll).count_documents(filter).limit(1).await?)
    }

    async fn find(&self, coll: &str, filter: Document, projection: Document) -> OpResult<Vec<Document>> {
        let cursor = self.0.collection::<Document>(coll).find(filter).projection(projection).await?;
        Ok(cursor.try_collect().await?)
    }

    async fn find_one(
        &self,
        coll: &str,
        filter: Document,
        projection: Document,
        sort: Document,
    ) -> OpResult<Option<Document>> {
        Ok(self.0.collection::<Document>(coll).find_one(filter).projection(projection).sort(sort).await?)
    }

    async fn update_many(&self, coll: &str, filter: Document, update: UpdateModifications) -> OpResult<u64> {
        Ok(self.0.collection::<Document>(coll).update_many(filter, update).await?.modified_count)
    }

    async fn update_one(&self, coll: &str, filter: Document, update: Document) -> OpResult<()> {
        self.0.collection::<Document>(coll).update_one(filter, update).await?;
        Ok(())
    }
}

pub async fn init_db(m: &impl Mongo, s: &Settings) {
    migrate_source_index(m).await;
    for (coll, specs) in indexes::beanie() {
        if let Err(e) = m.create_indexes(coll, specs).await {
            tracing::error!("mongo: indexes on {coll} failed: {e}");
        }
    }
    let rate_limit = doc! {"expireAfterSeconds": 0, "name": "expires_at_1", "key": {"expires_at": 1}};
    if let Err(e) = m.create_indexes(RATE_LIMIT_COLLECTION, vec![rate_limit]).await {
        tracing::error!("mongo: {RATE_LIMIT_COLLECTION} TTL index failed: {e}");
    }
    ensure_ttl_index(m, "usage_events", "created_at", s.usage_retention_days * 86400).await;
    ensure_ttl_index(m, "page_views", "created_at", s.pageview_retention_days * 86400).await;
    ensure_ttl_index(m, "email_outbox", "created_at", s.email_outbox_retention_days * 86400).await;
    drop_index_if_exists(m, "mod_releases", "project_id_1_tag_1").await;
    drop_index_if_exists(m, "mod_projects", "slug_1").await;
    backfill_last_release_at(m).await;
    backfill_issue_optin(m).await;
    backfill_release_announced_at(m).await;
    ensure_ttl_index(m, "mod_download_events", "created_at", 8 * 86400).await;
    ensure_ttl_index(m, "sessions", "expires_at", 0).await;
    ensure_ttl_index(m, "site_sessions", "expires_at", 0).await;
    for coll in ["sessions", "site_sessions"] {
        let filter = doc! {"$or": [{"ip": {"$exists": true}}, {"user_agent": {"$exists": true}}]};
        let _ = m.update_many(coll, filter, doc! {"$unset": {"ip": "", "user_agent": ""}}.into()).await;
    }
    let _ = m
        .update_many(
            "site_users",
            doc! {"discord_guilds": {"$exists": true}},
            doc! {"$unset": {"discord_guilds": "", "discord_guilds_synced_at": ""}}.into(),
        )
        .await;
    drop_index_if_exists(m, "site_users", "email_1").await;
    let purge = async {
        m.update_many("site_users", doc! {"email": {"$exists": true}}, doc! {"$unset": {"email": ""}}.into()).await?;
        m.update_many(
            "giveaways",
            doc! {"winner_email": {"$exists": true}},
            doc! {"$unset": {"winner_email": ""}}.into(),
        )
        .await?;
        m.update_many(
            "prize_codes",
            doc! {"awarded_to_email": {"$exists": true}},
            doc! {"$unset": {"awarded_to_email": ""}}.into(),
        )
        .await
    };
    let _ = purge.await;
}

/// The sparse `(source, source_id)` unique index became partial under the
/// same name. Python drops it on every boot; dropping only the old shape ends
/// in the same index.
async fn migrate_source_index(m: &impl Mongo) {
    let Ok(existing) = m.list_indexes("mod_projects").await else { return };
    let partial = Bson::Document(doc! {"source": {"$type": "string"}});
    let stale = existing
        .iter()
        .any(|ix| ix.get_str("name") == Ok(SOURCE_INDEX) && ix.get("partialFilterExpression") != Some(&partial));
    if stale {
        drop_index_if_exists(m, "mod_projects", SOURCE_INDEX).await;
    }
}

async fn drop_index_if_exists(m: &impl Mongo, coll: &str, name: &str) {
    let _ = m.drop_index(coll, name).await;
}

/// `_ensure_ttl_index`: a single-field TTL index named `{field}_ttl`; any
/// conflicting single-field index on `field` is dropped and the TTL index
/// recreated (no `collMod`: the app user only has readWrite).
pub async fn ensure_ttl_index(m: &impl Mongo, coll: &str, field: &str, seconds: i64) {
    let mut key = Document::new();
    key.insert(field, 1);
    let spec = doc! {"name": format!("{field}_ttl"), "expireAfterSeconds": bson_int(seconds), "key": key};
    if m.create_indexes(coll, vec![spec.clone()]).await.is_ok() {
        return;
    }
    for ix in m.list_indexes(coll).await.unwrap_or_default() {
        let Ok(name) = ix.get_str("name") else { continue };
        if name == "_id_" {
            continue;
        }
        let single = ix.get_document("key").is_ok_and(|k| k.len() == 1 && k.keys().next().is_some_and(|f| f == field));
        if single {
            let _ = m.drop_index(coll, name).await;
        }
    }
    if let Err(e) = m.create_indexes(coll, vec![spec]).await {
        tracing::error!("mongo: TTL index on {coll}.{field} failed: {e}");
    }
}

async fn backfill_last_release_at(m: &impl Mongo) {
    let run = async {
        let missing = || doc! {"$or": [{"last_release_at": {"$exists": false}}, {"last_release_at": null}]};
        if m.count_one("mod_projects", missing()).await? == 0 {
            return Ok(0);
        }
        let mut filled = 0;
        for p in m.find("mod_projects", missing(), doc! {"created_at": 1}).await? {
            let id = p.get("_id").cloned().unwrap_or(Bson::Null);
            let newest = m
                .find_one(
                    "mod_releases",
                    doc! {"project_id": id.clone(), "status": "published"},
                    doc! {"published_at": 1, "created_at": 1},
                    doc! {"published_at": -1, "created_at": -1},
                )
                .await?;
            let stamp = newest.and_then(|r| py_or(r.get("published_at"), r.get("created_at")));
            let stamp = py_or(stamp.as_ref(), p.get("created_at")).unwrap_or(Bson::Null);
            m.update_one("mod_projects", doc! {"_id": id}, doc! {"$set": {"last_release_at": stamp}}).await?;
            filled += 1;
        }
        Ok::<_, OpError>(filled)
    };
    match run.await {
        Ok(0) => {}
        Ok(n) => tracing::info!("mods_hub: backfilled last_release_at on {n} project(s)"),
        Err(e) => tracing::warn!("mods_hub: last_release_at backfill skipped: {e}"),
    }
}

/// Python's `a or b` over stored values (a missing field reads as None).
fn py_or(a: Option<&Bson>, b: Option<&Bson>) -> Option<Bson> {
    let truthy = |v: &Bson| match v {
        Bson::Null => false,
        Bson::Boolean(b) => *b,
        Bson::Int32(n) => *n != 0,
        Bson::Int64(n) => *n != 0,
        Bson::Double(f) => *f != 0.0,
        Bson::String(s) => !s.is_empty(),
        Bson::Array(a) => !a.is_empty(),
        Bson::Document(d) => !d.is_empty(),
        _ => true,
    };
    if a.is_some_and(truthy) { a.cloned() } else { b.cloned() }
}

async fn backfill_issue_optin(m: &impl Mongo) {
    let run = async {
        let absent = || doc! {"issues_enabled": {"$exists": false}};
        if m.count_one("mod_projects", absent()).await? == 0 {
            return Ok(None);
        }
        let mut owner = absent();
        owner.insert("owner_handle", ISSUES_OPTED_IN_HANDLE);
        let on = m.update_many("mod_projects", owner, doc! {"$set": {"issues_enabled": true}}.into()).await?;
        let off = m.update_many("mod_projects", absent(), doc! {"$set": {"issues_enabled": false}}.into()).await?;
        Ok::<_, OpError>(Some((on, off)))
    };
    match run.await {
        Ok(None) => {}
        Ok(Some((on, off))) => tracing::info!("mods_hub: issues on for {on} existing mod(s), off for {off}"),
        Err(e) => tracing::warn!("mods_hub: issue opt-in backfill skipped: {e}"),
    }
}

async fn backfill_release_announced_at(m: &impl Mongo) {
    let run = async {
        let missing =
            || doc! {"status": "published", "$or": [{"announced_at": {"$exists": false}}, {"announced_at": null}]};
        if m.count_one("mod_releases", missing()).await? == 0 {
            return Ok(None);
        }
        let public: Vec<Bson> = m
            .find("mod_projects", doc! {"visibility": "public"}, doc! {"_id": 1})
            .await?
            .into_iter()
            .filter_map(|p| p.get("_id").cloned())
            .collect();
        if public.is_empty() {
            return Ok(None);
        }
        let mut filter = missing();
        filter.insert("project_id", doc! {"$in": public});
        let stamp = vec![doc! {"$set": {"announced_at": {"$ifNull": ["$published_at", "$created_at"]}}}];
        Ok::<_, OpError>(Some(m.update_many("mod_releases", filter, stamp.into()).await?))
    };
    match run.await {
        Ok(None) => {}
        Ok(Some(n)) => tracing::info!("mods_hub: backfilled announced_at on {n} release(s)"),
        Err(e) => tracing::warn!("mods_hub: announced_at backfill skipped: {e}"),
    }
}
