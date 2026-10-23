//! Golden checks against `testdata/dressing/*` (gen_dressing_fixtures.py).

use std::{
    collections::HashMap,
    io::Read,
    sync::{Arc, OnceLock},
};

use axum::{body::Body, http::Request};
use kiwi_core::{dressing as kd, rig::Row};
use serde_json::{Value, json};
use tower::ServiceExt;

use super::service::{self, Selection, TestArchive};
use crate::{
    blueprints::cache::Fmt,
    pyjson, request_id,
    workshop::tests::{b64, body_of, load, s, state},
};

fn engine() -> &'static Value {
    static E: OnceLock<Value> = OnceLock::new();
    E.get_or_init(|| load("dressing", "engine.json"))
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array().unwrap().iter().map(|x| s(x).to_owned()).collect()
}

fn files(key: &str) -> Vec<(String, Vec<u8>)> {
    let e = engine();
    strs(&e[key]).into_iter().map(|p| (p.clone(), b64(&e["files"][&p]))).collect()
}

fn rows() -> Vec<Row> {
    engine()["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            let t = |i: usize| s(&r[i]).to_owned();
            (t(0), t(1), t(2), t(3), r[4].as_f64().unwrap(), r[5].as_f64().unwrap())
        })
        .collect()
}

fn map(v: &Value) -> HashMap<String, String> {
    v.as_object().unwrap().iter().map(|(k, v)| (k.clone(), s(v).to_owned())).collect()
}

fn bytes_map(v: &Value) -> HashMap<String, Vec<u8>> {
    v.as_object().unwrap().iter().map(|(k, v)| (k.clone(), b64(v))).collect()
}

fn catalogue() -> Arc<kd::Catalogue> {
    static C: OnceLock<Arc<kd::Catalogue>> = OnceLock::new();
    C.get_or_init(|| {
        let e = engine();
        let (loc, classes, skins, equipment) = (map(&e["loc"]), files("classes"), files("skins"), files("equipment"));
        let heads = b64(&e["files"][s(&e["heads"])]);
        let rig_map = kiwi_core::rig::RigMap::build(&rows());
        let inputs = kd::Inputs {
            loc: &loc,
            classes: &classes,
            skins: &skins,
            rig_map: &rig_map,
            equipment: &equipment,
            heads: Some(&heads),
        };
        Arc::new(kd::build(&inputs, crate::rigs::rig).unwrap())
    })
    .clone()
}

fn install() {
    let e = engine();
    let _ = super::catalogue::TEST_CATALOGUE.set(catalogue());
    let _ = crate::creature::TEST_ROWS.set(rows());
    let mut game = HashMap::new();
    for p in strs(&e["game_paths"]) {
        let data = match p.strip_prefix("blueprints/") {
            Some(rel) => b64(&e["blueprints"][rel.to_lowercase()]),
            None => b64(&e["game_files"][&p]),
        };
        game.insert(p, data);
    }
    let _ = service::TEST_ARCHIVE.set(TestArchive {
        paths: strs(&e["game_paths"]),
        basenames: map(&e["basenames"]),
        blueprints: bytes_map(&e["blueprints"]),
        files: game,
    });
}

fn same(a: &Value, b: &Value) -> bool {
    pyjson::dumps(a) == pyjson::dumps(b)
}

fn gunzip(gz: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(gz).read_to_end(&mut out).unwrap();
    out
}

#[test]
fn class_sockets() {
    for case in engine()["parse_class"].as_array().unwrap() {
        let data = b64(&engine()["files"][s(&case[0])]);
        let got = kd::parse_class(&data).map(|c| {
            let sockets: Vec<Value> = c
                .sockets
                .iter()
                .map(|x| json!({"bone": x.bone, "ap": x.ap, "slot": x.slot, "family": x.family, "flag": x.flag}))
                .collect();
            json!({"skeleton": c.skeleton, "name_key": c.name_key, "sockets": sockets})
        });
        assert!(same(&json!(got), &case[1]), "{}", case[0]);
    }
}

#[test]
fn custom_heads() {
    let data = b64(&engine()["files"][s(&engine()["heads"])]);
    let got: Vec<Value> = kd::parse_heads(&data)
        .into_iter()
        .map(|r| {
            let pieces: serde_json::Map<String, Value> = r.pieces.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
            json!({"race": r.race, "key": r.key, "name_key": r.name_key, "pieces": pieces})
        })
        .collect();
    assert!(same(&json!(got), &engine()["parse_heads"]));
    for case in engine()["humanize"].as_array().unwrap() {
        assert_eq!(kd::humanize(s(&case[0])), s(&case[1]), "{case}");
    }
    for case in engine()["format_g"].as_array().unwrap() {
        assert_eq!(service::format_g(case[0].as_f64().unwrap()), s(&case[1]), "{case}");
    }
}

