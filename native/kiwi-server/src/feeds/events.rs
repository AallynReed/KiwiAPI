//! `app/trove/events.py`: the Trovesaurus calendar relayed into
//! `trove_events` (the `trove_events` loop).

use anyhow::{anyhow, bail};
use mongodb::bson::{Bson, Document, doc};
use serde_json::Value;

use super::{
    http::{self, check_status, clients},
    pyval::{self, get, or},
    store,
};
use crate::state::AppState;

/// One normalized calendar event (`parse_events`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedEvent {
    pub event_id: String,
    pub name: String,
    pub url: String,
    pub category: String,
    pub image: Option<String>,
    pub icon: Option<String>,
    pub lookup: Option<String>,
    pub starts_at: i128,
    pub ends_at: i128,
}

/// `(ev.get(key) or "").strip()`.
fn stripped(ev: &Value, key: &str) -> anyhow::Result<String> {
    let empty = Value::String(String::new());
    match or(get(ev, key)?, &empty) {
        Value::String(s) => Ok(kiwi_core::feeds::py::strip(s).to_owned()),
        other => bail!("AttributeError: '{}' object has no attribute 'strip'", pyval::type_name(other)),
    }
}

/// `int(str(ev.get(key, "")))`.
fn unix(ev: &Value, key: &str) -> anyhow::Result<Option<i128>> {
    let text = get(ev, key)?.map_or_else(String::new, pyval::py_str);
    Ok(kiwi_core::feeds::py::int(&text, 10))
}

/// `parse_events(raw)`.
pub fn parse_events(raw: &Value) -> anyhow::Result<Vec<ParsedEvent>> {
    let Value::Array(items) = raw else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    for ev in items {
        let empty = Value::String(String::new());
        let id = pyval::py_str(or(get(ev, "id")?, &empty));
        let event_id = kiwi_core::feeds::py::strip(&id).to_owned();
        if event_id.is_empty() {
            continue;
        }
        let (Some(starts_at), Some(ends_at)) = (unix(ev, "startdate")?, unix(ev, "enddate")?) else { continue };
        let opt = |s: String| (!s.is_empty()).then_some(s);
        let name = stripped(ev, "name")?;
        let url = stripped(ev, "url")?;
        let category = stripped(ev, "category")?;
        out.push(ParsedEvent {
            event_id,
            name: if name.is_empty() { "Untitled".into() } else { name },
            url,
            category: if category.is_empty() { "Event".into() } else { category },
            image: opt(stripped(ev, "image")?),
            icon: opt(stripped(ev, "icon")?),
            lookup: opt(stripped(ev, "lookup")?),
            starts_at,
            ends_at,
        });
    }
    Ok(out)
}

fn opt_bson(v: &Option<String>) -> Bson {
    v.as_ref().map_or(Bson::Null, |s| Bson::String(s.clone()))
}

/// The upserted fields, in model order.
pub fn fields(ev: &ParsedEvent) -> anyhow::Result<Document> {
    Ok(doc! {
        "name": &ev.name,
        "url": &ev.url,
        "category": &ev.category,
        "image": opt_bson(&ev.image),
        "icon": opt_bson(&ev.icon),
        "lookup": opt_bson(&ev.lookup),
        "starts_at": pyval::int_bson(ev.starts_at)?,
        "ends_at": pyval::int_bson(ev.ends_at)?,
    })
}

/// `refresh_events`: fetch, upsert by `event_id`, prune the old ones.
pub async fn refresh(state: &AppState) -> anyhow::Result<usize> {
    let url = &state.settings().trove_events_feed_url;
    let resp = check_status(clients().fetch.get(url).send().await?)?;
    let raw = http::json(resp).await?;
    let events = parse_events(&raw)?;
    let db = state.mongo();
    for ev in &events {
        store::upsert_by(db, &store::EVENT_MODEL, "event_id", &ev.event_id, fields(ev)?).await?;
    }
    let cutoff =
        i128::from(chrono::Utc::now().timestamp()) - i128::from(state.settings().trove_events_history_days) * 86_400;
    db.collection::<Document>(store::EVENTS)
        .delete_many(doc! { "ends_at": { "$lt": pyval::int_bson(cutoff)? } })
        .await
        .map_err(|e| anyhow!(e))?;
    Ok(events.len())
}

/// One cycle of the `trove_events` loop.
pub async fn refresh_cycle(state: AppState) -> anyhow::Result<()> {
    let n = refresh(&state).await?;
    tracing::info!("Trovesaurus events refresh: {n} event(s)");
    Ok(())
}
