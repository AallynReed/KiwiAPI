//! `app/trove/store/service.py` reads, the texture lookup and the reset, over
//! [`Fetch`] so the exact Mongo calls Beanie makes can be replayed in tests.
//!
//! A store error, a document the model rejects or a float `json.dumps`
//! refuses is an unhandled exception in Python, answered by the outermost
//! error middleware: here it panics into the request-id boundary, which does
//! the same.

use std::future::Future;

use futures::TryStreamExt;
use kiwi_core::store::{Product, timeline_span};
use mongodb::{
    Database,
    bson::{Bson, Document, doc, oid::ObjectId},
    options::FindOptions,
};
use serde_json::{Value, json};

use super::model::{self, Flavour, Invalid, NonFinite};
use crate::auth::{IngestAuth, pyutil::bson_int};

pub const PRODUCTS: &str = "store_products";
pub const CATEGORIES: &str = "store_categories";
pub const STATE: &str = "store_state";
pub const UPDATE_STATE: &str = "update_state";
pub const UPDATE_CHANGES: &str = "update_changes";
pub const INGEST_LOG: &str = "ingest_log";
pub const TEXTURE_BRANCH: &str = "live-us";

const PRODUCT_FIELDS: &[&str] = &[
    "code",
    "kind",
    "name",
    "image",
    "info",
    "informational",
    "tradable",
    "prices",
    "price_string",
    "price_string_currency",
    "price_string_sale",
    "promo",
    "deal_expires_at",
    "interact_label",
    "interact_enabled",
    "trial_limits",
    "class_level",
    "class_power_rank",
    "class_shield_frame",
    "class_sub_name",
    "class_icon",
    "textures",
    "loot_title",
    "loot_body",
    "categories",
    "first_seen",
    "last_seen",
    "price_history",
    "availability",
];
const CATEGORY_FIELDS: &[&str] = &["index", "label", "icon", "codes", "last_seen"];
const STATE_FIELDS: &[&str] = &["key", "last_anchor", "title", "product_count", "category_count"];
const UPDATE_STATE_FIELDS: &[&str] =
    &["branch", "path", "content_sha256", "fnv_hash", "size", "archive", "archive_index", "last_ordinal"];
const UPDATE_CHANGE_FIELDS: &[&str] = &["branch", "ordinal", "path", "type", "content_sha256", "fnv_hash", "size"];

/// Beanie's projection: `_id`, `revision_id`, then every model field.
pub fn projection(fields: &[&str]) -> Document {
    let mut d = doc! { "_id": 1, "revision_id": 1 };
    for f in fields {
        d.insert(*f, 1);
    }
    d
}

