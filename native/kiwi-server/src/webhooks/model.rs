//! `SiteWebhook` (`site_webhooks`), field for field in Beanie's order.

use chrono::{DateTime, Utc};
use mongodb::bson::{Bson, Document, oid::ObjectId};

use crate::{
    embed_templates::EmbedTemplate,
    outbound::{Health, get_dt, get_str, get_strs},
};

pub const COLLECTION: &str = "site_webhooks";
pub const EVENT_TYPES: [&str; 3] = ["challenge", "mod_release", "game_update"];
pub const MAX_CONSECUTIVE_FAILURES: i64 = 12;
pub const MAX_WEBHOOKS_PER_USER: u64 = 15;

#[derive(Clone, Debug, PartialEq)]
pub struct Webhook {
    pub health: Health,
    pub id: Option<ObjectId>,
    pub owner_id: ObjectId,
    pub label: String,
    pub url: String,
    pub events: Vec<String>,
    pub templates: Vec<(String, EmbedTemplate)>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Webhook {
    pub fn from_doc(d: &Document) -> anyhow::Result<Self> {
        let templates = match d.get("templates") {
            None => Vec::new(),
            Some(Bson::Document(t)) => t
                .iter()
                .map(|(k, v)| match v {
                    Bson::Document(td) => Ok((k.clone(), EmbedTemplate::from_bson(td))),
                    other => anyhow::bail!("templates.{k}: not a template: {other:?}"),
                })
                .collect::<anyhow::Result<_>>()?,
            Some(other) => anyhow::bail!("templates: not a dict: {other:?}"),
        };
        Ok(Webhook {
            health: Health::from_doc(d)?,
            id: d.get_object_id("_id").ok(),
            owner_id: d.get_object_id("owner_id")?,
            label: get_str(d, "label")?.unwrap_or_default(),
            url: get_str(d, "url")?.ok_or_else(|| anyhow::anyhow!("url missing"))?,
            events: get_strs(d, "events")?,
            templates,
            created_at: get_dt(d, "created_at")?.unwrap_or_else(crate::dt::now),
            updated_at: get_dt(d, "updated_at")?.unwrap_or_else(crate::dt::now),
        })
    }

    /// `get_dict(doc, to_db=True)`: `_id` sits after the health fields, and is
    /// absent until the store assigns one.
    pub fn to_doc(&self) -> Document {
        let mut d = Document::new();
        self.health.write(&mut d);
        if let Some(id) = self.id {
            d.insert("_id", id);
        }
        d.insert("owner_id", self.owner_id);
        d.insert("label", &self.label);
        d.insert("url", &self.url);
        d.insert("events", self.events.clone());
        let mut t = Document::new();
        for (k, v) in &self.templates {
            t.insert(k.clone(), v.to_bson());
        }
        d.insert("templates", t);
        d.insert("created_at", crate::dt::to_bson(self.created_at));
        d.insert("updated_at", crate::dt::to_bson(self.updated_at));
        d
    }

    pub fn template(&self, event: &str) -> Option<&EmbedTemplate> {
        self.templates.iter().find(|(k, _)| k == event).map(|(_, t)| t)
    }
}
