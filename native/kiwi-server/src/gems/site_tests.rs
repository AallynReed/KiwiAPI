//! `/site/gems/*` against `testdata/site_live/gems.json`: every body below the
//! site gate, byte for byte; star-chart previews compared as maps and sets
//! because Python walks them in hash order.

use std::{collections::BTreeSet, path::PathBuf};

use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue, header},
};
use serde_json::Value;

use super::*;
use crate::{
    error::ApiError,
    gems::{EvaluateReq, HealthReq, SimpleReq},
    pyjson,
    site_live::tests::{assert_body, fixture},
    validation::{FromBody, read_body, validate},
};

fn store() -> Gamedata {
    Gamedata::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../app/trove/gamedata"), None)
}

fn render(reply: Reply) -> (u16, String) {
    match reply {
        Reply::Json(v) => (200, pyjson::dumps(&v)),
        Reply::Error(e) => (e.status.as_u16(), e.body()),
        Reply::Unhandled(_) => (500, ApiError::internal().body()),
    }
}

fn query<Q: FromParams>(qs: &str, run: impl FnOnce(Q) -> Reply) -> (u16, String) {
    let headers = HeaderMap::new();
    let mut p = Params::new(Vec::new(), Some(qs), &headers);
    let q = Q::from_params(&mut p);
    match p.v.finish(q) {
        Ok(q) => render(run(q)),
        Err(e) => render(Reply::Error(e.into())),
    }
}

async fn post<B: FromBody>(case: &Value, run: impl FnOnce(B) -> Reply) -> (u16, String) {
    let mut headers = HeaderMap::new();
    if let Some(ct) = case["content_type"].as_str() {
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_str(ct).expect("content type"));
    }
    let raw = case["request"].as_str().unwrap_or_default().to_owned();
    let body = match read_body(&headers, Body::from(raw)).await {
        Ok(b) => b,
        Err(e) => return render(Reply::Error(e)),
    };
    match validate::<B>(&body) {
        Ok(req) => render(run(req)),
        Err(e) => render(Reply::Error(e)),
    }
}

type ChartKey = (Vec<(String, String, String)>, BTreeSet<String>, Value, Value);

/// A preview as order-free sets, floats to 9 places.
fn chart_key(v: &Value) -> ChartKey {
    let mut stats: Vec<(String, String, String)> = v["stats"]
        .as_object()
        .map(|m| {
            m.iter()
                .map(|(k, x)| {
                    let f = |n: &Value| format!("{:.9}", n.as_f64().unwrap_or(f64::NAN));
                    (k.clone(), f(&x["flat"]), f(&x["pct"]))
                })
                .collect()
        })
        .unwrap_or_default();
    stats.sort();
    let abilities = v["abilities"].as_array().map(|a| a.iter().map(|x| x.to_string()).collect()).unwrap_or_default();
    (stats, abilities, v["paths_count"].clone(), v["bounty_hunt"].clone())
}

#[tokio::test]
async fn site_gem_routes_match_python() {
    let s = store();
    let cases = fixture("gems.json");
    let mut failed = Vec::new();
    for case in cases.as_array().expect("cases") {
        let path = case["path"].as_str().expect("path");
        let (route, qs) = path.split_once('?').unwrap_or((path, ""));
        let name = case["name"].as_str().unwrap_or_default();
        let (status, body) = match route.trim_start_matches("/site/gems/") {
            "lookups" => render(site_lookups(&s)),
            "builds/options" => render(run_build_options(&s)),
            "stat-range" => query::<StatRangeQ>(qs, |q| run_stat_range(&s, &q)),
            "evaluate" => post::<EvaluateReq>(case, |r| run_evaluate(&s, r)).await,
            "evaluate-simple" => post::<SimpleReq>(case, |r| run_evaluate_simple(&s, r)).await,
            "builds/calculate" => post::<BuildReq>(case, |r| run_calculate(&s, r)).await,
            "builds/health-optimizer" => {
                post::<HealthReq>(case, |r| match crate::gems::run_health(&s, r) {
                    Reply::Json(v) => Reply::Json(reorder_health(v)),
                    other => other,
                })
                .await
            }
            "parse-star-chart" => {
                let headers = HeaderMap::new();
                let mut p = Params::new(Vec::new(), Some(qs), &headers);
                let q = ChartQuery::from_params(&mut p);
                match p.v.finish(q) {
                    Ok(q) => {
                        let d = data::builds(&s);
                        let got = preview_json(&d.chart.preview(&q.code));
                        let want: Value = serde_json::from_str(case["body"].as_str().unwrap()).unwrap();
                        assert_eq!(chart_key(&got), chart_key(&want), "{name}");
                        continue;
                    }
                    Err(e) => render(Reply::Error(e.into())),
                }
            }
            other => panic!("unknown route {other}"),
        };
        let want_status = case["status"].as_u64().unwrap_or(0) as u16;
        if status != want_status {
            failed.push(format!("{name}: {status} vs {want_status}: {}", &body[..body.len().min(300)]));
            continue;
        }
        let r = std::panic::catch_unwind(|| assert_body(case, &body, name));
        if let Err(e) = r {
            failed.push(e.downcast_ref::<String>().cloned().unwrap_or_else(|| name.to_owned()));
        }
    }
    assert!(failed.is_empty(), "{} cases differ:\n{}", failed.len(), failed[..failed.len().min(6)].join("\n"));
}

#[test]
fn cache_headers_match_python() {
    for case in fixture("gems.json").as_array().unwrap() {
        let path = case["path"].as_str().unwrap();
        let hard = ["/site/gems/lookups", "/site/gems/stat-range", "/site/gems/builds/options"]
            .iter()
            .any(|p| path.split('?').next() == Some(*p));
        let want = case["cache_control"].as_str();
        let expect = if hard && case["status"] == 200 { Some(HARD_CACHE) } else { None };
        assert_eq!(want, expect, "{path}");
    }
}
