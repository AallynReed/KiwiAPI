//! `/admin/pageviews` and `/admin/pageviews/live` (`app/pageviews/service.py`):
//! the Site Analytics report over `page_views`, visits built at query time.
//! The pipelines are Python's, stage for stage and key for key.

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::{Duration as StdDuration, Instant},
};

use axum::extract::State;
use chrono::{DateTime, Duration, DurationRound, Utc};
use futures::TryStreamExt;
use mongodb::bson::{Bson, Document, doc};
use serde_json::{Map, Value, json};
use tokio::sync::Semaphore;

use super::{Args, Res, coll, internal};
use crate::{
    auth::Superuser,
    dt,
    error::ApiError,
    lb_ingest::admin::bson_json,
    pyfloat,
    pyjson::PydanticJson,
    state::AppState,
    validation::{FromParams, Params},
};

pub const PAGE_VIEWS: &str = "page_views";
const VISIT_GAP_MS: i32 = 30 * 60 * 1000;
const BOUNCE_ENGAGED_S: i32 = 10;
pub const VISIT_FILTERS: [&str; 7] = ["channel", "source", "device", "browser", "os", "lang", "entry"];
const ENG_BOUNDS: [i32; 11] = [1, 2, 4, 8, 15, 30, 60, 120, 240, 480, 960];
const ENG_SPLITS: [&str; 2] = ["channel", "device"];
const SPEED_BOUNDS: [i32; 5] = [1000, 2000, 3000, 5000, 8000];
const SPEED_SPLITS: [&str; 3] = ["device", "browser", "entry"];
const SLOW_MS: i32 = 3000;
const CLS_POOR: i32 = 250;
const BREAKDOWNS: [&str; 10] =
    ["channels", "sources", "entries", "exits", "devices", "browsers", "os", "langs", "pages", "sections"];

// --- query -------------------------------------------------------------------------

/// `site_pageviews`' query: `days` plus the visit filters (with their max lengths).
pub struct ReportQuery {
    pub days: i64,
    pub filters: Vec<(&'static str, Option<String>)>,
}

impl FromParams for ReportQuery {
    fn from_params(p: &mut Params<'_>) -> Self {
        let days = p.query("days").int().ge(1).le(90).default(30);
        let limits = [40, 80, 40, 40, 40, 10, 500];
        let filters =
            VISIT_FILTERS.iter().zip(limits).map(|(&k, n)| (k, p.query(k).str().max_len(n).optional())).collect();
        ReportQuery { days, filters }
    }
}

impl ReportQuery {
    /// `{k: v for k, v in filters.items() if k in VISIT_FILTERS and v}`.
    pub fn chosen(&self) -> Vec<(&'static str, String)> {
        self.filters.iter().filter_map(|(k, v)| v.clone().filter(|v| !v.is_empty()).map(|v| (*k, v))).collect()
    }
}

// --- pipeline pieces ---------------------------------------------------------------

fn tracked() -> Document {
    doc! {"pid": {"$type": "string"}}
}

fn eng_band() -> Document {
    let branches: Vec<Document> =
        ENG_BOUNDS.iter().enumerate().map(|(i, b)| doc! {"case": {"$lt": ["$engaged", b]}, "then": i as i32}).collect();
    doc! {"$switch": {"branches": branches, "default": ENG_BOUNDS.len() as i32}}
}

fn eng_median() -> Document {
    doc! {"$median": {"input": {"$cond": [{"$gt": ["$engaged", 0]}, "$engaged", Bson::Null]}, "method": "approximate"}}
}

fn vis() -> Document {
    doc! {"$cond": [{"$gt": ["$perf.lcp", 0]}, "$perf.lcp", {"$cond": [{"$gt": ["$perf.fcp", 0]}, "$perf.fcp", Bson::Null]}]}
}

fn speed_band() -> Document {
    let mut branches = vec![doc! {"case": {"$eq": [vis(), Bson::Null]}, "then": SPEED_BOUNDS.len() as i32 + 1}];
    branches.extend(SPEED_BOUNDS.iter().enumerate().map(|(i, b)| doc! {"case": {"$lt": [vis(), b]}, "then": i as i32}));
    doc! {"$switch": {"branches": branches, "default": SPEED_BOUNDS.len() as i32}}
}

fn has_perf() -> Document {
    doc! {"$match": {"perf": {"$type": "object"}}}
}

pub struct Window {
    pub since: DateTime<Utc>,
    pub until: DateTime<Utc>,
    pub hourly: bool,
    pub labels: Vec<String>,
}

impl Window {
    pub fn granularity(&self) -> &'static str {
        if self.hourly { "hour" } else { "day" }
    }
}

