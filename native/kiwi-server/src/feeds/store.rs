//! The `trove_news`, `trove_events` and `feed_cache` documents, read the way
//! Beanie validates them and written the way `upsert_by` writes them.

use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use mongodb::{
    Database,
    bson::{Bson, Document, doc, oid::ObjectId},
};
use serde_json::{Value, json};

use super::pyval;
use crate::dt;

pub const NEWS: &str = "trove_news";
pub const EVENTS: &str = "trove_events";
pub const FEED_CACHE: &str = "feed_cache";

/// Beanie's projection: every model field, `_id` and `revision_id` first.
fn projection(fields: &[&str]) -> Document {
    let mut d = doc! { "_id": 1, "revision_id": 1 };
    for f in fields {
        d.insert(*f, 1);
    }
    d
}

const NEWS_FIELDS: [&str; 10] = [
    "url",
    "title",
    "author",
    "summary",
    "category",
    "categories",
    "image",
    "published_at",
    "first_seen_at",
    "updated_at",
];
const EVENT_FIELDS: [&str; 11] = [
    "event_id",
    "name",
    "url",
    "category",
    "image",
    "icon",
    "lookup",
    "starts_at",
    "ends_at",
    "first_seen_at",
    "updated_at",
];
const FEED_FIELDS: [&str; 3] = ["feed", "items", "fetched_at"];

/// A store error or a document Python's model validation rejects: either is
/// an unhandled exception there (a 500 from the outermost handler).
pub fn unhandled(err: impl std::fmt::Display) -> ! {
    panic!("feeds: {err}")
}

/// pydantic `str` (required unless `default`).
fn str_field(d: &Document, key: &str, default: Option<&str>) -> String {
    match (d.get(key), default) {
        (Some(Bson::String(s)), _) => s.clone(),
        (None, Some(def)) => def.to_owned(),
        (other, _) => unhandled(format!("{key}: not a str: {other:?}")),
    }
}

/// pydantic `str | None = None`.
fn opt_str(d: &Document, key: &str) -> Option<String> {
    match d.get(key) {
        None | Some(Bson::Null) => None,
        Some(Bson::String(s)) => Some(s.clone()),
        Some(other) => unhandled(format!("{key}: not a str: {other:?}")),
    }
}

/// pydantic `int` in lax mode.
fn int_field(d: &Document, key: &str) -> i64 {
    match d.get(key) {
        Some(Bson::Int32(i)) => i64::from(*i),
        Some(Bson::Int64(i)) => *i,
        Some(Bson::Boolean(b)) => i64::from(*b),
        Some(Bson::Double(f)) if f.is_finite() && f.fract() == 0.0 && f.abs() < 9.2e18 => *f as i64,
        Some(Bson::String(s)) => {
            kiwi_core::feeds::py::int(s, 10).and_then(|i| i64::try_from(i).ok()).unwrap_or_else(|| unhandled(key))
        }
        other => unhandled(format!("{key}: not an int: {other:?}")),
    }
}

fn opt_datetime(d: &Document, key: &str) -> Option<DateTime<Utc>> {
    match d.get(key) {
        None | Some(Bson::Null) => None,
        Some(Bson::DateTime(t)) => Some(dt::from_bson(*t)),
        Some(other) => unhandled(format!("{key}: not a datetime: {other:?}")),
    }
}

fn pyd_dt(t: Option<DateTime<Utc>>) -> Value {
    t.map_or(Value::Null, |t| Value::String(dt::pydantic(&t)))
}

/// A `TroveNews` document.
#[derive(Clone, Debug)]
pub struct News {
    pub url: String,
    pub title: String,
    pub author: String,
    pub summary: String,
    pub category: String,
    pub categories: Vec<String>,
    pub image: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
}

impl News {
    pub fn from_doc(d: &Document) -> News {
        let categories = match d.get("categories") {
            None => Vec::new(),
            Some(Bson::Array(a)) => a
                .iter()
                .map(|c| match c {
                    Bson::String(s) => s.clone(),
                    other => unhandled(format!("categories: not a str: {other:?}")),
                })
                .collect(),
            Some(other) => unhandled(format!("categories: not a list: {other:?}")),
        };
        News {
            url: str_field(d, "url", None),
            title: str_field(d, "title", None),
            author: str_field(d, "author", Some("Team Trove")),
            summary: str_field(d, "summary", Some("")),
            category: str_field(d, "category", Some("News")),
            categories,
            image: opt_str(d, "image"),
            published_at: opt_datetime(d, "published_at"),
        }
    }

