//! Replays `testdata/trove_status/fixtures.json`, generated from the real
//! Python routes and prober by `native/port/parity/gen_trove_status_fixtures.py`.

use std::sync::{Arc, Mutex, OnceLock};

use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, header},
    response::{IntoResponse, Response},
    routing::get,
};
use kiwi_core::status::{self as core, Segment};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tower::ServiceExt;

use super::{HistoryQuery, cached, history_reply, prober, status_response};
use crate::{pyjson, validation::Query};

fn fixtures() -> &'static Value {
    static F: OnceLock<Value> = OnceLock::new();
    F.get_or_init(|| serde_json::from_str(include_str!("../../../testdata/trove_status/fixtures.json")).unwrap())
}

fn cases(key: &str) -> &'static Vec<Value> {
    fixtures()[key].as_array().unwrap()
}

async fn check(resp: Response, want: &Value, label: &str) {
    assert_eq!(resp.status().as_u16(), want["status"].as_u64().unwrap() as u16, "{label}");
    let cc = resp.headers().get(header::CACHE_CONTROL).map(|v| v.to_str().unwrap().to_owned());
    assert_eq!(cc.as_deref(), want["cache_control"].as_str(), "{label}");
    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(std::str::from_utf8(&body).unwrap(), want["body"].as_str().unwrap(), "{label}");
}

#[tokio::test]
async fn current_matches_python() {
    for (i, case) in cases("current").iter().enumerate() {
        let snapshot = case["redis"].as_str().map(|s| pyjson::loads(s.as_bytes()).unwrap());
        if let (Some(s), Some(raw)) = (&snapshot, case["redis"].as_str()) {
            assert_eq!(pyjson::dumps_spaced(s), raw, "redis write {i}");
        }
        let resp = match status_response(snapshot.as_ref()) {
            Ok(v) => cached("public, max-age=30", v),
            Err(e) => e.into_response(),
        };
        check(resp, &case["reply"], &format!("current {i}")).await;
    }
}

fn segment(v: &Value) -> Segment {
    Segment {
        status: v["status"].as_str().unwrap().into(),
        online: v["online"].as_bool().unwrap(),
        started_at: v["started_at"].as_i64().unwrap(),
        ended_at: v["ended_at"].as_i64(),
    }
}

#[tokio::test]
async fn history_matches_python() {
    for (i, case) in cases("history").iter().enumerate() {
        let rows: Vec<(String, Segment)> = case["rows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| (r["env"].as_str().unwrap().to_owned(), segment(r)))
            .collect();
        let now = case["now"].as_i64().unwrap();
        let app = Router::new().route(
            "/h",
            get(move |Query(q): Query<HistoryQuery>| async move {
                let env = core::history_env(&q.env);
                let mut picked: Vec<Segment> = rows.iter().filter(|(e, _)| e == env).map(|(_, s)| s.clone()).collect();
                picked.sort_by_key(|s| s.started_at);
                history_reply(env, q.days, now, &picked)
            }),
        );
        let uri = format!("/h{}", case["query"].as_str().unwrap());
        let resp = app.oneshot(Request::get(&uri).body(Body::empty()).unwrap()).await.unwrap();
        check(resp, &case["reply"], &format!("history {i} {uri}")).await;
    }
}

#[test]
fn prober_rules_match_python() {
    for c in cases("auth_reachable") {
        let names: Vec<&str> = c["headers"].as_array().unwrap().iter().map(|h| h.as_str().unwrap()).collect();
        let got = core::auth_is_reachable(c["status"].as_u64().unwrap() as u16, names, c["body"].as_str().unwrap());
        assert_eq!(got, (c["online"].as_bool().unwrap(), c["error"].as_str().map(str::to_owned)), "{c}");
    }
    for c in fixtures()["rollup"]["overall"].as_array().unwrap() {
        let pairs: Vec<(&str, &str)> = c[0]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p[0].as_str().unwrap(), p[1]["status"].as_str().unwrap()))
            .collect();
        assert_eq!(core::overall(&pairs), c[1].as_str().unwrap(), "{c}");
    }
    for c in fixtures()["rollup"]["verdict"].as_array().unwrap() {
        assert_eq!(core::verdict(c[0].as_bool().unwrap(), c[1].as_bool().unwrap()), c[2].as_str().unwrap());
    }
    for c in cases("anchors") {
        let got = core::parse_anchors(c["spec"].as_str().unwrap());
        let want: Vec<(String, u64)> = c["anchors"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| (a[0].as_str().unwrap().to_owned(), a[1].as_u64().unwrap()))
            .collect();
        assert_eq!(got, want, "{c}");
        assert_eq!(got.is_empty(), c["up_when_none_answer"].as_bool().unwrap(), "{c}");
    }
    for c in cases("fromhex") {
        assert_eq!(core::fromhex(c[0].as_str().unwrap()).map(hex::encode), c[1].as_str().map(str::to_owned), "{c}");
    }
}

