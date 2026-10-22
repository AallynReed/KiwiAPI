//! Golden tests against `testdata/gems/*.json` (native/port/parity/gen_gems_fixtures.py):
//! every route body below the scope check, byte for byte, random routes from
//! the same seed as Python.

use std::path::PathBuf;

use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue, header},
};
use kiwi_core::gems::{builds, pyrandom::PyRandom};
use regex::Regex;
use serde_json::Value;

use super::*;
use crate::{pyjson, validation::Params};

fn store() -> Gamedata {
    Gamedata::new(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../app/trove/gamedata"), None)
}

fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/gems").join(name);
    serde_json::from_slice(&std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
        .expect("fixture JSON")
}

fn seed(n: u64) {
    RNG.with(|c| *c.borrow_mut() = Some(PyRandom::seed(n)));
}

fn render(reply: Reply) -> (u16, String) {
    match reply {
        Reply::Json(v) => (200, pyjson::pydantic(&v)),
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

async fn post<B: FromBody>(case: &Value, run: fn(&Gamedata, B) -> Reply, s: &Gamedata) -> (u16, String) {
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
        Ok(req) => render(run(s, req)),
        Err(e) => render(Reply::Error(e)),
    }
}

async fn dispatch(s: &Gamedata, case: &Value) -> (u16, String) {
    let path = case["path"].as_str().expect("path");
    let (route, qs) = path.split_once('?').unwrap_or((path, ""));
    match route.trim_start_matches("/v1/gems/") {
        "lookups" => render(run_lookups(s)),
        "builds/options" => render(run_build_options(s)),
        "level-costs" => query::<LevelCostsQ>(qs, |q| run_level_costs(s, &q)),
        "stat-range" => query::<StatRangeQ>(qs, |q| run_stat_range(s, &q)),
        "generate" => post(case, run_generate, s).await,
        "augment" => post(case, run_augment, s).await,
        "spark" => post(case, run_spark, s).await,
        "flare" => post(case, run_flare, s).await,
        "level-up" => post(case, run_level_up, s).await,
        "set-level" => post(case, run_set_level, s).await,
        "evaluate" => post(case, run_evaluate, s).await,
        "evaluate-simple" => post(case, run_evaluate_simple, s).await,
        "builds/calculate" => post(case, run_builds, s).await,
        "builds/health-optimizer" => post(case, run_health, s).await,
        other => panic!("unknown route {other}"),
    }
}

#[tokio::test]
async fn routes_match_python() {
    let s = store();
    let id = Regex::new(r#""id":\d+"#).expect("regex");
    let cases = fixture("routes.json");
    let cases = cases.as_array().expect("cases");
    let mut failed = Vec::new();
    for case in cases {
        seed(case["seed"].as_u64().unwrap_or(0));
        let (status, mut body) = dispatch(&s, case).await;
        if case["mask_id"].as_bool() == Some(true) {
            body = id.replace_all(&body, r#""id":0"#).into_owned();
        }
        let name = case["name"].as_str().unwrap_or_default();
        if status != case["status"].as_u64().unwrap_or(0) as u16 || body != case["body"].as_str().unwrap_or_default() {
            failed.push(format!(
                "{name}: {status} vs {}\n  rust:   {}\n  python: {}",
                case["status"],
                &body[..body.len().min(600)],
                &case["body"].as_str().unwrap_or_default()[..case["body"].as_str().unwrap_or_default().len().min(600)]
            ));
        }
    }
    assert!(
        failed.is_empty(),
        "{} of {} cases differ:\n{}",
        failed.len(),
        cases.len(),
        failed[..failed.len().min(8)].join("\n")
    );
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64()
}

#[test]
fn builds_match_python() {
    let s = store();
    let d = data::builds(&s);
    let gem_stats = d.gem_stats.as_ref().expect("gem data");
    let foods: Vec<(String, Vec<_>)> = d.foods.iter().map(|f| (f.key.clone(), f.stats.clone())).collect();
    let allies: Vec<(String, Vec<_>)> = d.allies.iter().map(|a| (a.key.clone(), a.stats.clone())).collect();
    let sums = |name: &str| d.sums.get(name).copied().unwrap_or(0.0);
    let input = builds::BuildData {
        classes: &d.classes,
        foods: &foods,
        allies: &allies,
        face_damage: d.face_damage,
        sums: &sums,
        gem_stats,
    };
    for row in fixture("builds.json").as_array().expect("rows") {
        let c = &row["config"];
        let s = |k: &str| c[k].as_str().unwrap_or_default().to_owned();
        let b = |k: &str| c[k].as_bool().unwrap_or_default();
        let config = BuildConfig {
            build_type: s("build_type"),
            character: s("character"),
            subclass: s("subclass"),
            food: s("food"),
            ally: s("ally"),
            ally_buff: b("ally_buff"),
            berserker_battler: b("berserker_battler"),
            critical_damage_count: c["critical_damage_count"].as_i64().unwrap_or(3),
            no_face: b("no_face"),
            light: c["light"].as_i64().unwrap_or(0),
            subclass_active: b("subclass_active"),
            litany: b("litany"),
            bounty_hunt: b("bounty_hunt"),
            high_precision: b("high_precision"),
        };
        let chart = c["star_chart"].as_str().filter(|x| !x.is_empty()).map(|x| d.chart.parse(x));
        let got = builds::calculate(&input, &config, chart.as_ref());
        if let Some(err) = row["error"].as_str() {
            assert_eq!(got.err().as_deref(), Some(err), "{c}");
            continue;
        }
        let got = got.expect("rows");
        let want = row["results"].as_array().expect("results");
        assert_eq!(row["count"].as_u64().map_or(want.len(), |n| n as usize), got.len(), "{c}");
        for (g, w) in got.iter().zip(want) {
            let ctx = format!("{c} rank {}", g.rank);
            assert_eq!(w["rank"].as_u64(), Some(g.rank as u64), "{ctx}");
            assert_eq!(w["layout"].as_str(), Some(g.layout.as_str()), "{ctx}");
            for (k, v) in [
                ("base_dmg", g.base_dmg),
                ("crit_dmg", g.crit_dmg),
                ("light", g.light),
                ("bonus_dmg", g.bonus_dmg),
                ("crit_bonus", g.crit_bonus),
                ("total_dmg", g.total_dmg),
                ("coefficient", g.coefficient),
            ] {
                assert_eq!(num(&w[k]), Some(v), "{ctx} {k}");
            }
            assert_eq!(num(&w["class_bonus"]), g.class_bonus, "{ctx} class_bonus");
        }
    }
}

#[test]
fn star_chart_matches_python() {
    let s = store();
    let d = data::builds(&s);
    for row in fixture("star_chart.json").as_array().expect("rows") {
        let code = row["code"].as_str().expect("code");
        let chart = d.chart.parse(code);
        let want = row["stats"].as_object().expect("stats");
        assert_eq!(want.len(), chart.stats.len(), "{code:?}");
        for (name, flat, pct) in &chart.stats {
            let w = &want[name];
            assert_eq!(num(&w["flat"]), Some(*flat), "{code:?} {name}");
            assert_eq!(num(&w["pct"]), Some(*pct), "{code:?} {name}");
        }
        let bounty = &row["bounty_hunt"];
        match chart.bounty {
            Some((physical, magic)) => {
                assert_eq!(bounty["available"], Value::Bool(true), "{code:?}");
                assert_eq!(num(&bounty["physical"]), Some(physical), "{code:?}");
                assert_eq!(num(&bounty["magic"]), Some(magic), "{code:?}");
            }
            None => assert_eq!(bounty["available"], Value::Bool(false), "{code:?}"),
        }
    }
}

#[test]
fn base64_like_binascii() {
    use super::star_chart::a2b_base64;
    assert_eq!(a2b_base64(b"YWJj"), Some(b"abc".to_vec()));
    assert_eq!(a2b_base64(b"YW=Jj"), Some(b"abc".to_vec()));
    assert_eq!(a2b_base64(b"YQ==garbage"), Some(b"a".to_vec()));
    assert_eq!(a2b_base64(b"Y!W*J j"), Some(b"abc".to_vec()));
    assert_eq!(a2b_base64(b"YWJ"), None);
    assert_eq!(a2b_base64(b"Y"), None);
    assert_eq!(a2b_base64(b"=YWJj"), Some(b"abc".to_vec()));
}

/// The full stack without a token: every answer here comes before any store.
async fn call(method: &str, path: &str, ct: Option<&str>, body: &str) -> (u16, HeaderMap, Value) {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    if std::env::var("PY_UPSTREAM").is_err() {
        // SAFETY: set once before any thread reads it; the value is constant.
        unsafe { std::env::set_var("PY_UPSTREAM", "http://127.0.0.1:9") };
    }
    let state = AppState::new(crate::config::Settings::from_env().expect("settings")).await.expect("state");
    let mut req = axum::http::Request::builder().method(method).uri(path);
    if let Some(ct) = ct {
        req = req.header(header::CONTENT_TYPE, ct);
    }
    let req = req.body(Body::from(body.to_owned())).expect("request");
    let resp = crate::routes::app(state).oneshot(req).await.expect("response");
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (status, headers, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn body_is_parsed_before_the_scope_and_validated_after() {
    let json = Some("application/json");
    let (status, _, body) = call("POST", "/v1/gems/generate", json, r#"{"tier": "#).await;
    assert_eq!((status, body["error"]["details"][0]["type"].as_str()), (422, Some("json_invalid")));
    let (status, headers, body) = call("POST", "/v1/gems/generate", json, r#"{"tier": "x"}"#).await;
    assert_eq!((status, body["error"]["message"].as_str()), (401, Some("API token required")));
    assert_eq!(headers.get(header::WWW_AUTHENTICATE).and_then(|v| v.to_str().ok()), Some("Bearer"));
    let (status, _, _) = call("GET", "/v1/gems/level-costs?tier=x", None, "").await;
    assert_eq!(status, 401);
    let (status, headers, body) = call("GET", "/v1/gems/augment", None, "").await;
    assert_eq!((status, body["error"]["code"].as_str()), (405, Some("method_not_allowed")));
    assert_eq!(headers.get(header::ALLOW).and_then(|v| v.to_str().ok()), Some("POST"));
}
