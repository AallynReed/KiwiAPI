//! `app/dm_subs/delivery.py`: the `kiwi:dmsubs:queue` consumer (loop
//! `dm_subs`), the market watchlist check and the DM itself (bot token over
//! REST, no gateway).

use std::{future::Future, sync::Arc};

use futures::future::join_all;
use serde_json::{Map, Value, json};

use super::{
    embeds,
    model::{BUS_EVENTS, COLLECTION, DmSub, MARKET_NOTIFY_COOLDOWN, MAX_CONSECUTIVE_FAILURES},
};
use crate::{
    features::{self, DM_SUBS_FLAG},
    outbound::{
        self, Enqueued, Outcome, Rules, bson_json,
        discord::{Http, send_dm},
        job_get, py,
        store::{Coll, MongoColl},
    },
    state::AppState,
};

pub const RULES: Rules = Rules {
    permanent: &[403],
    permanent_reason: "The bot can't DM you (share a server with it and allow DMs).",
    auto_disable_reason: "Auto-disabled after {failures} failed DMs.",
    max_failures: MAX_CONSECUTIVE_FAILURES,
};

/// `discord_rest.send_dm`; `Err` is an exception it lets escape.
pub trait Dm: Send + Sync {
    fn send(&self, discord_id: i64, body: &Value) -> impl Future<Output = anyhow::Result<Outcome>> + Send;
}

pub struct LiveDm(pub Option<String>);

impl LiveDm {
    pub fn new(state: &AppState) -> Self {
        LiveDm(state.settings().discord_bot_token.clone())
    }
}

impl Dm for LiveDm {
    async fn send(&self, discord_id: i64, body: &Value) -> anyhow::Result<Outcome> {
        send_dm(Http::bot(), self.0.as_deref(), discord_id, body).await
    }
}

/// `_matches_filters`.
fn matches_filters(sub: &DmSub, event_type: &str, data: &Value) -> anyhow::Result<bool> {
    if event_type != "challenge" {
        return Ok(true);
    }
    let empty = json!([]);
    let wanted = sub.filters.get("challenge_types").map(bson_json);
    let wanted = py::or(wanted.as_ref(), &empty);
    if !py::truthy(wanted) {
        return Ok(true);
    }
    let name = py::get(data, "name")?.cloned().unwrap_or(Value::Null);
    let kind = embeds::classify_challenge(&name)?.map_or(Value::Null, |k| json!(k));
    py::contains(wanted, &kind)
}

/// `_deliver`: DM one bus event to every matching active subscription.
pub async fn deliver<C: Coll, D: Dm>(c: &C, dm: &D, payload: &Value) -> anyhow::Result<()> {
    let empty = Value::Object(Map::new());
    let event_type = job_get(payload, "type")?.cloned().unwrap_or(Value::Null);
    let data = py::or(job_get(payload, "data")?, &empty);
    let Some(et) = event_type.as_str() else { return Ok(()) };
    let Some(body) = embeds::build(et, data)? else { return Ok(()) };
    let subs = match c.active_for(et).await.and_then(|docs| docs.iter().map(DmSub::from_doc).collect()) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("dm-sub lookup failed ({et}): {e:#}");
            return Ok(());
        }
    };
    let subs: Vec<DmSub> = subs;
    let mut targets = Vec::new();
    for s in subs {
        if matches_filters(&s, et, data)? {
            targets.push(s);
        }
    }
    join_all(targets.into_iter().map(|s| deliver_one(c, dm, s, &body))).await;
    Ok(())
}

async fn deliver_one<C: Coll, D: Dm>(c: &C, dm: &D, mut sub: DmSub, body: &Value) {
    let Ok(out) = dm.send(sub.owner_discord_id, body).await else { return };
    outbound::record_delivery(&mut sub.health, &mut sub.updated_at, &out, &RULES);
    if let Err(e) = c.save(sub.to_doc()).await {
        tracing::warn!("dm-sub bookkeeping save failed: {e:#}");
    }
}