fn mask(mut v: Value, port: bool) -> Value {
    if v["error"] != "not_configured" {
        v["latency_ms"] = json!("<masked>");
    }
    if port {
        v["port"] = json!("<port>");
    }
    v
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

type Seen = Arc<Mutex<Vec<(String, String)>>>;

async fn auth_server(seen: Seen) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let seen = seen.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let (head, body_len) = loop {
                    let n = sock.read(&mut chunk).await.unwrap();
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..end]).to_string();
                        let len = head
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().to_owned())
                            })
                            .and_then(|v| v.parse::<usize>().ok())
                            .unwrap_or(0);
                        buf.drain(..end + 4);
                        break (head, len);
                    }
                };
                while buf.len() < body_len {
                    let n = sock.read(&mut chunk).await.unwrap();
                    buf.extend_from_slice(&chunk[..n]);
                }
                let path = head.split(' ').nth(1).unwrap_or("").to_owned();
                seen.lock().unwrap().push((head.clone(), String::from_utf8_lossy(&buf).into_owned()));
                let (status, extra, body) = match path.as_str() {
                    "/rejected" => ("401 Unauthorized", "X-Trionworlds-Error: InvalidCredentials\r\n", "denied"),
                    "/ticket" => ("200 OK", "", "Signature: zzz"),
                    "/gone" => ("404 Not Found", "", "404 page not found"),
                    "/broken" => ("502 Bad Gateway", "", "bad gateway"),
                    _ => ("418 I'm a Teapot", "", ""),
                };
                let reply = format!(
                    "HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(reply.as_bytes()).await;
            });
        }
    });
    port
}

#[tokio::test]
async fn auth_probe_matches_python() {
    let seen: Seen = Arc::default();
    let port = auth_server(seen.clone()).await;
    for c in cases("auth_probe") {
        let path = c["path"].as_str().unwrap();
        let url = match path {
            "refused" => format!("http://127.0.0.1:{}/x", free_port()),
            p => format!("http://127.0.0.1:{port}{p}"),
        };
        let got = mask(prober::probe_auth(&url, 8.0).await, false);
        assert_eq!(got, c["result"], "{path}");
        assert_eq!(pyjson::dumps(&got), pyjson::dumps(&c["result"]), "{path} key order");
    }
    let want = &fixtures()["auth_request"][0];
    let (head, body) = seen.lock().unwrap()[0].clone();
    let lower = head.to_ascii_lowercase();
    assert!(head.starts_with("POST /rejected HTTP/1.1\r\n"), "{head}");
    assert!(lower.contains(&format!("user-agent: {}", want["ua"].as_str().unwrap().to_ascii_lowercase())), "{head}");
    assert!(lower.contains(&format!("content-type: {}", want["ctype"].as_str().unwrap())), "{head}");
    assert_eq!(body, want["body"].as_str().unwrap());
}

async fn game_server(kind: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            tokio::spawn(async move {
                let mut buf = [0u8; 64];
                let _ = sock.read(&mut buf).await;
                match kind {
                    "hold" => tokio::time::sleep(std::time::Duration::from_secs(1)).await,
                    "drop" => {
                        let mut reply = vec![0x15u8];
                        reply.extend([0u8; 20]);
                        let _ = sock.write_all(&reply).await;
                    }
                    "chatty" => {
                        let _ = sock.write_all(&[b'x'; 300]).await;
                    }
                    _ => {
                        let _ = sock.write_all(&[b'y'; 40000]).await;
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                }
                let _ = sock.shutdown().await;
            });
        }
    });
    port
}

#[tokio::test]
async fn game_probe_matches_python() {
    let hold = i64::from(game_server("hold").await);
    let drop = i64::from(game_server("drop").await);
    let chatty = i64::from(game_server("chatty").await);
    let stream = i64::from(game_server("stream").await);
    let closed = i64::from(free_port());
    let hex_ok = format!("2000000000{}", "ab".repeat(32));
    let plan: [(&str, &str, i64, bool, &str, bool); 13] = [
        ("not_configured_host", "", 6560, false, "", false),
        ("not_configured_port", "127.0.0.1", 0, false, "", false),
        ("refused", "127.0.0.1", closed, false, "", false),
        ("connect_only", "127.0.0.1", hold, false, &hex_ok, false),
        ("empty_hello", "127.0.0.1", hold, true, "", false),
        ("bad_hex", "127.0.0.1", hold, true, "zz", false),
        ("blank_hex", "127.0.0.1", hold, true, "   ", false),
        ("held_random", "127.0.0.1", hold, true, "x", true),
        ("held_replay", "127.0.0.1", hold, true, &hex_ok, false),
        ("dropped", "127.0.0.1", drop, true, &hex_ok, false),
        ("chatty_close", "127.0.0.1", chatty, true, &hex_ok, false),
        ("streaming", "127.0.0.1", stream, true, &hex_ok, false),
        ("dns", "nonexistent.invalid", 6560, false, "", false),
    ];
    let want: Vec<&Value> = cases("game_probe").iter().collect();
    assert_eq!(want.len(), plan.len());
    for ((name, host, port, deep, hello, random), w) in plan.into_iter().zip(want) {
        assert_eq!(w["name"], name);
        let got = prober::probe_game(host, port, deep, hello, 0.3, random, 8.0).await;
        let got = mask(got, port != 0 && port != 6560);
        assert_eq!(pyjson::dumps(&got), pyjson::dumps(&w["result"]), "{name}");
    }
}

