//! Checks against `testdata/tools/golden.json`
//! (`native/port/parity/gen_tools_fixtures.py`).

use std::sync::OnceLock;

use axum::{
    body::Body,
    http::{HeaderMap, HeaderName, HeaderValue},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use kiwi_core::form;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{
    pybytes::{b64decode_strict, utf8_error},
    tmod::{self, KIWI_MOD_LOADER},
    unlock_debug,
};
use crate::{pyjson, request_id, validation::read_body};

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| serde_json::from_str(include_str!("../../testdata/tools/golden.json")).expect("fixture"))
}

fn cases(section: &str) -> &'static Vec<Value> {
    golden()[section].as_array().unwrap_or_else(|| panic!("section {section}"))
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("string: {v}"))
}

fn b64(v: &Value) -> Vec<u8> {
    STANDARD.decode(s(v)).expect("base64")
}

pub fn lcg(seed: u32, n: usize) -> Vec<u8> {
    let mut x = seed;
    (0..n)
        .map(|_| {
            x = x.wrapping_mul(1103515245).wrapping_add(12345) & 0x7FFF_FFFF;
            (x >> 16) as u8
        })
        .collect()
}

fn leb(out: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        out.push((v & 0x7F) as u8 | 0x80);
        v >>= 7;
    }
    out.push(v as u8);
}

/// The generator's `framed()` + `raw_tmod()`: Trove's stored-block framing.
fn framed_input() -> Vec<u8> {
    let files = [("ui/big.swf", lcg(3, 40000)), ("ui/tiny", b"abcde".to_vec())];
    let mut stream = Vec::new();
    let mut table = Vec::new();
    for (path, content) in &files {
        table.push(path.len() as u8);
        table.extend_from_slice(path.as_bytes());
        leb(&mut table, 0);
        leb(&mut table, if content.is_empty() { 0 } else { stream.len() as u64 });
        leb(&mut table, content.len() as u64);
        leb(&mut table, kiwi_core::troveio::calculate_hash(content) as u64);
        stream.extend_from_slice(content);
        stream.resize(stream.len().next_multiple_of(4), 0);
    }
    let mut body = b"\x78\x01\x00\xff\xff\x00\x00".to_vec();
    for (i, chunk) in stream.chunks(32768).enumerate() {
        if i > 0 {
            body.extend_from_slice(b"\x00\x13\x37\xec\xc8");
        }
        body.extend_from_slice(chunk);
    }
    body.extend_from_slice(b"\x00\x00\x00\xff\xff");
    let mut h = vec![0u8; 8];
    h.extend_from_slice(&[1, 0, 1, 0, 5]);
    h.extend_from_slice(b"title\x06Framed");
    h.extend_from_slice(&table);
    let n = h.len() as u64;
    h[..8].copy_from_slice(&n.to_le_bytes());
    h.extend_from_slice(&body);
    h
}

