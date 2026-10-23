//! Golden checks against `testdata/workshop/*` (gen_workshop_fixtures.py).

use std::sync::Arc;

use axum::body::Body;
use base64::Engine as _;
use kiwi_core::workshop::{self as ws, Fail};
use kiwi_core::zipread;
use serde_json::{Value, json};

use super::{archive, game};
use crate::{
    config::Settings,
    db::Db,
    pyjson, request_id,
    state::{AppState, Inner},
};

pub fn load(dir: &str, name: &str) -> Value {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata").join(dir).join(name);
    pyjson::loads(&std::fs::read(p).unwrap()).unwrap()
}

pub fn b64(v: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(v.as_str().unwrap()).unwrap()
}

pub fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_default()
}

pub fn state() -> AppState {
    let settings = Settings::default();
    AppState { inner: Arc::new(Inner { settings, db: Db { mongo: None, redis: None, pg: None } }) }
}

pub fn install_game() {
    let paths: Vec<String> =
        load("workshop", "engine.json")["game_paths"].as_array().unwrap().iter().map(|v| s(v).to_owned()).collect();
    let _ = game::TEST_PATHS.set(paths);
}

fn game_map() -> ws::GameMap {
    let e = load("workshop", "engine.json");
    game::from_paths(1, e["game_paths"].as_array().unwrap().iter().map(|v| s(v).to_owned())).map
}

fn same(a: &Value, b: &Value) -> bool {
    pyjson::dumps(a) == pyjson::dumps(b)
}

fn strs(v: &Value) -> Vec<String> {
    v.as_array().unwrap().iter().map(|x| s(x).to_owned()).collect()
}