#[tokio::test]
async fn local_network_check() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let up = listener.local_addr().unwrap().port();
    let down = free_port();
    assert!(prober::local_network_is_up("", 1.0).await);
    assert!(prober::local_network_is_up(&format!("127.0.0.1:{down},127.0.0.1:{up}"), 1.0).await);
    assert!(!prober::local_network_is_up(&format!("127.0.0.1:{down},h:99999"), 1.0).await);
}

/// Needs network: POSTs the probe form to `TROVE_STATUS_TLS_URL` over HTTPS.
#[tokio::test]
#[ignore = "needs TROVE_STATUS_TLS_URL and outbound HTTPS"]
async fn auth_probe_over_tls() {
    let url = std::env::var("TROVE_STATUS_TLS_URL").expect("TROVE_STATUS_TLS_URL");
    let got = prober::probe_auth(&url, 8.0).await;
    assert!(got["http_status"].is_u64(), "{got}");
}

/// Needs throwaway Mongo and Redis: MONGO_URI / MONGO_DB / REDIS_URL, then
/// `cargo test -p kiwi-server trove_status_store -- --ignored`.
#[tokio::test]
#[ignore = "writes to MONGO_URI / REDIS_URL"]
async fn trove_status_store_roundtrip() {
    use mongodb::bson::{Bson, Document, doc};

    let state = crate::state::AppState::new(crate::config::Settings::from_env().unwrap()).await.unwrap();
    let env = format!("test-{}", mongodb::bson::oid::ObjectId::new());
    assert!(prober::try_record(&state, &env, "online", true).await.unwrap());
    assert!(!prober::try_record(&state, &env, "online", true).await.unwrap());
    assert!(prober::try_record(&state, &env, "down", false).await.unwrap());
    let coll = state.mongo().collection::<Document>(super::COLLECTION);
    let mut docs = Vec::new();
    let mut cur = coll.find(doc! { "env": &env }).sort(doc! { "started_at": 1, "_id": 1 }).await.unwrap();
    while cur.advance().await.unwrap() {
        docs.push(cur.deserialize_current().unwrap());
    }
    assert_eq!(docs.len(), 2);
    let keys: Vec<&str> = docs[0].keys().map(String::as_str).collect();
    assert_eq!(keys, ["_id", "env", "status", "online", "started_at", "ended_at"]);
    assert!(matches!(docs[0].get("started_at"), Some(Bson::Int32(_))));
    assert!(matches!(docs[0].get("ended_at"), Some(Bson::Int32(_))));
    assert_eq!(docs[1].get("ended_at"), Some(&Bson::Null));
    coll.delete_many(doc! { "env": &env }).await.unwrap();

    let snap = json!({"overall": "online", "auth": null, "environments": {}, "checked_at": 1});
    prober::publish_snapshot(&state, &snap).await;
    assert_eq!(super::shared_snapshot(&state).await, Some(snap));
    let ttl: i64 = redis::cmd("TTL").arg(super::REDIS_KEY).query_async(&mut state.redis()).await.unwrap();
    assert!((590..=600).contains(&ttl));
    let resp = status_response(super::shared_snapshot(&state).await.as_ref()).unwrap();
    assert_eq!(resp["checked_at"], 1);

    let app = crate::routes::app(state.clone());
    let get = |uri: &str| Request::get(uri).body(Body::empty()).unwrap();
    let resp = app.clone().oneshot(get("/v1/misc/trove-status")).await.unwrap();
    assert_eq!(resp.status(), 200);
    assert!(resp.headers().contains_key("x-ratelimit-limit"));
    let want = r#"{"overall":"online","auth":null,"environments":{},"checked_at":1}"#;
    check(resp, &json!({"status": 200, "cache_control": "public, max-age=30", "body": want}), "full stack").await;
    let resp = app.clone().oneshot(get("/v1/misc/trove-status/history?env=eu")).await.unwrap();
    assert_eq!(resp.status(), 422);
    let resp = app.oneshot(get("/v1/misc/trove-status/history?env=pts&days=1")).await.unwrap();
    assert_eq!(resp.status(), 200);
}
