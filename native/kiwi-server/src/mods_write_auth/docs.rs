//! Reading Beanie documents with their model defaults, and writing values the
//! way pymongo encodes them.

use chrono::{DateTime, Utc};
use mongodb::bson::{Bson, Document, oid::ObjectId};

use crate::dt;

pub fn read_str(d: &Document, key: &str) -> String {
    d.get_str(key).unwrap_or_default().to_owned()
}

pub fn read_opt_str(d: &Document, key: &str) -> Option<String> {
    d.get_str(key).ok().map(str::to_owned)
}

pub fn read_bool(d: &Document, key: &str, default: bool) -> bool {
    d.get_bool(key).unwrap_or(default)
}

pub fn read_int(d: &Document, key: &str, default: i64) -> i64 {
    match d.get(key) {
        Some(Bson::Int32(i)) => i64::from(*i),
        Some(Bson::Int64(i)) => *i,
        Some(Bson::Double(f)) if f.fract() == 0.0 => *f as i64,
        _ => default,
    }
}

pub fn read_oid(d: &Document, key: &str) -> Option<ObjectId> {
    d.get_object_id(key).ok()
}

pub fn read_ids(d: &Document, key: &str) -> Vec<ObjectId> {
    d.get_array(key).map(|a| a.iter().filter_map(Bson::as_object_id).collect()).unwrap_or_default()
}

pub fn read_opt_date(d: &Document, key: &str) -> Option<DateTime<Utc>> {
    d.get_datetime(key).ok().map(|t| dt::from_bson(*t))
}

/// A `default_factory=utcnow` field; a row missing it reads as now.
pub fn read_date(d: &Document, key: &str) -> DateTime<Utc> {
    read_opt_date(d, key).unwrap_or_else(dt::now)
}

pub fn date(t: DateTime<Utc>) -> Bson {
    Bson::DateTime(dt::to_bson(t))
}

pub fn opt_date(t: Option<DateTime<Utc>>) -> Bson {
    t.map_or(Bson::Null, date)
}

pub fn opt_oid(id: Option<ObjectId>) -> Bson {
    id.map_or(Bson::Null, Bson::ObjectId)
}

pub fn int(n: i64) -> Bson {
    crate::auth::pyutil::bson_int(n)
}

/// `iso(dt)`.
pub fn iso(t: Option<DateTime<Utc>>) -> serde_json::Value {
    t.map_or(serde_json::Value::Null, |t| serde_json::Value::String(dt::isoformat(&t)))
}

/// `to_oid(value)`: `None` for anything `PydanticObjectId` rejects.
pub fn to_oid(value: &str) -> Option<ObjectId> {
    ObjectId::parse_str(value).ok()
}
