//! Golden tests against `testdata/site_live/*.json`
//! (native/port/parity/gen_site_live_fixtures.py).

use std::{path::PathBuf, sync::Arc};

use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use hyper_util::{client::legacy::Client, rt::TokioExecutor};
use mongodb::bson::{Bson, Document};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tower::ServiceExt;

use super::{
    clubs, crawler,
    flags::{self, Flags},
    reads, rotations, search, search_index as index,
};
use crate::{config::Settings, db::Db, dt, pyjson, state::AppState, state::Inner};

pub fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/site_live").join(name);
    serde_json::from_slice(&std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
        .expect("fixture JSON")
}

fn gamedata_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/gamedata")
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

/// The body a case recorded, or its sha256 when it was too large to keep.
pub fn assert_body(case: &Value, got: &str, ctx: &str) {
    if let Some(sha) = case.get("body_sha256").and_then(Value::as_str) {
        let digest = hex::encode(Sha256::digest(got.as_bytes()));
        assert_eq!(
            digest,
            sha,
            "{ctx}: body differs (len {} vs {})\n  rust: {}\n  python: {}",
            got.len(),
            case["body_len"],
            &got[..got.len().min(1500)],
            s(&case["body_head"])
        );
    } else {
        assert_eq!(got, s(&case["body"]), "{ctx}");
    }
}

fn flags_from(v: &Value) -> Flags {
    Flags(v.as_object().expect("flags").iter().map(|(k, v)| (k.clone(), v.as_bool().unwrap_or(false))).collect())
}

fn settings() -> Settings {
    Settings::from_vars([("PY_UPSTREAM", "http://py:8000")]).unwrap()
}

fn state() -> AppState {
    let upstream = Client::builder(TokioExecutor::new()).build_http();
    AppState {
        inner: Arc::new(Inner { settings: settings(), db: Db { mongo: None, redis: None, pg: None }, upstream }),
    }
}

#[test]
fn flag_map_matches_python() {
    let f = fixture("flags.json");
    let keys: Vec<&str> = flags::SITE_FEATURE_FLAGS.iter().map(|(k, _)| *k).collect();
    let flag_names: Vec<&str> = flags::SITE_FEATURE_FLAGS.iter().map(|(_, f)| *f).collect();
    assert_eq!(serde_json::to_value(&keys).unwrap(), f["keys"]);
    assert_eq!(serde_json::to_value(&flag_names).unwrap(), f["flags"]);
    let paths: Vec<String> = {
        let mut all = Vec::new();
        for case in f["cases"].as_array().unwrap() {
            for p in case["blocked"].as_array().unwrap() {
                all.push(s(p).to_owned());
            }
        }
        all.extend(
            [
                "/site/server-time",
                "/site/feeds/news",
                "/robots.txt",
                "/site/search",
                "/site/gems/parse-star-chart",
                "/abilities",
                "/guides",
                "/allies",
                "/gem-tools",
                "/",
            ]
            .map(str::to_owned),
        );
        all.sort();
        all.dedup();
        all
    };
    for case in f["cases"].as_array().unwrap() {
        let off: Vec<&str> = case["off"].as_array().unwrap().iter().map(s).collect();
        let mut got = Flags(keys.iter().map(|k| ((*k).to_owned(), !off.contains(k))).collect());
        flags::apply_derived(&mut got);
        assert_eq!(got.to_json(), case["resolved"], "{}", case["name"]);
        let blocked: Vec<&str> = case["blocked"].as_array().unwrap().iter().map(s).collect();
        for p in &paths {
            assert_eq!(flags::feature_blocks(p, &got), blocked.contains(&p.as_str()), "{} {p}", case["name"]);
        }
        let pages: Vec<&str> = flags::SITEMAP_PAGES
            .iter()
            .filter(|(_, a)| a.is_none_or(|a| got.get_or(a, true)))
            .map(|(p, _)| *p)
            .collect();
        assert_eq!(serde_json::to_value(&pages).unwrap(), case["sitemap"], "{}", case["name"]);
    }
    let st = settings();
    for r in f["robots"].as_array().unwrap() {
        assert_eq!(flags::robots_body(&st, s(&r["host"])), s(&r["body"]), "{}", r["host"]);
    }
}