    /// `TroveNewsItem`.
    pub fn item(&self) -> Value {
        json!({
            "title": self.title,
            "url": self.url,
            "author": self.author,
            "summary": self.summary,
            "category": self.category,
            "categories": self.categories,
            "image": self.image,
            "published_at": pyd_dt(self.published_at),
        })
    }
}

/// A `TroveEvent` document.
#[derive(Clone, Debug)]
pub struct Event {
    pub event_id: String,
    pub name: String,
    pub url: String,
    pub category: String,
    pub image: Option<String>,
    pub icon: Option<String>,
    pub lookup: Option<String>,
    pub starts_at: i64,
    pub ends_at: i64,
}

impl Event {
    pub fn from_doc(d: &Document) -> Event {
        Event {
            event_id: str_field(d, "event_id", None),
            name: str_field(d, "name", None),
            url: str_field(d, "url", Some("")),
            category: str_field(d, "category", Some("Event")),
            image: opt_str(d, "image"),
            icon: opt_str(d, "icon"),
            lookup: opt_str(d, "lookup"),
            starts_at: int_field(d, "starts_at"),
            ends_at: int_field(d, "ends_at"),
        }
    }

    /// `_event_item(ev, now)`: `TroveEventItem` with its live status.
    pub fn item(&self, now: i64) -> Value {
        let (status, until) = if now < self.starts_at {
            ("upcoming", self.starts_at - now)
        } else if now < self.ends_at {
            ("ongoing", self.ends_at - now)
        } else {
            ("ended", 0)
        };
        json!({
            "event_id": self.event_id,
            "name": self.name,
            "url": self.url,
            "category": self.category,
            "image": self.image,
            "icon": self.icon,
            "lookup": self.lookup,
            "starts_at": self.starts_at,
            "ends_at": self.ends_at,
            "status": status,
            "seconds_until": until,
        })
    }
}

async fn find_many(
    db: &Database,
    coll: &str,
    filter: Document,
    fields: &[&str],
    sort: Document,
    skip: Option<u64>,
    limit: i64,
) -> Vec<Document> {
    let c = db.collection::<Document>(coll);
    let find = c.find(filter).projection(projection(fields)).sort(sort).limit(limit);
    let find = match skip {
        Some(skip) => find.skip(skip),
        None => find,
    };
    let cursor = find.await.unwrap_or_else(|e| unhandled(e));
    cursor.try_collect().await.unwrap_or_else(|e| unhandled(e))
}

/// `TroveNews.find().sort("-published_at").skip(..).limit(..)`.
pub async fn news_page(db: &Database, skip: Option<u64>, limit: i64) -> Vec<News> {
    let docs = find_many(db, NEWS, doc! {}, &NEWS_FIELDS, doc! { "published_at": -1 }, skip, limit).await;
    docs.iter().map(News::from_doc).collect()
}

pub async fn news_count(db: &Database) -> u64 {
    db.collection::<Document>(NEWS).count_documents(doc! {}).await.unwrap_or_else(|e| unhandled(e))
}

pub async fn events(db: &Database, filter: Document, sort: Document, limit: i64) -> Vec<Event> {
    let docs = find_many(db, EVENTS, filter, &EVENT_FIELDS, sort, None, limit).await;
    docs.iter().map(Event::from_doc).collect()
}

/// The `$group` / `$sort` category list, falsy ids dropped.
pub async fn event_categories(db: &Database) -> Vec<String> {
    let pipeline = vec![doc! { "$group": { "_id": "$category" } }, doc! { "$sort": { "_id": 1 } }];
    let cursor = db.collection::<Document>(EVENTS).aggregate(pipeline).await.unwrap_or_else(|e| unhandled(e));
    let rows: Vec<Document> = cursor.try_collect().await.unwrap_or_else(|e| unhandled(e));
    rows.iter()
        .filter_map(|r| match r.get("_id") {
            None | Some(Bson::Null | Bson::Boolean(false) | Bson::Int32(0) | Bson::Int64(0)) => None,
            Some(Bson::String(s)) if s.is_empty() => None,
            Some(Bson::Double(f)) if *f == 0.0 => None,
            Some(Bson::Array(a)) if a.is_empty() => None,
            Some(Bson::Document(d)) if d.is_empty() => None,
            Some(Bson::String(s)) => Some(s.clone()),
            Some(other) => unhandled(format!("category: not a str: {other:?}")),
        })
        .collect()
}

