//! `DmSubscription` (`dm_subscriptions`), field for field in Beanie's order.

use chrono::{DateTime, Utc};
use mongodb::bson::{Bson, Document, oid::ObjectId};

use crate::outbound::{Health, get_dt, get_int, get_str, get_strs, int_bson};

pub const COLLECTION: &str = "dm_subscriptions";
pub const EVENT_TYPES: [&str; 5] = ["challenge", "corruxion", "fluxion", "game_update", "market_watch"];
pub const CHALLENGE_TYPES: [&str; 5] = ["collection", "rampage", "racing", "target", "dungeon"];
pub const BUS_EVENTS: [&str; 4] = ["challenge", "corruxion", "fluxion", "game_update"];
pub const MAX_SUBSCRIPTIONS_PER_USER: u64 = 10;
pub const MAX_WATCH_ITEMS: usize = 25;
pub const MAX_CONSECUTIVE_FAILURES: i64 = 8;
pub const MARKET_NOTIFY_COOLDOWN: i64 = 12 * 3600;

#[derive(Clone, Debug, PartialEq)]
pub struct DmSub {
    pub health: Health,
    pub id: Option<ObjectId>,
    pub owner_id: ObjectId,
    pub owner_discord_id: i64,
    pub label: String,
    pub events: Vec<String>,
    /// Stored as given: `{"challenge_types": [...]}` / `{"watch": [{name, max_price_each}]}`.
    pub filters: Document,
    pub watch_state: Vec<(String, i64)>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl DmSub {
    pub fn from_doc(d: &Document) -> anyhow::Result<Self> {
        let filters = match d.get("filters") {
            None => Document::new(),
            Some(Bson::Document(f)) => f.clone(),
            Some(other) => anyhow::bail!("filters: not a dict: {other:?}"),
        };
        let watch_state = match d.get("watch_state") {
            None => Vec::new(),
            Some(Bson::Document(w)) => w
                .keys()
                .map(|k| Ok((k.clone(), get_int(w, k)?.ok_or_else(|| anyhow::anyhow!("watch_state.{k}: null"))?)))
                .collect::<anyhow::Result<_>>()?,
            Some(other) => anyhow::bail!("watch_state: not a dict: {other:?}"),
        };
        Ok(DmSub {
            health: Health::from_doc(d)?,
            id: d.get_object_id("_id").ok(),
            owner_id: d.get_object_id("owner_id")?,
            owner_discord_id: get_int(d, "owner_discord_id")?.ok_or_else(|| anyhow::anyhow!("owner_discord_id"))?,
            label: get_str(d, "label")?.unwrap_or_default(),
            events: get_strs(d, "events")?,
            filters,
            watch_state,
            created_at: get_dt(d, "created_at")?.unwrap_or_else(crate::dt::now),
            updated_at: get_dt(d, "updated_at")?.unwrap_or_else(crate::dt::now),
        })
    }

    pub fn to_doc(&self) -> Document {
        let mut d = Document::new();
        self.health.write(&mut d);
        if let Some(id) = self.id {
            d.insert("_id", id);
        }
        d.insert("owner_id", self.owner_id);
        d.insert("owner_discord_id", int_bson(self.owner_discord_id));
        d.insert("label", &self.label);
        d.insert("events", self.events.clone());
        d.insert("filters", self.filters.clone());
        let mut w = Document::new();
        for (k, v) in &self.watch_state {
            w.insert(k.clone(), int_bson(*v));
        }
        d.insert("watch_state", w);
        d.insert("created_at", crate::dt::to_bson(self.created_at));
        d.insert("updated_at", crate::dt::to_bson(self.updated_at));
        d
    }

    pub fn watch_last(&self, name: &str) -> i64 {
        self.watch_state.iter().find(|(k, _)| k == name).map_or(0, |(_, v)| *v)
    }

    pub fn set_watch(&mut self, name: &str, at: i64) {
        match self.watch_state.iter_mut().find(|(k, _)| k == name) {
            Some(slot) => slot.1 = at,
            None => self.watch_state.push((name.to_owned(), at)),
        }
    }
}