/// `window(days, now)`.
pub fn window(days: i64, now: DateTime<Utc>) -> Window {
    if days <= 1 {
        let since = (now - Duration::hours(23)).duration_trunc(Duration::hours(1)).unwrap_or(now);
        let labels = (0..24).map(|i| (since + Duration::hours(i)).format("%Y-%m-%dT%H:00").to_string()).collect();
        return Window { since, until: now, hourly: true, labels };
    }
    let today = now.duration_trunc(Duration::days(1)).unwrap_or(now);
    let since = today - Duration::days(days - 1);
    let labels = (0..days).map(|i| (since + Duration::days(i)).format("%Y-%m-%d").to_string()).collect();
    Window { since, until: now, hourly: false, labels }
}

fn bucket(field: &str, hourly: bool) -> Document {
    let fmt = if hourly { "%Y-%m-%dT%H:00" } else { "%Y-%m-%d" };
    doc! {"$dateToString": {"date": format!("${field}"), "format": fmt, "timezone": "UTC"}}
}

/// `visits_pipeline(since, until)`.
pub fn visits_pipeline(since: DateTime<Utc>, until: DateTime<Utc>) -> Vec<Document> {
    let mut m = tracked();
    m.insert("created_at", doc! {"$gte": dt::to_bson(since), "$lt": dt::to_bson(until)});
    let by_time = || doc! {"partitionBy": "$visitor_hash", "sortBy": {"created_at": 1}};
    let mut prev = by_time();
    prev.insert("output", doc! {"_prev": {"$shift": {"output": "$created_at", "by": -1}}});
    let mut visit = by_time();
    visit.insert("output", doc! {"_visit": {"$sum": "$_new", "window": {"documents": ["unbounded", "current"]}}});
    vec![
        doc! {"$match": m},
        doc! {"$setWindowFields": prev},
        doc! {"$set": {"_new": {"$cond": [{"$or": [
            {"$eq": ["$_prev", Bson::Null]},
            {"$gt": [{"$subtract": ["$created_at", "$_prev"]}, VISIT_GAP_MS]},
        ]}, 1, 0]}}},
        doc! {"$setWindowFields": visit},
        doc! {"$sort": {"visitor_hash": 1, "created_at": 1}},
        doc! {"$group": {
            "_id": {"v": "$visitor_hash", "n": "$_visit"},
            "start": {"$first": "$created_at"},
            "pages": {"$sum": 1},
            "engaged": {"$sum": "$engaged_s"},
            "entry": {"$first": "$path"},
            "exit": {"$last": "$path"},
            "channel": {"$first": "$channel"},
            "source": {"$first": "$source"},
            "device": {"$first": "$device"},
            "browser": {"$first": "$browser"},
            "os": {"$first": "$os"},
            "lang": {"$first": "$lang"},
            "perf": {"$first": "$perf"},
            "hits": {"$push": {"p": "$path", "r": "$route", "e": "$engaged_s"}},
        }},
        doc! {"$set": {
            "channel": {"$cond": [{"$eq": ["$channel", "internal"]}, "direct", "$channel"]},
            "bounce": {"$cond": [{"$and": [
                {"$lte": ["$pages", 1]}, {"$lt": ["$engaged", BOUNCE_ENGAGED_S]}]}, 1, 0]},
        }},
    ]
}

fn visit_dim(key: impl Into<Bson>, limit: Option<i32>) -> Vec<Document> {
    let mut stages = vec![
        doc! {"$group": {"_id": {"k": key.into(), "v": "$_id.v"}, "visits": {"$sum": 1},
        "pages": {"$sum": "$pages"}, "bounces": {"$sum": "$bounce"},
        "engaged": {"$sum": "$engaged"}}},
        doc! {"$group": {"_id": "$_id.k", "visitors": {"$sum": 1}, "visits": {"$sum": "$visits"},
        "pages": {"$sum": "$pages"}, "bounces": {"$sum": "$bounces"},
        "engaged": {"$sum": "$engaged"}}},
        doc! {"$sort": {"visits": -1, "_id": 1}},
    ];
    if let Some(n) = limit {
        stages.push(doc! {"$limit": n});
    }
    stages
}

