//! discord.py's request rules in [`Rest`], against a scripted local server.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use axum::{Router, body::Bytes, extract::State, http::HeaderMap, response::IntoResponse};
use serde_json::json;

use super::{RestError, rest::Rest};

type Reply = (u16, Vec<(&'static str, String)>, String);
type Seen = Vec<(String, String, HeaderMap, Bytes)>;

#[derive(Clone, Default)]
struct Script {
    replies: Arc<Mutex<VecDeque<Reply>>>,
    seen: Arc<Mutex<Seen>>,
}

async fn handle(State(s): State<Script>, req: axum::extract::Request) -> impl IntoResponse {
    let (parts, body) = req.into_parts();
    let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    s.seen.lock().unwrap().push((parts.method.to_string(), parts.uri.path().to_owned(), parts.headers, body));
    let (status, headers, body) = s.replies.lock().unwrap().pop_front().unwrap_or((200, vec![], "{}".into()));
    let mut resp = axum::http::Response::builder().status(status);
    for (k, v) in headers {
        resp = resp.header(k, v);
    }
    resp.body(axum::body::Body::from(body)).unwrap()
}

async fn serve(replies: Vec<Reply>) -> (Rest, Script) {
    let script = Script { replies: Arc::new(Mutex::new(replies.into())), ..Default::default() };
    let app = Router::new().fallback(handle).with_state(script.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (Rest::with_base("tok", &format!("http://{addr}/api/v10")), script)
}

fn ok_msg(id: &str) -> Reply {
    (200, vec![], json!({"id": id}).to_string())
}

#[tokio::test]
async fn send_waits_out_a_429_and_returns_the_id() {
    let limited = (429, vec![("Via", "1.1 google".into())], json!({"retry_after": 0.05, "global": false}).to_string());
    let (rest, script) = serve(vec![limited, ok_msg("1300000000000000042")]).await;
    let payload = json!({"embeds": [{"title": "Café"}], "content": null});
    let t0 = Instant::now();
    assert_eq!(rest.send_message(1001, payload.clone(), None).await, Ok(1_300_000_000_000_000_042));
    assert!(t0.elapsed() >= Duration::from_millis(50));
    let seen = script.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    let (method, path, headers, body) = &seen[1];
    assert_eq!((method.as_str(), path.as_str()), ("POST", "/api/v10/channels/1001/messages"));
    assert_eq!(headers["authorization"], "Bot tok");
    assert_eq!(headers["content-type"], "application/json");
    assert_eq!(body.as_ref(), crate::pyjson::dumps_compact_ascii(&payload).as_bytes());
    assert!(headers["user-agent"].to_str().unwrap().starts_with("DiscordBot ("));
}

#[tokio::test]
async fn error_statuses_map_like_discord_py() {
    let (rest, _) = serve(vec![
        (429, vec![], "slow down".into()),
        (404, vec![], "{}".into()),
        (403, vec![], "{}".into()),
        (400, vec![], "{}".into()),
    ])
    .await;
    assert_eq!(rest.fetch_channel(1).await, Err(RestError::Http(429)));
    assert_eq!(rest.fetch_channel(2).await, Err(RestError::NotFound));
    assert_eq!(rest.delete_message(3, 4).await, Err(RestError::Forbidden));
    assert_eq!(rest.edit_message(3, 4, json!({})).await, Err(RestError::Http(400)));
}

#[tokio::test]
async fn server_errors_retry_after_a_backoff() {
    let (rest, script) = serve(vec![(502, vec![], String::new()), (200, vec![], "{}".into())]).await;
    let t0 = Instant::now();
    assert_eq!(rest.edit_message(7, 8, json!({"embeds": []})).await, Ok(()));
    assert!(t0.elapsed() >= Duration::from_secs(1));
    assert_eq!(script.seen.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn an_exhausted_bucket_waits_for_its_reset() {
    let headers = vec![("X-RateLimit-Remaining", "0".into()), ("X-RateLimit-Reset-After", "0.3".into())];
    let (rest, _) = serve(vec![(200, headers, "{}".into()), (200, vec![], "{}".into())]).await;
    rest.fetch_channel(5).await.unwrap();
    let t0 = Instant::now();
    rest.fetch_channel(5).await.unwrap();
    assert!(t0.elapsed() >= Duration::from_millis(250));
}

#[tokio::test]
async fn an_attachment_goes_multipart() {
    let (rest, script) = serve(vec![ok_msg("9")]).await;
    let payload = json!({"embeds": [{"image": {"url": "attachment://embed-image.png"}}]});
    assert_eq!(rest.send_message(1, payload, Some(b"PNGDATA".to_vec())).await, Ok(9));
    let seen = script.seen.lock().unwrap();
    let body = String::from_utf8_lossy(&seen[0].3);
    assert!(seen[0].2["content-type"].to_str().unwrap().starts_with("multipart/form-data; boundary="));
    assert!(body.contains("name=\"payload_json\""));
    assert!(body.contains("name=\"files[0]\"; filename=\"embed-image.png\""));
    assert!(body.contains("application/octet-stream"));
    assert!(body.contains("PNGDATA"));
}
