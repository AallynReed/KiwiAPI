//! Mongo reads and writes for the captured rotations, with the lax checks
//! Beanie's pydantic models apply when they load a document.

use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use mongodb::{
    Database,
    bson::{Bson, Document, doc},
    options::FindOptions,
};
use serde_json::{Map, Number, Value};

use super::{
    bodies::{ChallengeDoc, LuxionDoc},
    gamedata::{Invalid, Out},
};
use crate::{auth::pyutil::bson_int, dt};

pub const LUXION: &str = "luxion_appearances";
pub const CHAOS: &str = "chaos_chest_captures";
pub const CHALLENGE: &str = "challenge_captures";
pub const FEED_CACHE: &str = "feed_cache";
pub const DELVES: &str = "delve_rotations";

#[derive(Debug)]
pub enum StoreError {
    Mongo(mongodb::error::Error),
    Invalid,
}

impl From<mongodb::error::Error> for StoreError {
    fn from(e: mongodb::error::Error) -> Self {
        StoreError::Mongo(e)
    }
}

impl From<Invalid> for StoreError {
    fn from(_: Invalid) -> Self {
        StoreError::Invalid
    }
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::Mongo(e) => write!(f, "mongo: {e}"),
            StoreError::Invalid => f.write_str("stored document failed validation"),
        }
    }
}

impl std::error::Error for StoreError {}

pub type Res<T> = Result<T, StoreError>;

// --- pydantic (lax) field reads ---------------------------------------------

/// An `int` field: ints, bools and integral floats.
pub fn as_int(v: &Bson) -> Out<i64> {
    match v {
        Bson::Int32(i) => Ok(i64::from(*i)),
        Bson::Int64(i) => Ok(*i),
        Bson::Boolean(b) => Ok(i64::from(*b)),
        Bson::Double(f) if f.is_finite() && f.fract() == 0.0 && f.abs() < 9.2e18 => Ok(*f as i64),
        _ => Err(Invalid),
    }
}

pub fn int(d: &Document, key: &str) -> Out<i64> {
    d.get(key).ok_or(Invalid).and_then(as_int)
}

fn int_or(d: &Document, key: &str, default: i64) -> Out<i64> {
    d.get(key).map_or(Ok(default), as_int)
}

pub fn string(d: &Document, key: &str) -> Out<String> {
    match d.get(key) {
        Some(Bson::String(s)) => Ok(s.clone()),
        _ => Err(Invalid),
    }
}

fn boolean(d: &Document, key: &str) -> Out<bool> {
    match d.get(key) {
        Some(Bson::Boolean(b)) => Ok(*b),
        Some(v @ (Bson::Int32(_) | Bson::Int64(_) | Bson::Double(_))) => match as_int(v) {
            Ok(0) => Ok(false),
            Ok(1) => Ok(true),
            _ => Err(Invalid),
        },
        _ => Err(Invalid),
    }
}

/// A `datetime = Field(default_factory=utcnow)` field.
pub fn datetime(d: &Document, key: &str) -> Out<DateTime<Utc>> {
    match d.get(key) {
        None => Ok(dt::now()),
        Some(Bson::DateTime(b)) => Ok(dt::from_bson(*b)),
        _ => Err(Invalid),
    }
}

fn opt_datetime(d: &Document, key: &str) -> Out<Option<DateTime<Utc>>> {
    match d.get(key) {
        None | Some(Bson::Null) => Ok(None),
        Some(Bson::DateTime(b)) => Ok(Some(dt::from_bson(*b))),
        _ => Err(Invalid),
    }
}

/// A stored value as pydantic serializes it inside `dict` (`Any`): floats
/// stay floats, NaN/inf become null.
pub fn to_json(v: &Bson) -> Out {
    Ok(match v {
        Bson::Null | Bson::Undefined => Value::Null,
        Bson::Boolean(b) => Value::Bool(*b),
        Bson::Int32(i) => Value::from(*i),
        Bson::Int64(i) => Value::from(*i),
        Bson::Double(f) => Number::from_f64(*f).map_or(Value::Null, Value::Number),
        Bson::String(s) => Value::String(s.clone()),
        Bson::DateTime(b) => Value::String(dt::pydantic(&dt::from_bson(*b))),
        Bson::Array(items) => Value::Array(items.iter().map(to_json).collect::<Out<_>>()?),
        Bson::Document(d) => Value::Object(doc_to_json(d)?),
        _ => return Err(Invalid),
    })
}

pub fn doc_to_json(d: &Document) -> Out<Map<String, Value>> {
    d.iter().map(|(k, v)| Ok((k.clone(), to_json(v)?))).collect()
}