fn hit_dim(field: &str, limit: i32) -> Vec<Document> {
    vec![
        doc! {"$unwind": "$hits"},
        doc! {"$group": {"_id": {"k": format!("$hits.{field}"), "v": "$_id.v"}, "views": {"$sum": 1},
        "engaged": {"$sum": "$hits.e"}, "route": {"$first": "$hits.r"}}},
        doc! {"$group": {"_id": "$_id.k", "visitors": {"$sum": 1}, "views": {"$sum": "$views"},
        "engaged": {"$sum": "$engaged"}, "route": {"$first": "$route"}}},
        doc! {"$sort": {"views": -1, "_id": 1}},
        doc! {"$limit": limit},
    ]
}

fn pct(field: impl Into<Bson>, ps: &[f64]) -> Document {
    doc! {"$percentile": {"input": field.into(), "p": ps.to_vec(), "method": "approximate"}}
}

fn speed_group(key: impl Into<Bson>) -> Document {
    doc! {"$group": {
        "_id": key.into(), "n": {"$sum": 1},
        "seen": {"$sum": {"$cond": [{"$gt": [vis(), 0]}, 1, 0]}},
        "slow": {"$sum": {"$cond": [{"$gt": [vis(), SLOW_MS]}, 1, 0]}},
        "vis": pct(vis(), &[0.5, 0.75]),
        "ttfb": pct("$perf.ttfb", &[0.75]),
        "dcl": pct("$perf.dcl", &[0.75]),
        "load": pct("$perf.load", &[0.75]),
        "cls": pct("$perf.cls", &[0.75]),
        "cls_n": {"$sum": {"$cond": [{"$gte": ["$perf.cls", 0]}, 1, 0]}},
        "cls_poor": {"$sum": {"$cond": [{"$gt": ["$perf.cls", CLS_POOR]}, 1, 0]}},
    }}
}

fn stages(v: Vec<Document>) -> Bson {
    Bson::Array(v.into_iter().map(Bson::Document).collect())
}

fn speed_facets(f: &mut Document) {
    f.insert("speed_all", stages(vec![has_perf(), speed_group(Bson::Null)]));
    f.insert("speed_bands", stages(vec![has_perf(), doc! {"$group": {"_id": speed_band(), "n": {"$sum": 1}}}]));
    for k in SPEED_SPLITS {
        f.insert(
            format!("speed_{k}"),
            stages(vec![
                has_perf(),
                speed_group(format!("${k}")),
                doc! {"$sort": {"n": -1, "_id": 1}},
                doc! {"$limit": 12},
            ]),
        );
    }
}

fn eng_facets(f: &mut Document, split: bool) {
    f.insert("eng", stages(vec![doc! {"$group": {"_id": eng_band(), "n": {"$sum": 1}}}]));
    f.insert("eng_med", stages(vec![doc! {"$group": {"_id": Bson::Null, "m": eng_median()}}]));
    if split {
        for k in ENG_SPLITS {
            f.insert(
                format!("eng_{k}"),
                stages(vec![doc! {"$group": {"_id": {"k": format!("${k}"), "b": eng_band()}, "n": {"$sum": 1}}}]),
            );
            f.insert(
                format!("eng_med_{k}"),
                stages(vec![doc! {"$group": {"_id": format!("${k}"), "m": eng_median()}}]),
            );
        }
    }
}

/// `_totals_facets(granularity)`.
pub fn totals_facets(hourly: bool) -> Document {
    let mut f = Document::new();
    f.insert("totals", stages(visit_dim(doc! {"$literal": "all"}, None)));
    f.insert(
        "median",
        stages(vec![
            doc! {"$group": {"_id": Bson::Null, "m": {"$median": {"input": "$engaged", "method": "approximate"}}}},
        ]),
    );
    f.insert("series", stages(visit_dim(bucket("start", hourly), None)));
    eng_facets(&mut f, false);
    f
}