/// A `FeedCache` document: its items and `fetched_at`.
pub async fn feed(db: &Database, name: &str) -> Option<(Vec<Document>, DateTime<Utc>)> {
    let d = db
        .collection::<Document>(FEED_CACHE)
        .find_one(doc! { "feed": name })
        .projection(projection(&FEED_FIELDS))
        .await
        .unwrap_or_else(|e| unhandled(e))?;
    let _ = str_field(&d, "feed", None);
    let items = match d.get("items") {
        None => Vec::new(),
        Some(Bson::Array(a)) => a
            .iter()
            .map(|i| match i {
                Bson::Document(doc) => doc.clone(),
                other => unhandled(format!("items: not a dict: {other:?}")),
            })
            .collect(),
        Some(other) => unhandled(format!("items: not a list: {other:?}")),
    };
    let fetched_at = match d.get("fetched_at") {
        None => dt::now(),
        Some(Bson::DateTime(t)) => dt::from_bson(*t),
        Some(other) => unhandled(format!("fetched_at: not a datetime: {other:?}")),
    };
    Some((items, fetched_at))
}

/// `TwitchStream(**item)`.
pub fn twitch_stream(i: &Document) -> Value {
    json!({
        "channel": str_field(i, "channel", None),
        "login": str_field(i, "login", None),
        "url": str_field(i, "url", None),
        "title": str_field(i, "title", None),
        "viewers": int_field(i, "viewers"),
        "game": opt_str(i, "game"),
        "started_at": opt_str(i, "started_at"),
        "thumbnail": opt_str(i, "thumbnail"),
    })
}

/// `Video(**item)`.
pub fn video(i: &Document) -> Value {
    json!({
        "title": str_field(i, "title", None),
        "url": str_field(i, "url", None),
        "channel": str_field(i, "channel", None),
        "video_id": opt_str(i, "video_id"),
        "published_at": opt_str(i, "published_at"),
        "thumbnail_url": opt_str(i, "thumbnail_url"),
    })
}

/// Which model `upsert_by` writes.
pub struct Model {
    pub coll: &'static str,
    pub fields: &'static [&'static str],
    /// Fields after the caller's (`first_seen_at`, `updated_at`): `utcnow()`
    /// on insert, kept (or filled) on update.
    pub defaults: &'static [&'static str],
    /// `upsert_by(stamp=...)`: refreshed on update.
    pub stamp: Option<&'static str>,
}

pub const EVENT_MODEL: Model = Model {
    coll: EVENTS,
    fields: &EVENT_FIELDS,
    defaults: &["first_seen_at", "updated_at"],
    stamp: Some("updated_at"),
};
pub const FEED_MODEL: Model = Model { coll: FEED_CACHE, fields: &FEED_FIELDS, defaults: &[], stamp: None };

/// `upsert_by(model, key, value, fields, stamp)`: insert the model's fields in
/// order (pymongo puts `_id` first), or `$set` every field of the loaded
/// document as Beanie's `save()` does.
pub async fn upsert_by(db: &Database, model: &Model, key: &str, value: &str, fields: Document) -> anyhow::Result<()> {
    let c = db.collection::<Document>(model.coll);
    let now = dt::to_bson(dt::now());
    let existing = c.find_one(doc! { key: value }).projection(projection(model.fields)).await?;
    let Some(found) = existing else {
        let mut d = doc! { "_id": ObjectId::new(), key: value };
        d.extend(fields);
        for f in model.defaults {
            d.insert(*f, now);
        }
        c.insert_one(d).await?;
        return Ok(());
    };
    let id = found.get("_id").cloned().unwrap_or(Bson::Null);
    let mut set = doc! { "_id": id.clone(), key: value };
    set.extend(fields);
    for f in model.defaults {
        let kept = match found.get(*f) {
            Some(Bson::DateTime(t)) if Some(*f) != model.stamp => Bson::DateTime(*t),
            _ => Bson::DateTime(now),
        };
        set.insert(*f, kept);
    }
    c.update_one(doc! { "_id": id }, doc! { "$set": set }).upsert(true).await?;
    Ok(())
}

/// `pyval::to_bson` for a list of upstream items.
pub fn items_bson(items: &[Value]) -> anyhow::Result<Bson> {
    Ok(Bson::Array(items.iter().map(pyval::to_bson).collect::<anyhow::Result<_>>()?))
}
