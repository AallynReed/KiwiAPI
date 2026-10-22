//! Golden bodies from `native/port/parity/gen_rotations_fixtures.py` (the real
//! Python functions and route handlers over fake stores).

use std::{path::Path, sync::LazyLock};

use chrono::{DateTime, Utc};
use kiwi_core::rotations as rot;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::{
    bodies::{self, ChallengeDoc, LuxionDoc},
    chaos, delves,
    gamedata::Gamedata,
    store::{self, DelveDoc},
};
use crate::pyjson;

static GOLDEN: LazyLock<Value> =
    LazyLock::new(|| pyjson::loads(include_bytes!("../../../testdata/rotations/golden.json")).expect("golden.json"));

static DATA: LazyLock<Gamedata> = LazyLock::new(|| {
    Gamedata::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../").join(super::gamedata::BASELINE_DIR))
});

fn g(key: &str) -> &'static Value {
    &GOLDEN[key]
}

fn short(body: &str) -> String {
    hex::encode(Sha256::digest(body.as_bytes()))[..16].to_owned()
}

fn at(us: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_micros(us).unwrap()
}

fn ms(v: &Value) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(v.as_i64().unwrap()).unwrap()
}

fn opt_ms(v: &Value) -> Option<DateTime<Utc>> {
    (!v.is_null()).then(|| ms(v))
}

fn int(v: &Value) -> i64 {
    v.as_i64().unwrap()
}

fn luxion_runs() -> Vec<rot::LuxionRun> {
    g("luxion_runs")
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            LuxionDoc {
                started_at: int(&r[0]),
                first_seen_at: ms(&r[1]),
                last_seen_at: ms(&r[1]),
                ended_at: opt_ms(&r[2]),
            }
            .run()
        })
        .collect()
}

fn bodies_at(us: i64, runs: &[rot::LuxionRun]) -> Vec<(&'static str, String)> {
    let p = pyjson::pydantic;
    vec![
        ("server_time", p(&bodies::server_time(at(us)))),
        ("daily_buffs", p(&bodies::daily_buffs(&DATA, us).unwrap())),
        ("weekly_buffs", p(&bodies::weekly_buffs(&DATA, us).unwrap())),
        ("corruxion", p(&bodies::corruxion(us))),
        ("fluxion", p(&bodies::fluxion(us))),
        ("gardening", p(&bodies::gardening(us))),
        ("biomes", p(&bodies::biomes(&DATA, us).unwrap())),
        ("wild_mana", p(&bodies::wild_mana(&DATA, us).unwrap())),
        ("stampy", p(&bodies::stampy(&DATA, us).unwrap())),
        ("challenge", p(&bodies::challenge_current(us, None))),
        ("calendar", p(&bodies::calendar(&DATA, us, runs).unwrap())),
    ]
}

#[test]
fn full_bodies_match_python() {
    let runs = luxion_runs();
    for (us, want) in g("full").as_object().unwrap() {
        for (route, body) in bodies_at(us.parse().unwrap(), &runs) {
            if let Some(w) = want.get(route) {
                assert_eq!(body, w.as_str().unwrap(), "{route} at {us}");
            }
        }
    }
}

#[test]
fn sweep_matches_python() {
    let runs = luxion_runs();
    let rows = g("sweep").as_array().unwrap();
    assert!(rows.len() > 300);
    for row in rows {
        let us = int(&row["us"]);
        for (route, body) in bodies_at(us, &runs) {
            assert_eq!(short(&body), row["h"][route].as_str().unwrap(), "{route} at {us}: {body}");
        }
        assert_eq!(rot::chaos_chest_window(us), (int(&row["chaos"][0]), int(&row["chaos"][1])), "chaos at {us}");
        assert_eq!(rot::delve_week_id(us), int(&row["week"]), "delve week at {us}");
        let pull = rot::delve_next_pull(us) as f64 / 1e6;
        assert_eq!(pull, row["pull"].as_f64().unwrap(), "delve pull at {us}");
    }
}

