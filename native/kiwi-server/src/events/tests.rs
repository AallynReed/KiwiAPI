//! Golden checks from `native/port/parity/gen_events_fixtures.py` (the real
//! sources, bus.publish, scheduler and SSE generator over fakes and a pinned
//! clock).

use std::{sync::Arc, sync::LazyLock, time::Duration};

use chrono::{DateTime, Utc};
use futures::StreamExt;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use super::{
    bus, scheduler,
    sources::{self, GiveawayTop, NewsTop, SOURCES, Source, Version},
    stream,
};
use crate::{
    pyjson,
    routes::{
        rotations::{
            bodies::LuxionDoc,
            chaos,
            gamedata::{self, Gamedata},
        },
        trove_status,
    },
};

static DATA: LazyLock<Gamedata> = LazyLock::new(|| {
    Gamedata::load(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(gamedata::BASELINE_DIR))
});

static GOLDEN: LazyLock<Value> =
    LazyLock::new(|| pyjson::loads(include_bytes!("../../testdata/events/golden.json")).expect("golden.json"));

fn g(key: &str) -> &'static Value {
    &GOLDEN[key]
}

fn cases(key: &str) -> &'static [Value] {
    g(key).as_array().unwrap()
}

fn short(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))[..16].to_owned()
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

fn sig_of(v: &Value) -> Option<String> {
    v.as_str().map(str::to_owned)
}

fn dumps(v: &Value) -> String {
    pyjson::dumps_spaced(v)
}

fn check(source: Source, case: &Value, data: &Value, now: i64) {
    assert_eq!(dumps(data), case["data"].as_str().unwrap(), "{} data: {case}", source.kind());
    assert_eq!(source.sig(data, now), sig_of(&case["sig"]), "{} sig: {case}", source.kind());
}

#[test]
fn sources_match_python_registry() {
    let want: Vec<(String, bool)> =
        cases("sources").iter().map(|r| (r[0].as_str().unwrap().to_owned(), r[1].as_bool().unwrap())).collect();
    let got: Vec<(String, bool)> = SOURCES.iter().map(|s| (s.kind().to_owned(), s.scheduled())).collect();
    assert_eq!(got, want);
    assert_eq!(Source::by_type("wild_mana"), Some(Source::WildMana));
    assert_eq!(Source::by_type("leaderboard"), None);
}

#[test]
fn timed_sources_match_python_sweep() {
    let mut n = 0;
    for row in cases("timed") {
        let us = int(&row["us"]);
        for source in SOURCES.into_iter().filter(|s| s.scheduled()) {
            let want = &row[source.kind()];
            let data = source.timed_data(&DATA, us).unwrap();
            let text = dumps(&data);
            assert_eq!(short(&text), want["h"].as_str().unwrap(), "{} data at {us}: {text}", source.kind());
            assert_eq!(source.sig(&data, us), sig_of(&want["sig"]), "{} sig at {us}", source.kind());
            let next = source.next_at(&DATA, us);
            assert_eq!(next, want["next"].as_i64(), "{} next at {us}", source.kind());
            let delay = scheduler::delay(next, kiwi_core::rotations::unix(us));
            assert_eq!(delay, int(&want["delay"]), "{} delay at {us}", source.kind());
            n += 1;
        }
    }
    assert!(n > 1500, "{n}");
    for (us, texts) in g("timed_full").as_object().unwrap() {
        let us: i64 = us.parse().unwrap();
        for (kind, text) in texts.as_object().unwrap() {
            let data = Source::by_type(kind).unwrap().timed_data(&DATA, us).unwrap();
            assert_eq!(dumps(&data), text.as_str().unwrap(), "{kind} at {us}");
        }
    }
}

#[test]
fn scheduler_delay_clamps() {
    assert_eq!(scheduler::delay(None, 100), 3600);
    assert_eq!(scheduler::delay(Some(0), 100), 3600);
    assert_eq!(scheduler::delay(Some(100), 100), 5);
    assert_eq!(scheduler::delay(Some(110), 100), 12);
    assert_eq!(scheduler::delay(Some(100_000), 100), 3600);
}

#[test]
fn challenge_matches_python() {
    for case in cases("challenge") {
        let now = int(&case["now_us"]);
        let doc = case["doc"].as_array().map(|d| (d[0].as_str().unwrap(), ms(&d[1])));
        check(Source::Challenge, case, &sources::challenge_data(now, doc), now);
    }
}

