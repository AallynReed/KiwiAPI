//! An in-memory Mongo with the semantics the golden generator's fake pymongo
//! layer has: insertion order, stable sorts, `$set` adding new fields in
//! lexicographic order (MongoDB 5+), int32 `$inc`.

use std::{
    cmp::Ordering,
    collections::HashMap,
    sync::{
        Mutex,
        atomic::{AtomicU32, Ordering as AtomicOrdering},
    },
};

use chrono::{DateTime, Utc};
use mongodb::bson::{Bson, Document, oid::ObjectId};

use crate::auth::Rejection;

pub struct MemDb {
    pub colls: Mutex<HashMap<String, Vec<Document>>>,
    pub now: DateTime<Utc>,
    pub minted: String,
    pub master: String,
    ids: AtomicU32,
    pub limits: Mutex<Vec<String>>,
    /// Rate-limit keys that answer 429.
    pub deny: Vec<String>,
}

impl MemDb {
    pub fn new(colls: HashMap<String, Vec<Document>>, now: DateTime<Utc>, minted: &str) -> Self {
        MemDb {
            colls: Mutex::new(colls),
            now,
            minted: minted.to_owned(),
            master: "aallyn".into(),
            ids: AtomicU32::new(0),
            limits: Mutex::new(Vec::new()),
            deny: Vec::new(),
        }
    }

    pub fn snapshot(&self, coll: &str) -> Vec<Document> {
        self.colls.lock().unwrap().get(coll).cloned().unwrap_or_default()
    }

    pub fn find(&self, coll: &str, filter: &Document, sort: Option<&Document>, skip: u64, limit: i64) -> Vec<Document> {
        let colls = self.colls.lock().unwrap();
        let mut out: Vec<Document> =
            colls.get(coll).map(|v| v.iter().filter(|d| matches(d, filter)).cloned().collect()).unwrap_or_default();
        if let Some(sort) = sort {
            out.sort_by(|a, b| {
                for (k, dir) in sort {
                    let o = cmp(a.get(k), b.get(k));
                    let o = if dir.as_i32().unwrap_or(1) < 0 { o.reverse() } else { o };
                    if o != Ordering::Equal {
                        return o;
                    }
                }
                Ordering::Equal
            });
        }
        let out = out.into_iter().skip(skip as usize);
        if limit > 0 { out.take(limit as usize).collect() } else { out.collect() }
    }

    pub fn insert(&self, coll: &str, doc: Document) -> ObjectId {
        let n = self.ids.fetch_add(1, AtomicOrdering::SeqCst) + 1;
        let id = ObjectId::parse_str(format!("66000000000000000000{n:04x}")).unwrap();
        self.colls.lock().unwrap().entry(coll.to_owned()).or_default().push(crate::outbound::store::with_id(id, doc));
        id
    }

    /// Applies `update` to the first (or every) match; returns the updated documents.
    pub fn update(&self, coll: &str, filter: &Document, update: &Document, many: bool) -> Vec<Document> {
        let mut colls = self.colls.lock().unwrap();
        let docs = colls.entry(coll.to_owned()).or_default();
        let mut out = Vec::new();
        for d in docs.iter_mut().filter(|d| matches(d, filter)) {
            apply(d, update);
            out.push(d.clone());
            if !many {
                break;
            }
        }
        if out.is_empty()
            && !many
            && let Some(Bson::Document(set)) = update.get("$set")
            && set.contains_key("_id")
        {
            let mut d = Document::new();
            apply(&mut d, update);
            docs.push(d);
        }
        out
    }

    pub fn delete(&self, coll: &str, filter: &Document, many: bool) {
        let mut colls = self.colls.lock().unwrap();
        let Some(docs) = colls.get_mut(coll) else { return };
        if many {
            docs.retain(|d| !matches(d, filter));
        } else if let Some(i) = docs.iter().position(|d| matches(d, filter)) {
            docs.remove(i);
        }
    }

    pub fn limit(&self, _prefix: &str, key: &str) -> Result<(), Rejection> {
        self.limits.lock().unwrap().push(key.to_owned());
        if self.deny.iter().any(|k| k == key) {
            let info = crate::ratelimit::RateLimitInfo { limit: 1, remaining: 0, reset: 0 };
            return Err(crate::ratelimit::too_many(&info, 3600, 1));
        }
        Ok(())
    }
}

fn matches(d: &Document, filter: &Document) -> bool {
    filter.iter().all(|(k, cond)| match k.as_str() {
        "$and" => cond.as_array().is_some_and(|a| a.iter().all(|c| c.as_document().is_some_and(|c| matches(d, c)))),
        _ => field_matches(d.get(k), cond),
    })
}