#[test]
fn luxion_matches_python() {
    let cases = g("luxion").as_array().unwrap();
    for c in cases {
        let doc = LuxionDoc {
            started_at: int(&c["anchor"]),
            first_seen_at: ms(&c["first_ms"]),
            last_seen_at: ms(&c["last_ms"]),
            ended_at: opt_ms(&c["ended_ms"]),
        };
        let body = pyjson::pydantic(&bodies::luxion(Some(&doc), int(&c["now_us"])));
        match c.get("body") {
            Some(want) => assert_eq!(body, want.as_str().unwrap()),
            None => assert_eq!(short(&body), c["h"].as_str().unwrap(), "{c}: {body}"),
        }
    }
    assert_eq!(pyjson::pydantic(&bodies::luxion(None, 0)), g("luxion_away").as_str().unwrap());
}

#[test]
fn chaos_matches_python() {
    for c in g("chaos").as_array().unwrap() {
        let now = int(&c["now_us"]);
        let chest = if let Some(cap) = c["capture"].as_object() {
            let item = chaos::capture_item(cap["name"].as_str().unwrap(), rot::chaos_chest_window(now));
            chaos::build(Some(&item), Some(ms(&cap["captured_ms"])), now)
        } else if let Some(feed) = c["feed"].as_object() {
            let first = feed["items"].as_array().unwrap().first().map(|i| i.as_object().unwrap().clone());
            chaos::build(first.as_ref(), Some(ms(&feed["fetched_ms"])), now)
        } else {
            chaos::build(None, None, now)
        }
        .unwrap();
        let resolved = chaos::needs_blueprint(&chest).and(c["resolved"].as_str().map(str::to_owned));
        let body = pyjson::pydantic(&chaos::body(&chest, resolved, "https://api.aallyn.net"));
        assert_eq!(body, c["body"].as_str().unwrap(), "{c}");
    }
    for u in g("image_urls").as_array().unwrap() {
        assert_eq!(chaos::image_url(u[0].as_str().unwrap(), u[1].as_str()).as_deref(), u[2].as_str(), "{u}");
    }
}

#[test]
fn normalizers_match_python() {
    for pair in g("chaos_normalize").as_array().unwrap() {
        let got = chaos::normalize(&pair[0]).map(Value::Object).unwrap_or(Value::Null);
        assert_eq!(got, pair[1], "{}", pair[0]);
    }
    for pair in g("delve_normalize").as_array().unwrap() {
        let (depths, total) = delves::normalize(&pair[0]);
        assert_eq!(Value::Array(depths), pair[1]["depths"], "{}", pair[0]);
        assert_eq!(total, pair[1]["total"], "{}", pair[0]);
    }
    for pair in g("classify").as_array().unwrap() {
        assert_eq!(rot::classify_challenge(pair[0].as_str()), pair[1].as_str(), "{}", pair[0]);
    }
}