#[test]
fn chaos_matches_python() {
    let api_url = &crate::config::tests::settings_from(&Map::new()).unwrap().api_url;
    for case in cases("chaos") {
        let now = int(&case["now_us"]);
        let week = kiwi_core::rotations::chaos_chest_window(now);
        let chest = if let Some(cap) = case["capture"].as_object() {
            let item = chaos::capture_item(cap["name"].as_str().unwrap(), week);
            chaos::build(Some(&item), Some(ms(&cap["captured_ms"])), now)
        } else if let Some(feed) = case["feed"].as_object() {
            let cached = feed["items"].as_array().unwrap().first().map(|i| i.as_object().unwrap().clone());
            chaos::build(cached.as_ref(), Some(ms(&feed["fetched_ms"])), now)
        } else {
            chaos::build(None, None, now)
        }
        .unwrap();
        let resolved = chaos::needs_blueprint(&chest).and_then(|_| case["resolved"].as_str().map(str::to_owned));
        check(Source::Chaos, case, &sources::chaos_data(&chest, resolved, api_url), now);
    }
}

#[test]
fn luxion_matches_python() {
    for case in cases("luxion") {
        let now = int(&case["now_us"]);
        let doc = case["doc"].as_array().map(|d| LuxionDoc {
            started_at: int(&d[0]),
            first_seen_at: ms(&d[1]),
            last_seen_at: ms(&d[2]),
            ended_at: opt_ms(&d[3]),
        });
        check(Source::Luxion, case, &sources::luxion_data(doc.as_ref(), now), now);
    }
}

#[test]
fn server_status_matches_python() {
    for case in cases("server_status") {
        let data = match case["snapshot"].as_str() {
            Some(text) => pyjson::loads(text.as_bytes()).unwrap(),
            None => trove_status::unknown_snapshot(),
        };
        check(Source::ServerStatus, case, &data, 0);
    }
}

#[test]
fn news_giveaways_and_game_update_match_python() {
    for case in cases("trove_news") {
        let top = case["item"].as_array().map(|i| NewsTop {
            title: i[0].as_str().unwrap().into(),
            url: i[1].as_str().unwrap().into(),
            published_at: opt_ms(&i[2]),
        });
        check(Source::TroveNews, case, &sources::news_data(top.as_ref()), 0);
    }
    for case in cases("giveaways") {
        let top = case["doc"].as_array().map(|d| GiveawayTop {
            id: d[0].as_str().unwrap().into(),
            title: d[1].as_str().unwrap().into(),
            created_at: int(&d[2]) / 1000,
        });
        check(Source::Giveaways, case, &sources::giveaways_data(top.as_ref()), 0);
    }
    for case in cases("game_update") {
        let v = case["version"].as_array().map(|v| Version {
            branch: v[0].as_str().unwrap().into(),
            ordinal: int(&v[1]),
            version_tag: v[2].as_str().unwrap().into(),
            files_added: int(&v[3]),
            files_modified: int(&v[4]),
            files_removed: int(&v[5]),
            bytes_added: int(&v[6]),
            completed_at: opt_ms(&v[7]),
        });
        check(Source::GameUpdate, case, &sources::game_update_data(v.as_ref()), 0);
    }
}

#[test]
fn publish_writes_what_python_writes() {
    let names = crate::config::tests::settings_from(&Map::new()).unwrap();
    let set = g("settings");
    assert_eq!(names.events_channel, set["events_channel"].as_str().unwrap());
    assert_eq!(names.webhooks_queue, set["webhooks_queue"].as_str().unwrap());
    assert_eq!(names.dm_subs_queue, set["dm_subs_queue"].as_str().unwrap());
    let mut seen = std::collections::HashMap::new();
    for case in cases("publish") {
        let kind = case["type"].as_str().unwrap();
        let sig = case["sig"].as_str().unwrap();
        let calls = case["calls"].as_array().unwrap();
        assert_eq!(calls[0], serde_json::json!(["SET", format!("events:sig:{kind}"), sig, "GET"]));
        let emitted = seen.insert(kind, sig) != Some(sig);
        assert_eq!(emitted, case["emitted"].as_bool().unwrap(), "{case}");
        if !emitted {
            assert_eq!(calls.len(), 1);
            continue;
        }
        let ts = calls[1][2].as_str().map(|t| pyjson::loads(t.as_bytes()).unwrap()["ts"].as_i64().unwrap()).unwrap();
        let data = pyjson::loads(case["data"].as_str().unwrap().as_bytes()).unwrap();
        let text = dumps(&bus::envelope(kind, data, ts));
        let mut want = vec![serde_json::json!(["PUBLISH", names.events_channel, text])];
        if crate::webhooks::model::EVENT_TYPES.contains(&kind) && case["webhooks"].as_bool().unwrap() {
            want.push(serde_json::json!(["LPUSH", names.webhooks_queue, text]));
        }
        if crate::dm_subs::model::BUS_EVENTS.contains(&kind) && case["dm"].as_bool().unwrap() {
            want.push(serde_json::json!(["LPUSH", names.dm_subs_queue, text]));
        }
        assert_eq!(&calls[1..], &want[..], "{kind}");
    }
}

