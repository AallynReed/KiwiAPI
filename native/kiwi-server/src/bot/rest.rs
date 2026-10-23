//! Discord REST with discord.py's `HTTPClient.request` rules: per-route
//! buckets learned from `X-RateLimit-*` headers, 429s waited out (a global
//! one holds every request), 500/502/504/524 retried after `1 + 2 * try`
//! seconds, five tries in all.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use reqwest::{Method, StatusCode, header::HeaderMap};
use serde_json::Value;
use tokio::time::Instant;

use super::RestError;
use crate::{embed_templates::EMBED_IMAGE_ATTACHMENT, pyjson};

pub const API: &str = "https://discord.com/api/v10";
const UA: &str = "DiscordBot (https://github.com/AallynReed/KiwiAPI, 1.0) kiwi-bot";
const TRIES: u32 = 5;

pub enum Body {
    Empty,
    Json(Value),
    /// `payload_json` plus `files[0]`.
    Multipart(Value, Vec<u8>),
}

#[derive(Default)]
struct Bucket {
    remaining: Option<i64>,
    reset_at: Option<Instant>,
}

pub struct Rest {
    client: reqwest::Client,
    base: String,
    auth: String,
    hashes: Mutex<HashMap<String, String>>,
    buckets: Mutex<HashMap<String, Arc<tokio::sync::Mutex<Bucket>>>>,
    global: tokio::sync::RwLock<()>,
}

fn header_f64(h: &HeaderMap, name: &str) -> Option<f64> {
    h.get(name)?.to_str().ok()?.parse().ok()
}

/// The error text without the URL (never log the token-bearing request).
fn transport(e: &reqwest::Error) -> (bool, String) {
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        if let Some(io) = s.downcast_ref::<std::io::Error>() {
            return (io.kind() == std::io::ErrorKind::ConnectionReset, io.to_string());
        }
        src = s.source();
    }
    (false, e.to_string())
}

impl Rest {
    pub fn new(token: &str) -> Self {
        Self::with_base(token, API)
    }

    pub fn with_base(token: &str, base: &str) -> Self {
        let client =
            crate::http::client_builder().timeout(Duration::from_secs(300)).user_agent(UA).build().unwrap_or_else(
                |e| {
                    tracing::error!("discord rest client: {e}");
                    reqwest::Client::new()
                },
            );
        Rest {
            client,
            base: base.trim_end_matches('/').to_owned(),
            auth: format!("Bot {token}"),
            hashes: Mutex::new(HashMap::new()),
            buckets: Mutex::new(HashMap::new()),
            global: tokio::sync::RwLock::new(()),
        }
    }

    fn bucket(&self, route: &str, major: &str) -> Arc<tokio::sync::Mutex<Bucket>> {
        let hash = self.hashes.lock().unwrap_or_else(|e| e.into_inner()).get(route).cloned();
        let key = format!("{}:{major}", hash.as_deref().unwrap_or(route));
        self.buckets.lock().unwrap_or_else(|e| e.into_inner()).entry(key).or_default().clone()
    }

    fn build(&self, method: &Method, path: &str, body: &Body) -> reqwest::RequestBuilder {
        let req =
            self.client.request(method.clone(), format!("{}{path}", self.base)).header("Authorization", &self.auth);
        match body {
            Body::Empty => req,
            Body::Json(v) => req.header("Content-Type", "application/json").body(pyjson::dumps_compact_ascii(v)),
            Body::Multipart(v, png) => {
                let json = reqwest::multipart::Part::text(pyjson::dumps_compact_ascii(v))
                    .mime_str("text/plain; charset=utf-8")
                    .expect("static mime");
                let file = reqwest::multipart::Part::bytes(png.clone())
                    .file_name(EMBED_IMAGE_ATTACHMENT)
                    .mime_str("application/octet-stream")
                    .expect("static mime");
                let form = reqwest::multipart::Form::new()
                    .percent_encode_noop()
                    .part("payload_json", json)
                    .part("files[0]", file);
                req.multipart(form)
            }
        }
    }