#[test]
fn histories_match_python() {
    let h = g("histories");
    let check = |key: &str, body: Value| assert_eq!(pyjson::pydantic(&body), h[key]["body"].as_str().unwrap(), "{key}");
    let total = |key: &str| h[key]["total"].as_u64().unwrap();

    let lux: Vec<Value> = h["luxion_history"]["docs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| {
            let doc = LuxionDoc {
                started_at: int(&d[0]),
                first_seen_at: ms(&d[1]),
                last_seen_at: ms(&d[2]),
                ended_at: opt_ms(&d[3]),
            };
            bodies::luxion_history_item(&doc)
        })
        .collect();
    check("luxion_history", bodies::page(lux, total("luxion_history")));

    let chaos: Vec<Value> = h["chaos_history"]["docs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| bodies::chaos_history_item(int(&d[0]), d[1].as_str().unwrap(), &ms(&d[2])))
        .collect();
    check("chaos_history", bodies::page(chaos, total("chaos_history")));

    let challenge_doc = |d: &Value| ChallengeDoc {
        window_anchor: int(&d[0]),
        window_ends_at: int(&d[1]),
        name: d[2].as_str().unwrap().to_owned(),
        is_friday_window: d[3].as_bool().unwrap(),
        captured_at: ms(&d[4]),
    };
    let items = h["challenge_history"]["docs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| bodies::challenge_history_item(&challenge_doc(d)).unwrap())
        .collect();
    check("challenge_history", bodies::page(items, total("challenge_history")));

    for c in h["challenge_current"].as_array().unwrap() {
        let doc = c["doc"].as_array().map(|d| (d[0].as_str().unwrap(), ms(&d[1])));
        let body = pyjson::pydantic(&bodies::challenge_current(int(&c["now_us"]), doc));
        assert_eq!(body, c["body"].as_str().unwrap());
    }

    let stored_depths: Vec<Map<String, Value>> = h["delves"]["depths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| store::doc_to_json(&store::map_to_doc(d.as_object().unwrap()).unwrap()).unwrap())
        .collect();
    let docs: Vec<DelveDoc> = h["delves"]["docs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| DelveDoc {
            id: None,
            week: int(&d[0]),
            depths: if int(&d[2]) > 0 { stored_depths.clone() } else { Vec::new() },
            total: int(&d[1]),
            depth_count: int(&d[2]),
            fetched_at: ms(&d[3]),
        })
        .collect();
    for c in h["delves"]["cases"].as_array().unwrap() {
        let week = c["week"].as_i64();
        let current = int(&c["current"]);
        let doc = docs.iter().find(|d| d.week == week.unwrap_or(current));
        match bodies::delve_rotation(week, current, doc) {
            Ok(body) => assert_eq!(pyjson::pydantic(&body), c["body"].as_str().unwrap()),
            Err(msg) => {
                assert_eq!(c["status"], 404);
                assert_eq!(msg, c["message"].as_str().unwrap());
            }
        }
    }

    let rows: Vec<_> = h["delve_weeks"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (int(&r[0]), int(&r[1]), int(&r[2]), ms(&r[3])))
        .collect();
    check("delve_weeks", bodies::delve_week_list(int(&h["delve_weeks"]["current"]), &rows));
}

#[test]
fn blueprint_ranking() {
    assert_eq!(chaos::item_role_rank("prefabs/item/foo"), 0);
    assert_eq!(chaos::item_role_rank("Prefabs/Item/foo_notrade"), 1);
    assert_eq!(chaos::item_role_rank("prefabs/item/mount/foo"), 2);
    assert_eq!(chaos::item_role_rank("prefabs/collections/foo"), 3);
    assert_eq!(chaos::item_role_rank(""), 4);
    let rows = |v: &[(u8, &str)]| v.iter().map(|(r, b)| (*r, b.to_string())).collect::<Vec<_>>();
    assert_eq!(chaos::pick_blueprint(&rows(&[(3, "a"), (0, "b"), (0, "b")])).as_deref(), Some("b"));
    assert_eq!(chaos::pick_blueprint(&rows(&[(0, "a"), (0, "b"), (1, "c")])), None);
    assert_eq!(chaos::pick_blueprint(&[]), None);
}

#[test]
fn bson_round_trip_keeps_python_types() {
    let v = pyjson::loads(br#"{"a":1,"b":2.0,"c":3000000000,"d":[null,true,"x"],"e":{"z":1,"a":-0.0}}"#).unwrap();
    let doc = store::map_to_doc(v.as_object().unwrap()).unwrap();
    assert!(matches!(doc.get("a"), Some(mongodb::bson::Bson::Int32(1))));
    assert!(matches!(doc.get("c"), Some(mongodb::bson::Bson::Int64(3_000_000_000))));
    assert!(matches!(doc.get("b"), Some(mongodb::bson::Bson::Double(_))));
    assert_eq!(
        pyjson::pydantic(&Value::Object(store::doc_to_json(&doc).unwrap())),
        r#"{"a":1,"b":2.0,"c":3000000000,"d":[null,true,"x"],"e":{"z":1,"a":-0.0}}"#
    );
    let big = pyjson::loads(br#"{"n":18446744073709551615}"#).unwrap();
    assert!(store::map_to_doc(big.as_object().unwrap()).is_err());
}
