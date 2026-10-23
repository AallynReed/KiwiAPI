//! `insert_dump` against the real Python service run with recorders for the
//! upsert, the interest list, the clock and the DM check
//! (`testdata/producers/golden.json`, `market`).

use std::{collections::BTreeSet, sync::OnceLock};

use kiwi_core::market::parser::{parse_dump, uuid_str};
use mongodb::bson::{Bson, Document};
use serde_json::{Value, json};

use super::insert::{Summary, cheapest, last_seen, log_summary, response_body, select_rows};

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| {
        serde_json::from_str(include_str!("../../testdata/producers/golden.json")).expect("producers golden")
    })
}

const NOW: i64 = 1_792_576_800;

#[test]
fn insert_dump_matches_python() {
    let interest: BTreeSet<String> = ["Glim", "Plasmium"].into_iter().map(String::from).collect();
    for case in golden()["market"].as_array().unwrap() {
        let text = case["text"].as_str().unwrap();
        let ts = case["timestamp"].as_i64();
        let listings = parse_dump(text);
        if listings.is_empty() {
            let s = Summary { parsed: 0, imported: 0, ignored: 0, last_seen: None };
            assert_eq!(response_body(&s), case["summary"]);
            assert!(case["rows"].is_null() && case["cheapest"].is_null());
            continue;
        }
        let ls = last_seen(ts, NOW);
        let (rows, ignored) = select_rows(&listings, &interest);
        let got_rows: Vec<Value> = rows
            .iter()
            .map(|r| json!([uuid_str(r.id), r.name, r.kind, r.stack, r.price, r.price_each, r.created_at, ls]))
            .collect();
        assert_eq!(Value::Array(got_rows), case["rows"], "{ts:?}");
        let got_cheapest: Vec<Value> = cheapest(&rows).into_iter().map(|(n, p)| json!([n, p])).collect();
        assert_eq!(Value::Array(got_cheapest), case["cheapest"], "{ts:?}");
        let s = Summary { parsed: listings.len(), imported: rows.len(), ignored, last_seen: Some(ls) };
        assert_eq!(response_body(&s), case["summary"], "{ts:?}");
    }
}

/// An upload through form parsing, parameter validation and the empty-file
/// check, as `(status, body)` under the case's request id.
async fn upload_reply(case: &Value) -> Option<(u16, String)> {
    use axum::http::{HeaderMap, StatusCode, header};
    use kiwi_core::form::{FormError, parse_form};

    use super::insert::{EMPTY_UPLOAD, validate_upload};
    use crate::error::ApiError;

    let body: Vec<u8> = case["body"].as_str().unwrap().chars().map(|c| c as u8).collect();
    let ctype = case["content_type"].as_str().unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, ctype.parse().unwrap());
    let url = case["url"].as_str().unwrap();
    let query = url.split_once('?').map(|(_, q)| q);
    let err = match parse_form(Some(ctype.as_bytes()), &body) {
        Err(FormError::Http(msg)) => ApiError::http(StatusCode::BAD_REQUEST, Some(&msg)),
        Ok(items) => match validate_upload(query, &headers, items, url.starts_with("/v1/market")) {
            Err(e) => e.into(),
            Ok((file, _)) if file.is_empty() => ApiError::bad_request(EMPTY_UPLOAD),
            Ok(_) => return None,
        },
    };
    let rid = case["request_id"].as_str().unwrap().to_owned();
    let status = err.status.as_u16();
    Some((status, crate::request_id::scope(rid, async move { err.body() }).await))
}

#[tokio::test]
async fn upload_errors_match_fastapi() {
    for case in golden()["http"]["upload"].as_array().unwrap() {
        let want = (case["status"].as_u64().unwrap() as u16, case["response"].as_str().unwrap().to_owned());
        assert_eq!(upload_reply(case).await, Some(want), "{case}");
    }
}

#[test]
fn log_summary_matches_beanie() {
    let s = Summary { parsed: 0, imported: 0, ignored: 0, last_seen: None };
    let got: Document = log_summary(&s, 12);
    assert_eq!(Bson::Document(got).into_canonical_extjson(), golden()["documents"]["log_market"]);
}
