//! `GET /v1/mods/hub/me/stats`: `pageviews.paths_report` over the caller's
//! mod pages, folded into one row per mod.

use chrono::Duration;
use mongodb::bson::{Document, doc};
use serde_json::{Map, Value, json};

use crate::{
    admin::pageviews::{bucket, memo_get, memo_put, row, run, tracking_since, window},
    dt,
    error::ApiError,
};

fn path_dim(key: impl Into<mongodb::bson::Bson>, limit: Option<i64>) -> Vec<Document> {
    let mut stages = vec![
        doc! {"$group": {"_id": {"k": key.into(), "v": "$visitor_hash"}, "views": {"$sum": 1}, "engaged": {"$sum": "$engaged_s"}}},
        doc! {"$group": {"_id": "$_id.k", "visitors": {"$sum": 1}, "views": {"$sum": "$views"}, "engaged": {"$sum": "$engaged"}}},
        doc! {"$sort": {"views": -1, "_id": 1}},
    ];
    if let Some(l) = limit {
        stages.push(doc! {"$limit": l});
    }
    stages
}

fn tot(facet: Option<&Document>) -> Value {
    let t = facet
        .and_then(|f| f.get_array("totals").ok())
        .and_then(|a| a.first())
        .and_then(|d| d.as_document())
        .cloned()
        .unwrap_or_default();
    let get = |k: &str| t.get(k).map_or(json!(0), crate::lb_ingest::admin::bson_json);
    json!({"views": get("views"), "visitors": get("visitors"), "engaged": get("engaged")})
}

fn rows(facet: &Document, key: &str, name: &str) -> Result<Vec<Value>, ApiError> {
    facet
        .get_array(key)
        .map(|a| a.iter().filter_map(|d| d.as_document()).map(|d| row(d, name)).collect())
        .unwrap_or_else(|_| Ok(Vec::new()))
}

/// `paths_report(paths, days)`, cached 120s per `(days, paths)`.
pub async fn paths_report(
    state: &crate::state::AppState,
    paths: &[String],
    days: i64,
) -> Result<Map<String, Value>, ApiError> {
    let mut paths: Vec<String> = paths.to_vec();
    paths.sort();
    paths.dedup();
    paths.truncate(1000);
    let key = format!("paths|{days}|{}", serde_json::to_string(&paths).unwrap_or_default());
    if let Some(Value::Object(m)) = memo_get(&key) {
        return Ok(m);
    }
    let w = window(days.max(2), dt::now());
    let shift = Duration::days(days);
    let started = tracking_since(state).await?;
    let has_prev = started.is_some_and(|s| w.since - shift >= s);
    let matcher = |lo, hi| {
        doc! {"$match": {"pid": {"$type": "string"}, "path": {"$in": paths.clone()},
        "created_at": {"$gte": dt::to_bson(lo), "$lt": dt::to_bson(hi)}}}
    };
    let mut sources = vec![doc! {"$match": {"source": {"$ne": null}}}];
    sources.extend(path_dim("$source", Some(15)));
    let current = vec![
        matcher(w.since, w.until),
        doc! {"$facet": {
            "totals": path_dim(doc! {"$literal": "all"}, None),
            "series": path_dim(bucket("created_at", w.hourly), None),
            "pages": path_dim("$path", None),
            "channels": path_dim("$channel", None),
            "sources": sources,
            "devices": path_dim("$device", None),
        }},
    ];
    let facet = run(state, current).await?.unwrap_or_default();
    let prev = if has_prev {
        let p = vec![
            matcher(w.since - shift, w.until - shift),
            doc! {"$facet": {"totals": path_dim(doc! {"$literal": "all"}, None)}},
        ];
        Some(run(state, p).await?.unwrap_or_default())
    } else {
        None
    };
    let by: Vec<(String, Document)> = facet
        .get_array("series")
        .map(|a| {
            a.iter()
                .filter_map(|d| d.as_document())
                .filter_map(|d| Some((d.get_str("_id").ok()?.to_owned(), d.clone())))
                .collect()
        })
        .unwrap_or_default();
    let series: Vec<Value> = w
        .labels
        .iter()
        .map(|b| {
            let hit = by.iter().find(|(k, _)| k == b).map(|(_, d)| d);
            let get = |k: &str| hit.and_then(|d| d.get(k)).map_or(json!(0), crate::lb_ingest::admin::bson_json);
            json!({"bucket": b, "views": get("views"), "visitors": get("visitors")})
        })
        .collect();
    let mut m = Map::new();
    m.insert("window_days".into(), days.into());
    m.insert("since".into(), dt::pydantic(&w.since).into());
    m.insert("tracking_since".into(), started.map_or(Value::Null, |s| dt::pydantic(&s).into()));
    m.insert("totals".into(), tot(Some(&facet)));
    m.insert("previous".into(), prev.as_ref().map_or(Value::Null, |p| tot(Some(p))));
    m.insert("series".into(), Value::Array(series));
    m.insert("pages".into(), Value::Array(rows(&facet, "pages", "path")?));
    m.insert("channels".into(), Value::Array(rows(&facet, "channels", "key")?));
    m.insert("sources".into(), Value::Array(rows(&facet, "sources", "key")?));
    m.insert("devices".into(), Value::Array(rows(&facet, "devices", "key")?));
    memo_put(key, 120, &Value::Object(m.clone()));
    Ok(m)
}