/// One `collection.find(...)` as Beanie issues it (`skip`/`limit` 0 = unset).
#[derive(Clone, Debug, PartialEq)]
pub struct Find {
    pub filter: Document,
    pub projection: Document,
    pub sort: Vec<(&'static str, i32)>,
    pub skip: u64,
    pub limit: i64,
}

/// The store calls the routes make; a failed call panics (see the module doc).
pub trait Fetch: Sync {
    fn find_one(
        &self,
        coll: &'static str,
        filter: Document,
        projection: Document,
    ) -> impl Future<Output = Option<Document>> + Send;
    fn find(&self, coll: &'static str, find: Find) -> impl Future<Output = Vec<Document>> + Send;
    fn count(&self, coll: &'static str, filter: Document) -> impl Future<Output = u64> + Send;
    fn delete_all(&self, coll: &'static str) -> impl Future<Output = u64> + Send;
    /// The one write whose failure Python swallows (`ingest_log.record`).
    fn insert(&self, coll: &'static str, doc: Document) -> impl Future<Output = Result<(), String>> + Send;
    /// The CAS blob for `sha`, `None` when it is not a file.
    fn blob(&self, sha: &str) -> impl Future<Output = Option<Vec<u8>>> + Send;
}

pub fn unhandled(e: impl std::fmt::Display) -> ! {
    panic!("store: {e}")
}

fn valid<T>(r: Result<T, Invalid>) -> T {
    r.unwrap_or_else(|e| unhandled(format!("document failed validation: {}", e.0)))
}

fn finite<T>(r: Result<T, NonFinite>) -> T {
    r.unwrap_or_else(|_| unhandled("ValueError: Out of range float values are not JSON compliant"))
}

/// The live store.
pub struct Mongo<'a> {
    pub db: &'a Database,
    pub blob_root: std::path::PathBuf,
}

impl Fetch for Mongo<'_> {
    async fn find_one(&self, coll: &'static str, filter: Document, projection: Document) -> Option<Document> {
        let coll = self.db.collection::<Document>(coll);
        coll.find_one(filter).projection(projection).await.unwrap_or_else(|e| unhandled(e))
    }

    async fn find(&self, coll: &'static str, find: Find) -> Vec<Document> {
        let mut sort = Document::new();
        for (k, d) in &find.sort {
            sort.insert(*k, *d);
        }
        let mut opts = FindOptions::builder().projection(find.projection).build();
        if !sort.is_empty() {
            opts.sort = Some(sort);
        }
        if find.skip > 0 {
            opts.skip = Some(find.skip);
        }
        if find.limit > 0 {
            opts.limit = Some(find.limit);
        }
        let coll = self.db.collection::<Document>(coll);
        let cursor = coll.find(find.filter).with_options(opts).await.unwrap_or_else(|e| unhandled(e));
        cursor.try_collect().await.unwrap_or_else(|e| unhandled(e))
    }

    async fn count(&self, coll: &'static str, filter: Document) -> u64 {
        self.db.collection::<Document>(coll).count_documents(filter).await.unwrap_or_else(|e| unhandled(e))
    }

    async fn delete_all(&self, coll: &'static str) -> u64 {
        self.db.collection::<Document>(coll).delete_many(doc! {}).await.unwrap_or_else(|e| unhandled(e)).deleted_count
    }

    async fn insert(&self, coll: &'static str, doc: Document) -> Result<(), String> {
        self.db.collection::<Document>(coll).insert_one(doc).await.map(|_| ()).map_err(|e| e.to_string())
    }

    async fn blob(&self, sha: &str) -> Option<Vec<u8>> {
        let shard: String = sha.chars().take(2).collect();
        let path = self.blob_root.join("objects").join(shard).join(sha);
        match tokio::fs::metadata(&path).await {
            Ok(m) if m.is_file() => Some(tokio::fs::read(&path).await.unwrap_or_else(|e| unhandled(e))),
            _ => None,
        }
    }
}

/// `_get_state().last_anchor`.
pub async fn anchor<F: Fetch>(f: &F) -> Option<i64> {
    let d = f.find_one(STATE, doc! {"key": "state"}, projection(STATE_FIELDS)).await?;
    valid(model::state(&d)).last_anchor
}

fn products(docs: &[Document]) -> Vec<Product> {
    docs.iter().map(|d| valid(model::product(d))).collect()
}

/// `/products` query parameters.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProductsQuery {
    pub category: Option<i64>,
    pub kind: Option<String>,
    pub currency: Option<String>,
    pub q: Option<String>,
    pub active: bool,
    pub on_sale: bool,
    pub limit: i64,
    pub offset: i64,
}

/// `list_products`' Mongo filter.
pub fn products_filter(q: &ProductsQuery, anchor: Option<i64>) -> Document {
    let mut f = Document::new();
    if let (true, Some(a)) = (q.active, anchor) {
        f.insert("last_seen", bson_int(a));
    }
    if let Some(c) = q.category {
        f.insert("categories", bson_int(c));
    }
    if let Some(k) = &q.kind {
        f.insert("kind", k.as_str());
    }
    if let Some(c) = &q.currency {
        f.insert("prices.currency", c.as_str());
    }
    if q.on_sale {
        f.insert(
            "$or",
            vec![
                Bson::Document(doc! {"prices": {"$elemMatch": {"sale": {"$ne": ""}}}}),
                Bson::Document(doc! {"price_string_sale": {"$ne": ""}}),
            ],
        );
    }
    if let Some(q) = q.q.as_deref().filter(|q| !q.is_empty()) {
        f.insert("name", doc! {"$regex": q, "$options": "i"});
    }
    f
}

/// `list_products` + the page envelope.
pub async fn products_page<F: Fetch>(f: &F, q: &ProductsQuery, fl: Flavour) -> Value {
    let anchor = anchor(f).await;
    let filter = products_filter(q, anchor);
    let total = f.count(PRODUCTS, filter.clone()).await;
    let find = Find {
        filter,
        projection: projection(PRODUCT_FIELDS),
        sort: vec![("code", 1)],
        skip: q.offset as u64,
        limit: q.limit,
    };
    let docs = products(&f.find(PRODUCTS, find).await);
    let items: Vec<Value> = docs.iter().map(|p| finite(model::product_json(p, anchor, None, fl))).collect();
    json!({"items": items, "count": items.len(), "total": total, "anchor": anchor})
}

/// `get_product(code)`; `None` is the route's 404.
pub async fn product_detail<F: Fetch>(f: &F, code: &str, now: i64, fl: Flavour) -> Option<Value> {
    let anchor = anchor(f).await;
    let d = f.find_one(PRODUCTS, doc! {"code": code}, projection(PRODUCT_FIELDS)).await?;
    Some(finite(model::product_json(&valid(model::product(&d)), anchor, Some(now), fl)))
}

/// `list_categories()` + the envelope.
pub async fn categories<F: Fetch>(f: &F) -> Value {
    let anchor = anchor(f).await;
    let find =
        Find { filter: doc! {}, projection: projection(CATEGORY_FIELDS), sort: vec![("index", 1)], skip: 0, limit: 0 };
    let cats: Vec<_> = f.find(CATEGORIES, find).await.iter().map(|d| valid(model::category(d))).collect();
    let items: Vec<Value> = cats.iter().map(|c| model::category_json(c, anchor)).collect();
    json!({"items": items, "count": items.len(), "anchor": anchor})
}

/// `timeline(kind=, limit=)`.
pub async fn timeline<F: Fetch>(f: &F, kind: Option<&str>, limit: i64, now: i64) -> Value {
    let anchor = anchor(f).await;
    let mut filter = Document::new();
    if let Some(k) = kind {
        filter.insert("kind", k);
    }
    let find = Find { filter, projection: projection(PRODUCT_FIELDS), sort: vec![("last_seen", -1)], skip: 0, limit };
    let docs = products(&f.find(PRODUCTS, find).await);
    let items: Vec<Value> = docs.iter().map(|p| model::timeline_item(p, anchor)).collect();
    let (start, end) = timeline_span(&docs, anchor, now);
    json!({"anchor": anchor, "span": {"start": start, "end": end}, "items": items, "count": items.len()})
}

/// `re.escape(s)`.
pub fn re_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        if "()[]{}?*+-|^$\\.&~# \t\n\r\x0b\x0c".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `updates.read.last_known_meta(...)["content_sha256"]` (outer `None`: no meta).
async fn last_known_sha<F: Fetch>(f: &F, branch: &str, path: &str) -> Option<Option<String>> {
    let last = Find {
        filter: doc! {"branch": branch, "path": path},
        projection: projection(UPDATE_CHANGE_FIELDS),
        sort: vec![("ordinal", -1)],
        skip: 0,
        limit: 1,
    };
    let last = valid(f.find(UPDATE_CHANGES, last).await.first().map(model::update_change).transpose())?;
    if last.1 != "removed" {
        return None;
    }
    let prev = Find {
        filter: doc! {
            "branch": branch, "path": path,
            "ordinal": {"$lt": bson_int(last.0)}, "content_sha256": {"$ne": Bson::Null},
        },
        projection: projection(UPDATE_CHANGE_FIELDS),
        sort: vec![("ordinal", -1)],
        skip: 0,
        limit: 1,
    };
    let prev = valid(f.find(UPDATE_CHANGES, prev).await.first().map(model::update_change).transpose())?;
    Some(prev.2)
}

/// `get_file_meta(branch, path)["content_sha256"]` (outer `None`: no meta).
async fn file_meta_sha<F: Fetch>(f: &F, branch: &str, path: &str) -> Option<Option<String>> {
    match f.find_one(UPDATE_STATE, doc! {"branch": branch, "path": path}, projection(UPDATE_STATE_FIELDS)).await {
        Some(d) => Some(Some(valid(model::update_state_sha(&d)))),
        None => last_known_sha(f, branch, path).await,
    }
}

/// `resolve_texture_sha(path, branch)`: exact, lowercased (queried even when
/// identical), then an anchored case-insensitive regex.
pub async fn texture_sha<F: Fetch>(f: &F, path: &str, branch: &str) -> Option<String> {
    let replaced = path.replace('\\', "/");
    let norm = kiwi_core::pystr::strip(&replaced).trim_start_matches('/');
    for candidate in [norm.to_owned(), norm.to_lowercase()] {
        if let Some(Some(sha)) = file_meta_sha(f, branch, &candidate).await
            && !sha.is_empty()
        {
            return Some(sha);
        }
    }
    let filter = doc! {"branch": branch, "path": {"$regex": format!("^{}$", re_escape(norm)), "$options": "i"}};
    let d = f.find_one(UPDATE_STATE, filter, projection(UPDATE_STATE_FIELDS)).await?;
    Some(valid(model::update_state_sha(&d))).filter(|s| !s.is_empty())
}

/// The texture route's media type, from the requested path's extension.
pub fn texture_media_type(path: &str) -> &'static str {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

/// `ingest_log.record(...)`'s row (`_id` first, as pymongo writes it).
pub fn ingest_log_doc(
    endpoint: &str,
    auth: &IngestAuth,
    summary: Document,
    now: chrono::DateTime<chrono::Utc>,
) -> Document {
    let token_name = auth.token.as_ref().map_or(Bson::Null, |t| Bson::String(t.name.clone()));
    doc! {
        "_id": ObjectId::new(),
        "endpoint": endpoint,
        "timestamp": crate::dt::to_bson(now),
        "user_id": auth.user.id,
        "user_email": auth.user.email.as_str(),
        "success": true,
        "summary": summary,
        "error": Bson::Null,
        "auth_via": if auth.token.is_some() { "token" } else { "session" },
        "token_name": token_name,
    }
}

/// `ingest_log.record`: a failed write is logged, never raised.
pub async fn record_ingest<F: Fetch>(f: &F, doc: Document) {
    if let Err(e) = f.insert(INGEST_LOG, doc).await {
        tracing::error!("ingest_log.record: failed to write entry: {e}");
    }
}

/// `reset()`: every product, category and the state doc; returns docs removed.
pub async fn reset<F: Fetch>(f: &F) -> u64 {
    let mut removed = 0;
    for coll in [PRODUCTS, CATEGORIES, STATE] {
        removed += f.delete_all(coll).await;
    }
    removed
}