/// `{**_totals_facets(g), **_breakdown_facets()}`: a repeated key keeps its first place.
pub fn report_facets(hourly: bool) -> Document {
    let mut f = totals_facets(hourly);
    eng_facets(&mut f, true);
    speed_facets(&mut f);
    let dims: [(&str, &str); 8] = [
        ("channels", "$channel"),
        ("sources", "$source"),
        ("entries", "$entry"),
        ("exits", "$exit"),
        ("devices", "$device"),
        ("browsers", "$browser"),
        ("os", "$os"),
        ("langs", "$lang"),
    ];
    for (name, key) in dims {
        let mut v = visit_dim(key, Some(30));
        if name == "sources" {
            v.insert(0, doc! {"$match": {"source": {"$ne": Bson::Null}}});
        }
        f.insert(name, stages(v));
    }
    f.insert("pages", stages(hit_dim("p", 100)));
    f.insert("sections", stages(hit_dim("r", 60)));
    f
}

fn filter_stage(filters: &[(&str, String)]) -> Vec<Document> {
    if filters.is_empty() {
        return Vec::new();
    }
    let m: Document = filters.iter().map(|(k, v)| ((*k).to_owned(), Bson::String(v.clone()))).collect();
    vec![doc! {"$match": m}]
}

/// The current and (when tracked that far back) previous windows' pipelines.
pub fn report_pipelines(
    days: i64,
    now: DateTime<Utc>,
    started: Option<DateTime<Utc>>,
    filters: &[(&str, String)],
) -> (Window, Vec<String>, Vec<Document>, Option<Vec<Document>>) {
    let w = window(days, now);
    let span = w.until - w.since;
    let shift = if w.hourly { Duration::hours(24) } else { Duration::days(days) };
    let (prev_since, prev_until) = (w.since - shift, w.since - shift + span);
    let prev_labels = window(days, w.until - shift).labels;
    let flt = filter_stage(filters);
    let mut current = visits_pipeline(w.since, w.until);
    current.extend(flt.iter().cloned());
    current.push(doc! {"$facet": report_facets(w.hourly)});
    let previous = started.filter(|s| prev_since >= *s).map(|_| {
        let mut p = visits_pipeline(prev_since, prev_until);
        p.extend(flt.iter().cloned());
        p.push(doc! {"$facet": totals_facets(w.hourly)});
        p
    });
    (w, prev_labels, current, previous)
}

// --- shaping the answer ---------------------------------------------------------------

fn rows<'a>(facet: &'a Document, name: &str) -> Vec<&'a Document> {
    match facet.get(name) {
        Some(Bson::Array(a)) => a.iter().filter_map(Bson::as_document).collect(),
        _ => Vec::new(),
    }
}

fn first<'a>(facet: &'a Document, name: &str) -> Option<&'a Document> {
    rows(facet, name).into_iter().next()
}

fn get_or_zero(d: Option<&Document>, key: &str) -> Value {
    d.and_then(|d| d.get(key)).map_or(json!(0), bson_json)
}

/// Python `round(x)` of a stored number (`None` stays `None`).
fn py_round(b: Option<&Bson>) -> Value {
    match b {
        Some(Bson::Double(f)) if f.is_finite() => json!(pyfloat::round_int(*f)),
        Some(Bson::Int32(n)) => json!(n),
        Some(Bson::Int64(n)) => json!(n),
        _ => Value::Null,
    }
}

fn int_of(b: Option<&Bson>) -> Result<i64, ApiError> {
    match b {
        Some(Bson::Int32(n)) => Ok(i64::from(*n)),
        Some(Bson::Int64(n)) => Ok(*n),
        other => Err(internal(format!("page_views count: {other:?}"))),
    }
}

/// `_row(r, name)`: the `_id` renamed, every other field after it.
pub fn row(r: &Document, name: &str) -> Result<Value, ApiError> {
    let id = r.get("_id").ok_or_else(|| internal("page_views row without _id"))?;
    let mut out = Map::new();
    out.insert(name.to_owned(), bson_json(id));
    for (k, v) in r {
        if k != "_id" {
            out.insert(k.clone(), bson_json(v));
        }
    }
    Ok(Value::Object(out))
}

