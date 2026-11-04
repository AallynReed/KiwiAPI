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

fn source(branch: &str) -> super::data::Source {
    super::data::Source {
        kind: super::data::Kind::Game,
        ident: "blueprints/a.blueprint".into(),
        title: String::new(),
        tmod: None,
        game_path: None,
        prefab: None,
        outfit: None,
        tmod_sha: None,
        branch: branch.into(),
    }
}

#[test]
fn branch_keys_only_off_live() {
    assert_eq!(source("live-us").cache_key(), "game:blueprints/a.blueprint");
    assert_eq!(source("pts").cache_key(), "game:pts:blueprints/a.blueprint");
}

async fn get(uri: &str) -> (u16, String) {
    use tower::ServiceExt;
    let app = super::router().with_state(crate::workshop::tests::state());
    let req = axum::http::Request::get(uri).body(axum::body::Body::empty()).unwrap();
    let (status, _, body) = crate::workshop::tests::body_of(app.oneshot(req).await.unwrap()).await;
    (status, String::from_utf8_lossy(&body).into_owned())
}

#[tokio::test]
async fn unknown_branch_is_refused() {
    let (status, body) = get("/site/embed/manifest?game=a.blueprint&branch=beta").await;
    assert_eq!(status, 404, "{body}");
    assert!(body.contains("Unknown branch 'beta' (known: live-us, pts)"), "{body}");
    assert_eq!(get("/embed/viewer?game=a.blueprint&branch=beta").await.0, 422);
}

#[test]
fn viewer_page_carries_branch() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../site/templates/embed_viewer.html");
    let template = std::fs::read_to_string(path).unwrap();
    let c = &golden()["pages"][0];
    let mut vars: Vec<(&str, String)> =
        c["vars"].as_object().unwrap().iter().map(|(k, v)| (k.as_str(), v.as_str().unwrap().to_owned())).collect();
    vars.iter_mut().find(|(k, _)| *k == "branch").unwrap().1 = "pts".into();
    assert!(render_viewer(&template, &vars).unwrap().contains("data-branch=\"pts\""));
}