#[test]
fn frames_match_python() {
    let f = g("frames");
    for row in f["payloads"].as_array().unwrap() {
        let want = row[1].as_str().unwrap();
        match stream::frame(&row[0]) {
            Some(text) => assert_eq!(text, want),
            None => assert!(want.ends_with("Error"), "{row}"),
        }
    }
    for row in f["messages"].as_array().unwrap() {
        let raw = row[0].as_str().unwrap();
        let want = row[1].as_str().unwrap();
        match bus::decode_message(raw.as_bytes()) {
            None => assert_eq!(want, "skip", "{raw}"),
            Some(payload) => match stream::frame(&payload) {
                Some(text) => assert_eq!(text, want, "{raw}"),
                None => assert!(want.ends_with("Error"), "{raw}"),
            },
        }
    }
}

/// Every check that touches the process-wide subscriber registry, in one test.
#[tokio::test]
async fn stream_matches_python_generator() {
    let s = g("stream");
    for name in ["full", "empty", "bad"] {
        let case = &s[name];
        let snapshot: Vec<Value> = case["snapshot"].as_array().unwrap().clone();
        let live: Vec<Option<Value>> =
            case["live"].as_array().unwrap().iter().map(|v| (!v.is_null()).then(|| v.clone())).collect();
        let waits: Vec<i64> = case["waits"].as_array().unwrap().iter().map(int).collect();
        let configured = match name {
            "full" => 20,
            "empty" => 3,
            _ => 7,
        };
        assert!(waits.iter().all(|w| Duration::from_secs(*w as u64) == stream::heartbeat(configured)));

        let first = live.first().cloned().flatten();
        let subscribe = move || {
            let sub = bus::subscribe();
            if let Some(ev) = first {
                bus::broadcast_local(Arc::new(ev));
            }
            sub
        };
        let has_snapshot = !snapshot.is_empty();
        let mut body = Box::pin(stream::body(async move { snapshot }, subscribe, Duration::from_millis(40)));
        let mut out = Vec::new();
        out.push(body.next().await.unwrap().unwrap());
        if has_snapshot {
            out.push(body.next().await.unwrap().unwrap());
        }
        let mut errored = false;
        for (i, item) in live.iter().enumerate() {
            if i > 0
                && let Some(ev) = item
            {
                bus::broadcast_local(Arc::new(ev.clone()));
            }
            match body.next().await {
                Some(Ok(chunk)) => out.push(chunk),
                Some(Err(_)) => {
                    errored = true;
                    break;
                }
                None => break,
            }
        }
        assert_eq!(bus::connection_count(), usize::from(!errored));
        drop(body);
        assert_eq!(bus::connection_count(), 0);
        let text: Vec<u8> = out.concat();
        assert_eq!(String::from_utf8(text).unwrap(), case["body"].as_str().unwrap(), "{name}");
        assert_eq!(errored, !case["error"].is_null(), "{name}");

        let res = stream::response(futures::stream::empty());
        let headers: Vec<Value> =
            res.headers().iter().map(|(k, v)| serde_json::json!([k.as_str(), v.to_str().unwrap()])).collect();
        assert_eq!(&headers, case["headers"].as_array().unwrap());
    }
    let cap = &s["capacity"];
    let err = stream::at_capacity();
    assert_eq!(i64::from(err.status.as_u16()), int(&cap["status"]));
    assert_eq!(err.code, cap["code"].as_str().unwrap());
    assert_eq!(err.message, cap["message"].as_str().unwrap());
}

#[test]
fn signature_edges() {
    let sig = |s: Source, d: &Value| s.sig(d, 0);
    assert_eq!(sig(Source::Activity, &serde_json::json!({"window_start": 5})), Some("5".into()));
    assert_eq!(sig(Source::ServerStatus, &serde_json::json!({"overall": null})), Some("status:None".into()));
    assert_eq!(sig(Source::ServerStatus, &serde_json::json!({})), None);
    assert_eq!(sig(Source::Giveaways, &serde_json::json!({"newest": {}})), None);
}
