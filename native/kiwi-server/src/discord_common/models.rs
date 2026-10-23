//! `app/bot/models.py` as Beanie loads and saves it: `GuildConfig`
//! (`guild_configs`) and `TrackedAnnouncement` (`tracked_announcements`).
//! A load validates like pydantic; `to_doc` is the `$set` of `save()`
//! (`_id` first, every field in model order, explicit nulls, ints as int32
//! when they fit).

use anyhow::{Context, Result, bail};
use indexmap::IndexMap;
use mongodb::bson::{Bson, DateTime, Document, doc, oid::ObjectId};

use crate::{embed_templates::EmbedTemplate, outbound::int_bson};

pub const GUILD_CONFIGS: &str = "guild_configs";
pub const TRACKED: &str = "tracked_announcements";

/// pydantic lax `int`.
fn int(b: &Bson, key: &str) -> Result<i64> {
    match b {
        Bson::Int32(i) => Ok(i64::from(*i)),
        Bson::Int64(i) => Ok(*i),
        Bson::Boolean(v) => Ok(i64::from(*v)),
        Bson::Double(f) if f.fract() == 0.0 && f.is_finite() => Ok(*f as i64),
        Bson::String(s) => s.trim().parse().with_context(|| format!("{key}: not an int")),
        other => bail!("{key}: not an int: {other:?}"),
    }
}

fn opt_int(d: &Document, key: &str) -> Result<Option<i64>> {
    match d.get(key) {
        None | Some(Bson::Null) => Ok(None),
        Some(b) => int(b, key).map(Some),
    }
}

fn boolean(d: &Document, key: &str, default: bool) -> Result<bool> {
    match d.get(key) {
        None => Ok(default),
        Some(Bson::Boolean(b)) => Ok(*b),
        Some(Bson::Int32(i)) if *i == 0 || *i == 1 => Ok(*i == 1),
        Some(Bson::Int64(i)) if *i == 0 || *i == 1 => Ok(*i == 1),
        Some(other) => bail!("{key}: not a bool: {other:?}"),
    }
}

fn opt_str(d: &Document, key: &str) -> Result<Option<String>> {
    match d.get(key) {
        None | Some(Bson::Null) => Ok(None),
        Some(Bson::String(s)) => Ok(Some(s.clone())),
        Some(other) => bail!("{key}: not a str: {other:?}"),
    }
}

fn ints(d: &Document, key: &str) -> Result<Vec<i64>> {
    match d.get(key) {
        None => Ok(Vec::new()),
        Some(Bson::Array(a)) => a.iter().map(|b| int(b, key)).collect(),
        Some(other) => bail!("{key}: not a list: {other:?}"),
    }
}

fn sub<'a>(d: &'a Document, key: &str) -> Result<Option<&'a Document>> {
    match d.get(key) {
        None => Ok(None),
        Some(Bson::Document(s)) => Ok(Some(s)),
        Some(other) => bail!("{key}: not a dict: {other:?}"),
    }
}

fn dt_field(d: &Document, key: &str, now: DateTime) -> Result<DateTime> {
    match d.get(key) {
        None => Ok(now),
        Some(Bson::DateTime(t)) => Ok(*t),
        Some(other) => bail!("{key}: not a datetime: {other:?}"),
    }
}

fn opt_int_bson(v: Option<i64>) -> Bson {
    v.map_or(Bson::Null, int_bson)
}

fn ints_bson(v: &[i64]) -> Bson {
    Bson::Array(v.iter().map(|i| int_bson(*i)).collect())
}

fn opt_str_bson(v: &Option<String>) -> Bson {
    v.as_ref().map_or(Bson::Null, |s| Bson::String(s.clone()))
}

