//! `POST /internal/pageviews`: batches of page views and their time and perf
//! beacons from the website container. Views are inserted (a replayed `pid`
//! hits the unique index and is ignored), then each page load's beacons
//! become one update. Storage failures are logged; the reply is always 202.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use mongodb::{
    Database,
    bson::{Bson, Document, doc},
    error::ErrorKind,
};
use serde_json::json;

use super::{InternalKey, internal_key_ok, not_found};
use crate::{
    auth::pyutil::bson_int,
    dt,
    pyjson::PyJson,
    state::AppState,
    validation::{Checked, Field, FromBody},
};

pub const PAGE_VIEWS: &str = "page_views";
const KINDS: &[&str] = &["pv", "end", "perf"];

/// `PagePerf`: load timings in ms, CLS x1000.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Perf {
    pub ttfb: Option<i64>,
    pub fcp: Option<i64>,
    pub lcp: Option<i64>,
    pub dcl: Option<i64>,
    pub load: Option<i64>,
    pub cls: Option<i64>,
}

impl Perf {
    /// `model_dump(exclude_none=True)`, in field order.
    pub fn present(&self) -> Vec<(&'static str, i64)> {
        [
            ("ttfb", self.ttfb),
            ("fcp", self.fcp),
            ("lcp", self.lcp),
            ("dcl", self.dcl),
            ("load", self.load),
            ("cls", self.cls),
        ]
        .into_iter()
        .filter_map(|(k, v)| v.map(|v| (k, v)))
        .collect()
    }
}

/// `BeaconEvent`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Event {
    pub kind: String,
    pub pid: Option<String>,
    pub route: String,
    pub path: String,
    pub visitor_hash: String,
    pub engaged_s: i64,
    pub channel: Option<String>,
    pub source: Option<String>,
    pub device: Option<String>,
    pub browser: Option<String>,
    pub os: Option<String>,
    pub lang: Option<String>,
    pub perf: Option<Perf>,
}

/// `BeaconBatch`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Batch {
    pub events: Vec<Event>,
}

fn label(f: Field<'_>) -> Option<String> {
    f.str().max_len(80).optional()
}

fn timing(f: Field<'_>, max: i64) -> Option<i64> {
    f.int().ge(0).le(max).optional()
}

fn event(f: Field<'_>) -> Event {
    f.model(|m| Event {
        kind: m.field("kind").literal(KINDS).default("pv".into()),
        pid: m.field("pid").str().max_len(40).optional(),
        route: m.field("route").str().max_len(200).default(String::new()),
        path: m.field("path").str().max_len(500).default(String::new()),
        visitor_hash: m.field("visitor_hash").str().max_len(64).default(String::new()),
        engaged_s: m.field("engaged_s").int().ge(0).le(86_400).default(0),
        channel: label(m.field("channel")),
        source: label(m.field("source")),
        device: label(m.field("device")),
        browser: label(m.field("browser")),
        os: label(m.field("os")),
        lang: label(m.field("lang")),
        perf: m
            .field("perf")
            .model(|p| Perf {
                ttfb: timing(p.field("ttfb"), 120_000),
                fcp: timing(p.field("fcp"), 120_000),
                lcp: timing(p.field("lcp"), 120_000),
                dcl: timing(p.field("dcl"), 120_000),
                load: timing(p.field("load"), 120_000),
                cls: timing(p.field("cls"), 10_000),
            })
            .optional(),
    })
    .required()
}

impl FromBody for Batch {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(Batch { events: m.field("events").list(event).max_len(500).required() })).required()
    }
}

fn opt(v: &Option<String>) -> Bson {
    v.as_ref().map_or(Bson::Null, |s| Bson::String(s.clone()))
}

