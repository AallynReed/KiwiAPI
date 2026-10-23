//! Checks against `testdata/custom_art/golden.json`
//! (`native/port/parity/gen_custom_art_fixtures.py`), plus store tests that
//! need a throwaway Mongo.

use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, HeaderValue, Method, Request, StatusCode},
    response::{IntoResponse, Response},
};
use base64::Engine;
use hyper_util::{client::legacy::Client, rt::TokioExecutor};
use kiwi_core::email_validator;
use mongodb::bson::{Bson, Document};
use serde_json::Value;
use tower::ServiceExt;

use super::{
    ArtRequest,
    admin::{ApproveBody, DenyBody, load},
    builder::{self, Run},
    changelog, check_shape, clean_name, denied_mail, intake, lanes, measure, mods_of, released_mail, view, what,
};
use crate::{
    config::Settings,
    db::Db,
    error::ApiError,
    middleware::{self, EmbedSource, Mw, MwConfig, forwarded::TrustedHosts},
    pyjson, request_id,
    state::{AppState, Inner},
    validation::{read_body, validate},
};

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| serde_json::from_str(include_str!("../../testdata/custom_art/golden.json")).expect("fixture"))
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("string: {v}"))
}

fn arr<'a>(v: &'a Value, key: &str) -> &'a Vec<Value> {
    v[key].as_array().unwrap_or_else(|| panic!("section {key}"))
}

fn latin1(s: &str) -> Vec<u8> {
    s.chars().map(|c| c as u32 as u8).collect()
}

fn to_doc(v: &Value) -> Document {
    match Bson::try_from(v.clone()).expect("extjson") {
        Bson::Document(d) => d,
        other => panic!("not a document: {other:?}"),
    }
}

fn settings() -> Settings {
    let mut vars: Vec<(String, String)> =
        golden()["env"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), s(v).to_owned())).collect();
    vars.push(("PY_UPSTREAM".into(), "http://py:8000".into()));
    Settings::from_vars(vars).expect("settings")
}

fn state_with(mongo: Option<mongodb::Database>) -> AppState {
    let upstream = Client::builder(TokioExecutor::new()).build_http();
    AppState { inner: Arc::new(Inner { settings: settings(), db: Db { mongo, redis: None, pg: None }, upstream }) }
}

fn app(state: AppState) -> Router {
    let mw =
        Mw::new(MwConfig::from_lookup(&|_| None), None, EmbedSource::Fixed(vec![]), TrustedHosts::parse("127.0.0.1"));
    let inner = middleware::wrap(super::router(), Arc::new(mw), None)
        .fallback(|| async { StatusCode::IM_A_TEAPOT })
        .with_state(state);
    middleware::outer(inner)
}

async fn parts(resp: Response) -> (u16, HeaderMap, Vec<u8>) {
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.expect("body");
    (status, headers, body.to_vec())
}

/// `{"ok": x}` or the APIError's status, code and message.
fn expect_err(want: &Value, got: &ApiError, what: &str) {
    assert_eq!(
        (u64::from(got.status.as_u16()), got.code, got.message.as_str()),
        (want["status"].as_u64().unwrap(), s(&want["code"]), s(&want["message"])),
        "{what}"
    );
}

#[test]
fn names() {
    for pair in arr(golden(), "names") {
        let (input, want) = (s(&pair[0]), &pair[1]);
        match clean_name(input) {
            Ok(n) => assert_eq!(n, s(&want["ok"]), "{input:?}"),
            Err(e) => expect_err(want, &e, input),
        }
    }
}

#[test]
fn emails() {
    for pair in arr(golden(), "emails") {
        let (input, want) = (s(&pair[0]), &pair[1]);
        match intake::clean_email(input) {
            Ok(Some(a)) => assert_eq!(a, s(&want["ok"]), "{input:?}"),
            Ok(None) => assert!(email_validator::needs_unicode(input), "{input:?} deferred without needing Unicode"),
            Err(e) => expect_err(want, &e, input),
        }
    }
}

#[test]
fn shapes() {
    for c in arr(golden(), "shapes") {
        let c = c.as_array().unwrap();
        let got = check_shape(s(&c[0]), s(&c[1]), c[2].as_i64().unwrap(), c[3].as_i64().unwrap());
        match got {
            Ok(()) => assert!(c[4].get("ok").is_some(), "{c:?}"),
            Err(e) => expect_err(&c[4], &e, &format!("{c:?}")),
        }
    }
}