#[tokio::test]
async fn http_answers_match_python() {
    let app = super::router().with_state(state());
    for case in fixture("http.json").as_array().unwrap() {
        if case["flags"].as_object().is_some_and(|m| !m.is_empty()) {
            continue;
        }
        let mut req = Request::builder().uri(s(&case["path"]));
        for (k, v) in case["headers"].as_object().unwrap() {
            req = req.header(k.as_str(), s(v));
        }
        let res = app.clone().oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
        let name = s(&case["name"]);
        assert_eq!(res.status().as_u16() as u64, case["status"].as_u64().unwrap(), "{name}");
        let header = |h: &str| res.headers().get(h).and_then(|v| v.to_str().ok()).map(str::to_owned);
        assert_eq!(header("content-type").as_deref(), case["content_type"].as_str(), "{name}");
        assert_eq!(header("cache-control").as_deref(), case["cache_control"].as_str(), "{name}");
        let body = res.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(String::from_utf8_lossy(&body), s(&case["body"]), "{name}");
    }
}

fn lookup(flags: &Value) -> impl Fn(&str) -> Option<bool> + '_ {
    move |k| flags.get(k).and_then(Value::as_bool)
}

#[test]
fn search_index_matches_python() {
    let f = fixture("search_index.json");
    let variants = &f["variants"];
    for case in f["cases"].as_array().unwrap() {
        let flags = &variants[s(&case["variant"])];
        let q = s(&case["query"]);
        let got: Vec<Value> = index::search_destinations(q, &lookup(flags), 200)
            .iter()
            .map(|d| serde_json::json!([d.name, index::score(d, q)]))
            .collect();
        assert_eq!(Value::Array(got), case["hits"], "{} {q:?}", case["variant"]);
    }
    for (name, want) in f["nav"].as_object().unwrap() {
        let flags = &variants[name];
        let menu: Vec<Value> = index::nav_menu(&lookup(flags))
            .iter()
            .map(|(g, items)| serde_json::json!([g, items.iter().map(|d| d.name).collect::<Vec<_>>()]))
            .collect();
        assert_eq!(Value::Array(menu), want["menu"], "{name}");
        assert_eq!(serde_json::to_value(index::nav_paths(&lookup(flags))).unwrap(), want["paths"], "{name}");
        for (p, active) in want["active"].as_object().unwrap() {
            assert_eq!(Value::Bool(index::nav_is_active(p, &lookup(flags))), *active, "{name} {p}");
        }
    }
    for (raw, norm) in f["normalize"].as_object().unwrap() {
        assert_eq!(index::normalize(raw), s(norm), "{raw:?}");
    }
}

#[test]
fn search_assembly_matches_python() {
    let f = fixture("search.json");
    for case in f["cases"].as_array().unwrap() {
        let flags = flags_from(&f["flags"][s(&case["flags"])]);
        let query = kiwi_core::pystr::strip(s(&case["query"])).to_owned();
        let subject = case["subject"].as_str();
        let (limit, offset) = (case["limit"].as_i64().unwrap(), case["offset"].as_i64().unwrap());
        let subjects = search::available_subjects(&flags);
        let ctx = format!("{} {query:?} {subject:?} {limit} {offset}", case["flags"]);
        let got = if query.chars().count() < search::MIN_QUERY
            || subjects.is_empty()
            || subject.is_some_and(|x| !subjects.contains(&x))
        {
            search::empty(&query, subject)
        } else {
            let mut calls = case["calls"].as_array().unwrap().iter();
            let mut backend = |sub: &str, limit: i64, offset: i64| {
                if sub == index::SUBJECT_PAGES {
                    return search::pages(&query, &flags, limit, offset);
                }
                let c = calls.next().expect("recorded call");
                assert_eq!(
                    (s(&c["subject"]), c["limit"].as_i64(), c["offset"].as_i64()),
                    (sub, Some(limit), Some(offset)),
                    "{ctx}"
                );
                (c["rows"].as_array().unwrap().clone(), c["total"].as_i64().unwrap())
            };
            let preview: Vec<(Vec<Value>, i64)> =
                subjects.iter().map(|x| backend(x, search::PREVIEW_PER_SUBJECT, 0)).collect();
            let paged = subject.map(|x| backend(x, limit, offset));
            search::assemble(&query, &subjects, subject, preview, paged)
        };
        assert_eq!(pyjson::dumps(&got), pyjson::dumps(&case["out"]), "{ctx}");
    }
}