/// The `/me/stats` body from `list_owned` cards and the report.
pub fn fold(mods: &[Value], report: &Map<String, Value>) -> Value {
    let mut by_path: Vec<(String, &Value)> = Vec::new();
    for m in mods {
        let (h, s) = (m["handle"].as_str().unwrap_or(""), m["slug"].as_str().unwrap_or(""));
        if h.is_empty() || s.is_empty() {
            continue;
        }
        let path = format!("/mods/{h}/{s}");
        match by_path.iter_mut().find(|(p, _)| *p == path) {
            Some(e) => e.1 = m,
            None => by_path.push((path, m)),
        }
    }
    let pages: &[Value] = report.get("pages").and_then(Value::as_array).map_or(&[], Vec::as_slice);
    let mut rows: Vec<Value> = by_path
        .iter()
        .map(|(path, m)| {
            let seen = pages.iter().find(|r| r["path"].as_str() == Some(path));
            let get = |k: &str| seen.and_then(|r| r.get(k)).cloned().unwrap_or(json!(0));
            let title = m["title"].as_str().filter(|t| !t.is_empty()).unwrap_or(m["slug"].as_str().unwrap_or(""));
            json!({
                "title": title, "handle": m["handle"], "slug": m["slug"],
                "downloads": m.get("download_count").cloned().unwrap_or(json!(0)),
                "views": get("views"), "visitors": get("visitors"), "engaged": get("engaged"),
            })
        })
        .collect();
    let views = |r: &Value| r["views"].as_f64().unwrap_or(0.0);
    rows.sort_by(|a, b| {
        views(b).partial_cmp(&views(a)).unwrap_or(std::cmp::Ordering::Equal).then_with(|| {
            let t = |r: &Value| r["title"].as_str().unwrap_or("").to_lowercase();
            t(a).cmp(&t(b))
        })
    });
    let mut out: Map<String, Value> =
        report.iter().filter(|(k, _)| k.as_str() != "pages").map(|(k, v)| (k.clone(), v.clone())).collect();
    out.insert("mods".into(), Value::Array(rows));
    Value::Object(out)
}

/// The paths `/me/stats` asks the report about.
pub fn mod_paths(mods: &[Value]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in mods {
        let (h, s) = (m["handle"].as_str().unwrap_or(""), m["slug"].as_str().unwrap_or(""));
        let p = format!("/mods/{h}/{s}");
        if !h.is_empty() && !s.is_empty() && !out.contains(&p) {
            out.push(p);
        }
    }
    out
}