#[test]
fn catalogue_views() {
    let cat = catalogue();
    let want = &engine()["catalogue"];
    assert!(same(&super::classes_value(&cat), &want["classes"]));
    assert!(same(&super::races_value(&cat), &want["races"]));
    for case in engine()["options"].as_array().unwrap() {
        let q = &case["query"];
        let opt = |k: &str| q.get(k).and_then(Value::as_str).map(str::to_owned);
        let oq = super::OptionsQ {
            slot: opt("slot").unwrap(),
            class_key: opt("class"),
            race: opt("race"),
            q: opt("q"),
            offset: q.get("offset").and_then(Value::as_i64).unwrap_or(0),
            limit: q.get("limit").and_then(Value::as_i64).unwrap_or(100),
        };
        match super::options_value(&cat, &oq) {
            Ok(v) => assert!(same(&v, &case["ok"]), "{q}"),
            Err(e) => assert_eq!(json!([e.status.as_u16(), e.code, e.message]), case["error"], "{q}"),
        }
    }
}

fn selection(q: &Value) -> Selection {
    let opt = |k: &str| q.get(k).and_then(Value::as_str).map(str::to_owned);
    Selection {
        class_key: opt("class").unwrap_or_default(),
        costume: opt("costume"),
        picks: ["hat", "face", "weapon", "head", "hair", "eyes"].iter().map(|k| ((*k).to_owned(), opt(k))).collect(),
        weapon_family: opt("weapon_family"),
        race: opt("race"),
        colors: vec![("hair_color".into(), opt("hair_color")), ("eye_color".into(), opt("eye_color"))],
        hair_scale: q.get("hair_scale").and_then(Value::as_f64),
    }
}