#[test]
fn search_rows_match_python() {
    let f = fixture("search_rows.json");
    let rows: Vec<Map<String, Value>> =
        f["codex_rows"].as_array().unwrap().iter().map(|r| r.as_object().unwrap().clone()).collect();
    for (subject, want) in f["codex"].as_object().unwrap() {
        let types = index::codex_types(subject);
        let got: Vec<Value> = rows
            .iter()
            .filter(|r| {
                types.len() <= 1 || r.get("codex_type").and_then(Value::as_str).is_some_and(|t| types.contains(&t))
            })
            .map(search::codex_row)
            .collect();
        assert_eq!(pyjson::dumps(&Value::Array(got)), pyjson::dumps(want), "{subject}");
    }
    let names: Vec<&str> = f["names"].as_array().unwrap().iter().map(s).collect();
    let got: Vec<Value> = names.iter().skip(1).take(2).map(|n| search::player_row(n)).collect();
    assert_eq!(serde_json::json!([got, names.len()]), f["players"]);
    for (key, modpacks) in [("mods", false), ("modpacks", true)] {
        let got: Vec<Value> =
            f["cards"].as_array().unwrap().iter().filter_map(|c| search::mod_row(c, modpacks)).collect();
        assert_eq!(pyjson::dumps(&serde_json::json!([got, 42])), pyjson::dumps(&f[key]), "{key}");
    }
}

fn ts(v: &Value) -> Option<chrono::DateTime<chrono::Utc>> {
    v.as_i64().and_then(|t| chrono::DateTime::from_timestamp(t, 0))
}

#[test]
fn rotations_match_python() {
    let data = crate::routes::rotations::gamedata::Gamedata::load(&gamedata_dir());
    for case in fixture("rotations.json").as_array().unwrap() {
        let now = case["now_us"].as_i64().unwrap();
        let now_dt = dt::parse_utc(s(&case["now"])).unwrap();
        let lux = case["lux"].as_object().map(|l| crate::routes::rotations::bodies::LuxionDoc {
            started_at: l["started_at"].as_i64().unwrap(),
            first_seen_at: ts(&l["first_seen_at"]).unwrap(),
            last_seen_at: ts(&l["last_seen_at"]).unwrap(),
            ended_at: ts(&l["ended_at"]),
        });
        let c = &case["chaos"];
        let chaos = rotations::Chaos {
            starts_at: c["starts_at"].as_i64().unwrap(),
            ends_at: c["ends_at"].as_i64().unwrap(),
            seconds_remaining: c["seconds_remaining"].as_i64().unwrap(),
            item: c["item"].clone(),
        };
        let got = pyjson::dumps(&rotations::body(&data, now, now_dt, lux.as_ref(), &chaos));
        assert_body(case, &got, &format!("{} {:?}", case["now"], case["lux"]));
        assert_eq!(s(&case["cache_control"]), "public, max-age=30");
    }
}

#[test]
fn yearly_calendar_matches_python() {
    let data = crate::routes::rotations::gamedata::Gamedata::load(&gamedata_dir());
    for case in fixture("calendar.json").as_array().unwrap() {
        let runs: Vec<kiwi_core::rotations::LuxionRun> = case["runs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| kiwi_core::rotations::LuxionRun {
                day_anchor: r["day_anchor"].as_i64().unwrap(),
                first_seen: r["first_seen"].as_i64(),
                ended: r["ended"].as_i64(),
            })
            .collect();
        let got = rotations::yearly_body(&data, case["now_us"].as_i64().unwrap(), &runs).unwrap();
        assert_body(case, &pyjson::dumps(&got), &format!("{} {:?}", case["now_us"], case["runs"]));
    }
}

#[test]
fn classes_match_python() {
    let case = fixture("classes.json");
    let gd = crate::gamedata::Gamedata::new(gamedata_dir(), None);
    let got = String::from_utf8(reads::classes_body(&gd).expect("classes")).unwrap();
    assert_body(&case, &got, "classes");
    assert_eq!(s(&case["cache_control"]), "public, max-age=3600");
}

/// A fixture dict as the Mongo document Python read it from.
pub fn doc_of(v: &Value, dates: &[&str]) -> Document {
    let mut d = Document::new();
    for (k, x) in v.as_object().unwrap() {
        let b = match x {
            Value::String(t) if dates.contains(&k.as_str()) => Bson::DateTime(dt::to_bson(dt::parse_utc(t).unwrap())),
            other => crate::routes::rotations::store::to_bson(other).unwrap(),
        };
        d.insert(k.clone(), b);
    }
    d
}

