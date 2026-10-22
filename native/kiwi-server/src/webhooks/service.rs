//! `app/webhooks/service.py`: CRUD, Discord-URL validation and the DTO.

use chrono::{DateTime, Utc};
use mongodb::bson::oid::ObjectId;
use serde_json::{Map, Value, json};

use super::{
    delivery::Poster,
    embeds,
    model::{EVENT_TYPES, MAX_WEBHOOKS_PER_USER, Webhook},
    url::{mask_url, normalise_url},
};
use crate::{
    embed_templates::EmbedTemplate,
    error::{ApiError, ApiResult},
    outbound::{Health, iso, store::Coll},
    validation::{Field, FromBody},
};

/// `WebhookCreate`.
pub struct WebhookCreate {
    pub url: String,
    pub events: Vec<String>,
    pub label: String,
    pub templates: Option<Vec<(String, EmbedTemplate)>>,
}

impl FromBody for WebhookCreate {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(WebhookCreate {
                url: m.field("url").str().max_len(400).required(),
                events: m.field("events").list(|f| f.str().required()).min_len(1).required(),
                label: m.field("label").str().max_len(80).default(String::new()),
                templates: m.field("templates").dict(EmbedTemplate::validate).optional(),
            })
        })
        .required()
    }
}

/// `WebhookUpdate`.
pub struct WebhookUpdate {
    pub url: Option<String>,
    pub events: Option<Vec<String>>,
    pub label: Option<String>,
    pub active: Option<bool>,
    pub templates: Option<Vec<(String, EmbedTemplate)>>,
}

impl FromBody for WebhookUpdate {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(WebhookUpdate {
                url: m.field("url").str().max_len(400).optional(),
                events: m.field("events").list(|f| f.str().required()).optional(),
                label: m.field("label").str().max_len(80).optional(),
                active: m.field("active").bool().optional(),
                templates: m.field("templates").dict(EmbedTemplate::validate).optional(),
            })
        })
        .required()
    }
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    tracing::error!("webhooks: {e}");
    ApiError::internal()
}

fn normalise(raw: &str) -> ApiResult<String> {
    normalise_url(raw).map_err(|e| e.unwrap_or_else(|| internal("urlparse ValueError")))
}

/// `_clean_events`: the known types in canonical order, at least one.
pub fn clean_events(events: &[String]) -> ApiResult<Vec<String>> {
    let chosen: Vec<&str> = EVENT_TYPES.iter().copied().filter(|e| events.iter().any(|x| x == e)).collect();
    if chosen.is_empty() {
        return Err(ApiError::bad_request(format!("Pick at least one event: {}.", EVENT_TYPES.join(", "))));
    }
    Ok(chosen.into_iter().map(str::to_owned).collect())
}

/// `_clean_templates`: only templates for subscribed, known events.
pub fn clean_templates(templates: &[(String, EmbedTemplate)], events: &[String]) -> Vec<(String, EmbedTemplate)> {
    templates.iter().filter(|(k, _)| EVENT_TYPES.contains(&k.as_str()) && events.contains(k)).cloned().collect()
}

fn label_clean(raw: &str) -> String {
    kiwi_core::pystr::prefix(kiwi_core::pystr::strip(raw), 80).to_owned()
}

/// `_dto`.
pub fn dto(w: &Webhook) -> ApiResult<Value> {
    let masked = mask_url(&w.url).map_err(|_| internal("mask_url ValueError"))?;
    let templates: Map<String, Value> = w.templates.iter().map(|(k, t)| (k.clone(), t.to_value())).collect();
    Ok(json!({
        "id": w.id.map(|o| o.to_hex()),
        "label": w.label,
        "url": masked,
        "events": w.events,
        "templates": templates,
        "active": w.health.active,
        "last_status": w.health.last_status,
        "last_error": w.health.last_error,
        "last_delivered_at": iso(&w.health.last_delivered_at),
        "disabled_reason": w.health.disabled_reason,
        "created_at": crate::dt::isoformat(&w.created_at),
    }))
}

