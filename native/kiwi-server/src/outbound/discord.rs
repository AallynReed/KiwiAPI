//! Discord over HTTPS, as the Python httpx calls do it: the webhook POST with
//! its retry rules (`app/webhooks/delivery.post_to_discord`) and the bot-token
//! DM (`app/bot/discord_rest.send_dm`). The loops run over [`Transport`] so
//! tests script the replies.

use std::{future::Future, sync::LazyLock, time::Duration};

use serde_json::{Value, json};

use super::{Outcome, clip};
use crate::{embed_templates::EMBED_IMAGE_ATTACHMENT, feeds::pyval, pyjson};

pub const API: &str = "https://discord.com/api/v10";

pub enum Payload {
    /// httpx `json=`: compact, UTF-8.
    Json(String),
    /// httpx `data={"payload_json": ...}, files={"files[0]": (name, png, "image/png")}`.
    Multipart { payload_json: String, png: Vec<u8> },
}

pub struct Reply {
    pub status: u16,
    pub retry_after: Option<String>,
    pub body: Vec<u8>,
}

impl Reply {
    /// `(r.text or "")[:200]`.
    pub fn text200(&self) -> String {
        clip(&String::from_utf8_lossy(&self.body))
    }
}

pub trait Transport: Send + Sync {
    /// One POST; `Err` is the text of the network error httpx would raise.
    fn post(
        &self,
        url: &str,
        auth: Option<&str>,
        payload: Payload,
    ) -> impl Future<Output = Result<Reply, String>> + Send;
    fn sleep(&self, secs: f64) -> impl Future<Output = ()> + Send;
}

/// `min(float(Retry-After or "1"), 5.0)`, 1.0 when it does not parse.
pub fn retry_after(raw: Option<&str>) -> f64 {
    match kiwi_core::pynum::float(raw.unwrap_or("1")) {
        Some(v) if v.is_nan() => 0.0,
        Some(v) => v.min(5.0),
        None => 1.0,
    }
}

const ATTEMPTS: usize = 3;

/// `post_to_discord`: three attempts; a 429 waits out `Retry-After` (at most
/// 5 s), a 5xx or network error backs off 0.5 s × attempt, any other 4xx is final.
pub async fn post_webhook<T: Transport>(t: &T, url: &str, body: &Value, png: Option<&[u8]>) -> Outcome {
    let mut status = None;
    let mut error = None;
    for attempt in 0..ATTEMPTS {
        let payload = match png {
            Some(p) => Payload::Multipart { payload_json: pyjson::dumps_spaced(body), png: p.to_vec() },
            None => Payload::Json(pyjson::dumps(body)),
        };
        let backoff = 0.5 * (attempt + 1) as f64;
        let r = match t.post(url, None, payload).await {
            Ok(r) => r,
            Err(e) => {
                status = None;
                error = Some(clip(&e));
                t.sleep(backoff).await;
                continue;
            }
        };
        let code = i64::from(r.status);
        status = Some(code);
        if (200..300).contains(&code) {
            return Outcome { ok: true, status, error: None };
        }
        if code == 429 {
            error = Some("rate limited by Discord".to_owned());
            if attempt < ATTEMPTS - 1 {
                t.sleep(retry_after(r.retry_after.as_deref())).await;
                continue;
            }
            return Outcome { ok: false, status, error };
        }
        let text = r.text200();
        if (400..500).contains(&code) {
            return Outcome::fail(status, if text.is_empty() { "rejected by Discord".to_owned() } else { text });
        }
        error = Some(if text.is_empty() { format!("HTTP {code}") } else { text });
        t.sleep(backoff).await;
    }
    Outcome { ok: false, status, error }
}