fn totals(facet: &Document) -> Value {
    let t = first(facet, "totals");
    let med = first(facet, "median").and_then(|m| m.get("m")).filter(|m| !matches!(m, Bson::Null));
    json!({
        "visitors": get_or_zero(t, "visitors"),
        "visits": get_or_zero(t, "visits"),
        "pageviews": get_or_zero(t, "pages"),
        "bounces": get_or_zero(t, "bounces"),
        "engaged": get_or_zero(t, "engaged"),
        "median_engaged": py_round(med),
    })
}

fn series(facet: &Document, labels: &[String]) -> Value {
    let by: HashMap<String, &Document> = rows(facet, "series")
        .into_iter()
        .filter_map(|r| match r.get("_id") {
            Some(Bson::String(s)) => Some((s.clone(), r)),
            _ => None,
        })
        .collect();
    Value::Array(
        labels
            .iter()
            .map(|b| {
                let r = by.get(b).copied();
                json!({
                    "bucket": b,
                    "visitors": get_or_zero(r, "visitors"),
                    "visits": get_or_zero(r, "visits"),
                    "pageviews": get_or_zero(r, "pages"),
                    "bounces": get_or_zero(r, "bounces"),
                    "engaged": get_or_zero(r, "engaged"),
                })
            })
            .collect(),
    )
}

/// `_pick(values, i)`.
fn pick(values: Option<&Bson>, i: usize) -> Value {
    match values {
        Some(Bson::Array(a)) => py_round(a.get(i)),
        _ => Value::Null,
    }
}

fn speed_row(r: &Document) -> Value {
    json!({
        "key": r.get("_id").map_or(Value::Null, bson_json),
        "n": get_or_zero(Some(r), "n"),
        "seen": get_or_zero(Some(r), "seen"),
        "slow": get_or_zero(Some(r), "slow"),
        "p50": pick(r.get("vis"), 0),
        "p75": pick(r.get("vis"), 1),
        "ttfb": pick(r.get("ttfb"), 0),
        "dcl": pick(r.get("dcl"), 0),
        "load": pick(r.get("load"), 0),
        "cls": pick(r.get("cls"), 0),
        "cls_n": get_or_zero(Some(r), "cls_n"),
        "cls_poor": get_or_zero(Some(r), "cls_poor"),
    })
}

fn add_band(bands: &mut [i64], idx: Option<&Bson>, n: Option<&Bson>) -> Result<(), ApiError> {
    let i = int_of(idx)?;
    let slot = usize::try_from(i).ok().and_then(|i| bands.get_mut(i)).ok_or_else(|| internal(format!("band {i}")))?;
    *slot += int_of(n)?;
    Ok(())
}

fn speed(facet: &Document) -> Result<Value, ApiError> {
    let mut bands = vec![0i64; SPEED_BOUNDS.len() + 2];
    for r in rows(facet, "speed_bands") {
        add_band(&mut bands, r.get("_id"), r.get("n"))?;
    }
    let mut split = Map::new();
    for k in SPEED_SPLITS {
        split.insert(
            k.to_owned(),
            Value::Array(rows(facet, &format!("speed_{k}")).into_iter().map(speed_row).collect()),
        );
    }
    Ok(json!({
        "bounds": SPEED_BOUNDS,
        "slow_ms": SLOW_MS,
        "bands": bands,
        "total": first(facet, "speed_all").map_or(Value::Null, speed_row),
        "split": split,
    }))
}

fn eng_bands<'a>(rs: impl IntoIterator<Item = (Option<&'a Bson>, Option<&'a Bson>)>) -> Result<Vec<i64>, ApiError> {
    let mut n = vec![0i64; ENG_BOUNDS.len() + 1];
    for (band, count) in rs {
        add_band(&mut n, band, count)?;
    }
    Ok(n)
}

fn eng_total(facet: &Document) -> Result<Value, ApiError> {
    let n = eng_bands(rows(facet, "eng").into_iter().map(|r| (r.get("_id"), r.get("n"))))?;
    let med = first(facet, "eng_med").and_then(|r| r.get("m"));
    Ok(json!({"n": n, "median": py_round(med)}))
}

/// `str(key)` for the split sort.
fn py_str(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::String(s) => s.clone(),
        Value::Bool(b) => if *b { "True" } else { "False" }.into(),
        other => crate::pyjson::dumps(other),
    }
}