fn pictures() -> HashMap<String, Vec<u8>> {
    let mut m: HashMap<String, Vec<u8>> = arr(golden(), "measure")
        .iter()
        .map(|c| (s(&c["name"]).to_owned(), base64::engine::general_purpose::STANDARD.decode(s(&c["b64"])).unwrap()))
        .collect();
    m.insert("empty".into(), Vec::new());
    m
}

#[test]
fn measures_like_pillow() {
    let pics = pictures();
    let mut bad = Vec::new();
    for c in arr(golden(), "measure") {
        let name = s(&c["name"]);
        let got = measure::measure(&pics[name]).map(|(ct, w, h)| serde_json::json!([ct, w, h]));
        if got.as_ref().unwrap_or(&Value::Null) != &c["result"] {
            bad.push(format!("{name}: got {got:?}, want {}", c["result"]));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn submit_checks_and_document() {
    let pics = pictures();
    for c in arr(golden(), "submits") {
        let name = s(&c["name"]);
        let files: Vec<(&str, Vec<u8>)> = c["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| (if s(&f[0]) == "pfp" { "pfp" } else { "banner" }, pics[s(&f[1])].clone()))
            .collect();
        let kind = s(&c["kind"]);
        let got = intake::check(kind, s(&c["input_name"]), s(&c["email"]), files, c["limit"].as_i64().unwrap());
        match got {
            Err(e) => expect_err(&c["result"], &e, name),
            Ok(checked) => {
                assert_eq!(c["result"]["ok"], "captcha", "{name}");
                let shas: Vec<String> = checked
                    .pictures
                    .iter()
                    .map(|p| hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&p.1)))
                    .collect();
                let want = to_doc(&c["doc"]);
                let mut r = intake::new_request(kind, &checked, &shas, c["note"].as_str());
                r.id = want.get_object_id("_id").unwrap();
                r.created_at = crate::dt::from_bson(*want.get_datetime("created_at").unwrap());
                assert_eq!(r.to_doc(), want, "{name}");
            }
        }
    }
}

#[test]
fn views_and_saves() {
    for c in arr(golden(), "views") {
        let r = ArtRequest::from_doc(&to_doc(&c["doc"])).unwrap();
        assert_eq!(pyjson::pydantic(&view(&r).unwrap()), s(&c["view"]));
        assert_eq!(serde_json::json!(lanes(&r)), c["lanes"]);
        assert_eq!(serde_json::json!(mods_of(&r)), c["mods"]);
        for (m, w) in c["what"].as_object().unwrap() {
            if let Some(w) = w.as_str() {
                assert_eq!(what(&r, Some(m)).as_deref(), Some(w));
            }
        }
        assert_eq!(r.to_doc(), to_doc(&c["saved"]));
    }
}

#[test]
fn changelogs() {
    for c in arr(golden(), "changelogs") {
        let reqs: Vec<ArtRequest> =
            c["docs"].as_array().unwrap().iter().map(|d| ArtRequest::from_doc(&to_doc(d)).unwrap()).collect();
        assert_eq!(changelog(&reqs, s(&c["mod"])), s(&c["text"]), "{} {}", c["set"], c["mod"]);
    }
}

fn mail_json(m: &super::Mail) -> Value {
    serde_json::json!({"to": m.to, "subject": m.subject, "text": m.text, "html": m.html})
}

#[test]
fn mails() {
    let app_url = settings().app_url;
    for c in arr(golden(), "mails") {
        let r = ArtRequest::from_doc(&to_doc(&c["doc"])).unwrap();
        assert_eq!(mail_json(&denied_mail(&r, &app_url)), c["denied"]);
        if let Some(rel) = c["released"].as_object() {
            let pages: HashMap<String, String> =
                rel["pages"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), s(v).to_owned())).collect();
            assert_eq!(mail_json(&released_mail(&r, &pages).unwrap()), rel["mail"]);
        }
    }
}

