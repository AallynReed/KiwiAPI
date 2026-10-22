//! `app/dm_subs/service.py`: CRUD, filter validation and the DTO.

use chrono::{DateTime, Utc};
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use serde_json::{Map, Value, json};

use super::{
    delivery::Dm,
    embeds,
    model::{CHALLENGE_TYPES, DmSub, EVENT_TYPES, MAX_SUBSCRIPTIONS_PER_USER, MAX_WATCH_ITEMS},
};
use crate::{
    error::{ApiError, ApiResult},
    outbound::{Health, bson_json, doc_json, iso, py, store::Coll},
    validation::{Field, FromBody},
};

/// `DmSubCreate`.
pub struct DmSubCreate {
    pub events: Vec<String>,
    pub filters: Option<Map<String, Value>>,
    pub label: String,
}

fn filters_field(f: Field<'_>) -> Option<Map<String, Value>> {
    f.dict(|v| v.any().required()).optional().map(|pairs| pairs.into_iter().collect())
}

impl FromBody for DmSubCreate {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(DmSubCreate {
                events: m.field("events").list(|f| f.str().required()).default(Vec::new()),
                filters: filters_field(m.field("filters")),
                label: m.field("label").str().default(String::new()),
            })
        })
        .required()
    }
}

/// `DmSubUpdate`.
pub struct DmSubUpdate {
    pub events: Option<Vec<String>>,
    pub filters: Option<Map<String, Value>>,
    pub label: Option<String>,
    pub active: Option<bool>,
}

impl FromBody for DmSubUpdate {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(DmSubUpdate {
                events: m.field("events").list(|f| f.str().required()).optional(),
                filters: filters_field(m.field("filters")),
                label: m.field("label").str().optional(),
                active: m.field("active").bool().optional(),
            })
        })
        .required()
    }
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    tracing::error!("dm_subs: {e}");
    ApiError::internal()
}

/// `_clean_events`: the known types in canonical order, at least one.
pub fn clean_events(events: &[String]) -> ApiResult<Vec<String>> {
    let chosen: Vec<&str> = EVENT_TYPES.iter().copied().filter(|e| events.iter().any(|x| x == e)).collect();
    if chosen.is_empty() {
        return Err(ApiError::bad_request(format!("Pick at least one alert: {}.", EVENT_TYPES.join(", "))));
    }
    Ok(chosen.into_iter().map(str::to_owned).collect())
}

/// What `_clean_filters` keeps.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Filters {
    pub challenge_types: Option<Vec<String>>,
    pub watch: Option<Vec<(String, f64)>>,
}

impl Filters {
    pub fn to_doc(&self) -> Document {
        let mut d = Document::new();
        if let Some(t) = &self.challenge_types {
            d.insert("challenge_types", t.clone());
        }
        if let Some(w) = &self.watch {
            let items: Vec<Bson> =
                w.iter().map(|(n, p)| Bson::Document(doc! {"name": n, "max_price_each": *p})).collect();
            d.insert("watch", items);
        }
        d
    }
}

/// Stored filters as `_clean_filters` reads them back (a non-finite price
/// stays one through `float()`).
pub fn stored_filters(d: &Document) -> Map<String, Value> {
    fn conv(b: &Bson) -> Value {
        match b {
            Bson::Double(f) if !f.is_finite() => Value::String(f.to_string().to_lowercase()),
            Bson::Array(a) => Value::Array(a.iter().map(conv).collect()),
            Bson::Document(d) => Value::Object(d.iter().map(|(k, v)| (k.clone(), conv(v))).collect()),
            other => bson_json(other),
        }
    }
    d.iter().map(|(k, v)| (k.clone(), conv(v))).collect()
}

/// `_clean_filters`: only what the chosen events use, validated.
pub fn clean_filters(filters: &Map<String, Value>, events: &[String]) -> ApiResult<Filters> {
    let mut out = Filters::default();
    let has = |e: &str| events.iter().any(|x| x == e);
    if has("challenge")
        && let Some(types) = filters.get("challenge_types").filter(|v| py::truthy(v))
    {
        let items = py::iter(types).map_err(internal)?;
        let bad: Vec<Value> =
            items.into_iter().filter(|t| !t.as_str().is_some_and(|t| CHALLENGE_TYPES.contains(&t))).collect();
        if !bad.is_empty() {
            let joined = py::join(&bad, ", ").map_err(internal)?;
            return Err(ApiError::bad_request(format!("Unknown challenge type(s): {joined}.")));
        }
        let mut kept = Vec::new();
        for c in CHALLENGE_TYPES {
            if py::contains(types, &json!(c)).map_err(internal)? {
                kept.push(c.to_owned());
            }
        }
        out.challenge_types = Some(kept);
    }
    if has("market_watch") {
        let empty = json!([]);
        let raw = py::or(filters.get("watch"), &empty);
        let mut watch = Vec::new();
        for w in py::iter(raw).map_err(internal)? {
            let (name, thr) = match &w {
                Value::Object(o) => {
                    let blank = json!("");
                    let name = py::strip_str(py::or(o.get("name"), &blank)).map_err(internal)?;
                    (name, o.get("max_price_each").and_then(py::py_float).unwrap_or(0.0))
                }
                _ => (String::new(), 0.0),
            };
            if name.is_empty() || thr <= 0.0 {
                continue;
            }
            watch.push((kiwi_core::pystr::prefix(&name, 120).to_owned(), thr));
        }
        if watch.is_empty() {
            return Err(ApiError::bad_request("Add at least one item (with a price) to your market watchlist."));
        }
        if watch.len() > MAX_WATCH_ITEMS {
            return Err(ApiError::bad_request(format!("At most {MAX_WATCH_ITEMS} watchlist items.")));
        }
        out.watch = Some(watch);
    }
    Ok(out)
}

