//! Replays native/port/parity/gen_embed_fixtures.py.

use base64::Engine;
use serde_json::Value;

use super::{render_viewer, tmod};

fn golden() -> Value {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/embed/golden.json");
    serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap()
}

fn b64(v: &Value) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD.decode(v.as_str().unwrap()).unwrap()
}

#[test]
fn viewer_page_matches_jinja() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../site/templates/embed_viewer.html");
    let template = std::fs::read_to_string(path).unwrap();
    for c in golden()["pages"].as_array().unwrap() {
        let vars: Vec<(&str, String)> =
            c["vars"].as_object().unwrap().iter().map(|(k, v)| (k.as_str(), v.as_str().unwrap().to_owned())).collect();
        assert_eq!(render_viewer(&template, &vars).unwrap(), c["html"].as_str().unwrap());
    }
}

#[test]
fn tmod_verdicts_match_read_tmod() {
    for c in golden()["tmods"].as_array().unwrap() {
        let got = tmod::validate(&b64(&c["data"])).err();
        assert_eq!(got.as_deref(), c["error"].as_str(), "{}", c["data"]);
    }
}

#[test]
fn utf8_errors_match_python() {
    for c in golden()["utf8"].as_array().unwrap() {
        assert_eq!(tmod::utf8_error(&b64(&c["data"])).as_deref(), c["error"].as_str(), "{}", c["data"]);
    }
}