#[test]
fn feeds_match_python() {
    let f = fixture("feeds.json");
    let news: Vec<crate::feeds::store::News> = f["news"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| crate::feeds::store::News::from_doc(&doc_of(n, &["published_at"])))
        .collect();
    let items: Vec<Value> = news.iter().map(reads::news_item).collect();
    assert_eq!(pyjson::dumps(&serde_json::json!({"items": items})), s(&f["news_body"]));
    for v in f["videos"].as_array().unwrap() {
        let platform = s(&v["platform"]);
        let feed = v["fetched_at"].as_str().map(|at| {
            let items = v["items"].as_array().unwrap().iter().map(|i| doc_of(i, &[])).collect();
            (items, dt::parse_utc(at).unwrap())
        });
        let shown = if matches!(platform, "youtube" | "twitch") { platform } else { "youtube" };
        assert_eq!(pyjson::dumps(&reads::videos_body(shown, feed)), s(&v["body"]), "{platform}");
    }
    let now = f["now"].as_i64().unwrap();
    let events: Vec<Value> = f["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| reads::calendar_event(&crate::feeds::store::Event::from_doc(&doc_of(e, &[])), now))
        .collect();
    assert_eq!(pyjson::dumps(&Value::Array(events)), s(&f["event_rows"]));
}

#[test]
fn clubs_match_python() {
    for case in fixture("clubs.json").as_array().unwrap() {
        let clubs: Vec<clubs::Club> =
            case["clubs"].as_array().unwrap().iter().map(|c| clubs::Club::from_doc(&doc_of(c, &[])).unwrap()).collect();
        let entries: Vec<(String, i64)> =
            case["entries"].as_array().unwrap().iter().map(|e| (s(&e[0]).to_owned(), e[1].as_i64().unwrap())).collect();
        let items = clubs::ordered(clubs, &entries);
        let count = items.len();
        assert_eq!(
            pyjson::dumps(&serde_json::json!({"items": items, "count": count})),
            s(&case["body"]),
            "{}",
            case["name"]
        );
    }
}

#[test]
fn sitemap_matches_python() {
    let f = fixture("sitemap.json");
    let list = |k: &str| f[k].as_array().unwrap().clone();
    for case in f["cases"].as_array().unwrap() {
        let overrides = case["flags"].as_object().unwrap();
        let mut got = Flags(
            flags::SITE_FEATURE_FLAGS
                .iter()
                .map(|(k, flag)| ((*k).to_owned(), overrides.get(*flag).and_then(Value::as_bool).unwrap_or(true)))
                .collect(),
        );
        flags::apply_derived(&mut got);
        let body = crawler::render(s(&f["base"]), &got, &list("mods"), &list("modders"), &list("packs"));
        assert_eq!(body, s(&case["body"]), "{}", case["name"]);
    }
}

#[tokio::test]
async fn sitemap_caps_each_section() {
    let pages = std::sync::Mutex::new(Vec::new());
    let cards = crawler::all_cards(
        |offset| {
            pages.lock().unwrap().push(offset);
            async move {
                let rows =
                    (0..1000).map(|i| serde_json::json!({"handle": "h", "slug": format!("{}", offset + i)})).collect();
                Ok((rows, 30_000))
            }
        },
        2500,
        "mods",
    )
    .await
    .unwrap();
    assert_eq!(cards.len(), 2500);
    assert_eq!(*pages.lock().unwrap(), vec![0, 1000, 2000]);
}

#[test]
fn screenshots_match_python() {
    let f = fixture("screenshots.json");
    let root = std::env::temp_dir().join(format!("kiwi-site-shots-{}", std::process::id()));
    let folder = root.join("static").join("trove-screens");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&folder).unwrap();
    for n in f["files"].as_array().unwrap() {
        std::fs::write(folder.join(s(n)), b"x").unwrap();
    }
    for d in f["dirs"].as_array().unwrap() {
        std::fs::create_dir_all(folder.join(s(d))).unwrap();
    }
    let got = pyjson::dumps(&reads::screenshots_body(root.to_str().unwrap()));
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(got, s(&f["body"]));
    assert_eq!(pyjson::dumps(&reads::screenshots_body("/nonexistent/kiwi")), s(&f["missing_root_body"]));
    assert_eq!(s(&f["cache_control"]), "public, max-age=60");
}