/// `send_dm`: open the DM channel, then post (retrying 429s up to three
/// times). `Err` is what Python lets escape (an undecodable channel reply).
pub async fn send_dm<T: Transport>(
    t: &T,
    token: Option<&str>,
    discord_id: i64,
    body: &Value,
) -> anyhow::Result<Outcome> {
    let Some(token) = token.filter(|t| !t.is_empty()) else {
        return Ok(Outcome::fail(None, "DISCORD_BOT_TOKEN is not configured."));
    };
    let auth = format!("Bot {token}");
    let open = json!({"recipient_id": discord_id.to_string()});
    let ch = match t.post(&format!("{API}/users/@me/channels"), Some(&auth), Payload::Json(pyjson::dumps(&open))).await
    {
        Ok(r) => r,
        Err(e) => return Ok(Outcome::fail(None, clip(&e))),
    };
    let ch_status = i64::from(ch.status);
    if ch_status >= 300 {
        let text = ch.text200();
        return Ok(Outcome::fail(Some(ch_status), if text.is_empty() { "could not open DM".to_owned() } else { text }));
    }
    let parsed = pyjson::loads(&ch.body).map_err(|e| anyhow::anyhow!("JSONDecodeError: {e:?}"))?;
    let channel_id = match pyval::get(&parsed, "id")? {
        Some(id) if pyval::truthy(id) => pyval::py_str(id),
        _ => return Ok(Outcome::fail(Some(ch_status), "no DM channel id returned")),
    };
    let url = format!("{API}/channels/{channel_id}/messages");
    for _ in 0..ATTEMPTS {
        let msg = match t.post(&url, Some(&auth), Payload::Json(pyjson::dumps(body))).await {
            Ok(r) => r,
            Err(e) => return Ok(Outcome::fail(None, clip(&e))),
        };
        let code = i64::from(msg.status);
        if (200..300).contains(&code) {
            return Ok(Outcome { ok: true, status: Some(code), error: None });
        }
        if code == 429 {
            t.sleep(retry_after(msg.retry_after.as_deref())).await;
            continue;
        }
        let text = msg.text200();
        return Ok(Outcome::fail(Some(code), if text.is_empty() { "rejected by Discord".to_owned() } else { text }));
    }
    Ok(Outcome::fail(Some(429), "rate limited by Discord"))
}

/// What `str(exc)` of the httpx error reads: empty for a timeout, the OS
/// error as `[Errno N] text`, else the innermost cause. Never the URL, whose
/// last segment is a webhook's secret.
fn error_text(e: reqwest::Error) -> String {
    if e.is_timeout() {
        return String::new();
    }
    let mut inner = None;
    let mut src = std::error::Error::source(&e);
    while let Some(s) = src {
        if let Some(code) = s.downcast_ref::<std::io::Error>().and_then(std::io::Error::raw_os_error) {
            let text = s.to_string();
            let text = text.strip_suffix(&format!(" (os error {code})")).unwrap_or(&text);
            return format!("[Errno {code}] {text}");
        }
        inner = Some(s.to_string());
        src = s.source();
    }
    let Some(inner) = inner else { return e.without_url().to_string() };
    match inner.strip_prefix("failed to lookup address information: ") {
        Some(gai) => {
            let code = match gai {
                "Name or service not known" => -2,
                "Temporary failure in name resolution" => -3,
                "Non-recoverable failure in name resolution" => -4,
                "No address associated with hostname" => -5,
                _ => return inner,
            };
            format!("[Errno {code}] {gai}")
        }
        None => inner,
    }
}

/// `httpx.AsyncClient(timeout=secs)`: no redirects, per-phase timeouts.
pub struct Http(reqwest::Client);

impl Http {
    fn build(secs: u64) -> Self {
        let client = crate::http::client_builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(secs))
            .read_timeout(Duration::from_secs(secs))
            .user_agent(crate::http::KIWI_UA)
            .build()
            .unwrap_or_else(|e| {
                tracing::error!("discord http client: {e}");
                reqwest::Client::new()
            });
        Http(client)
    }

    /// The webhook client (10 s).
    pub fn webhooks() -> &'static Http {
        static C: LazyLock<Http> = LazyLock::new(|| Http::build(10));
        &C
    }

    /// The bot REST client (15 s).
    pub fn bot() -> &'static Http {
        static C: LazyLock<Http> = LazyLock::new(|| Http::build(15));
        &C
    }
}

impl Transport for Http {
    async fn post(&self, url: &str, auth: Option<&str>, payload: Payload) -> Result<Reply, String> {
        let mut req = self.0.post(url);
        if let Some(a) = auth {
            req = req.header(reqwest::header::AUTHORIZATION, a);
        }
        req = match payload {
            Payload::Json(s) => req.header(reqwest::header::CONTENT_TYPE, "application/json").body(s),
            Payload::Multipart { payload_json, png } => {
                let part = reqwest::multipart::Part::bytes(png)
                    .file_name(EMBED_IMAGE_ATTACHMENT)
                    .mime_str("image/png")
                    .map_err(|e| e.to_string())?;
                req.multipart(reqwest::multipart::Form::new().text("payload_json", payload_json).part("files[0]", part))
            }
        };
        let resp = req.send().await.map_err(error_text)?;
        let status = resp.status().as_u16();
        let retry_after = resp.headers().get("retry-after").map(|v| crate::middleware::pyurl::latin1(v.as_bytes()));
        let body = resp.bytes().await.map_err(error_text)?;
        Ok(Reply { status, retry_after, body: body.to_vec() })
    }

    async fn sleep(&self, secs: f64) {
        if secs > 0.0 && secs.is_finite() {
            tokio::time::sleep(Duration::from_secs_f64(secs)).await;
        }
    }
}