#[test]
fn zips() {
    let mut bad = Vec::new();
    for case in load("workshop", "engine.json")["zips"].as_array().unwrap() {
        let name = s(&case["name"]);
        let got = archive::read_zip(&b64(&case["data"]));
        let want = case.get("ok").map(|v| Ok(v.clone())).unwrap_or_else(|| {
            Err(case
                .get("error")
                .map(|e| Fail::Bad(s(e).into()))
                .unwrap_or(Fail::Unhandled(s(&case["unhandled"]).into())))
        });
        let ok = match (&got, &want) {
            (Ok(files), Ok(w)) => {
                let files: Vec<Value> = files
                    .iter()
                    .map(|(p, d)| json!([p, base64::engine::general_purpose::STANDARD.encode(d)]))
                    .collect();
                Value::Array(files) == *w
            }
            (Err(Fail::Bad(a)), Err(Fail::Bad(b))) => a == b,
            (Err(Fail::Unhandled(_)), Err(Fail::Unhandled(_))) => true,
            _ => false,
        };
        if !ok {
            bad.push(format!("{name}: got {got:?}\n   want {want:?}"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn zip_limits() {
    let mut w = crate::media::zipfile::ZipWriter::new();
    w.clock = Some([2024, 1, 1, 0, 0, 0]);
    for i in 0..4001 {
        w.writestr(&format!("blueprints/f{i}.blueprint"), b"");
    }
    let err = archive::read_zip(&w.finish()).unwrap_err();
    assert_eq!(err, Fail::Bad("That .zip holds more than 4000 files - too many for one mod.".into()));
}

#[test]
fn cp437_and_filemode() {
    let e = load("workshop", "engine.json");
    let high: Vec<u8> = (128u8..=255).collect();
    assert_eq!(zipread::cp437(&high), s(&e["cp437"]));
    for pair in e["filemode"].as_array().unwrap() {
        assert_eq!(zipread::filemode(pair[0].as_u64().unwrap() as u32), s(&pair[1]), "{pair}");
    }
}

#[test]
fn plans_and_previews() {
    let e = load("workshop", "engine.json");
    let gm = game_map();
    let empty = ws::GameMap::new();
    let mut bad = Vec::new();
    for case in e["plans"].as_array().unwrap() {
        let map = if case.get("no_index").is_some() { &empty } else { &gm };
        let got = Value::Object(
            ws::plan(&strs(&case["paths"]), case["fix"].as_bool().unwrap(), &strs(&case["keep"]), map).to_map(),
        );
        if !same(&got, &case["plan"]) {
            bad.push(format!(
                "plan {}:\n got  {}\n want {}",
                case["name"],
                pyjson::dumps(&got),
                pyjson::dumps(&case["plan"])
            ));
        }
    }
    for case in e["previews"].as_array().unwrap() {
        let got = Value::Object(ws::preview(&strs(&case["paths"]), &gm).to_map());
        if !same(&got, &case["plan"]) {
            bad.push(format!(
                "preview {}:\n got  {}\n want {}",
                case["name"],
                pyjson::dumps(&got),
                pyjson::dumps(&case["plan"])
            ));
        }
    }
    for case in e["candidates"].as_array().unwrap() {
        let paths = strs(&case["paths"]);
        assert_eq!(json!(ws::config_candidates(&paths)), case["config"]);
        assert_eq!(json!(ws::preview_candidates(&paths)), case["preview"]);
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn properties_titles_nearest() {
    let e = load("workshop", "engine.json");
    for case in e["clean"].as_array().unwrap() {
        let raw = &case["raw"];
        let got = ws::clean_properties(if raw.is_null() { None } else { Some(raw) });
        match (got, case.get("ok")) {
            (Ok(props), Some(want)) => {
                let props: Vec<Value> = props.iter().map(|(k, v)| json!([k, v])).collect();
                assert_eq!(Value::Array(props), *want, "{raw}");
            }
            (Err(Fail::Unhandled(_)), None) => assert!(case.get("unhandled").is_some(), "{raw}"),
            (got, _) => panic!("{raw}: {got:?} vs {case}"),
        }
    }
    for t in e["titles"].as_array().unwrap() {
        let v = ws::py_str(Some(&t[0]));
        assert_eq!(ws::safe_title(&v), s(&t[1]), "{t}");
        assert_eq!(ws::safe_filename(&v), s(&t[2]), "{t}");
    }
    for n in e["nearest"].as_array().unwrap() {
        let cands = strs(&n[0]);
        assert_eq!(ws::nearest_path(&cands, s(&n[1])).map(|p| json!(p)).unwrap_or(Value::Null), n[2], "{n}");
    }
}

#[test]
fn describe() {
    for case in load("workshop", "engine.json")["describe"].as_array().unwrap() {
        let got = archive::describe(&b64(&case["data"]));
        match (got, case.get("ok"), case.get("error")) {
            (Ok(v), Some(want), _) => assert!(same(&v, want), "{}: {}", case["name"], pyjson::dumps(&v)),
            (Err(Fail::Bad(m)), _, Some(e)) => assert_eq!(m, s(e)),
            (got, _, _) => panic!("{}: {got:?}", case["name"]),
        }
    }
}

/// Time, date and creating system zeroed: what a clock can't pin.
pub fn zip_normalized(z: &[u8]) -> Vec<u8> {
    let mut z = z.to_vec();
    let mut i = 0;
    while i + 4 <= z.len() {
        match &z[i..i + 4] {
            b"PK\x03\x04" => {
                z[i + 10..i + 14].fill(0);
                i += 30;
            }
            b"PK\x01\x02" => {
                z[i + 5] = 0;
                z[i + 12..i + 16].fill(0);
                i += 46;
            }
            _ => i += 1,
        }
    }
    z
}

pub async fn body_of(resp: axum::response::Response) -> (u16, axum::http::HeaderMap, Vec<u8>) {
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    (status, headers, axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap().to_vec())
}

pub fn compare(
    case: &Value,
    status: u16,
    headers: &axum::http::HeaderMap,
    body: &[u8],
    zip: bool,
    bad: &mut Vec<String>,
) {
    let name = s(&case["name"]);
    let want = b64(&case["response"]);
    if u64::from(status) != case["status"].as_u64().unwrap() {
        bad.push(format!(
            "{name}: status {status} vs {}: {}",
            case["status"],
            String::from_utf8_lossy(&body[..body.len().min(400)])
        ));
        return;
    }
    let ok = if zip { zip_normalized(body) == zip_normalized(&want) } else { body == want.as_slice() };
    if !ok {
        bad.push(format!(
            "{name}: body differs\n  got  {}\n  want {}",
            String::from_utf8_lossy(&body[..body.len().min(600)]),
            String::from_utf8_lossy(&want[..want.len().min(600)])
        ));
    }
    for h in case["headers"].as_array().unwrap() {
        let (k, v) = (s(&h[0]), s(&h[1]));
        let got = headers.get(k).map(|g| g.as_bytes().iter().map(|&b| char::from(b)).collect::<String>());
        // The CORS layer adds `Origin` to Vary; the handler only owns the rest.
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

pub fn parts_for(case: &Value) -> axum::http::request::Parts {
    let req =
        axum::http::Request::builder().method(s(&case["method"])).uri(s(&case["path"])).body(Body::empty()).unwrap();
    req.into_parts().0
}

pub fn items_for(case: &Value) -> super::form::Items {
    let ct = s(&case["content_type"]);
    if ct.is_empty() {
        return Vec::new();
    }
    kiwi_core::form::parse_form(Some(ct.as_bytes()), &b64(&case["body"])).unwrap()
}

#[tokio::test]
async fn routes_replay() {
    install_game();
    let st = state();
    let mut bad = Vec::new();
    for case in load("workshop", "routes.json").as_array().unwrap() {
        let path = s(&case["path"]).split('?').next().unwrap().to_owned();
        let route = path.trim_start_matches("/site/mod-workshop/").to_owned();
        let (parts, items) = (parts_for(case), items_for(case));
        let fut = async {
            match route.as_str() {
                "inspect" => super::inspect_form(&st, items).await,
                "build" => super::build_form(&st, items).await,
                "extract" => super::extract_form(&st, items).await,
                "preview/blueprint" => super::preview_blueprint_form(&parts, items).await,
                "swf/scripts" => super::swf_scripts_form(&st, &parts, items, false).await,
                "preview/binfab" => super::preview_binfab_form(items).await,
                _ => super::extract_download_form(&parts, items).await,
            }
        };
        let resp = request_id::scope(s(&case["request_id"]).to_owned(), fut).await;
        let (status, headers, body) = body_of(resp).await;
        let zip = s(&case["name"]).starts_with("download_zip");
        compare(case, status, &headers, &body, zip, &mut bad);
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