/// `_owned`: any failure to load is the same 404.
pub async fn owned<C: Coll>(c: &C, actor: ObjectId, webhook_id: &str) -> ApiResult<Webhook> {
    let found = match ObjectId::parse_str(webhook_id) {
        Ok(id) => c.get(id).await.ok().flatten().and_then(|d| Webhook::from_doc(&d).ok()),
        Err(_) => None,
    };
    found.filter(|w| w.owner_id == actor).ok_or_else(|| ApiError::not_found("Webhook not found."))
}

pub async fn list<C: Coll>(c: &C, actor: ObjectId) -> ApiResult<Value> {
    let docs = c.list_owned(actor).await.map_err(internal)?;
    let items = docs
        .iter()
        .map(|d| Webhook::from_doc(d).map_err(internal).and_then(|w| dto(&w)))
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(json!({"items": items}))
}

pub async fn create<C: Coll>(c: &C, actor: ObjectId, req: &WebhookCreate, now: DateTime<Utc>) -> ApiResult<Value> {
    let count = c.count_owned(actor).await.map_err(internal)?;
    if count >= MAX_WEBHOOKS_PER_USER {
        return Err(ApiError::bad_request(format!("You can have at most {MAX_WEBHOOKS_PER_USER} webhooks.")));
    }
    let events = clean_events(&req.events)?;
    let url = normalise(&req.url)?;
    let templates = clean_templates(req.templates.as_deref().unwrap_or_default(), &events);
    let mut w = Webhook {
        health: Health::default(),
        id: None,
        owner_id: actor,
        label: label_clean(&req.label),
        url,
        events,
        templates,
        created_at: now,
        updated_at: now,
    };
    w.id = Some(c.insert(w.to_doc()).await.map_err(internal)?);
    dto(&w)
}

pub async fn update<C: Coll>(
    c: &C,
    actor: ObjectId,
    webhook_id: &str,
    req: &WebhookUpdate,
    now: DateTime<Utc>,
) -> ApiResult<Value> {
    let mut w = owned(c, actor, webhook_id).await?;
    if let Some(url) = &req.url {
        w.url = normalise(url)?;
    }
    if let Some(events) = &req.events {
        w.events = clean_events(events)?;
    }
    if let Some(t) = &req.templates {
        w.templates = clean_templates(t, &w.events);
    } else if req.events.is_some() {
        w.templates = clean_templates(&w.templates, &w.events);
    }
    if let Some(label) = &req.label {
        w.label = label_clean(label);
    }
    if let Some(active) = req.active {
        w.health.active = active;
        if active {
            w.health.consecutive_failures = 0;
            w.health.disabled_reason = None;
        }
    }
    w.updated_at = now;
    c.save(w.to_doc()).await.map_err(internal)?;
    dto(&w)
}

pub async fn delete<C: Coll>(c: &C, actor: ObjectId, webhook_id: &str) -> ApiResult<()> {
    let w = owned(c, actor, webhook_id).await?;
    let Some(id) = w.id else { return Err(internal("webhook without _id")) };
    c.delete(id).await.map_err(internal)
}

/// The body and attached design `send_test` posts.
pub fn test_payload(w: &Webhook, event: Option<&str>, asset: &str) -> (Value, Option<String>) {
    match event.filter(|e| EVENT_TYPES.contains(e)) {
        Some(event) => {
            let tmpl = w.template(event);
            let body = embeds::render_sample(event, tmpl, asset).unwrap_or_else(embeds::test_body);
            (body, EmbedTemplate::attached_design(tmpl))
        }
        None => (embeds::test_body(), None),
    }
}

pub async fn send_test<C: Coll, P: Poster>(
    c: &C,
    poster: &P,
    actor: ObjectId,
    webhook_id: &str,
    event: Option<&str>,
    asset: &str,
) -> ApiResult<Value> {
    let w = owned(c, actor, webhook_id).await?;
    let (body, design) = test_payload(&w, event, asset);
    let out = poster.post(&w.url, &body, design.as_deref()).await;
    Ok(out.to_value())
}
