//! Checks against `testdata/mod_stats/golden.json`
//! (`native/port/parity/gen_mod_stats_fixtures.py`).

use std::sync::OnceLock;

use mongodb::bson::{Bson, Document};
use serde_json::{Map, Value};

use super::{HubProject, Snapshot, build_rows, public_stats, to_bson, totals};
use crate::pyjson;

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| serde_json::from_str(include_str!("../../testdata/mod_stats/golden.json")).expect("fixture"))
}

fn arr<'a>(v: &'a Value, key: &str) -> &'a Vec<Value> {
    v[key].as_array().unwrap_or_else(|| panic!("section {key}"))
}

fn project(v: &Value) -> HubProject {
    let i = |k: &str| v[k].as_i64().unwrap();
    HubProject {
        title: v["title"].as_str().unwrap().into(),
        owner_handle: v["owner_handle"].as_str().unwrap().into(),
        slug: v["slug"].as_str().unwrap().into(),
        top_release: i("top_release"),
        downloads_7d: i("downloads_7d"),
        star_count: i("star_count"),
    }
}

#[test]
fn rows_like_python() {
    for c in arr(golden(), "rows") {
        let projects: Vec<HubProject> = arr(c, "projects").iter().map(project).collect();
        let rows = build_rows(&projects, &c["trovesaurus_ids"], &c["steam_ids"], arr(c, "catalog"), arr(c, "details"))
            .unwrap();
        let stored: Vec<Bson> = rows.iter().map(to_bson).collect();
        let want: Vec<Bson> = arr(c, "stored").iter().map(|v| Bson::try_from(v.clone()).unwrap()).collect();
        assert_eq!(stored, want, "{}", c["name"]);
        let maps: Vec<Map<String, Value>> = rows.into_iter().map(|r| r.as_object().unwrap().clone()).collect();
        assert_eq!(pyjson::dumps(&totals(&maps).unwrap()), c["totals"].as_str().unwrap(), "{}", c["name"]);
    }
}

#[test]
fn route_bodies() {
    for c in arr(golden(), "route") {
        let mut docs: Vec<Document> = arr(c, "docs")
            .iter()
            .map(|d| match Bson::try_from(d.clone()).unwrap() {
                Bson::Document(d) => d,
                other => panic!("{other:?}"),
            })
            .collect();
        docs.sort_by(|a, b| b.get_str("day").unwrap().cmp(a.get_str("day").unwrap()));
        let snaps: Vec<Snapshot> = docs.iter().map(|d| Snapshot::from_doc(d).unwrap()).collect();
        assert_eq!(pyjson::pydantic(&public_stats(&snaps).unwrap()), c["resp"].as_str().unwrap(), "{}", c["name"]);
        let cc = c["resp_headers"].as_array().unwrap().iter().find(|h| h[0] == "cache-control").unwrap();
        assert_eq!(cc[1], "public, max-age=600");
    }
}

#[test]
fn totals_refuse_what_python_cannot_add() {
    let row = |v: Value| v.as_object().unwrap().clone();
    assert!(totals(&[row(serde_json::json!({"hub": {}, "trovesaurus": null}))]).is_none());
    assert!(totals(&[row(serde_json::json!({"hub": {}, "trovesaurus": [1], "steam": null}))]).is_none());
    assert!(
        totals(&[row(serde_json::json!({"hub": {"top_release": null}, "trovesaurus": null, "steam": null}))]).is_none()
    );
    assert!(totals(&[row(serde_json::json!({"hub": {}, "trovesaurus": 0, "steam": ""}))]).is_some());
}