/// A Python object from `json.loads` as pymongo encodes it. Ints past int64
/// (which pymongo refuses) are an error.
pub fn to_bson(v: &Value) -> Result<Bson, Invalid> {
    Ok(match v {
        Value::Null => Bson::Null,
        Value::Bool(b) => Bson::Boolean(*b),
        Value::Number(n) => match (n.as_i64(), n.as_f64()) {
            (Some(i), _) => bson_int(i),
            (None, Some(_)) if n.is_u64() => return Err(Invalid),
            (None, Some(f)) => Bson::Double(f),
            (None, None) => return Err(Invalid),
        },
        Value::String(s) => Bson::String(s.clone()),
        Value::Array(items) => Bson::Array(items.iter().map(to_bson).collect::<Result<_, _>>()?),
        Value::Object(m) => Bson::Document(map_to_doc(m)?),
    })
}

pub fn map_to_doc(m: &Map<String, Value>) -> Result<Document, Invalid> {
    let mut d = Document::new();
    for (k, v) in m {
        d.insert(k.clone(), to_bson(v)?);
    }
    Ok(d)
}

// --- Luxion -------------------------------------------------------------------

pub fn luxion_doc(d: &Document) -> Out<LuxionDoc> {
    Ok(LuxionDoc {
        started_at: int(d, "started_at")?,
        first_seen_at: datetime(d, "first_seen_at")?,
        last_seen_at: datetime(d, "last_seen_at")?,
        ended_at: opt_datetime(d, "ended_at")?,
    })
}

fn newest_first(key: &str) -> Document {
    doc! { key: -1 }
}

pub async fn latest_luxion(db: &Database) -> Res<Option<LuxionDoc>> {
    let found = db.collection::<Document>(LUXION).find_one(doc! {}).sort(newest_first("started_at")).await?;
    Ok(found.as_ref().map(luxion_doc).transpose()?)
}

pub async fn luxion_runs(db: &Database) -> Res<Vec<LuxionDoc>> {
    let docs: Vec<Document> =
        db.collection::<Document>(LUXION).find(doc! {}).sort(newest_first("started_at")).await?.try_collect().await?;
    Ok(docs.iter().map(luxion_doc).collect::<Out<_>>()?)
}

/// `paginate(Model.find_all(), sort="-<key>", limit, offset)`: `(docs, total)`.
async fn paginate(db: &Database, coll: &str, key: &str, limit: i64, offset: i64) -> Res<(Vec<Document>, u64)> {
    let coll = db.collection::<Document>(coll);
    let total = coll.count_documents(doc! {}).await?;
    let opts = FindOptions::builder().sort(newest_first(key)).skip(offset as u64).limit(limit).build();
    let docs = coll.find(doc! {}).with_options(opts).await?.try_collect().await?;
    Ok((docs, total))
}

pub async fn luxion_history(db: &Database, limit: i64, offset: i64) -> Res<(Vec<LuxionDoc>, u64)> {
    let (docs, total) = paginate(db, LUXION, "started_at", limit, offset).await?;
    Ok((docs.iter().map(luxion_doc).collect::<Out<_>>()?, total))
}

// --- chaos chest ----------------------------------------------------------------

pub struct ChaosCapture {
    pub week_anchor: i64,
    pub name: String,
    pub captured_at: DateTime<Utc>,
}

pub fn chaos_doc(d: &Document) -> Out<ChaosCapture> {
    Ok(ChaosCapture {
        week_anchor: int(d, "week_anchor")?,
        name: string(d, "name")?,
        captured_at: datetime(d, "captured_at")?,
    })
}

pub async fn chaos_for_week(db: &Database, week_anchor: i64) -> Res<Option<ChaosCapture>> {
    let found = db.collection::<Document>(CHAOS).find_one(doc! { "week_anchor": week_anchor }).await?;
    Ok(found.as_ref().map(chaos_doc).transpose()?)
}

pub async fn chaos_history(db: &Database, limit: i64, offset: i64) -> Res<(Vec<ChaosCapture>, u64)> {
    let (docs, total) = paginate(db, CHAOS, "week_anchor", limit, offset).await?;
    Ok((docs.iter().map(chaos_doc).collect::<Out<_>>()?, total))
}

/// The relayed chaos chest: `FeedCache(feed="chaos_chest")`'s first item and
/// `fetched_at`. `None` without the document.
pub async fn chaos_feed(db: &Database) -> Res<Option<(Option<Map<String, Value>>, DateTime<Utc>)>> {
    let Some(d) = db.collection::<Document>(FEED_CACHE).find_one(doc! { "feed": super::chaos::FEED }).await? else {
        return Ok(None);
    };
    let items = match d.get("items") {
        None => Vec::new(),
        Some(Bson::Array(items)) => items.clone(),
        Some(_) => return Err(StoreError::Invalid),
    };
    let mut first = None;
    for (i, item) in items.iter().enumerate() {
        let Bson::Document(item) = item else { return Err(StoreError::Invalid) };
        if i == 0 {
            first = Some(doc_to_json(item)?);
        }
    }
    Ok(Some((first, datetime(&d, "fetched_at")?)))
}

// --- challenge ----------------------------------------------------------------

