//! Which Python routes Rust answers. `KIWI_PY_ROUTES` names a JSON list of
//! `{"path", "methods"}` (native/port/parity/py_routes.py); every route is
//! sent once through `routes::app` and reported as `rust` or `python`.

use std::{
    net::SocketAddr,
    sync::{Arc, Mutex},
};

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Method, Request},
};
use hyper_util::{client::legacy::Client, rt::TokioExecutor};
use serde_json::Value;
use tower::ServiceExt;

fn concrete(path: &str) -> String {
    let re = regex::Regex::new(r"\{([A-Za-z_]\w*)(?::(\w+))?\}").unwrap();
    re.replace_all(path, |c: &regex::Captures| match c.get(2).map(|m| m.as_str()) {
        Some("int") => "1".to_owned(),
        Some("path") => "a/b".to_owned(),
        _ => "x".to_owned(),
    })
    .into_owned()
}

#[tokio::test]
#[ignore = "needs KIWI_PY_ROUTES"]
async fn route_coverage() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let file = std::env::var("KIWI_PY_ROUTES").expect("KIWI_PY_ROUTES");
    let routes: Vec<Value> = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(0usize));
    let hits = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = vec![0u8; 8192];
            let _ = sock.read(&mut buf).await;
            *hits.lock().unwrap() += 1;
            let _ = sock.write_all(b"HTTP/1.1 299 OK\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await;
        }
    });
    let settings = crate::config::Settings::from_vars([("PY_UPSTREAM", format!("http://{addr}"))]).unwrap();
    let db = crate::db::Db { mongo: None, redis: None, pg: None };
    let upstream = Client::builder(TokioExecutor::new()).build_http();
    let state = crate::state::AppState { inner: Arc::new(crate::state::Inner { settings, db, upstream }) };
    let app = crate::routes::app(state);
    let peer: SocketAddr = "203.0.113.9:5000".parse().unwrap();
    let (mut rust, mut python) = (Vec::new(), Vec::new());
    for r in &routes {
        let path = r["path"].as_str().unwrap();
        for m in r["methods"].as_array().unwrap() {
            let method = Method::from_bytes(m.as_str().unwrap().as_bytes()).unwrap();
            let req = Request::builder()
                .method(method.clone())
                .uri(concrete(path))
                .extension(ConnectInfo(peer))
                .body(Body::empty())
                .unwrap();
            let status = tokio::time::timeout(std::time::Duration::from_secs(5), app.clone().oneshot(req)).await;
            let line = format!("{method} {path}");
            match status {
                Ok(Ok(res)) if res.status().as_u16() == 299 => python.push(line),
                _ => rust.push(line),
            }
        }
    }
    let out = serde_json::json!({"rust": rust, "python": python});
    let dest = std::env::var("KIWI_COVERAGE_OUT").unwrap_or_else(|_| "coverage.json".into());
    std::fs::write(&dest, serde_json::to_vec_pretty(&out).unwrap()).unwrap();
    println!("rust {} python {}", rust.len(), python.len());
}