#[tokio::test]
async fn form_replies() {
    let app = app(state_with(None));
    let mut bad = Vec::new();
    for c in arr(golden(), "http") {
        let mut req = Request::builder()
            .method(Method::POST)
            .uri("/site/custom-art")
            .body(Body::from(latin1(s(&c["body"]))))
            .unwrap();
        if let Some(ct) = c["content_type"].as_str() {
            req.headers_mut().insert("content-type", HeaderValue::from_str(ct).unwrap());
        }
        req.headers_mut().insert("x-request-id", HeaderValue::from_str(s(&c["request_id"])).unwrap());
        let (status, headers, out) = parts(app.clone().oneshot(req).await.unwrap()).await;
        let out = String::from_utf8(out).unwrap();
        let ct = headers.get("content-type").map(|v| v.to_str().unwrap().to_owned());
        let want_ct =
            c["resp_headers"].as_array().unwrap().iter().find(|h| h[0] == "content-type").map(|h| s(&h[1]).to_owned());
        if u64::from(status) != c["status"].as_u64().unwrap() || out != s(&c["resp"]) || ct != want_ct {
            bad.push(format!("{}: {status} {out}\n  want {} {}", c["name"], c["status"], s(&c["resp"])));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[tokio::test]
async fn form_settings() {
    let c = &golden()["config"];
    let req = Request::builder()
        .uri("/site/custom-art/config")
        .header("x-request-id", "req_cart_cfg")
        .body(Body::empty())
        .unwrap();
    let (status, headers, out) = parts(app(state_with(None)).oneshot(req).await.unwrap()).await;
    assert_eq!(u64::from(status), c["status"].as_u64().unwrap());
    assert_eq!(String::from_utf8(out).unwrap(), s(&c["resp"]));
    for h in c["resp_headers"].as_array().unwrap() {
        assert_eq!(headers.get(s(&h[0])).map(|v| v.to_str().unwrap()), Some(s(&h[1])), "{}", h[0]);
    }
}

/// The admin bodies up to the first store read (a malformed id 404s first).
#[tokio::test]
async fn admin_bodies() {
    let state = state_with(None);
    let mut bad = Vec::new();
    for c in arr(golden(), "admin_bodies") {
        let state = state.clone();
        let mut headers = HeaderMap::new();
        headers.insert("content-type", HeaderValue::from_static("application/json"));
        let body = s(&c["body"]).to_owned();
        let route = s(&c["route"]).to_owned();
        let resp = request_id::scope(s(&c["request_id"]).to_owned(), async move {
            let input = match read_body(&headers, Body::from(body)).await {
                Ok(i) => i,
                Err(e) => return e.into_response(),
            };
            let res = if route == "approve" {
                validate::<ApproveBody>(&input).map(|_| ())
            } else {
                validate::<DenyBody>(&input).map(|_| ())
            };
            match res {
                Ok(()) => load(&state, "not-an-id").await.map(|_| ()).unwrap_err().into_response(),
                Err(e) => e.into_response(),
            }
        })
        .await;
        let (status, _, out) = parts(resp).await;
        let out = String::from_utf8(out).unwrap();
        if u64::from(status) != c["status"].as_u64().unwrap() || out != s(&c["resp"]) {
            bad.push(format!("{} {}: {status} {out}\n  want {} {}", c["route"], c["body"], c["status"], s(&c["resp"])));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn defers_only_unicode_addresses() {
    let ct = Some(&b"application/x-www-form-urlencoded"[..]);
    assert!(intake::needs_python(ct, "kind=player&name=x&email=jos%C3%A9%40x.io".as_bytes()));
    assert!(intake::needs_python(ct, b"email=a%40xn--bcher-kva.de"));
    assert!(!intake::needs_python(ct, b"kind=player&name=%C3%A9&email=a%40x.io"));
    assert!(!intake::needs_python(ct, b"email=a%40x.io&email2=%C3%A9"));
    assert!(!intake::needs_python(Some(&b"multipart/form-data; boundary=XyZ"[..]), b"garbage"));
}

#[test]
fn worker_helpers() {
    let w = &golden()["worker"];
    for c in arr(w, "art_message") {
        let reqs: Vec<ArtRequest> = c[0]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| {
                let d = mongodb::bson::doc! {
                    "_id": mongodb::bson::oid::ObjectId::new(), "kind": "player", "name": s(n), "email": "a@x.io",
                    "pictures": {"pfp": {"sha": "00", "content_type": "image/png", "width": 1, "height": 1}},
                };
                ArtRequest::from_doc(&d).unwrap()
            })
            .collect();
        assert_eq!(builder::art_message(&reqs), s(&c[1]));
    }
    for c in arr(w, "short") {
        assert_eq!(builder::short(s(&c[0])), s(&c[1]));
    }
    for c in arr(w, "vdf_safe") {
        assert_eq!(builder::vdf_safe(s(&c[0])), s(&c[1]));
    }
    for c in arr(w, "versions") {
        assert_eq!(builder::find_version(s(&c[0])).as_deref(), c[1].as_str(), "{}", c[0]);
    }
    for c in arr(w, "newlines") {
        assert_eq!(builder::text_of(&latin1(s(&c[0]))), s(&c[1]));
    }
    let mut run = Run::new(Vec::new());
    run.lines = w["log_text"]["lines"].as_array().unwrap().iter().map(|l| s(l).to_owned()).collect();
    assert_eq!(run.text(), s(&w["log_text"]["text"]));
    let v = &w["vdf"];
    assert_eq!(builder::vdf(s(&v["item"]), "<CONTENT>", s(&v["note"])), s(&v["vdf"]));
}

// --- store tests (KIWI_TEST_MONGO=mongodb://127.0.0.1:27017) -----------------------

async fn test_db() -> mongodb::Database {
    let uri = std::env::var("KIWI_TEST_MONGO").unwrap_or_else(|_| "mongodb://127.0.0.1:27017".into());
    let client = mongodb::Client::with_uri_str(&uri).await.expect("mongo");
    let db = client.database(&format!("kiwi_custom_art_{}", mongodb::bson::oid::ObjectId::new().to_hex()));
    db.drop().await.ok();
    db
}

#[tokio::test]
#[ignore = "needs a throwaway Mongo (KIWI_TEST_MONGO)"]
async fn review_flow_against_mongo() {
    use mongodb::bson::doc;
    let db = test_db().await;
    let state = state_with(Some(db.clone()));
    let c = db.collection::<Document>(super::COLLECTION);
    let golden_doc = to_doc(&arr(golden(), "views")[0]["doc"]);
    c.insert_one(golden_doc.clone()).await.unwrap();
    let id = golden_doc.get_object_id("_id").unwrap().to_hex();
    let mut r = load(&state, &id).await.unwrap();
    r.status = "approved".into();
    r.decided_at = Some(crate::dt::now());
    super::admin::save(&state, &r).await.unwrap();
    let stored = c.find_one(doc! {"_id": r.id}).await.unwrap().unwrap();
    assert_eq!(stored.keys().collect::<Vec<_>>(), golden_doc.keys().collect::<Vec<_>>());
    assert_eq!(stored.get_str("status").unwrap(), "approved");
    assert!(matches!(load(&state, "000000000000000000000000").await, Err(e) if e.status == StatusCode::NOT_FOUND));
    db.drop().await.ok();
}

// --- the worker's steps against real repos (git and python on PATH) ---------------

const PLACE_SCRIPT: &str = r#"import pathlib, shutil, sys
lane, name, src = sys.argv[1:4]
mods = {"pfp": ["Zakros UI - Chat"], "club": ["Zakros UI - Chat", "Zakros UI - Clubs"],
        "banner": ["Zakros UI - Chat", "Zakros UI - Nameplate", "Zakros UI - Clubs"]}[lane]
for m in mods:
    d = pathlib.Path(m) / "art"
    d.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(src, d / f"{lane}-{name}.png")
o = pathlib.Path("originals")
o.mkdir(exist_ok=True)
shutil.copyfile(src, o / f"{lane}-{name}.png")
print("placed", lane, name)
"#;

const IDENT: [(&str, &str); 4] = [
    ("GIT_AUTHOR_NAME", "kiwi-test"),
    ("GIT_AUTHOR_EMAIL", "kiwi-test@example.invalid"),
    ("GIT_COMMITTER_NAME", "kiwi-test"),
    ("GIT_COMMITTER_EMAIL", "kiwi-test@example.invalid"),
];

fn vcs(args: &[&str], cwd: &std::path::Path) -> String {
    let out = std::process::Command::new("git").args(args).current_dir(cwd).envs(IDENT).output().expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[tokio::test]
#[ignore = "needs git and python on PATH"]
async fn worker_steps_against_real_repos() {
    let t = std::env::temp_dir().join(format!("kiwi-builder-{}", mongodb::bson::oid::ObjectId::new().to_hex()));
    std::fs::create_dir_all(&t).unwrap();
    for bare in ["troveui.git", "lib.git"] {
        vcs(&["init", "--bare", "-b", "master", bare], &t);
    }
    let seed = t.join("seed");
    vcs(&["init", "-b", "master", "seed"], &t);
    std::fs::write(seed.join("custom_art.py"), PLACE_SCRIPT).unwrap();
    for m in super::MODS {
        std::fs::create_dir_all(seed.join(m)).unwrap();
        std::fs::write(seed.join(m).join("build.py"), "PROPS = {\"modVersion\":  \"1.0.0\"}\n").unwrap();
    }
    vcs(&["add", "-A"], &seed);
    vcs(&["commit", "-m", "seed"], &seed);
    vcs(&["push", &t.join("troveui.git").to_string_lossy(), "master"], &seed);
    std::fs::write(seed.join("lib.txt"), "lib").unwrap();
    vcs(&["add", "-A"], &seed);
    vcs(&["commit", "-m", "lib"], &seed);
    vcs(&["push", &t.join("lib.git").to_string_lossy(), "master"], &seed);
    std::fs::write(t.join("troveui.env"), "API_TOKEN=x\n").unwrap();

    let p = |n: &str| t.join(n).to_string_lossy().into_owned();
    let vars = vec![
        ("PY_UPSTREAM".to_owned(), "http://py:8000".to_owned()),
        ("CUSTOM_ART_WORKSPACE".into(), p("ws")),
        ("CUSTOM_ART_REMOTE".into(), p("troveui.git")),
        ("CUSTOM_ART_LIB_REMOTE".into(), p("lib.git")),
        ("CUSTOM_ART_ENV_FILE".into(), p("troveui.env")),
        ("MODS_STORE_DIR".into(), p("mods")),
    ];
    let upstream = Client::builder(TokioExecutor::new()).build_http();
    let state = AppState {
        inner: Arc::new(Inner {
            settings: Settings::from_vars(vars).unwrap(),
            db: Db { mongo: None, redis: None, pg: None },
            upstream,
        }),
    };
    let python = std::env::var("KIWI_TEST_PYTHON").unwrap_or_else(|_| "python".into());
    let b = builder::Builder {
        state,
        python,
        steamcmd: "steamcmd".into(),
        steam_home: String::new(),
        env: IDENT.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect(),
    };
    let png = pictures()["png_rgba"].clone();
    let (sha, _) = crate::media::cas::ContentStore::new(t.join("mods")).put(&png).unwrap();
    let d = mongodb::bson::doc! {
        "_id": mongodb::bson::oid::ObjectId::new(), "kind": "player", "name": "Kiwi", "email": "a@x.io",
        "pictures": {"pfp": {"sha": &sha, "content_type": "image/png", "width": 4, "height": 4}},
    };
    let reqs = vec![ArtRequest::from_doc(&d).unwrap()];

    let mut run = b.new_run();
    let tree = b.checkout(&mut run).await.unwrap();
    assert!(tree.join(".env").is_file() && tree.join("lib").join("lib.txt").is_file());
    b.place(&tree, &reqs, &mut run).await.unwrap();
    let message = builder::art_message(&reqs);
    b.commit(&mut run, &tree, &builder::ART, &message).await.unwrap();
    let log = vcs(&["--git-dir", &p("troveui.git"), "log", "--format=%s", "master"], &t);
    assert_eq!(log.lines().next(), Some("Custom art: Kiwi"));
    let shipped = vcs(&["--git-dir", &p("troveui.git"), "show", "--name-only", "--format=", "master"], &t);
    assert!(shipped.contains("Zakros UI - Chat/art/pfp-Kiwi.png") && shipped.contains("originals/pfp-Kiwi.png"));
    b.commit(&mut run, &tree, &builder::ART, &message).await.unwrap();
    assert_eq!(run.lines.last().map(String::as_str), Some("nothing to commit for 'Custom art: Kiwi'"));
    assert_eq!(b.version(&tree, super::CHAT).unwrap(), "1.0.0");
    assert!(!b.steam(&tree, super::CHAT, "note", &mut run).await.unwrap());
    assert_eq!(run.lines.last().unwrap(), "WARNING no Steam account configured - Zakros UI - Chat not sent to Steam");
    let text = run.text();
    assert!(text.starts_with(&format!("$ git clone {} {}", p("troveui.git"), tree.to_string_lossy())), "{text}");
    assert!(text.contains("placed pfp Kiwi"));
    b.checkout(&mut run).await.unwrap();
    assert!(run.lines.iter().any(|l| l == "$ git reset --hard FETCH_HEAD"));
    let missing = builder::Builder { python: "kiwi-no-such-interpreter".into(), ..b };
    match missing.place(&tree, &reqs, &mut run).await {
        Err(builder::Fail::Other(e)) => {
            assert!(
                e.starts_with("FileNotFoundError: [Errno 2] ") && e.ends_with(": 'kiwi-no-such-interpreter'"),
                "{e}"
            )
        }
        other => panic!("{other:?}"),
    }
    let _ = std::fs::remove_dir_all(&t);
}