fn bson_num(b: Option<&mongodb::bson::Bson>) -> Option<f64> {
    match b? {
        mongodb::bson::Bson::Double(f) => Some(*f),
        mongodb::bson::Bson::Int32(i) => Some(f64::from(*i)),
        mongodb::bson::Bson::Int64(i) => Some(*i as f64),
        _ => None,
    }
}

/// `check_market` past its flag check: DM every watchlist whose threshold
/// this ingest's cheapest prices meet, at most once per item per 12 h.
pub async fn check_market_with<C: Coll, D: Dm>(
    c: &C,
    dm: &D,
    prices: &[(String, f64)],
    now: i64,
) -> anyhow::Result<()> {
    let subs = match c.active_for("market_watch").await.and_then(|docs| docs.iter().map(DmSub::from_doc).collect()) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("dm-sub market lookup failed: {e:#}");
            return Ok(());
        }
    };
    let subs: Vec<DmSub> = subs;
    for mut sub in subs {
        let watch: Vec<mongodb::bson::Document> = match sub.filters.get("watch") {
            Some(mongodb::bson::Bson::Array(a)) => a.iter().filter_map(|b| b.as_document().cloned()).collect(),
            _ => Vec::new(),
        };
        let mut fired = false;
        for w in watch {
            let Some(name) = w.get_str("name").ok().filter(|n| !n.is_empty()) else { continue };
            let Some(thr) = bson_num(w.get("max_price_each")) else { continue };
            let Some(&(_, price)) = prices.iter().find(|(n, _)| n == name) else { continue };
            if price > thr || now - sub.watch_last(name) < MARKET_NOTIFY_COOLDOWN {
                continue;
            }
            let data = json!({"name": name, "price_each": price, "max_price_each": thr});
            let Some(body) = embeds::build("market_watch", &data)? else { continue };
            let out = dm.send(sub.owner_discord_id, &body).await?;
            outbound::record_delivery(&mut sub.health, &mut sub.updated_at, &out, &RULES);
            if out.ok {
                sub.set_watch(name, now);
                fired = true;
            } else if out.status == Some(403) || !sub.health.active {
                break;
            }
        }
        if (fired || sub.health.consecutive_failures != 0 || !sub.health.active)
            && let Err(e) = c.save(sub.to_doc()).await
        {
            tracing::warn!("dm-sub market save failed: {e:#}");
        }
    }
    Ok(())
}

/// `check_market`, for the market ingest: `{item_name: cheapest_price_each}`.
pub async fn check_market(state: &AppState, prices: &[(String, f64)]) -> anyhow::Result<()> {
    if prices.is_empty() || !features::is_enabled(state, DM_SUBS_FLAG).await? {
        return Ok(());
    }
    let coll = MongoColl::new(state.mongo(), COLLECTION);
    check_market_with(&coll, &LiveDm::new(state), prices, chrono::Utc::now().timestamp()).await
}

/// `enqueue`, for the event bus: queue one event for DM fan-out.
pub async fn enqueue(state: &AppState, payload: &Value) {
    let queue = &state.settings().dm_subs_queue;
    if outbound::enqueue(state, queue, DM_SUBS_FLAG, &BUS_EVENTS, payload).await == Enqueued::Inline
        && let Some(db) = state.inner.db.mongo.clone()
    {
        let (dm, payload) = (LiveDm::new(state), payload.clone());
        tokio::spawn(async move {
            if let Err(e) = deliver(&MongoColl::new(&db, COLLECTION), &dm, &payload).await {
                tracing::warn!("dm-sub delivery failed: {e:#}");
            }
        });
    }
}

/// Loop `dm_subs`: the queue consumer.
pub fn start(state: &AppState) {
    crate::jobs::spawn(state, "dm_subs", |state| async move {
        let Some(db) = state.inner.db.mongo.clone() else {
            tracing::warn!("dm-sub consumer: MONGO_URI not configured");
            return;
        };
        let queue = state.settings().dm_subs_queue.clone();
        let ctx = Arc::new((MongoColl::new(&db, COLLECTION), LiveDm::new(&state)));
        outbound::consume("dm-sub", queue, move |payload| {
            let ctx = ctx.clone();
            async move { deliver(&ctx.0, &ctx.1, &payload).await }
        })
        .await;
    });
}
