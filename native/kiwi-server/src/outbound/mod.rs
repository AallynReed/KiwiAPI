//! What the outbound-notification features share (`app/core/delivery_health.py`,
//! `app/core/queue_worker.py`): the per-target delivery-health fields, the
//! Redis-list queue (LPUSH enqueue, one BRPOP consumer per process), Mongo
//! encoding and the Discord HTTP plumbing.

pub mod discord;
pub mod py;
pub mod store;
#[cfg(test)]
pub mod testkit;

use std::{future::Future, time::Duration};

use chrono::{DateTime, Utc};
use mongodb::bson::{Bson, Document};
use serde_json::{Map, Value};

use crate::{dt, features, pyjson, state::AppState};

/// pymongo's encoding of a Python int: int32 when it fits.
pub fn int_bson(i: i64) -> Bson {
    i32::try_from(i).map_or(Bson::Int64(i), Bson::Int32)
}

pub fn opt_str_bson(v: &Option<String>) -> Bson {
    v.as_ref().map_or(Bson::Null, |s| Bson::String(s.clone()))
}

pub fn opt_dt_bson(v: &Option<DateTime<Utc>>) -> Bson {
    v.map_or(Bson::Null, |d| Bson::DateTime(dt::to_bson(d)))
}

pub fn get_int(d: &Document, key: &str) -> anyhow::Result<Option<i64>> {
    Ok(match d.get(key) {
        None | Some(Bson::Null) => None,
        Some(Bson::Int32(i)) => Some(i64::from(*i)),
        Some(Bson::Int64(i)) => Some(*i),
        Some(Bson::Double(f)) if f.fract() == 0.0 && f.is_finite() => Some(*f as i64),
        Some(Bson::Boolean(b)) => Some(i64::from(*b)),
        Some(other) => anyhow::bail!("{key}: not an int: {other:?}"),
    })
}

pub fn get_str(d: &Document, key: &str) -> anyhow::Result<Option<String>> {
    Ok(match d.get(key) {
        None | Some(Bson::Null) => None,
        Some(Bson::String(s)) => Some(s.clone()),
        Some(other) => anyhow::bail!("{key}: not a str: {other:?}"),
    })
}

pub fn get_dt(d: &Document, key: &str) -> anyhow::Result<Option<DateTime<Utc>>> {
    Ok(match d.get(key) {
        None | Some(Bson::Null) => None,
        Some(Bson::DateTime(b)) => Some(dt::from_bson(*b)),
        Some(other) => anyhow::bail!("{key}: not a datetime: {other:?}"),
    })
}

pub fn get_strs(d: &Document, key: &str) -> anyhow::Result<Vec<String>> {
    match d.get(key) {
        None => Ok(Vec::new()),
        Some(Bson::Array(a)) => a
            .iter()
            .map(|v| v.as_str().map(str::to_owned).ok_or_else(|| anyhow::anyhow!("{key}: not a list of str")))
            .collect(),
        Some(other) => anyhow::bail!("{key}: not a list: {other:?}"),
    }
}

/// pydantic's JSON for a value pymongo decoded (non-finite floats are null).
pub fn bson_json(b: &Bson) -> Value {
    match b {
        Bson::Null | Bson::Undefined => Value::Null,
        Bson::Boolean(v) => Value::Bool(*v),
        Bson::Int32(i) => Value::from(*i),
        Bson::Int64(i) => Value::from(*i),
        Bson::Double(f) => serde_json::Number::from_f64(*f).map_or(Value::Null, Value::Number),
        Bson::String(s) => Value::String(s.clone()),
        Bson::Array(a) => Value::Array(a.iter().map(bson_json).collect()),
        Bson::Document(d) => Value::Object(doc_json(d)),
        Bson::ObjectId(o) => Value::String(o.to_hex()),
        Bson::DateTime(d) => Value::String(dt::pydantic(&dt::from_bson(*d))),
        other => Value::String(other.to_string()),
    }
}

pub fn doc_json(d: &Document) -> Map<String, Value> {
    d.iter().map(|(k, v)| (k.clone(), bson_json(v))).collect()
}

/// `iso(dt)`.
pub fn iso(d: &Option<DateTime<Utc>>) -> Value {
    d.as_ref().map_or(Value::Null, |d| Value::String(dt::isoformat(d)))
}

/// `DeliveryHealth`: the per-target fields every delivery attempt updates.
#[derive(Clone, Debug, PartialEq)]
pub struct Health {
    pub active: bool,
    pub consecutive_failures: i64,
    pub last_status: Option<i64>,
    pub last_error: Option<String>,
    pub last_delivered_at: Option<DateTime<Utc>>,
    pub disabled_reason: Option<String>,
}

impl Default for Health {
    fn default() -> Self {
        Health {
            active: true,
            consecutive_failures: 0,
            last_status: None,
            last_error: None,
            last_delivered_at: None,
            disabled_reason: None,
        }
    }
}

impl Health {
    pub fn from_doc(d: &Document) -> anyhow::Result<Self> {
        Ok(Health {
            active: match d.get("active") {
                None => true,
                Some(Bson::Boolean(b)) => *b,
                Some(other) => anyhow::bail!("active: not a bool: {other:?}"),
            },
            consecutive_failures: get_int(d, "consecutive_failures")?.unwrap_or(0),
            last_status: get_int(d, "last_status")?,
            last_error: get_str(d, "last_error")?,
            last_delivered_at: get_dt(d, "last_delivered_at")?,
            disabled_reason: get_str(d, "disabled_reason")?,
        })
    }

