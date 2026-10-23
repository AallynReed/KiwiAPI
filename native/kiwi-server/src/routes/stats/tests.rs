//! Against `testdata/stats/golden.json` (native/port/parity/gen_stats_fixtures.py):
//! every body byte for byte, PNGs by their decoded pixels.

use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, HeaderValue, Request, StatusCode, Uri, header},
};
use http_body_util::BodyExt;
use kiwi_core::{imaging, stats as core};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tower::ServiceExt;

use super::{
    CLASS_PREFIX, SigilQuery, b64decode, classes, coefficient_body, parse_coefficient, render_sigil, sigil_png,
    stat_table,
};
use crate::{
    config::Settings,
    error::ApiError,
    gamedata::Gamedata,
    middleware::pyurl::scope_path,
    request_id,
    state::AppState,
    validation::{FromParams, Params, read_body},
};

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| serde_json::from_str(include_str!("../../../testdata/stats/golden.json")).unwrap())
}

fn baseline() -> &'static Gamedata {
    static G: OnceLock<Gamedata> = OnceLock::new();
    G.get_or_init(|| Gamedata::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/trove/gamedata"), None))
}

fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn pixels(png: &[u8]) -> String {
    sha(&imaging::decode_png(png).unwrap().pixels)
}

/// What the route answers: a JSON body, or a PNG by its pixels.
enum Got {
    Json(Result<String, ApiError>),
    Png(Result<Bytes, ApiError>),
}

fn text(r: Result<Bytes, ApiError>) -> Got {
    Got::Json(r.map(|b| String::from_utf8(b.to_vec()).unwrap()))
}

/// Compares inside the fixture's request id, so error envelopes match too.
async fn check(name: &str, got: Got, want: &Value) {
    request_id::scope(golden()["rid"].as_str().unwrap().to_owned(), async {
        let (status, body, png) = match got {
            Got::Json(Ok(b)) => (200u64, Some(b), None),
            Got::Png(Ok(p)) => (200, None, Some(p)),
            Got::Json(Err(e)) | Got::Png(Err(e)) => (e.status.as_u16().into(), Some(e.body()), None),
        };
        assert_eq!(status, want["status"].as_u64().unwrap(), "{name}: status (body {body:?})");
        if let Some(px) = want["pixels"].as_str() {
            assert_eq!(pixels(&png.expect("png")), px, "{name}: pixels");
            assert_eq!(want["content_type"], "image/png");
            assert_eq!(want["cache_control"], "public, max-age=86400");
            return;
        }
        let body = body.expect("json body");
        match want["body_sha256"].as_str() {
            Some(h) => assert_eq!(sha(body.as_bytes()), h, "{name}: body hash"),
            None => assert_eq!(body, want["body"].as_str().unwrap(), "{name}: body"),
        }
        assert_eq!(want["content_type"], "application/json", "{name}");
    })
    .await;
}

fn sigil(gd: &Gamedata, query: &str) -> Got {
    let headers = HeaderMap::new();
    let mut p = Params::new(vec![], Some(query), &headers);
    let q = SigilQuery::from_params(&mut p);
    match p.v.finish(q) {
        Err(e) => Got::Json(Err(e.into())),
        Ok(q) => Got::Png(sigil_png(gd, q.power_rank, q.mastery)),
    }
}

/// A GET the way the handlers route it.
fn get(gd: &Gamedata, path: &str) -> Got {
    if let Some(query) = path.strip_prefix("/v1/stats/sigil?") {
        return sigil(gd, query);
    }
    if path == "/v1/stats/classes" {
        return text(classes::list_body(gd));
    }
    let uri: Uri = path.parse().unwrap();
    let decoded = scope_path(&uri);
    text(classes::class_body(gd, decoded.strip_prefix(CLASS_PREFIX).unwrap()))
}

#[tokio::test]
async fn baseline_tables() {
    for (key, want) in golden()["baseline"]["tables"].as_object().unwrap() {
        check(key, Got::Json(stat_table(baseline(), key)), want).await;
    }
}

