//! The update archive as the indexer reads it: `update_state` / `update_changes`
//! in Mongo (same filters and projections as Python, so the same order) and the
//! content-addressed blobs on disk (`<root>/objects/<sha[:2]>/<sha>`).

use std::path::PathBuf;

use futures::TryStreamExt;
use kiwi_core::codex::extract::Blobs;
use mongodb::{
    Database,
    bson::{Bson, Document, doc},
};

/// One `update_changes` row.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub path: String,
    pub kind: String,
    pub sha: Option<String>,
}

/// What the indexer reads from the archive.
#[allow(async_fn_in_trait)]
pub trait Archive: Sync {
    type Blobs: Blobs + Clone + Send + 'static;
    fn blobs(&self) -> Self::Blobs;
    /// `find(_prefix_query(branch, prefix))`: `(path, sha)` in the server's order.
    async fn prefix(&self, prefix: &str) -> anyhow::Result<Vec<(String, Option<String>)>>;
    /// `find({"branch", "path": {"$regex": pattern[, "$options": "i"]}})`.
    async fn regex(&self, pattern: &str, case_insensitive: bool) -> anyhow::Result<Vec<(String, Option<String>)>>;
    /// `UpdateState.find_one({"branch", "path"})`'s sha.
    async fn file_sha(&self, path: &str) -> anyhow::Result<Option<String>>;
    /// `update_changes` of one ordinal.
    async fn changes(&self, ordinal: i64) -> anyhow::Result<Vec<Change>>;

    /// `_load_file`.
    async fn load_file(&self, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let Some(sha) = self.file_sha(path).await? else { return Ok(None) };
        let blobs = self.blobs();
        Ok(tokio::task::spawn_blocking(move || blobs.get(&sha)).await?)
    }

    /// `store.get` off the event loop.
    async fn blob(&self, sha: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let (blobs, sha) = (self.blobs(), sha.to_owned());
        Ok(tokio::task::spawn_blocking(move || blobs.get(&sha)).await?)
    }
}

/// `ContentStore(settings.trove_update_store_dir)`.
#[derive(Clone, Debug)]
pub struct Cas {
    pub root: PathBuf,
}

impl Cas {
    pub fn path_for(&self, sha: &str) -> PathBuf {
        let shard: String = sha.chars().take(2).collect();
        self.root.join("objects").join(shard).join(sha)
    }
}

impl Blobs for Cas {
    fn get(&self, sha: &str) -> Option<Vec<u8>> {
        let p = self.path_for(sha);
        if p.is_file() { std::fs::read(p).ok() } else { None }
    }
}

/// The live archive for one branch.
pub struct MongoArchive {
    pub db: Database,
    pub branch: String,
    pub cas: Cas,
}

fn path_sha(d: &Document) -> (String, Option<String>) {
    let path = d.get_str("path").unwrap_or_default().to_owned();
    let sha = match d.get("content_sha256") {
        Some(Bson::String(s)) => Some(s.clone()),
        _ => None,
    };
    (path, sha)
}

impl MongoArchive {
    async fn find(&self, coll: &str, filter: Document) -> anyhow::Result<Vec<Document>> {
        let cursor = self
            .db
            .collection::<Document>(coll)
            .find(filter)
            .projection(doc! {"path": 1, "content_sha256": 1, "_id": 0})
            .await?;
        Ok(cursor.try_collect().await?)
    }
}

impl Archive for MongoArchive {
    type Blobs = Cas;

    fn blobs(&self) -> Cas {
        self.cas.clone()
    }

    async fn prefix(&self, prefix: &str) -> anyhow::Result<Vec<(String, Option<String>)>> {
        let filter = doc! {"branch": &self.branch, "path": {"$gte": prefix, "$lt": format!("{prefix}\u{ffff}")}};
        Ok(self.find("update_state", filter).await?.iter().map(path_sha).collect())
    }

    async fn regex(&self, pattern: &str, case_insensitive: bool) -> anyhow::Result<Vec<(String, Option<String>)>> {
        let re = if case_insensitive {
            doc! {"$regex": pattern, "$options": "i"}
        } else {
            doc! {"$regex": pattern}
        };
        let filter = doc! {"branch": &self.branch, "path": re};
        Ok(self.find("update_state", filter).await?.iter().map(path_sha).collect())
    }

    async fn file_sha(&self, path: &str) -> anyhow::Result<Option<String>> {
        let d = self
            .db
            .collection::<Document>("update_state")
            .find_one(doc! {"branch": &self.branch, "path": path})
            .await?;
        Ok(d.and_then(|d| d.get_str("content_sha256").ok().map(str::to_owned)))
    }

    async fn changes(&self, ordinal: i64) -> anyhow::Result<Vec<Change>> {
        let cursor = self
            .db
            .collection::<Document>("update_changes")
            .find(doc! {"branch": &self.branch, "ordinal": ordinal})
            .projection(doc! {"path": 1, "type": 1, "content_sha256": 1, "_id": 0})
            .await?;
        let docs: Vec<Document> = cursor.try_collect().await?;
        Ok(docs
            .iter()
            .map(|d| {
                let (path, sha) = path_sha(d);
                Change { path, kind: d.get_str("type").unwrap_or_default().to_owned(), sha }
            })
            .collect())
    }
}
