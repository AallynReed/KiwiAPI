//! `aggregate_activity` (`app/usage/service.py`): usage totals, per UTC day
//! and per endpoint over the last `days` days, as an `ActivitySummary`.

use chrono::Duration;
use futures::TryStreamExt;
use mongodb::bson::{Bson, Document, doc};
use serde_json::{Value, json};

use crate::{
    auth::{Rejection, pyutil::round2},
    dt,
    state::AppState,
    usage::USAGE_COLLECTION,
};

fn error_cond() -> Document {
    doc! { "$cond": [{ "$gte": ["$status_code", 400] }, 1, 0] }
}

fn int(d: &Document, key: &str) -> Result<i64, Rejection> {
    match d.get(key) {
        Some(Bson::Int32(i)) => Ok(i64::from(*i)),
        Some(Bson::Int64(i)) => Ok(*i),
        Some(Bson::Double(f)) if f.fract() == 0.0 => Ok(*f as i64),
        other => Err(Rejection::internal(format!("activity {key}: {other:?}"))),
    }
}

/// `round(x or 0, 2)` as a float.
fn avg(d: &Document, key: &str) -> Value {
    let x = match d.get(key) {
        Some(Bson::Double(f)) => *f,
        Some(Bson::Int32(i)) => f64::from(*i),
        Some(Bson::Int64(i)) => *i as f64,
        _ => 0.0,
    };
    json!(round2(x))
}

fn id_str(d: &Document) -> Result<String, Rejection> {
    match d.get("_id") {
        Some(Bson::String(s)) => Ok(s.clone()),
        other => Err(Rejection::internal(format!("activity _id: {other:?}"))),
    }
}

async fn run(state: &AppState, pipeline: Vec<Document>) -> Result<Vec<Document>, Rejection> {
    let db = super::store::db(state)?;
    let cur = db.collection::<Document>(USAGE_COLLECTION).aggregate(pipeline).await.map_err(Rejection::internal)?;
    cur.try_collect().await.map_err(Rejection::internal)
}

pub fn pipelines(base: &Document, since: mongodb::bson::DateTime) -> [Vec<Document>; 3] {
    let mut m = base.clone();
    m.insert("created_at", doc! { "$gte": since });
    let totals = vec![
        doc! { "$match": m.clone() },
        doc! { "$group": {
            "_id": Bson::Null,
            "count": { "$sum": 1 },
            "error_count": { "$sum": error_cond() },
            "rate_limited": { "$sum": { "$cond": [{ "$eq": ["$status_code", 429] }, 1, 0] } },
            "avg_duration_ms": { "$avg": "$duration_ms" },
        } },
    ];
    let by_day = vec![
        doc! { "$match": m.clone() },
        doc! { "$group": {
            "_id": { "$dateToString": { "format": "%Y-%m-%d", "date": "$created_at", "timezone": "UTC" } },
            "count": { "$sum": 1 },
            "error_count": { "$sum": error_cond() },
            "avg_duration_ms": { "$avg": "$duration_ms" },
        } },
        doc! { "$sort": { "_id": 1 } },
    ];
    let by_endpoint = vec![
        doc! { "$match": m },
        doc! { "$group": {
            "_id": { "route": "$route", "method": "$method" },
            "count": { "$sum": 1 },
            "error_count": { "$sum": error_cond() },
            "avg_duration_ms": { "$avg": "$duration_ms" },
        } },
        doc! { "$sort": { "count": -1 } },
    ];
    [totals, by_day, by_endpoint]
}

/// The `ActivitySummary` body from the three aggregation results.
pub fn summary(
    days: i64,
    totals: &[Document],
    by_day: &[Document],
    by_endpoint: &[Document],
) -> Result<Value, Rejection> {
    let (total, errors, limited, mean) = match totals.first() {
        Some(t) => (int(t, "count")?, int(t, "error_count")?, int(t, "rate_limited")?, avg(t, "avg_duration_ms")),
        None => (0, 0, 0, json!(0.0)),
    };
    let mut days_out = Vec::with_capacity(by_day.len());
    for r in by_day {
        days_out.push(json!({
            "date": id_str(r)?,
            "count": int(r, "count")?,
            "error_count": int(r, "error_count")?,
            "avg_duration_ms": avg(r, "avg_duration_ms"),
        }));
    }
    let mut endpoints = Vec::with_capacity(by_endpoint.len());
    for r in by_endpoint {
        let id = r.get_document("_id").map_err(Rejection::internal)?;
        let text = |k: &str| match id.get(k) {
            Some(Bson::String(s)) => Ok(s.clone()),
            other => Err(Rejection::internal(format!("activity {k}: {other:?}"))),
        };
        endpoints.push(json!({
            "route": text("route")?,
            "method": text("method")?,
            "count": int(r, "count")?,
            "error_count": int(r, "error_count")?,
            "avg_duration_ms": avg(r, "avg_duration_ms"),
        }));
    }
    Ok(json!({
        "window_days": days,
        "total_requests": total,
        "error_count": errors,
        "rate_limited": limited,
        "avg_duration_ms": mean,
        "by_day": days_out,
        "by_endpoint": endpoints,
    }))
}

/// `aggregate_activity(base_match, days)`.
pub async fn aggregate_activity(state: &AppState, base: Document, days: i64) -> Result<Value, Rejection> {
    let since = dt::to_bson(dt::now() - Duration::days(days));
    let [totals, by_day, by_endpoint] = pipelines(&base, since);
    let totals = run(state, totals).await?;
    let by_day = run(state, by_day).await?;
    let by_endpoint = run(state, by_endpoint).await?;
    summary(days, &totals, &by_day, &by_endpoint)
}
