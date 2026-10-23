//! `app/webhooks/delivery.py`: the `kiwi:webhooks:queue` consumer (loop
//! `webhooks`) and the Discord POST. The bus enqueues each event exactly once
//! and BRPOP hands it to exactly one consumer.

use std::{future::Future, sync::Arc, time::Duration};

use futures::future::join_all;
use mongodb::bson::oid::ObjectId;
use serde_json::{Map, Value};

use super::{
    embeds,
    model::{COLLECTION, EVENT_TYPES, MAX_CONSECUTIVE_FAILURES, Webhook},
};
use crate::{
    embed_templates::EmbedTemplate,
    features::WEBHOOKS_FLAG,
    outbound::{
        self, Enqueued, Outcome, Rules,
        discord::{Http, post_webhook},
        job_get, py,
        store::{Coll, MongoColl},
    },
    state::AppState,
};

pub const RULES: Rules = Rules {
    permanent: &[404, 410],
    permanent_reason: "Discord webhook was deleted (HTTP {status}).",
    auto_disable_reason: "Auto-disabled after {failures} failed deliveries.",
    max_failures: MAX_CONSECUTIVE_FAILURES,
};

/// `post_to_discord` with the design rendered and attached.
pub trait Poster: Send + Sync {
    fn post(&self, url: &str, body: &Value, design: Option<&str>) -> impl Future<Output = Outcome> + Send;
}

pub struct LivePoster(pub AppState);

impl Poster for LivePoster {
    async fn post(&self, url: &str, body: &Value, design: Option<&str>) -> Outcome {
        let png = render_attachment(&self.0, design).await;
        post_webhook(Http::webhooks(), url, body, png.as_deref()).await
    }
}

/// `_render_attachment`: the design rendered with live data, through the
/// public PNG route on the internal API; any failure just sends the embed
/// without the image.
async fn render_attachment(state: &AppState, design: Option<&str>) -> Option<Vec<u8>> {
    static CLIENT: std::sync::LazyLock<reqwest::Client> = std::sync::LazyLock::new(|| {
        crate::http::client_builder().redirect(reqwest::redirect::Policy::none()).build().expect("http client")
    });
    let oid = ObjectId::parse_str(design.filter(|d| !d.is_empty())?).ok()?;
    let base = state.settings().internal_api_url.trim_end_matches('/').to_owned();
    let uri = format!("{base}/site/images/{}.png", oid.to_hex());
    let fetch = async {
        let resp = CLIENT.get(uri).send().await.map_err(|e| e.to_string())?;
        if resp.status() != reqwest::StatusCode::OK {
            return Ok(None);
        }
        let bytes = resp.bytes().await.map_err(|e| e.to_string())?;
        Ok::<_, String>(Some(bytes.to_vec()))
    };
    match tokio::time::timeout(Duration::from_secs(60), fetch).await {
        Ok(Ok(png)) => png,
        Ok(Err(e)) => {
            tracing::warn!("webhook image render failed ({oid}): {e}");
            None
        }
        Err(_) => {
            tracing::warn!("webhook image render failed ({oid}): timed out");
            None
        }
    }
}

/// `_deliver`: POST one event to every active webhook subscribed to it.
pub async fn deliver<C: Coll, P: Poster>(c: &C, poster: &P, payload: &Value, asset: &str) -> anyhow::Result<()> {
    let empty = Value::Object(Map::new());
    let event_type = job_get(payload, "type")?.cloned().unwrap_or(Value::Null);
    let data = py::or(job_get(payload, "data")?, &empty);
    let Some(et) = event_type.as_str() else { return Ok(()) };
    if embeds::render(et, data, None, asset)?.is_none() {
        return Ok(());
    }
    let hooks = match c.active_for(et).await.and_then(|docs| docs.iter().map(Webhook::from_doc).collect()) {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!("webhook lookup failed ({et}): {e:#}");
            return Ok(());
        }
    };
    let hooks: Vec<Webhook> = hooks;
    join_all(hooks.into_iter().map(|h| deliver_one(c, poster, h, et, data, asset))).await;
    Ok(())
}

async fn deliver_one<C: Coll, P: Poster>(c: &C, poster: &P, mut h: Webhook, et: &str, data: &Value, asset: &str) {
    let tmpl = h.template(et).cloned();
    let body = match embeds::render(et, data, tmpl.as_ref(), asset) {
        Ok(Some(b)) => b,
        _ => return,
    };
    let out = poster.post(&h.url, &body, EmbedTemplate::attached_design(tmpl.as_ref()).as_deref()).await;
    outbound::record_delivery(&mut h.health, &mut h.updated_at, &out, &RULES);
    if let Err(e) = c.save(h.to_doc()).await {
        tracing::warn!("webhook bookkeeping save failed: {e:#}");
    }
}

/// `enqueue`, for the event bus: queue one event for fan-out.
pub async fn enqueue(state: &AppState, payload: &Value) {
    let queue = &state.settings().webhooks_queue;
    if outbound::enqueue(state, queue, WEBHOOKS_FLAG, &EVENT_TYPES, payload).await == Enqueued::Inline
        && let Some(db) = state.inner.db.mongo.clone()
    {
        let (state, payload) = (state.clone(), payload.clone());
        tokio::spawn(async move {
            let asset = state.settings().asset_url();
            let coll = MongoColl::new(&db, COLLECTION);
            if let Err(e) = deliver(&coll, &LivePoster(state), &payload, &asset).await {
                tracing::warn!("webhook delivery failed: {e:#}");
            }
        });
    }
}

/// Loop `webhooks`: the queue consumer.
pub fn start(state: &AppState) {
    crate::jobs::spawn(state, "webhooks", |state| async move {
        let Some(db) = state.inner.db.mongo.clone() else {
            tracing::warn!("webhooks consumer: MONGO_URI not configured");
            return;
        };
        let queue = state.settings().webhooks_queue.clone();
        let ctx = Arc::new((MongoColl::new(&db, COLLECTION), LivePoster(state.clone()), state.settings().asset_url()));
        outbound::consume("webhook", queue, move |payload| {
            let ctx = ctx.clone();
            async move { deliver(&ctx.0, &ctx.1, &payload, &ctx.2).await }
        })
        .await;
    });
}