    /// One call; `route` is the path template that keys its bucket and
    /// `major` the channel id.
    pub async fn request(
        &self,
        method: Method,
        route: &str,
        major: &str,
        path: &str,
        body: Body,
    ) -> Result<Vec<u8>, RestError> {
        let route_key = format!("{method} {route}");
        let bucket = self.bucket(&route_key, major);
        let mut b = bucket.lock().await;
        let mut last = None;
        for tries in 0..TRIES {
            if b.remaining == Some(0)
                && let Some(at) = b.reset_at
            {
                tokio::time::sleep_until(at).await;
                b.remaining = None;
            }
            let resp = {
                let _g = self.global.read().await;
                self.build(&method, path, &body).send().await
            };
            let resp = match resp {
                Ok(r) => r,
                Err(e) => {
                    let (reset, text) = transport(&e);
                    if reset && tries < TRIES - 1 {
                        tokio::time::sleep(Duration::from_secs(u64::from(1 + tries * 2))).await;
                        continue;
                    }
                    return Err(RestError::Transport(text));
                }
            };
            let status = resp.status();
            let headers = resp.headers().clone();
            let data = match resp.bytes().await {
                Ok(b) => b.to_vec(),
                Err(e) => return Err(RestError::Transport(transport(&e).1)),
            };
            if let Some(hash) = headers.get("X-Ratelimit-Bucket").and_then(|h| h.to_str().ok()) {
                self.hashes.lock().unwrap_or_else(|e| e.into_inner()).insert(route_key.clone(), hash.to_owned());
            }
            if status != StatusCode::TOO_MANY_REQUESTS
                && let Some(rem) = header_f64(&headers, "X-Ratelimit-Remaining")
            {
                b.remaining = Some(rem as i64);
                b.reset_at = header_f64(&headers, "X-Ratelimit-Reset-After")
                    .map(|s| Instant::now() + Duration::from_secs_f64(s.max(0.0)));
            }
            let code = status.as_u16();
            last = Some(code);
            if status.is_success() {
                return Ok(data);
            }
            if code == 429 {
                let json = pyjson::loads(&data).ok().filter(Value::is_object);
                let Some(json) = json.filter(|_| headers.contains_key("Via")) else {
                    return Err(RestError::Http(429));
                };
                let wait = json.get("retry_after").and_then(Value::as_f64).unwrap_or(1.0).max(0.0);
                tracing::warn!(
                    "We are being rate limited. {method} {route} responded with 429. Retrying in {wait:.2} seconds."
                );
                if json.get("global").and_then(Value::as_bool).unwrap_or(false) {
                    let _g = self.global.write().await;
                    tokio::time::sleep(Duration::from_secs_f64(wait)).await;
                } else {
                    tokio::time::sleep(Duration::from_secs_f64(wait)).await;
                }
                continue;
            }
            if matches!(code, 500 | 502 | 504 | 524) {
                tokio::time::sleep(Duration::from_secs(u64::from(1 + tries * 2))).await;
                continue;
            }
            return Err(match code {
                403 => RestError::Forbidden,
                404 => RestError::NotFound,
                _ => RestError::Http(code),
            });
        }
        Err(RestError::Http(last.unwrap_or(0)))
    }

    pub async fn fetch_channel(&self, id: i64) -> Result<(), RestError> {
        self.request(Method::GET, "/channels/{channel_id}", &id.to_string(), &format!("/channels/{id}"), Body::Empty)
            .await
            .map(|_| ())
    }

    pub async fn send_message(&self, channel: i64, payload: Value, png: Option<Vec<u8>>) -> Result<i64, RestError> {
        let body = match png {
            Some(p) => Body::Multipart(payload, p),
            None => Body::Json(payload),
        };
        let path = format!("/channels/{channel}/messages");
        let data =
            self.request(Method::POST, "/channels/{channel_id}/messages", &channel.to_string(), &path, body).await?;
        let msg = pyjson::loads(&data).map_err(|e| RestError::Transport(format!("bad message JSON: {e:?}")))?;
        msg.get("id")
            .and_then(|v| v.as_str().map(str::to_owned).or_else(|| v.as_i64().map(|i| i.to_string())))
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| RestError::Transport("message without an id".into()))
    }

    pub async fn edit_message(&self, channel: i64, message: i64, payload: Value) -> Result<(), RestError> {
        let path = format!("/channels/{channel}/messages/{message}");
        self.request(
            Method::PATCH,
            "/channels/{channel_id}/messages/{message_id}",
            &channel.to_string(),
            &path,
            Body::Json(payload),
        )
        .await
        .map(|_| ())
    }

    /// `delete_message`, with discord.py's sub-limits for messages under ten
    /// seconds or over two weeks old.
    pub async fn delete_message(&self, channel: i64, message: i64) -> Result<(), RestError> {
        let created_ms = (message >> 22) + 1_420_070_400_000;
        let age = chrono::Utc::now().timestamp_millis() - created_ms;
        let route = if age <= 10_000 {
            "/channels/{channel_id}/messages/{message_id}:sub-10-seconds"
        } else if age >= 14 * 86_400_000 {
            "/channels/{channel_id}/messages/{message_id}:older-than-two-weeks"
        } else {
            "/channels/{channel_id}/messages/{message_id}"
        };
        let path = format!("/channels/{channel}/messages/{message}");
        self.request(Method::DELETE, route, &channel.to_string(), &path, Body::Empty).await.map(|_| ())
    }
}