fn recipe(name: &str) -> Vec<u8> {
    if name.starts_with("framed") {
        return framed_input();
    }
    let r = &golden()["recipes"][name];
    let props: Vec<(String, String)> =
        r["properties"].as_array().unwrap().iter().map(|p| (s(&p[0]).to_owned(), s(&p[1]).to_owned())).collect();
    let files: Vec<(String, Vec<u8>)> = r["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (s(&f[0]).to_owned(), lcg(f[1].as_u64().unwrap() as u32, f[2].as_u64().unwrap() as usize)))
        .collect();
    tmod::build(r["version"].as_i64().unwrap(), &props, &files, KIWI_MOD_LOADER, true).expect("recipe builds")
}

fn rid_of(c: &Value) -> String {
    if c.get("body_b64").is_none() {
        return "-".into();
    }
    let body = b64(&c["body_b64"]);
    serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|v| v["error"]["request_id"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "-".into())
}

fn check_body(what: &str, c: &Value, got: &[u8]) {
    match c.get("body_b64") {
        Some(b) => assert_eq!(String::from_utf8_lossy(got), String::from_utf8_lossy(&b64(b)), "{what}: body"),
        None => {
            assert_eq!(got.len() as u64, c["body_len"].as_u64().unwrap(), "{what}: body length");
            assert_eq!(hex::encode(Sha256::digest(got)), s(&c["body_sha256"]), "{what}: body sha256");
        }
    }
}

async fn check_response(what: &str, c: &Value, resp: Response) {
    assert_eq!(u64::from(resp.status().as_u16()), c["status"].as_u64().unwrap(), "{what}: status");
    for (k, v) in c["headers"].as_object().unwrap() {
        let got = resp.headers().get(k.as_str()).map(|h| h.to_str().unwrap().to_owned());
        assert_eq!(got.as_deref(), Some(s(v)), "{what}: header {k}");
    }
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    check_body(what, c, &body);
}

#[test]
fn utf8_errors_match_cpython() {
    for c in golden()["codecs"]["utf8"].as_array().unwrap() {
        let b = b64(&c["b64"]);
        assert_eq!(utf8_error(&b).as_deref(), c["error"].as_str(), "{b:?}");
    }
}

#[test]
fn strict_base64_matches_cpython() {
    for c in golden()["codecs"]["b64"].as_array().unwrap() {
        let got = b64decode_strict(s(&c["s"]));
        match c.get("ok") {
            Some(ok) => assert_eq!(got, Ok(b64(ok)), "{}", c["s"]),
            None => assert_eq!(got, Err(s(&c["error"]).to_owned()), "{}", c["s"]),
        }
    }
}

#[test]
fn safe_filenames() {
    for c in cases("filenames") {
        assert_eq!(tmod::safe_filename(s(&c["in"])), s(&c["out"]), "{}", c["in"]);
    }
}

#[tokio::test]
async fn read_route_replies() {
    for (i, c) in cases("read").iter().enumerate() {
        let what = format!("read #{i} {:?} {:?}", c["input"], c.get("name"));
        let (data, headers) = match c["input"].as_str() {
            Some(name) => {
                let data = match c.get("input_b64").and_then(Value::as_str) {
                    Some(b) => STANDARD.decode(b).unwrap(),
                    None => recipe(name),
                };
                assert_eq!(hex::encode(Sha256::digest(&data)), s(&c["input_sha256"]), "{what}: input");
                let mut h = HeaderMap::new();
                h.insert("content-type", HeaderValue::from_static("application/octet-stream"));
                (data, h)
            }
            None => {
                let mut h = HeaderMap::new();
                for (k, v) in c["req_headers"].as_object().unwrap() {
                    h.insert(HeaderName::from_bytes(k.to_lowercase().as_bytes()).unwrap(), s(v).parse().unwrap());
                }
                (b64(&c["req_body_b64"]), h)
            }
        };
        let query = c["query"].as_str().map(|q| q.trim_start_matches('?').to_owned()).filter(|q| !q.is_empty());
        let rid = rid_of(c);
        let resp = request_id::scope(rid, async {
            match tmod::read_raw_body(&headers, Body::from(data)).await {
                Ok(raw) => match tmod::read_reply(raw, query.as_deref(), &headers).await {
                    Ok(v) => pyjson::PydanticJson(v).into_response(),
                    Err(e) => e.into_response(),
                },
                Err(e) => e.into_response(),
            }
        })
        .await;
        check_response(&what, c, resp).await;
    }
}

#[tokio::test]
async fn build_route_replies() {
    // pyjson reads a JSON integer past u64 as a float, so pydantic's big int
    // (Python: OverflowError, 500) is a 422 int_parsing_size here.
    for c in cases("build").iter().filter(|c| c["name"] != "version_huge") {
        let what = format!("build {}", s(&c["name"]));
        let (raw, headers) = if let Some(b) = c.get("req_raw_b64") {
            let mut h = HeaderMap::new();
            for (k, v) in c["req_headers"].as_object().unwrap() {
                h.insert(HeaderName::from_bytes(k.to_lowercase().as_bytes()).unwrap(), s(v).parse().unwrap());
            }
            (b64(b), h)
        } else {
            let body = if c["req_body"].is_null() {
                let files: Vec<Value> = [("blob/big.bin", 7, 70001), ("blob/small", 9, 33)]
                    .iter()
                    .map(|(p, seed, n)| serde_json::json!({"path": p, "content_base64": STANDARD.encode(lcg(*seed, *n))}))
                    .collect();
                serde_json::json!({"version": 7, "properties": {"title": "Big"}, "files": files})
            } else {
                c["req_body"].clone()
            };
            let mut h = HeaderMap::new();
            h.insert("content-type", HeaderValue::from_static("application/json"));
            (pyjson::dumps_spaced(&body).into_bytes(), h)
        };
        let rid = rid_of(c);
        let resp = request_id::scope(rid, async {
            match read_body(&headers, Body::from(raw)).await {
                Ok(body) => tmod::build_reply(&body).await,
                Err(e) => e.into_response(),
            }
        })
        .await;
        check_response(&what, c, resp).await;
    }
}

#[tokio::test]
async fn unlock_debug_replies() {
    for c in cases("unlock").iter().filter(|c| c["enabled"] == true) {
        let what = format!("unlock {}", s(&c["name"]));
        let body = b64(&c["req_b64"]);
        let rid = rid_of(c);
        let ctype = c["req_ctype"].as_str().map(str::as_bytes);
        let resp = request_id::scope(rid, async {
            match form::parse_form(ctype, &body) {
                Ok(items) => unlock_debug::form_reply(items).await,
                Err(form::FormError::Http(msg)) => {
                    crate::error::ApiError::http(axum::http::StatusCode::BAD_REQUEST, Some(&msg)).into_response()
                }
            }
        })
        .await;
        check_response(&what, c, resp).await;
    }
}