pub fn challenge_doc(d: &Document) -> Out<ChallengeDoc> {
    Ok(ChallengeDoc {
        window_anchor: int(d, "window_anchor")?,
        window_ends_at: int(d, "window_ends_at")?,
        name: string(d, "name")?,
        is_friday_window: boolean(d, "is_friday_window")?,
        captured_at: datetime(d, "captured_at")?,
    })
}

pub async fn challenge_for_window(db: &Database, window_anchor: i64) -> Res<Option<ChallengeDoc>> {
    let found = db.collection::<Document>(CHALLENGE).find_one(doc! { "window_anchor": window_anchor }).await?;
    Ok(found.as_ref().map(challenge_doc).transpose()?)
}

pub async fn challenge_history(db: &Database, limit: i64, offset: i64) -> Res<(Vec<ChallengeDoc>, u64)> {
    let (docs, total) = paginate(db, CHALLENGE, "window_anchor", limit, offset).await?;
    Ok((docs.iter().map(challenge_doc).collect::<Out<_>>()?, total))
}

// --- delves -------------------------------------------------------------------

pub struct DelveDoc {
    pub id: Option<Bson>,
    pub week: i64,
    pub depths: Vec<Map<String, Value>>,
    pub total: i64,
    pub depth_count: i64,
    pub fetched_at: DateTime<Utc>,
}

fn delve_doc(d: &Document) -> Out<DelveDoc> {
    let depths = match d.get("depths") {
        None => Vec::new(),
        Some(Bson::Array(items)) => items
            .iter()
            .map(|v| match v {
                Bson::Document(m) => doc_to_json(m),
                _ => Err(Invalid),
            })
            .collect::<Out<_>>()?,
        Some(_) => return Err(Invalid),
    };
    Ok(DelveDoc {
        id: d.get("_id").cloned(),
        week: int(d, "week")?,
        depths,
        total: int_or(d, "total", 0)?,
        depth_count: int_or(d, "depth_count", 0)?,
        fetched_at: datetime(d, "fetched_at")?,
    })
}

pub async fn delve_week(db: &Database, week: i64) -> Res<Option<DelveDoc>> {
    let found = db.collection::<Document>(DELVES).find_one(doc! { "week": week }).await?;
    Ok(found.as_ref().map(delve_doc).transpose()?)
}

/// `delves.list_weeks`: `(week, total, count, fetched_at)`, newest first,
/// without loading the depth lists.
pub async fn delve_weeks(db: &Database) -> Res<Vec<(i64, i64, i64, DateTime<Utc>)>> {
    let projection = doc! { "week": 1, "total": 1, "depth_count": 1, "fetched_at": 1, "_id": 0 };
    let opts = FindOptions::builder().projection(projection).sort(doc! { "week": -1 }).build();
    let rows: Vec<Document> =
        db.collection::<Document>(DELVES).find(doc! {}).with_options(opts).await?.try_collect().await?;
    let row = |r: &Document| -> Out<_> {
        let fetched_at = match r.get("fetched_at") {
            Some(Bson::DateTime(b)) => dt::from_bson(*b),
            _ => return Err(Invalid),
        };
        Ok((int(r, "week")?, int_or(r, "total", 0)?, int_or(r, "depth_count", 0)?, fetched_at))
    };
    Ok(rows.iter().map(row).collect::<Out<_>>()?)
}

/// `delves.store_week`'s write: Beanie's insert, or its `save()` (`$set` of
/// every field) on the existing document.
pub async fn save_delve_week(
    db: &Database,
    existing: Option<&DelveDoc>,
    week: i64,
    depths: Vec<Bson>,
    total: Bson,
) -> Res<()> {
    let count = depths.len() as i64;
    let fields = doc! {
        "week": bson_int(week),
        "depths": depths,
        "total": total,
        "depth_count": bson_int(count),
        "fetched_at": dt::to_bson(dt::now()),
    };
    let coll = db.collection::<Document>(DELVES);
    match existing.and_then(|e| e.id.clone()) {
        None => {
            coll.insert_one(fields).await?;
        }
        Some(id) => {
            coll.update_one(doc! { "_id": id }, doc! { "$set": fields }).upsert(true).await?;
        }
    }
    Ok(())
}

/// `refresh_chaos_chest`'s write to `FeedCache(feed="chaos_chest")`.
pub async fn save_chaos_feed(db: &Database, item: &Map<String, Value>) -> Res<()> {
    let coll = db.collection::<Document>(FEED_CACHE);
    let existing = coll.find_one(doc! { "feed": super::chaos::FEED }).await?;
    let fields = doc! {
        "feed": super::chaos::FEED,
        "items": [Bson::Document(map_to_doc(item)?)],
        "fetched_at": dt::to_bson(dt::now()),
    };
    match existing.and_then(|d| d.get("_id").cloned()) {
        None => {
            coll.insert_one(fields).await?;
        }
        Some(id) => {
            coll.update_one(doc! { "_id": id }, doc! { "$set": fields }).upsert(true).await?;
        }
    }
    Ok(())
}