fn field_matches(v: Option<&Bson>, cond: &Bson) -> bool {
    if let Bson::Document(ops) = cond
        && !ops.is_empty()
        && ops.keys().all(|k| k.starts_with('$'))
    {
        return ops.iter().all(|(op, arg)| match op.as_str() {
            "$in" => arg.as_array().is_some_and(|a| a.iter().any(|x| eq(v, x))),
            "$ne" => !eq(v, arg),
            other => panic!("unsupported operator {other}"),
        });
    }
    eq(v, cond)
}

fn num(b: &Bson) -> Option<f64> {
    match b {
        Bson::Int32(i) => Some(f64::from(*i)),
        Bson::Int64(i) => Some(*i as f64),
        Bson::Double(f) => Some(*f),
        _ => None,
    }
}

fn same(a: &Bson, b: &Bson) -> bool {
    match (num(a), num(b)) {
        (Some(x), Some(y)) => x == y,
        _ => a == b,
    }
}

fn eq(v: Option<&Bson>, want: &Bson) -> bool {
    match v {
        Some(Bson::Array(a)) => a.iter().any(|x| same(x, want)) || matches!(want, Bson::Array(w) if w == a),
        Some(Bson::Null) | None => matches!(want, Bson::Null),
        Some(x) => same(x, want),
    }
}

fn rank(b: Option<&Bson>) -> u8 {
    match b {
        None | Some(Bson::Null) => 0,
        Some(Bson::Int32(_) | Bson::Int64(_) | Bson::Double(_)) => 1,
        Some(Bson::String(_)) => 2,
        Some(Bson::Document(_)) => 3,
        Some(Bson::Array(_)) => 4,
        Some(Bson::ObjectId(_)) => 6,
        Some(Bson::Boolean(_)) => 7,
        Some(Bson::DateTime(_)) => 8,
        Some(_) => 9,
    }
}

fn cmp(a: Option<&Bson>, b: Option<&Bson>) -> Ordering {
    let r = rank(a).cmp(&rank(b));
    if r != Ordering::Equal {
        return r;
    }
    match (a, b) {
        (Some(Bson::String(x)), Some(Bson::String(y))) => x.as_bytes().cmp(y.as_bytes()),
        (Some(Bson::DateTime(x)), Some(Bson::DateTime(y))) => x.cmp(y),
        (Some(Bson::ObjectId(x)), Some(Bson::ObjectId(y))) => x.bytes().cmp(&y.bytes()),
        (Some(Bson::Boolean(x)), Some(Bson::Boolean(y))) => x.cmp(y),
        (Some(x), Some(y)) => num(x).partial_cmp(&num(y)).unwrap_or(Ordering::Equal),
        _ => Ordering::Equal,
    }
}

fn apply(d: &mut Document, update: &Document) {
    let mut ops: Vec<(&String, &str, &Bson)> = Vec::new();
    for (op, fields) in update {
        if let Bson::Document(fields) = fields {
            ops.extend(fields.iter().map(|(k, v)| (k, op.as_str(), v)));
        }
    }
    ops.sort_by(|a, b| a.0.cmp(b.0));
    for (k, op, v) in ops {
        match op {
            "$set" => {
                d.insert(k.clone(), v.clone());
            }
            "$inc" => {
                let new = match (d.get(k), v) {
                    (None | Some(Bson::Null), v) => v.clone(),
                    (Some(Bson::Int32(a)), Bson::Int32(b)) => {
                        a.checked_add(*b).map_or(Bson::Int64(i64::from(*a) + i64::from(*b)), Bson::Int32)
                    }
                    (Some(a @ Bson::Double(_)), b) | (Some(a), b @ Bson::Double(_)) => {
                        Bson::Double(num(a).unwrap_or(0.0) + num(b).unwrap_or(0.0))
                    }
                    (Some(a), b) => Bson::Int64(num(a).unwrap_or(0.0) as i64 + num(b).unwrap_or(0.0) as i64),
                };
                d.insert(k.clone(), new);
            }
            "$addToSet" => match d.get_mut(k) {
                Some(Bson::Array(a)) => {
                    if !a.iter().any(|x| same(x, v)) {
                        a.push(v.clone());
                    }
                }
                _ => {
                    d.insert(k.clone(), Bson::Array(vec![v.clone()]));
                }
            },
            other => panic!("unsupported update operator {other}"),
        }
    }
}
