//! `app/events/bus.py`: publish an event exactly once across every process
//! (the `SET events:sig:{type} sig GET` compare-and-set), fan it out over the
//! Redis channel and into the webhook and DM-subscription queues (through
//! their own `enqueue`, the one queue writer), and hand channel messages to
//! this process's SSE streams.

use std::{
    collections::HashMap,
    sync::{
        Arc, LazyLock, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use futures::StreamExt;
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::sources::{SOURCES, Source};
use crate::{pyjson, state::AppState};

const QUEUE_SIZE: usize = 64;

pub type Event = Arc<Value>;

static SUBSCRIBERS: LazyLock<Mutex<HashMap<u64, mpsc::Sender<Event>>>> = LazyLock::new(Default::default);
static NEXT_ID: AtomicU64 = AtomicU64::new(0);
static LOCAL_SIG: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Default::default);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Open SSE streams in this process.
pub fn connection_count() -> usize {
    lock(&SUBSCRIBERS).len()
}

/// One stream's queue, registered until dropped.
pub struct Subscription {
    id: u64,
    rx: mpsc::Receiver<Event>,
}

impl Subscription {
    pub async fn recv(&mut self) -> Option<Event> {
        self.rx.recv().await
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        lock(&SUBSCRIBERS).remove(&self.id);
    }
}

pub fn subscribe() -> Subscription {
    let (tx, rx) = mpsc::channel(QUEUE_SIZE);
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    lock(&SUBSCRIBERS).insert(id, tx);
    Subscription { id, rx }
}

/// `_broadcast_local`: a full queue drops the event so one slow stream never
/// holds up the others.
pub fn broadcast_local(event: Event) {
    for tx in lock(&SUBSCRIBERS).values() {
        let _ = tx.try_send(event.clone());
    }
}

/// `{"type": ..., "data": ..., "ts": ...}`.
pub fn envelope(kind: &str, data: Value, ts: i64) -> Value {
    json!({"type": kind, "data": data, "ts": ts})
}

fn unix_now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// `publish(type, signature, data)`: emits iff the signature differs from the
/// last one stored for the type. `data` carries datetimes already as
/// `str(datetime)` (what `json.dumps(default=str)` writes).
pub async fn publish(state: &AppState, kind: &str, signature: &str, data: Value) -> bool {
    let payload = envelope(kind, data, unix_now());
    let Some(mut con) = state.inner.db.redis.clone() else {
        {
            let mut sigs = lock(&LOCAL_SIG);
            if sigs.get(kind).map(String::as_str) == Some(signature) {
                return false;
            }
            sigs.insert(kind.to_owned(), signature.to_owned());
        }
        let payload = Arc::new(payload);
        broadcast_local(payload.clone());
        fan_out(state, &payload).await;
        return true;
    };
    let text = pyjson::dumps_spaced(&payload);
    let emitted: redis::RedisResult<bool> = async {
        let prev: Option<String> =
            redis::cmd("SET").arg(format!("events:sig:{kind}")).arg(signature).arg("GET").query_async(&mut con).await?;
        if prev.as_deref() == Some(signature) {
            return Ok(false);
        }
        redis::cmd("PUBLISH").arg(&state.settings().events_channel).arg(&text).query_async::<()>(&mut con).await?;
        Ok(true)
    }
    .await;
    match emitted {
        Ok(true) => {
            fan_out(state, &payload).await;
            true
        }
        Ok(false) => false,
        Err(e) => {
            tracing::warn!("event publish failed ({kind}): {e}");
            false
        }
    }
}

/// `_fanout_webhooks` + `_fanout_dm_subs`: each feature's own `enqueue`
/// (LPUSH of the published JSON, or inline delivery without Redis).
async fn fan_out(state: &AppState, payload: &Value) {
    crate::webhooks::enqueue(state, payload).await;
    crate::dm_subs::enqueue(state, payload).await;
}

/// `publish_source`: the source's current state, if its signature changed.
pub async fn publish_source(state: &AppState, source: Source) -> anyhow::Result<bool> {
    let data = source.data(state).await?;
    let Some(sig) = source.sig(&data, super::sources::now_us()) else { return Ok(false) };
    Ok(publish(state, source.kind(), &sig, data).await)
}

/// `publish_type(name)`: for the state-change producers (status prober, news
/// refresher, giveaway worker, update archiver).
pub async fn publish_type(state: &AppState, name: &str) -> anyhow::Result<bool> {
    match Source::by_type(name) {
        Some(source) => publish_source(state, source).await,
        None => Ok(false),
    }
}

pub async fn publish_challenge(state: &AppState) -> anyhow::Result<bool> {
    publish_source(state, Source::Challenge).await
}

pub async fn publish_chaos(state: &AppState) -> anyhow::Result<bool> {
    publish_source(state, Source::Chaos).await
}

pub async fn publish_luxion(state: &AppState) -> anyhow::Result<bool> {
    publish_source(state, Source::Luxion).await
}

/// `current_snapshot()`: every source that has something to announce, sharing
/// one `ts`.
pub async fn current_snapshot(state: &AppState) -> Vec<Value> {
    let ts = unix_now();
    let mut out = Vec::new();
    for source in SOURCES {
        match source.data(state).await {
            Ok(data) => {
                if source.sig(&data, super::sources::now_us()).is_some() {
                    out.push(envelope(source.kind(), data, ts));
                }
            }
            Err(e) => tracing::warn!("snapshot: {} fetch failed: {e:#}", source.kind()),
        }
    }
    out
}

/// What `json.loads` of a channel message gives `_broadcast_local` (Python
/// reads it as decoded text, so a BOM or bad UTF-8 is skipped).
pub fn decode_message(raw: &[u8]) -> Option<Value> {
    let text = std::str::from_utf8(raw).ok()?;
    if text.starts_with('\u{feff}') {
        return None;
    }
    pyjson::loads(text.as_bytes()).ok()
}

/// `_listen` (`event_listener`): one SUBSCRIBE per process, reconnecting the
/// way redis-py's pubsub does.
pub async fn listen(state: AppState) {
    let Some(url) = state.settings().redis_url.clone().filter(|u| !u.is_empty()) else { return };
    let channel = state.settings().events_channel.clone();
    loop {
        if let Err(e) = listen_once(&url, &channel).await {
            tracing::warn!("event listener: {e}");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

async fn listen_once(url: &str, channel: &str) -> redis::RedisResult<()> {
    let client = redis::Client::open(url)?;
    let mut pubsub = client.get_async_pubsub().await?;
    pubsub.subscribe(channel).await?;
    let mut messages = pubsub.on_message();
    while let Some(msg) = messages.next().await {
        if let Some(payload) = decode_message(msg.get_payload_bytes()) {
            broadcast_local(Arc::new(payload));
        }
    }
    Ok(())
}

/// `_watch` (`event_watch`): re-publish challenge and chaos on a timer so a
/// missed insert-driven publish still goes out.
pub async fn watch(state: AppState) {
    let interval = Duration::from_secs(state.settings().events_watch_seconds.max(5) as u64);
    loop {
        let tick = async {
            publish_challenge(&state).await?;
            publish_chaos(&state).await
        };
        if let Err(e) = tick.await {
            tracing::warn!("event watch tick failed: {e:#}");
        }
        tokio::time::sleep(interval).await;
    }
}
