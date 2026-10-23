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

#[test]
fn log_summary_matches_beanie() {
    let s = Summary { parsed: 0, imported: 0, ignored: 0, last_seen: None };
    let got: Document = log_summary(&s, 12);
    assert_eq!(Bson::Document(got).into_canonical_extjson(), golden()["documents"]["log_market"]);
}