#[tokio::test]
async fn outfits_models_stills() {
    install();
    let st = state();
    let mut bad = Vec::new();
    for case in engine()["outfits"].as_array().unwrap() {
        let q = &case["query"];
        let o = match super::resolve_query(&st, &selection(q)).await {
            Ok(o) => o,
            Err(e) => {
                if json!([e.status.as_u16(), e.code, e.message]) != case["error"] {
                    bad.push(format!("{q}: error {e:?} vs {}", case["error"]));
                }
                continue;
            }
        };
        if !same(&o.outfit_value(), &case["outfit"]) {
            bad.push(format!("{q}: outfit {} vs {}", pyjson::dumps(&o.outfit_value()), pyjson::dumps(&case["outfit"])));
        }
        if o.ident() != s(&case["ident"]) || o.issue_header() != s(&case["header"]) {
            bad.push(format!(
                "{q}: ident {} / {} vs {} / {}",
                o.ident(),
                o.issue_header(),
                case["ident"],
                case["header"]
            ));
        }
        let model = service::model(&st, &o, Fmt::Json).await.unwrap_or_else(|_| panic!("{q}: model failed"));
        let got = model.map(|c| gunzip(&c.gz));
        let want = case["model"].as_str().map(|_| b64(&case["model"]));
        if got != want {
            bad.push(format!(
                "{q}: model {:?}\n  vs {:?}",
                got.map(|g| String::from_utf8_lossy(&g[..g.len().min(300)]).into_owned()),
                want.map(|g| String::from_utf8_lossy(&g[..g.len().min(300)]).into_owned())
            ));
        }
        let png = service::still(&st, &o, 64, 30.0, 8.0).await.unwrap_or_else(|_| panic!("{q}: still failed"));
        let want = case["still"].as_str().map(|_| b64(&case["still"]));
        if png != want {
            bad.push(format!("{q}: still differs ({:?} vs {:?} bytes)", png.map(|p| p.len()), want.map(|p| p.len())));
        }
    }
    for case in engine()["stills"].as_array().unwrap() {
        let o = super::resolve_query(&st, &selection(&case["query"])).await.unwrap();
        let (az, el) = (case["az"].as_f64().unwrap(), case["el"].as_f64().unwrap());
        let png = service::still(&st, &o, case["dim"].as_i64().unwrap(), az, el).await.unwrap();
        if png != Some(b64(&case["png"])) {
            bad.push(format!("still {} {az} {el} differs", case["dim"]));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[tokio::test]
async fn creature_thumbnails() {
    install();
    let st = state();
    for case in engine()["creature_png"].as_array().unwrap() {
        let png = crate::creature::creature_png(&st, s(&case[0]), case[1].as_i64().unwrap(), "live-us").await.unwrap();
        let want = case[2].as_str().map(|_| b64(&case[2]));
        assert!(png == want, "{}: {:?} vs {:?}", case[0], png.map(|p| p.len()), want.map(|p| p.len()));
    }
}

#[tokio::test]
async fn tmod_assembly() {
    install();
    let st = state();
    for case in engine()["tmod_assembly"].as_array().unwrap() {
        let got = match crate::creature::build_tmod(&st, b64(&case["tmod"])).await {
            Ok(m) => Some(Fmt::Json.encode(&m)),
            Err(crate::blueprints::cache::BuildError::Other(_)) => None,
            Err(e) => panic!("{}: {e:?}", case["name"]),
        };
        let want = case["model"].as_str().map(|_| b64(&case["model"]));
        assert!(got == want, "{}", case["name"]);
    }
}

#[tokio::test]
async fn backdrops() {
    install();
    let st = state();
    for (kind, want) in engine()["backdrop"].as_object().unwrap() {
        let got = crate::embed::backdrop::backdrop(&st, kind).await.unwrap_or_else(|_| panic!("{kind}"));
        let got = got.unwrap_or(Value::Null);
        assert_eq!(pyjson::dumps(&got), pyjson::dumps(want), "{kind}");
    }
}

#[test]
fn backdrop_matrices() {
    let rig = crate::rigs::rig("biped_medium").unwrap();
    let hat = rig.rest.get("hat").unwrap();
    for case in engine()["matrices"].as_array().unwrap() {
        let part = rig.rest.get(s(&case["part"])).unwrap();
        let size = case["size"].as_f64().unwrap();
        let m = kd::head_matrix(hat, part, size, |v| crate::pyfloat::round(v, 6)).unwrap();
        assert_eq!(json!(m), case["m"], "{}", case["part"]);
        let raw = kd::head_matrix(hat, part, size, |v| v).unwrap();
        assert_eq!(json!(raw), case["raw"], "{} {size}", case["part"]);
    }
    let mut bad = Vec::new();
    for (n, case) in engine()["inverses"].as_array().unwrap().iter().enumerate() {
        let a: Vec<f64> = case[0].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        let m: [[f64; 4]; 4] = std::array::from_fn(|i| std::array::from_fn(|j| a[i * 4 + j]));
        let got = kd::inv4(&m).unwrap();
        let want: Vec<f64> = case[1].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        let off: Vec<usize> = (0..16).filter(|&k| got[k / 4][k % 4].to_bits() != want[k].to_bits()).collect();
        if !off.is_empty() {
            bad.push(format!("{n}: {off:?}"));
        }
    }
    assert!(bad.is_empty(), "inverses off: {}", bad.join("; "));
}

async fn call(path: &str, rid: &str) -> (u16, axum::http::HeaderMap, Vec<u8>) {
    let st = state();
    let req = Request::get(path).body(Body::empty()).unwrap();
    let fut = async {
        if path.starts_with("/v1/dressing/") {
            let (parts, _) = req.into_parts();
            let rl = crate::ratelimit::RateLimitInfo { limit: 1, remaining: 0, reset: 0 };
            super::v1_route(&st, &parts, rl).await
        } else {
            let app = super::router().merge(crate::embed::router()).with_state(st.clone());
            app.oneshot(req).await.unwrap()
        }
    };
    body_of(request_id::scope(rid.to_owned(), fut).await).await
}

#[tokio::test]
async fn routes_replay() {
    install();
    let mut bad = Vec::new();
    for case in load("dressing", "routes.json").as_array().unwrap() {
        let (name, path) = (s(&case["name"]), s(&case["path"]));
        let (status, headers, body) = call(path, s(&case["request_id"])).await;
        let want = b64(&case["response"]);
        if u64::from(status) != case["status"].as_u64().unwrap() || body != want {
            bad.push(format!(
                "{name}: {status} {}\n   want {} {}",
                String::from_utf8_lossy(&body[..body.len().min(300)]),
                case["status"],
                String::from_utf8_lossy(&want[..want.len().min(300)])
            ));
            continue;
        }
        for h in case["headers"].as_array().unwrap() {
            let (k, v) = (s(&h[0]), s(&h[1]));
            let got = headers.get(k).map(|g| g.as_bytes().iter().map(|&b| char::from(b)).collect::<String>());
            let want = if k == "vary" {
                v.split(", ").filter(|t| *t != "Origin").collect::<Vec<_>>().join(", ")
            } else {
                v.to_owned()
            };
            let want = (!want.is_empty()).then_some(want);
            if got != want {
                bad.push(format!("{name}: header {k} {got:?} vs {want:?}"));
            }
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