#[tokio::test]
async fn baseline_classes() {
    let b = &golden()["baseline"];
    check("list", get(baseline(), "/v1/stats/classes"), &b["list"]).await;
    for (raw, want) in b["classes"].as_object().unwrap() {
        check(raw, get(baseline(), &format!("{CLASS_PREFIX}{raw}")), want).await;
    }
}

#[tokio::test]
async fn coefficient() {
    for case in golden()["coefficient"].as_array().unwrap() {
        let mut headers = HeaderMap::new();
        if let Some(ct) = case["ctype"].as_str() {
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_str(ct).unwrap());
        }
        let request = case["request"].as_str().unwrap();
        let got = match read_body(&headers, Body::from(request.to_owned())).await {
            Ok(body) => parse_coefficient(&body).and_then(|req| coefficient_body(&req)),
            Err(e) => Err(e),
        };
        check(request, Got::Json(got), case).await;
    }
}

#[tokio::test]
async fn sigil_queries() {
    for (query, want) in golden()["sigil"]["queries"].as_object().unwrap() {
        check(query, sigil(baseline(), query), want).await;
    }
}

#[test]
fn sigil_every_tier() {
    let data = baseline().load("insignia.json");
    for (key, want) in golden()["sigil"]["tiers"].as_object().unwrap() {
        let (s, w) = key.split_once(',').unwrap();
        let png = render_sigil(data.as_deref(), s.parse().unwrap(), w.parse().unwrap()).unwrap();
        assert_eq!(pixels(&png), want.as_str().unwrap(), "tier {key}");
        assert_eq!(sha(&png), golden()["sigil"]["png_sha256"][key].as_str().unwrap(), "tier {key} bytes");
    }
}

#[test]
fn composite_matches_pillow() {
    let (mut dst, mut src) = (Vec::new(), Vec::new());
    for i in 0u32..65536 {
        let b = |x: u32| (x & 255) as u8;
        dst.extend([b(i * 7 + 3), b(i * 13 + 5), b(i * 29 + 11), (i >> 8) as u8]);
        src.extend([b(i * 31 + 17), b(i * 37 + 19), b(i * 41 + 23), b(i)]);
    }
    let img = |pixels| imaging::Rgba { width: 256, height: 256, pixels };
    let out = imaging::alpha_composite(&img(dst), &img(src)).unwrap();
    assert_eq!(sha(&out.pixels), golden()["composite_sha256"].as_str().unwrap());
}

#[test]
fn decode_matches_pillow_convert() {
    for case in golden()["decode"].as_array().unwrap() {
        let png = b64decode(case["png_b64"].as_str().unwrap()).unwrap();
        let rgba = imaging::decode_png(&png).unwrap();
        assert_eq!(hex::encode(&rgba.pixels), case["rgba_hex"].as_str().unwrap(), "{}", case["name"]);
    }
}

#[test]
fn int_digits() {
    for case in golden()["int_digits"].as_array().unwrap() {
        let bits = u64::from_str_radix(case[0].as_str().unwrap(), 16).unwrap();
        assert_eq!(core::py_int_digits(f64::from_bits(bits)).as_deref(), case[1].as_str());
    }
}

#[test]
fn board_class_helpers() {
    let b = &golden()["boards"];
    assert_eq!(serde_json::to_value(core::BOARD_CLASS_ORDER).unwrap(), b["order"]);
    assert_eq!(serde_json::to_value(core::class_board_uuids()).unwrap(), b["board_uuids"]);
    for pair in b["index_for"].as_array().unwrap() {
        assert_eq!(core::class_index_for_board(pair[0].as_i64().unwrap()), pair[1].as_i64().unwrap());
    }
}

fn write_files(dir: &Path, files: &Value) {
    std::fs::create_dir_all(dir).unwrap();
    for (name, text) in files.as_object().unwrap() {
        std::fs::write(dir.join(name), text.as_str().unwrap()).unwrap();
    }
}

fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kiwi-stats-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[tokio::test]
async fn gamedata_cases() {
    let root = scratch("cases");
    for case in golden()["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let dir = root.join(name);
        let gd = if name == "override" {
            write_files(&dir.join("base"), &case["base"]);
            write_files(&dir.join("rt"), &case["runtime"]);
            Gamedata::new(dir.join("base"), Some(dir.join("rt")))
        } else {
            write_files(&dir, &case["files"]);
            Gamedata::new(&dir, None)
        };
        for (path, want) in case["results"].as_object().unwrap() {
            check(&format!("{name} {path}"), get(&gd, path), want).await;
        }
    }
    let _ = std::fs::remove_dir_all(&root);
}

#[tokio::test]
async fn table_cases() {
    let root = scratch("tables");
    for case in golden()["tables"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let dir = root.join(name);
        std::fs::create_dir_all(dir.join("stats")).unwrap();
        std::fs::write(dir.join("stats/power_rank.json"), case["file"].as_str().unwrap()).unwrap();
        check(name, Got::Json(stat_table(&Gamedata::new(&dir, None), "power-rank")), case).await;
    }
    let _ = std::fs::remove_dir_all(&root);
}

// --- through the router and middleware (no store is reached) ----------------

async fn app() -> axum::Router {
    if std::env::var("PY_UPSTREAM").is_err() {
        // SAFETY: set once before any thread reads it; the value is constant.
        unsafe { std::env::set_var("PY_UPSTREAM", "http://127.0.0.1:9") };
    }
    crate::routes::app(AppState::new(Settings::from_env().unwrap()).await.unwrap())
}

async fn send(req: Request<Body>) -> (StatusCode, HeaderMap, Value) {
    let resp = app().await.oneshot(req).await.unwrap();
    let (parts, body) = resp.into_parts();
    let bytes = body.collect().await.unwrap().to_bytes();
    (parts.status, parts.headers, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

#[tokio::test]
async fn token_routes_need_a_token() {
    for path in ["/v1/stats/power-rank", "/v1/stats/classes", "/v1/stats/classes/knight", "/v1/stats/light"] {
        let (status, headers, body) = send(Request::get(path).body(Body::empty()).unwrap()).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
        assert_eq!(headers["www-authenticate"], "Bearer");
        assert_eq!(body["error"]["message"], "API token required");
    }
}

#[tokio::test]
async fn unrouted_method_is_405() {
    let (status, headers, body) = send(Request::post("/v1/stats/classes").body(Body::empty()).unwrap()).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(headers[header::ALLOW], "GET");
    assert_eq!(body["error"]["code"], "method_not_allowed");
}

#[tokio::test]
async fn coefficient_json_error_answers_before_auth() {
    let req = Request::post("/v1/stats/coefficient")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, "Bearer kiwi_broken")
        .body(Body::from("{"))
        .unwrap();
    let (status, _, body) = send(req).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["details"][0]["type"], "json_invalid");
}

/// Needs REDIS_URL (or MONGO_URI) for the anonymous rate limit.
#[tokio::test]
#[ignore]
async fn sigil_end_to_end() {
    let repo_gamedata = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/trove/gamedata");
    // SAFETY: set before the settings are read; ignored tests run on their own.
    unsafe { std::env::set_var("GAMEDATA_DIR", repo_gamedata) };
    let resp = app()
        .await
        .oneshot(Request::get("/v1/stats/sigil?power_rank=40000&mastery=500").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.headers()[header::CONTENT_TYPE], "image/png");
    assert_eq!(resp.headers()[header::CACHE_CONTROL], "public, max-age=86400");
    assert!(resp.headers().get("x-ratelimit-limit").is_none());
    let png = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(pixels(&png), golden()["sigil"]["queries"]["power_rank=40000&mastery=500"]["pixels"]);
}