fn engagement(facet: &Document, prev: Option<&Document>) -> Result<Value, ApiError> {
    let mut split = Map::new();
    for k in ENG_SPLITS {
        let mut groups: Vec<(Value, Vec<&Document>)> = Vec::new();
        for r in rows(facet, &format!("eng_{k}")) {
            let key = r.get_document("_id").ok().and_then(|id| id.get("k")).map_or(Value::Null, bson_json);
            match groups.iter_mut().find(|(g, _)| *g == key) {
                Some((_, list)) => list.push(r),
                None => groups.push((key, vec![r])),
            }
        }
        let meds: Vec<(Value, Option<&Bson>)> = rows(facet, &format!("eng_med_{k}"))
            .into_iter()
            .map(|r| (r.get("_id").map_or(Value::Null, bson_json), r.get("m")))
            .collect();
        let mut out: Vec<(i64, String, Value)> = Vec::new();
        for (key, rs) in groups {
            let n = eng_bands(rs.iter().map(|r| (r.get_document("_id").ok().and_then(|id| id.get("b")), r.get("n"))))?;
            let med = meds.iter().rev().find(|(m, _)| *m == key).and_then(|(_, m)| *m);
            let total: i64 = n.iter().sum();
            let sort_key = py_str(&key);
            out.push((total, sort_key, json!({"key": key, "n": n, "median": py_round(med)})));
        }
        out.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        split.insert(k.to_owned(), Value::Array(out.into_iter().take(12).map(|(_, _, v)| v).collect()));
    }
    Ok(json!({
        "bounds": ENG_BOUNDS,
        "total": eng_total(facet)?,
        "previous": match prev { Some(p) => eng_total(p)?, None => Value::Null },
        "split": split,
    }))
}

fn pyd_utc(d: &DateTime<Utc>) -> String {
    dt::pydantic(d)
}

/// `_site_report`'s answer from the facet rows.
pub fn build_report(
    days: i64,
    w: &Window,
    prev_labels: &[String],
    started: Option<DateTime<Utc>>,
    filters: &[(&str, String)],
    facet: &Document,
    prev: Option<&Document>,
) -> Result<Value, ApiError> {
    let flt: Map<String, Value> = filters.iter().map(|(k, v)| ((*k).to_owned(), Value::String(v.clone()))).collect();
    let mut out = Map::new();
    out.insert("window_days".into(), json!(days));
    out.insert("granularity".into(), json!(w.granularity()));
    out.insert("since".into(), json!(pyd_utc(&w.since)));
    out.insert("until".into(), json!(pyd_utc(&w.until)));
    out.insert("tracking_since".into(), started.map_or(Value::Null, |s| json!(pyd_utc(&s))));
    out.insert("filters".into(), Value::Object(flt));
    out.insert("totals".into(), totals(facet));
    out.insert("series".into(), series(facet, &w.labels));
    out.insert(
        "previous".into(),
        match prev {
            Some(p) => json!({"totals": totals(p), "series": series(p, prev_labels)}),
            None => Value::Null,
        },
    );
    out.insert("engagement".into(), engagement(facet, prev)?);
    out.insert("speed".into(), speed(facet)?);
    for name in BREAKDOWNS {
        let list = rows(facet, name).into_iter().map(|r| row(r, "key")).collect::<Result<Vec<_>, _>>()?;
        out.insert(name.into(), Value::Array(list));
    }
    Ok(Value::Object(out))
}

/// `live()`'s pipeline.
pub fn live_pipeline(now: DateTime<Utc>) -> Vec<Document> {
    let mut m = tracked();
    m.insert("created_at", doc! {"$gte": dt::to_bson(now - Duration::hours(6))});
    m.insert("last_at", doc! {"$gte": dt::to_bson(now - Duration::minutes(5))});
    vec![
        doc! {"$match": m},
        doc! {"$facet": {
            "visitors": [{"$group": {"_id": "$visitor_hash"}}, {"$count": "n"}],
            "pages": [{"$group": {"_id": {"p": "$path", "v": "$visitor_hash"}}},
                      {"$group": {"_id": "$_id.p", "visitors": {"$sum": 1}}},
                      {"$sort": {"visitors": -1, "_id": 1}}, {"$limit": 5}],
        }},
    ]
}