fn template(d: &Document, key: &str) -> Result<Option<EmbedTemplate>> {
    match d.get(key) {
        None | Some(Bson::Null) => Ok(None),
        Some(Bson::Document(t)) => Ok(Some(EmbedTemplate::from_bson(t))),
        Some(other) => bail!("{key}: not a template: {other:?}"),
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AnnouncementSetting {
    pub enabled: bool,
    pub channel_id: Option<i64>,
    pub ping_role_ids: Vec<i64>,
    pub ping_role_id: Option<i64>,
    pub template: Option<EmbedTemplate>,
    pub last_anchor: Option<String>,
    pub channel_missing: bool,
}

impl AnnouncementSetting {
    pub fn from_doc(d: &Document) -> Result<Self> {
        Ok(AnnouncementSetting {
            enabled: boolean(d, "enabled", false)?,
            channel_id: opt_int(d, "channel_id")?,
            ping_role_ids: ints(d, "ping_role_ids")?,
            ping_role_id: opt_int(d, "ping_role_id")?,
            template: template(d, "template")?,
            last_anchor: opt_str(d, "last_anchor")?,
            channel_missing: boolean(d, "channel_missing", false)?,
        })
    }

    pub fn to_doc(&self) -> Document {
        doc! {
            "enabled": self.enabled,
            "channel_id": opt_int_bson(self.channel_id),
            "ping_role_ids": ints_bson(&self.ping_role_ids),
            "ping_role_id": opt_int_bson(self.ping_role_id),
            "template": self.template.as_ref().map_or(Bson::Null, |t| Bson::Document(t.to_bson())),
            "last_anchor": opt_str_bson(&self.last_anchor),
            "channel_missing": self.channel_missing,
        }
    }

    /// Posting is on: enabled, pointed at a channel that still exists.
    pub fn live(&self) -> bool {
        self.enabled && self.channel_id.is_some_and(|c| c != 0) && !self.channel_missing
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiveBoard {
    pub enabled: bool,
    pub channel_id: Option<i64>,
    pub message_id: Option<i64>,
    pub channel_missing: bool,
}

impl LiveBoard {
    fn from_doc(d: &Document) -> Result<Self> {
        Ok(LiveBoard {
            enabled: boolean(d, "enabled", false)?,
            channel_id: opt_int(d, "channel_id")?,
            message_id: opt_int(d, "message_id")?,
            channel_missing: boolean(d, "channel_missing", false)?,
        })
    }

    fn to_doc(&self) -> Document {
        doc! {
            "enabled": self.enabled,
            "channel_id": opt_int_bson(self.channel_id),
            "message_id": opt_int_bson(self.message_id),
            "channel_missing": self.channel_missing,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MarketWatchItem {
    pub name: String,
    pub max_price_each: Option<f64>,
    pub last_alert_sig: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MarketWatch {
    pub enabled: bool,
    pub channel_id: Option<i64>,
    pub ping_role_ids: Vec<i64>,
    pub channel_missing: bool,
    pub items: Vec<MarketWatchItem>,
}

fn opt_float(d: &Document, key: &str) -> Result<Option<f64>> {
    match d.get(key) {
        None | Some(Bson::Null) => Ok(None),
        Some(Bson::Double(f)) => Ok(Some(*f)),
        Some(Bson::Int32(i)) => Ok(Some(f64::from(*i))),
        Some(Bson::Int64(i)) => Ok(Some(*i as f64)),
        Some(Bson::Boolean(b)) => Ok(Some(f64::from(u8::from(*b)))),
        Some(Bson::String(s)) => kiwi_core::pynum::float(s).map(Some).with_context(|| format!("{key}: not a float")),
        Some(other) => bail!("{key}: not a float: {other:?}"),
    }
}

impl MarketWatch {
    fn from_doc(d: &Document) -> Result<Self> {
        let items = match d.get("items") {
            None => Vec::new(),
            Some(Bson::Array(a)) => a
                .iter()
                .map(|b| {
                    let i = b.as_document().context("items: not a dict")?;
                    Ok(MarketWatchItem {
                        name: match i.get("name") {
                            Some(Bson::String(s)) => s.clone(),
                            other => bail!("items.name: not a str: {other:?}"),
                        },
                        max_price_each: opt_float(i, "max_price_each")?,
                        last_alert_sig: opt_str(i, "last_alert_sig")?,
                    })
                })
                .collect::<Result<_>>()?,
            Some(other) => bail!("items: not a list: {other:?}"),
        };
        Ok(MarketWatch {
            enabled: boolean(d, "enabled", false)?,
            channel_id: opt_int(d, "channel_id")?,
            ping_role_ids: ints(d, "ping_role_ids")?,
            channel_missing: boolean(d, "channel_missing", false)?,
            items,
        })
    }

    fn to_doc(&self) -> Document {
        let items: Vec<Bson> = self
            .items
            .iter()
            .map(|i| {
                Bson::Document(doc! {
                    "name": &i.name,
                    "max_price_each": i.max_price_each.map_or(Bson::Null, Bson::Double),
                    "last_alert_sig": opt_str_bson(&i.last_alert_sig),
                })
            })
            .collect();
        doc! {
            "enabled": self.enabled,
            "channel_id": opt_int_bson(self.channel_id),
            "ping_role_ids": ints_bson(&self.ping_role_ids),
            "channel_missing": self.channel_missing,
            "items": items,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GuildConfig {
    pub id: ObjectId,
    pub guild_id: i64,
    pub announcements: IndexMap<String, AnnouncementSetting>,
    pub live_board: LiveBoard,
    pub market_watch: MarketWatch,
    pub language: String,
    pub config_perms: IndexMap<String, Vec<i64>>,
    pub hourly_challenge_enabled: bool,
    pub announce_channel_id: Option<i64>,
    pub last_announced_challenge_anchor: Option<i64>,
    pub updated_by: Option<i64>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
}

impl GuildConfig {
    /// A stored document; `now` fills a missing `created_at`/`updated_at`.
    pub fn from_doc(d: &Document, now: DateTime) -> Result<Self> {
        let id = d.get_object_id("_id").context("_id")?;
        let guild_id = match d.get("guild_id") {
            Some(b) => int(b, "guild_id")?,
            None => bail!("guild_id: missing"),
        };
        let mut announcements = IndexMap::new();
        if let Some(a) = sub(d, "announcements")? {
            for (k, v) in a {
                let s = v.as_document().with_context(|| format!("announcements.{k}: not a dict"))?;
                announcements.insert(k.clone(), AnnouncementSetting::from_doc(s)?);
            }
        }
        let mut config_perms = IndexMap::new();
        if let Some(p) = sub(d, "config_perms")? {
            for (k, v) in p {
                let roles = match v {
                    Bson::Array(a) => a.iter().map(|b| int(b, "config_perms")).collect::<Result<Vec<_>>>()?,
                    other => bail!("config_perms.{k}: not a list: {other:?}"),
                };
                config_perms.insert(k.clone(), roles);
            }
        }
        Ok(GuildConfig {
            id,
            guild_id,
            announcements,
            live_board: sub(d, "live_board")?.map(LiveBoard::from_doc).transpose()?.unwrap_or_default(),
            market_watch: sub(d, "market_watch")?.map(MarketWatch::from_doc).transpose()?.unwrap_or_default(),
            language: opt_str(d, "language")?.unwrap_or_else(|| "en".into()),
            config_perms,
            hourly_challenge_enabled: boolean(d, "hourly_challenge_enabled", false)?,
            announce_channel_id: opt_int(d, "announce_channel_id")?,
            last_announced_challenge_anchor: opt_int(d, "last_announced_challenge_anchor")?,
            updated_by: opt_int(d, "updated_by")?,
            created_at: dt_field(d, "created_at", now)?,
            updated_at: dt_field(d, "updated_at", now)?,
        })
    }

    pub fn to_doc(&self) -> Document {
        let mut ann = Document::new();
        for (k, v) in &self.announcements {
            ann.insert(k.clone(), v.to_doc());
        }
        let mut perms = Document::new();
        for (k, v) in &self.config_perms {
            perms.insert(k.clone(), ints_bson(v));
        }
        doc! {
            "_id": self.id,
            "guild_id": int_bson(self.guild_id),
            "announcements": ann,
            "live_board": self.live_board.to_doc(),
            "market_watch": self.market_watch.to_doc(),
            "language": &self.language,
            "config_perms": perms,
            "hourly_challenge_enabled": self.hourly_challenge_enabled,
            "announce_channel_id": opt_int_bson(self.announce_channel_id),
            "last_announced_challenge_anchor": opt_int_bson(self.last_announced_challenge_anchor),
            "updated_by": opt_int_bson(self.updated_by),
            "created_at": self.created_at,
            "updated_at": self.updated_at,
        }
    }

    /// `migrate_legacy`: fold the pre-registry challenge fields and the single
    /// ping role into the current shape. True when anything changed.
    pub fn migrate_legacy(&mut self) -> bool {
        let mut changed = false;
        if self.hourly_challenge_enabled
            || self.announce_channel_id.is_some()
            || self.last_announced_challenge_anchor.is_some()
        {
            if !self.announcements.contains_key("hourly_challenge") {
                self.announcements.insert(
                    "hourly_challenge".into(),
                    AnnouncementSetting {
                        enabled: self.hourly_challenge_enabled,
                        channel_id: self.announce_channel_id,
                        last_anchor: self.last_announced_challenge_anchor.map(|a| a.to_string()),
                        ..Default::default()
                    },
                );
            }
            self.hourly_challenge_enabled = false;
            self.announce_channel_id = None;
            self.last_announced_challenge_anchor = None;
            changed = true;
        }
        if let Some(legacy) = self.announcements.shift_remove("hourly_challenge") {
            for cat in ["collection", "rampage", "racing", "target", "dungeon"] {
                let key = format!("challenge_{cat}");
                if !self.announcements.contains_key(&key) {
                    self.announcements.insert(
                        key,
                        AnnouncementSetting {
                            enabled: legacy.enabled,
                            channel_id: legacy.channel_id,
                            ping_role_ids: legacy.ping_role_ids.clone(),
                            last_anchor: legacy.last_anchor.clone(),
                            channel_missing: legacy.channel_missing,
                            ..Default::default()
                        },
                    );
                }
            }
            changed = true;
        }
        for s in self.announcements.values_mut() {
            if let Some(r) = s.ping_role_id.take() {
                if !s.ping_role_ids.contains(&r) {
                    s.ping_role_ids.push(r);
                }
                changed = true;
            }
        }
        changed
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tracked {
    pub id: ObjectId,
    pub guild_id: i64,
    pub channel_id: i64,
    pub message_id: i64,
    pub kind: String,
    pub anchor: String,
    pub expires_at: Option<i64>,
    pub refresh: bool,
    pub refresh_v: Option<String>,
    pub created_at: DateTime,
}

impl Tracked {
    pub fn from_doc(d: &Document, now: DateTime) -> Result<Self> {
        let req_int = |k: &str| d.get(k).with_context(|| format!("{k}: missing")).and_then(|b| int(b, k));
        let req_str = |k: &str| match d.get(k) {
            Some(Bson::String(s)) => Ok(s.clone()),
            other => bail!("{k}: not a str: {other:?}"),
        };
        Ok(Tracked {
            id: d.get_object_id("_id").context("_id")?,
            guild_id: req_int("guild_id")?,
            channel_id: req_int("channel_id")?,
            message_id: req_int("message_id")?,
            kind: req_str("kind")?,
            anchor: req_str("anchor")?,
            expires_at: opt_int(d, "expires_at")?,
            refresh: boolean(d, "refresh", false)?,
            refresh_v: opt_str(d, "refresh_v")?,
            created_at: dt_field(d, "created_at", now)?,
        })
    }

    /// The document `insert()` writes (no `_id`: the store assigns it).
    pub fn fields(&self) -> Document {
        doc! {
            "guild_id": int_bson(self.guild_id),
            "channel_id": int_bson(self.channel_id),
            "message_id": int_bson(self.message_id),
            "kind": &self.kind,
            "anchor": &self.anchor,
            "expires_at": opt_int_bson(self.expires_at),
            "refresh": self.refresh,
            "refresh_v": opt_str_bson(&self.refresh_v),
            "created_at": self.created_at,
        }
    }

    pub fn to_doc(&self) -> Document {
        let mut d = doc! {"_id": self.id};
        d.extend(self.fields());
        d
    }
}