    /// The mixin's fields, first in every document that carries them.
    pub fn write(&self, d: &mut Document) {
        d.insert("active", self.active);
        d.insert("consecutive_failures", int_bson(self.consecutive_failures));
        d.insert("last_status", self.last_status.map_or(Bson::Null, int_bson));
        d.insert("last_error", opt_str_bson(&self.last_error));
        d.insert("last_delivered_at", opt_dt_bson(&self.last_delivered_at));
        d.insert("disabled_reason", opt_str_bson(&self.disabled_reason));
    }
}

/// One delivery attempt's `(ok, http_status, error)`.
#[derive(Clone, Debug, PartialEq)]
pub struct Outcome {
    pub ok: bool,
    pub status: Option<i64>,
    pub error: Option<String>,
}

impl Outcome {
    pub fn fail(status: Option<i64>, error: impl Into<String>) -> Self {
        Outcome { ok: false, status, error: Some(error.into()) }
    }

    pub fn to_value(&self) -> Value {
        serde_json::json!({"ok": self.ok, "status": self.status, "error": self.error})
    }
}

/// What differs between the features in `record_delivery`.
pub struct Rules {
    pub permanent: &'static [i64],
    /// `{status}` is filled in.
    pub permanent_reason: &'static str,
    /// `{failures}` is filled in.
    pub auto_disable_reason: &'static str,
    pub max_failures: i64,
}

/// `record_delivery`: fold one outcome into the health fields (no save).
pub fn record_delivery(h: &mut Health, updated_at: &mut DateTime<Utc>, o: &Outcome, rules: &Rules) {
    h.last_status = o.status;
    *updated_at = dt::now();
    if o.ok {
        h.consecutive_failures = 0;
        h.last_error = None;
        h.last_delivered_at = Some(dt::now());
        return;
    }
    if let Some(s) = o.status.filter(|s| rules.permanent.contains(s)) {
        h.active = false;
        h.last_error = o.error.clone();
        h.disabled_reason = Some(rules.permanent_reason.replace("{status}", &s.to_string()));
        return;
    }
    h.consecutive_failures += 1;
    h.last_error = o.error.clone();
    if h.consecutive_failures >= rules.max_failures {
        h.active = false;
        h.disabled_reason = Some(rules.auto_disable_reason.replace("{failures}", &h.consecutive_failures.to_string()));
    }
}

/// `str(e)[:200]`.
pub fn clip(s: &str) -> String {
    kiwi_core::pystr::prefix(s, 200).to_owned()
}

fn redis_url() -> Option<String> {
    std::env::var("REDIS_URL").ok().filter(|v| !v.is_empty())
}

#[derive(Debug, PartialEq, Eq)]
pub enum Enqueued {
    Skipped,
    Queued,
    /// No Redis: the caller delivers inline.
    Inline,
}

/// `enqueue_or_inline`: LPUSH the payload when its type is deliverable and
/// the feature is on. Called by the event bus.
pub async fn enqueue(state: &AppState, queue: &str, flag: &str, deliverable: &[&str], payload: &Value) -> Enqueued {
    let ty = payload.get("type").and_then(Value::as_str);
    if !ty.is_some_and(|t| deliverable.contains(&t)) {
        return Enqueued::Skipped;
    }
    match features::is_enabled(state, flag).await {
        Ok(true) => {}
        Ok(false) => return Enqueued::Skipped,
        Err(e) => {
            tracing::warn!("{queue} enqueue: flag lookup failed: {e:#}");
            return Enqueued::Skipped;
        }
    }
    let Some(mut redis) = state.inner.db.redis.clone() else {
        return Enqueued::Inline;
    };
    let raw = pyjson::dumps_spaced(payload);
    if let Err(e) = redis::cmd("LPUSH").arg(queue).arg(raw).query_async::<i64>(&mut redis).await {
        tracing::warn!("{queue} enqueue failed: {e}");
    }
    Enqueued::Queued
}

/// `RedisListConsumer`: BRPOP the queue (5 s timeout) on a connection of its
/// own (a blocking command would stall the shared multiplexed one), decode
/// each job and hand it over. A failing job or BRPOP is logged and the loop
/// goes on; a job is popped exactly once across every consumer.
pub async fn consume<F, Fut>(log_name: &'static str, queue: String, handler: F)
where
    F: Fn(Value) -> Fut,
    Fut: Future<Output = anyhow::Result<()>>,
{
    let Some(url) = redis_url() else {
        return;
    };
    let client = match redis::Client::open(url) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("{log_name} consumer: {e}");
            return;
        }
    };
    let cfg = redis::AsyncConnectionConfig::new().set_response_timeout(Some(Duration::from_secs(30)));
    let mut conn = None;
    loop {
        if conn.is_none() {
            match client.get_multiplexed_async_connection_with_config(&cfg).await {
                Ok(c) => conn = Some(c),
                Err(e) => {
                    tracing::warn!("{log_name} BRPOP failed: {e}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            }
        }
        let Some(c) = conn.as_mut() else { continue };
        let item: redis::RedisResult<Option<(Vec<u8>, Vec<u8>)>> =
            redis::cmd("BRPOP").arg(&queue).arg(5).query_async(c).await;
        let raw = match item {
            Ok(Some((_, raw))) => raw,
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!("{log_name} BRPOP failed: {e}");
                conn = None;
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        let result = match pyjson::loads(&raw) {
            Ok(payload) => handler(payload).await,
            Err(e) => Err(anyhow::anyhow!("undecodable job: {e:?}")),
        };
        if let Err(e) = result {
            tracing::warn!("{log_name} delivery failed: {e:#}");
        }
    }
}

/// `payload.get(key)` on a decoded job (AttributeError on a non-dict).
pub fn job_get<'a>(payload: &'a Value, key: &str) -> anyhow::Result<Option<&'a Value>> {
    crate::feeds::pyval::get(payload, key)
}
