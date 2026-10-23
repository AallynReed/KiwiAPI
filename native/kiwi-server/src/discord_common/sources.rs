//! The service reads behind the announcements and their embeds, as the
//! gateway bot process sees them (Mongo + Redis, no Postgres: activity and
//! market summaries come from the API over `INTERNAL_API_URL`).

use std::{future::Future, time::Duration};

use anyhow::Result;
use chrono::{DateTime, Utc};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Value, json};

use crate::feeds::store::News;

pub trait Sources: Send + Sync {
    /// The clock `time.time()` and `utcnow()` read, in microseconds.
    fn now_us(&self) -> i64;
    fn challenge(&self) -> impl Future<Output = Result<Value>> + Send;
    fn chaos(&self) -> impl Future<Output = Result<Value>> + Send;
    fn luxion(&self) -> impl Future<Output = Result<Value>> + Send;
    /// `get_status_shared()`.
    fn status(&self) -> impl Future<Output = Value> + Send;
    fn server_time(&self) -> Value;
    fn biome_rotation(&self) -> Result<Value>;
    fn wild_mana(&self) -> Result<Value>;
    fn stampy(&self) -> Result<Value>;
    /// `server_time.corruxion()` / `fluxion()`.
    fn corruxion(&self) -> Value;
    fn fluxion(&self) -> Value;
    /// `daily_buffs()["current"]` / `weekly_buffs()["current"]`.
    fn daily_buff(&self) -> Value;
    fn weekly_buff(&self) -> Value;
    /// `latest_news(limit)`.
    fn news(&self, limit: usize) -> impl Future<Output = Result<Vec<News>>> + Send;
    /// `created_at` of the newest open giveaway.
    fn newest_open_giveaway(&self) -> impl Future<Output = Result<Option<DateTime<Utc>>>> + Send;
    /// `list_ongoing()`, `list_upcoming()`, `list_ended(days=7)`.
    fn giveaways(&self) -> impl Future<Output = Result<[Vec<Value>; 3]>> + Send;
    /// `latest_version("live-us")` as its document's fields.
    fn latest_update(&self) -> impl Future<Output = Result<Option<Value>>> + Send;
    /// `internal_get(path_and_query)`: `None` on any failure.
    fn internal_get(&self, path: &str) -> impl Future<Output = Option<Value>> + Send;
}

/// `estimate_active_players()` in the bot process.
pub async fn activity_current<S: Sources>(s: &S) -> Value {
    s.internal_get("/v1/activity/current").await.unwrap_or_else(|| {
        json!({
            "window_start": null, "window_end": null, "duration_hours": null,
            "estimate": null, "estimate_24h": null, "estimate_7d": null,
            "by_board": [], "boards_analyzed": 0, "methodology": "unavailable",
            "computed_at": 0,
        })
    })
}

/// `activity_series("7d")` in the bot process.
pub async fn activity_series<S: Sources>(s: &S) -> Value {
    s.internal_get("/v1/activity/series?period=7d").await.unwrap_or_else(|| {
        json!({
            "period": "7d", "bucket_seconds": 0, "window_start": 0, "window_end": 0,
            "points": [], "peak": null, "low": null, "average": null, "latest": null,
            "methodology": "unavailable",
        })
    })
}

/// `urllib.parse.quote(s, safe="")`.
const QUOTE: &AsciiSet = &NON_ALPHANUMERIC.remove(b'_').remove(b'.').remove(b'-').remove(b'~');

/// `market_service.item_summary(name)` in the bot process.
pub async fn market_summary<S: Sources>(s: &S, name: &str) -> Option<Value> {
    s.internal_get(&format!("/site/market/items/{}/summary", utf8_percent_encode(name, QUOTE))).await
}

/// `internal_get`: GET `{internal_api_url}{path}` (8 s), JSON or `None`.
pub async fn http_internal_get(client: &reqwest::Client, base: &str, path: &str) -> Option<Value> {
    let url = format!("{}{path}", base.trim_end_matches('/'));
    let got = async {
        let resp = client.get(&url).timeout(Duration::from_secs(8)).send().await?.error_for_status()?;
        resp.bytes().await
    }
    .await;
    match got {
        Ok(body) => match crate::pyjson::loads(&body) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!("internal GET {path} failed: {e:?}");
                None
            }
        },
        Err(e) => {
            tracing::warn!("internal GET {path} failed: {e}");
            None
        }
    }
}