pub fn build_live(facet: &Document) -> Result<Value, ApiError> {
    let n = get_or_zero(first(facet, "visitors"), "n");
    let pages = rows(facet, "pages").into_iter().map(|r| row(r, "path")).collect::<Result<Vec<_>, _>>()?;
    Ok(json!({"visitors": n, "pages": pages}))
}

// --- running it ----------------------------------------------------------------------

static MEMO: LazyLock<Mutex<HashMap<String, (Instant, Value)>>> = LazyLock::new(Default::default);
/// `tracking_since`'s cached answer and when it expires.
type SinceMemo = Option<(Instant, Option<DateTime<Utc>>)>;
static SINCE: Mutex<SinceMemo> = Mutex::new(None);
static HEAVY: Semaphore = Semaphore::const_new(3);

fn memo_get(key: &str) -> Option<Value> {
    let memo = MEMO.lock().unwrap_or_else(|e| e.into_inner());
    memo.get(key).filter(|(exp, _)| *exp > Instant::now()).map(|(_, v)| v.clone())
}

fn memo_put(key: String, ttl: u64, v: &Value) {
    let mut memo = MEMO.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    memo.insert(key, (now + StdDuration::from_secs(ttl), v.clone()));
    if memo.len() > 256 {
        memo.retain(|_, (exp, _)| *exp > now);
    }
}

async fn run(state: &AppState, pipeline: Vec<Document>) -> Result<Option<Document>, ApiError> {
    let _permit = HEAVY.acquire().await.map_err(internal)?;
    let docs: Vec<Document> = coll(state, PAGE_VIEWS)
        .aggregate(pipeline)
        .allow_disk_use(true)
        .await
        .map_err(internal)?
        .try_collect()
        .await
        .map_err(internal)?;
    Ok(docs.into_iter().next())
}

/// `tracking_since()`: the oldest tracked view, cached 300s.
async fn tracking_since(state: &AppState) -> Result<Option<DateTime<Utc>>, ApiError> {
    if let Some((exp, v)) = *SINCE.lock().unwrap_or_else(|e| e.into_inner())
        && exp > Instant::now()
    {
        return Ok(v);
    }
    let found = coll(state, PAGE_VIEWS)
        .find_one(tracked())
        .sort(doc! {"created_at": 1})
        .projection(doc! {"created_at": 1})
        .await
        .map_err(internal)?;
    let v = match found {
        None => None,
        Some(d) => match d.get("created_at") {
            Some(Bson::DateTime(t)) => Some(dt::from_bson(*t)),
            Some(Bson::Null) => None,
            other => return Err(internal(format!("page_views created_at: {other:?}"))),
        },
    };
    *SINCE.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now() + StdDuration::from_secs(300), v));
    Ok(v)
}

async fn site_report(state: &AppState, days: i64, chosen: &[(&str, String)]) -> Result<Value, ApiError> {
    let mut sorted: Vec<&(&str, String)> = chosen.iter().collect();
    sorted.sort();
    let key = format!("site|{days}|{}", serde_json::to_string(&sorted).unwrap_or_default());
    if let Some(v) = memo_get(&key) {
        return Ok(v);
    }
    let now = dt::now();
    let started = tracking_since(state).await?;
    let (w, prev_labels, current, previous) = report_pipelines(days, now, started, chosen);
    let facet = run(state, current).await?.unwrap_or_default();
    let prev = match previous {
        Some(p) => Some(run(state, p).await?.unwrap_or_default()),
        None => None,
    };
    let v = build_report(days, &w, &prev_labels, started, chosen, &facet, prev.as_ref())?;
    memo_put(key, if days <= 31 { 60 } else { 600 }, &v);
    Ok(v)
}

pub async fn site_pageviews(State(state): State<AppState>, _: Superuser, Args(q): Args<ReportQuery>) -> Res {
    let chosen = q.chosen();
    Ok(PydanticJson(site_report(&state, q.days, &chosen).await?))
}

pub async fn site_pageviews_live(State(state): State<AppState>, _: Superuser) -> Res {
    if let Some(v) = memo_get("live") {
        return Ok(PydanticJson(v));
    }
    let facet = run(&state, live_pipeline(dt::now())).await?.unwrap_or_default();
    let v = build_live(&facet)?;
    memo_put("live".into(), 15, &v);
    Ok(PydanticJson(v))
}