/// `_dto`.
pub fn dto(s: &DmSub) -> Value {
    json!({
        "id": s.id.map(|o| o.to_hex()),
        "label": s.label,
        "events": s.events,
        "filters": Value::Object(doc_json(&s.filters)),
        "active": s.health.active,
        "last_status": s.health.last_status,
        "last_error": s.health.last_error,
        "last_delivered_at": iso(&s.health.last_delivered_at),
        "disabled_reason": s.health.disabled_reason,
        "created_at": crate::dt::isoformat(&s.created_at),
    })
}

/// `_require_discord`.
pub fn require_discord(discord_id: Option<i64>) -> ApiResult<i64> {
    discord_id
        .filter(|d| *d != 0)
        .ok_or_else(|| ApiError::bad_request("Your account isn't linked to Discord, so we can't DM you."))
}

/// `_owned`: any failure to load is the same 404.
pub async fn owned<C: Coll>(c: &C, actor: ObjectId, sub_id: &str) -> ApiResult<DmSub> {
    let found = match ObjectId::parse_str(sub_id) {
        Ok(id) => c.get(id).await.ok().flatten().and_then(|d| DmSub::from_doc(&d).ok()),
        Err(_) => None,
    };
    found.filter(|s| s.owner_id == actor).ok_or_else(|| ApiError::not_found("Subscription not found."))
}

pub async fn list<C: Coll>(c: &C, actor: ObjectId, discord_id: Option<i64>) -> ApiResult<Value> {
    let docs = c.list_owned(actor).await.map_err(internal)?;
    let items = docs.iter().map(|d| DmSub::from_doc(d).map(|s| dto(&s))).collect::<anyhow::Result<Vec<_>>>();
    Ok(json!({"items": items.map_err(internal)?, "discord_linked": discord_id.is_some_and(|d| d != 0)}))
}

pub async fn create<C: Coll>(
    c: &C,
    actor: ObjectId,
    discord_id: Option<i64>,
    req: &DmSubCreate,
    now: DateTime<Utc>,
) -> ApiResult<Value> {
    let discord_id = require_discord(discord_id)?;
    let count = c.count_owned(actor).await.map_err(internal)?;
    if count >= MAX_SUBSCRIPTIONS_PER_USER {
        return Err(ApiError::bad_request(format!(
            "You can have at most {MAX_SUBSCRIPTIONS_PER_USER} DM subscriptions."
        )));
    }
    let events = clean_events(&req.events)?;
    let filters = clean_filters(&req.filters.clone().unwrap_or_default(), &events)?.to_doc();
    let label = kiwi_core::pystr::prefix(kiwi_core::pystr::strip(&req.label), 80).to_owned();
    let mut s = DmSub {
        health: Health::default(),
        id: None,
        owner_id: actor,
        owner_discord_id: discord_id,
        label,
        events,
        filters,
        watch_state: Vec::new(),
        created_at: now,
        updated_at: now,
    };
    s.id = Some(c.insert(s.to_doc()).await.map_err(internal)?);
    Ok(dto(&s))
}

pub async fn update<C: Coll>(
    c: &C,
    actor: ObjectId,
    discord_id: Option<i64>,
    sub_id: &str,
    req: &DmSubUpdate,
    now: DateTime<Utc>,
) -> ApiResult<Value> {
    let mut s = owned(c, actor, sub_id).await?;
    if let Some(events) = &req.events {
        s.events = clean_events(events)?;
    }
    if req.filters.is_some() || req.events.is_some() {
        let source = match &req.filters {
            Some(f) => f.clone(),
            None => stored_filters(&s.filters),
        };
        s.filters = clean_filters(&source, &s.events)?.to_doc();
    }
    if let Some(label) = &req.label {
        s.label = kiwi_core::pystr::prefix(kiwi_core::pystr::strip(label), 80).to_owned();
    }
    if let Some(active) = req.active {
        s.health.active = active;
        if active {
            s.health.consecutive_failures = 0;
            s.health.disabled_reason = None;
            s.owner_discord_id = require_discord(discord_id)?;
        }
    }
    s.updated_at = now;
    c.save(s.to_doc()).await.map_err(internal)?;
    Ok(dto(&s))
}

pub async fn delete<C: Coll>(c: &C, actor: ObjectId, sub_id: &str) -> ApiResult<()> {
    let s = owned(c, actor, sub_id).await?;
    let Some(id) = s.id else { return Err(internal("subscription without _id")) };
    c.delete(id).await.map_err(internal)
}

pub async fn send_test<C: Coll, D: Dm>(c: &C, dm: &D, actor: ObjectId, sub_id: &str) -> ApiResult<Value> {
    let s = owned(c, actor, sub_id).await?;
    let out = dm.send(s.owner_discord_id, &embeds::test_body()).await.map_err(internal)?;
    Ok(out.to_value())
}