/// The page-view documents a batch inserts (`record_batch`'s `views`).
pub fn view_docs(events: &[Event], now: DateTime<Utc>) -> Vec<Document> {
    let now = dt::to_bson(now);
    events
        .iter()
        .filter(|e| e.kind == "pv" && !e.route.is_empty() && !e.path.is_empty() && e.visitor_hash.chars().count() >= 8)
        .map(|e| {
            doc! {
                "pid": opt(&e.pid),
                "route": &e.route,
                "path": &e.path,
                "visitor_hash": &e.visitor_hash,
                "engaged_s": 0_i32,
                "channel": opt(&e.channel),
                "source": opt(&e.source),
                "device": opt(&e.device),
                "browser": opt(&e.browser),
                "os": opt(&e.os),
                "lang": opt(&e.lang),
                "created_at": now,
                "last_at": now,
            }
        })
        .collect()
}

fn as_i64(b: Option<&Bson>) -> i64 {
    match b {
        Some(Bson::Int32(n)) => i64::from(*n),
        Some(Bson::Int64(n)) => *n,
        _ => 0,
    }
}

/// `_updates`: one update per page load, in first-seen order.
pub fn updates(events: &[Event], now: DateTime<Utc>) -> Vec<(String, Document)> {
    let mut out: Vec<(String, Document, Document)> = Vec::new();
    for e in events {
        let Some(pid) = e.pid.as_ref().filter(|p| !p.is_empty()) else { continue };
        if e.kind == "pv" {
            continue;
        }
        let i = match out.iter().position(|(p, _, _)| p == pid) {
            Some(i) => i,
            None => {
                out.push((pid.clone(), doc! {"last_at": dt::to_bson(now)}, Document::new()));
                out.len() - 1
            }
        };
        let (_, set, grow) = &mut out[i];
        if e.kind == "end" {
            let v = as_i64(grow.get("engaged_s")).max(e.engaged_s);
            grow.insert("engaged_s", bson_int(v));
        } else if let Some(perf) = &e.perf {
            for (k, v) in perf.present() {
                if k == "cls" {
                    let v = as_i64(grow.get("perf.cls")).max(v);
                    grow.insert("perf.cls", bson_int(v));
                } else {
                    set.insert(format!("perf.{k}"), bson_int(v));
                }
            }
        }
    }
    out.into_iter()
        .map(|(pid, set, grow)| {
            let mut op = doc! {"$set": set};
            if !grow.is_empty() {
                op.insert("$max", grow);
            }
            (pid, op)
        })
        .collect()
}

fn only_duplicates(err: &mongodb::error::Error) -> bool {
    match err.kind.as_ref() {
        ErrorKind::InsertMany(e) => e.write_errors.as_ref().is_none_or(|w| w.iter().all(|we| we.code == 11000)),
        _ => false,
    }
}

/// `record_batch`.
pub async fn record_batch(db: &Database, events: &[Event]) -> anyhow::Result<()> {
    let now = dt::now();
    let coll = db.collection::<Document>(PAGE_VIEWS);
    let views = view_docs(events, now);
    if !views.is_empty()
        && let Err(err) = coll.insert_many(views).ordered(false).await
        && !only_duplicates(&err)
    {
        return Err(err.into());
    }
    let ops = updates(events, now);
    let failures: Vec<String> = futures::stream::iter(ops)
        .map(|(pid, op)| {
            let coll = coll.clone();
            async move { coll.update_one(doc! {"pid": pid}, op).await.err().map(|e| e.to_string()) }
        })
        .buffer_unordered(16)
        .filter_map(|e| async move { e })
        .collect()
        .await;
    if let Some(first) = failures.first() {
        anyhow::bail!("{} page-view update(s) failed, first: {first}", failures.len());
    }
    Ok(())
}

pub async fn ingest(
    State(state): State<AppState>,
    Checked(InternalKey(supplied), batch): Checked<InternalKey, Batch>,
) -> Response {
    match internal_key_ok(&state.settings().internal_key, &supplied) {
        Ok(true) => {}
        Ok(false) => return not_found().into_response(),
        Err(e) => return e.into_response(),
    }
    let recorded = match state.inner.db.mongo.as_ref() {
        Some(db) => record_batch(db, &batch.events).await,
        None => Err(anyhow::anyhow!("MONGO_URI not configured")),
    };
    if let Err(err) = recorded {
        tracing::error!("page-view batch failed: {err:#}");
    }
    (StatusCode::ACCEPTED, PyJson(json!({"accepted": batch.events.len()}))).into_response()
}
