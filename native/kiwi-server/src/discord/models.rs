//! `app/bot/models.py` (`GuildConfig`, `Club` and the club ranks) and
//! `app/bot/reconcile.py`, loaded and written the way Beanie does: every model
//! field in declaration order, explicit nulls, ints as int32 when they fit.

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use indexmap::IndexMap;
use mongodb::bson::{Bson, Document, oid::ObjectId};

use crate::{
    dt,
    embed_templates::EmbedTemplate,
    outbound::{get_dt, get_int, get_str, int_bson, opt_str_bson},
};

pub const GUILD_CONFIGS: &str = "guild_configs";
pub const CLUBS: &str = "clubs";

fn opt_int(v: Option<i64>) -> Bson {
    v.map_or(Bson::Null, int_bson)
}

fn ints(v: &[i64]) -> Bson {
    Bson::Array(v.iter().map(|i| int_bson(*i)).collect())
}

fn get_bool(d: &Document, key: &str, default: bool) -> anyhow::Result<bool> {
    Ok(match d.get(key) {
        None => default,
        Some(Bson::Boolean(b)) => *b,
        Some(Bson::Int32(i)) if *i == 0 || *i == 1 => *i == 1,
        Some(Bson::Int64(i)) if *i == 0 || *i == 1 => *i == 1,
        Some(other) => anyhow::bail!("{key}: not a bool: {other:?}"),
    })
}

fn get_ints(d: &Document, key: &str) -> anyhow::Result<Vec<i64>> {
    match d.get(key) {
        None => Ok(Vec::new()),
        Some(Bson::Array(a)) => a
            .iter()
            .map(|v| match v {
                Bson::Int32(i) => Ok(i64::from(*i)),
                Bson::Int64(i) => Ok(*i),
                Bson::Double(f) if f.fract() == 0.0 && f.is_finite() => Ok(*f as i64),
                other => anyhow::bail!("{key}: not an int: {other:?}"),
            })
            .collect(),
        Some(other) => anyhow::bail!("{key}: not a list: {other:?}"),
    }
}

fn get_doc<'a>(d: &'a Document, key: &str) -> anyhow::Result<Option<&'a Document>> {
    match d.get(key) {
        None => Ok(None),
        Some(Bson::Document(x)) => Ok(Some(x)),
        Some(other) => anyhow::bail!("{key}: not a dict: {other:?}"),
    }
}

fn req_int(d: &Document, key: &str) -> anyhow::Result<i64> {
    get_int(d, key)?.ok_or_else(|| anyhow::anyhow!("{key}: missing"))
}

fn get_float(d: &Document, key: &str) -> anyhow::Result<Option<f64>> {
    Ok(match d.get(key) {
        None | Some(Bson::Null) => None,
        Some(Bson::Double(f)) => Some(*f),
        Some(Bson::Int32(i)) => Some(f64::from(*i)),
        Some(Bson::Int64(i)) => Some(*i as f64),
        Some(other) => anyhow::bail!("{key}: not a float: {other:?}"),
    })
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
    pub fn from_doc(d: &Document) -> anyhow::Result<Self> {
        let template = match d.get("template") {
            None | Some(Bson::Null) => None,
            Some(Bson::Document(t)) => Some(EmbedTemplate::from_bson(t)),
            Some(other) => anyhow::bail!("template: not a dict: {other:?}"),
        };
        Ok(AnnouncementSetting {
            enabled: get_bool(d, "enabled", false)?,
            channel_id: get_int(d, "channel_id")?,
            ping_role_ids: get_ints(d, "ping_role_ids")?,
            ping_role_id: get_int(d, "ping_role_id")?,
            template,
            last_anchor: get_str(d, "last_anchor")?,
            channel_missing: get_bool(d, "channel_missing", false)?,
        })
    }

    pub fn to_doc(&self) -> Document {
        let mut d = Document::new();
        d.insert("enabled", self.enabled);
        d.insert("channel_id", opt_int(self.channel_id));
        d.insert("ping_role_ids", ints(&self.ping_role_ids));
        d.insert("ping_role_id", opt_int(self.ping_role_id));
        d.insert("template", self.template.as_ref().map_or(Bson::Null, |t| Bson::Document(t.to_bson())));
        d.insert("last_anchor", opt_str_bson(&self.last_anchor));
        d.insert("channel_missing", self.channel_missing);
        d
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
    fn from_doc(d: &Document) -> anyhow::Result<Self> {
        Ok(LiveBoard {
            enabled: get_bool(d, "enabled", false)?,
            channel_id: get_int(d, "channel_id")?,
            message_id: get_int(d, "message_id")?,
            channel_missing: get_bool(d, "channel_missing", false)?,
        })
    }

    fn to_doc(&self) -> Document {
        let mut d = Document::new();
        d.insert("enabled", self.enabled);
        d.insert("channel_id", opt_int(self.channel_id));
        d.insert("message_id", opt_int(self.message_id));
        d.insert("channel_missing", self.channel_missing);
        d
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
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

impl MarketWatch {
    fn from_doc(d: &Document) -> anyhow::Result<Self> {
        let items = match d.get("items") {
            None => Vec::new(),
            Some(Bson::Array(a)) => a
                .iter()
                .map(|it| {
                    let it = it.as_document().ok_or_else(|| anyhow::anyhow!("items: not a dict"))?;
                    Ok(MarketWatchItem {
                        name: get_str(it, "name")?.ok_or_else(|| anyhow::anyhow!("items.name: missing"))?,
                        max_price_each: get_float(it, "max_price_each")?,
                        last_alert_sig: get_str(it, "last_alert_sig")?,
                    })
                })
                .collect::<anyhow::Result<_>>()?,
            Some(other) => anyhow::bail!("items: not a list: {other:?}"),
        };
        Ok(MarketWatch {
            enabled: get_bool(d, "enabled", false)?,
            channel_id: get_int(d, "channel_id")?,
            ping_role_ids: get_ints(d, "ping_role_ids")?,
            channel_missing: get_bool(d, "channel_missing", false)?,
            items,
        })
    }

    fn to_doc(&self) -> Document {
        let items: Vec<Bson> = self
            .items
            .iter()
            .map(|it| {
                let mut d = Document::new();
                d.insert("name", &it.name);
                d.insert("max_price_each", it.max_price_each.map_or(Bson::Null, Bson::Double));
                d.insert("last_alert_sig", opt_str_bson(&it.last_alert_sig));
                Bson::Document(d)
            })
            .collect();
        let mut d = Document::new();
        d.insert("enabled", self.enabled);
        d.insert("channel_id", opt_int(self.channel_id));
        d.insert("ping_role_ids", ints(&self.ping_role_ids));
        d.insert("channel_missing", self.channel_missing);
        d.insert("items", items);
        d
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct GuildConfig {
    pub id: Option<ObjectId>,
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
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl GuildConfig {
    /// `GuildConfig(guild_id=...)`.
    pub fn new(guild_id: i64, now: DateTime<Utc>) -> Self {
        GuildConfig {
            id: None,
            guild_id,
            announcements: IndexMap::new(),
            live_board: LiveBoard::default(),
            market_watch: MarketWatch::default(),
            language: "en".into(),
            config_perms: IndexMap::new(),
            hourly_challenge_enabled: false,
            announce_channel_id: None,
            last_announced_challenge_anchor: None,
            updated_by: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Beanie's load; `Err` where validation raises.
    pub fn from_doc(d: &Document, now: DateTime<Utc>) -> anyhow::Result<Self> {
        let mut announcements = IndexMap::new();
        if let Some(a) = get_doc(d, "announcements")? {
            for (k, v) in a {
                let v = v.as_document().ok_or_else(|| anyhow::anyhow!("announcements.{k}: not a dict"))?;
                announcements.insert(k.clone(), AnnouncementSetting::from_doc(v)?);
            }
        }
        let mut config_perms = IndexMap::new();
        if let Some(p) = get_doc(d, "config_perms")? {
            for k in p.keys() {
                config_perms.insert(k.clone(), get_ints(p, k)?);
            }
        }
        let language = match d.get("language") {
            None => "en".to_owned(),
            Some(Bson::String(s)) => s.clone(),
            Some(other) => anyhow::bail!("language: not a str: {other:?}"),
        };
        Ok(GuildConfig {
            id: d.get_object_id("_id").ok(),
            guild_id: req_int(d, "guild_id")?,
            announcements,
            live_board: get_doc(d, "live_board")?.map(LiveBoard::from_doc).transpose()?.unwrap_or_default(),
            market_watch: get_doc(d, "market_watch")?.map(MarketWatch::from_doc).transpose()?.unwrap_or_default(),
            language,
            config_perms,
            hourly_challenge_enabled: get_bool(d, "hourly_challenge_enabled", false)?,
            announce_channel_id: get_int(d, "announce_channel_id")?,
            last_announced_challenge_anchor: get_int(d, "last_announced_challenge_anchor")?,
            updated_by: get_int(d, "updated_by")?,
            created_at: get_dt(d, "created_at")?.unwrap_or(now),
            updated_at: get_dt(d, "updated_at")?.unwrap_or(now),
        })
    }

    /// Beanie's `get_dict(to_db=True)`: `_id` (when set) then every field.
    pub fn to_doc(&self) -> Document {
        let mut d = Document::new();
        if let Some(id) = self.id {
            d.insert("_id", id);
        }
        d.insert("guild_id", int_bson(self.guild_id));
        let mut a = Document::new();
        for (k, v) in &self.announcements {
            a.insert(k.clone(), v.to_doc());
        }
        d.insert("announcements", a);
        d.insert("live_board", self.live_board.to_doc());
        d.insert("market_watch", self.market_watch.to_doc());
        d.insert("language", &self.language);
        let mut p = Document::new();
        for (k, v) in &self.config_perms {
            p.insert(k.clone(), ints(v));
        }
        d.insert("config_perms", p);
        d.insert("hourly_challenge_enabled", self.hourly_challenge_enabled);
        d.insert("announce_channel_id", opt_int(self.announce_channel_id));
        d.insert("last_announced_challenge_anchor", opt_int(self.last_announced_challenge_anchor));
        d.insert("updated_by", opt_int(self.updated_by));
        d.insert("created_at", Bson::DateTime(dt::to_bson(self.created_at)));
        d.insert("updated_at", Bson::DateTime(dt::to_bson(self.updated_at)));
        d
    }

    /// `migrate_legacy`: fold the pre-registry fields in; true when changed.
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

/// `reconcile`: flag deleted channels, drop deleted roles; true when changed.
pub fn reconcile(cfg: &mut GuildConfig, channels: &HashSet<i64>, roles: &HashSet<i64>) -> bool {
    let mut changed = false;
    for s in cfg.announcements.values_mut() {
        match s.channel_id {
            Some(c) if !channels.contains(&c) => {
                if s.enabled || !s.channel_missing {
                    s.enabled = false;
                    s.channel_missing = true;
                    changed = true;
                }
            }
            _ => {
                if s.channel_missing && s.channel_id.is_some_and(|c| channels.contains(&c)) {
                    s.channel_missing = false;
                    changed = true;
                }
            }
        }
        if !s.ping_role_ids.is_empty() {
            let kept: Vec<i64> = s.ping_role_ids.iter().copied().filter(|r| roles.contains(r)).collect();
            if kept.len() != s.ping_role_ids.len() {
                s.ping_role_ids = kept;
                changed = true;
            }
        }
    }
    let board = &mut cfg.live_board;
    match board.channel_id {
        Some(c) if !channels.contains(&c) => {
            if board.enabled || !board.channel_missing {
                board.enabled = false;
                board.channel_missing = true;
                board.message_id = None;
                changed = true;
            }
        }
        _ => {
            if board.channel_missing && board.channel_id.is_some_and(|c| channels.contains(&c)) {
                board.channel_missing = false;
                changed = true;
            }
        }
    }
    let caps: Vec<String> = cfg.config_perms.keys().cloned().collect();
    for cap in caps {
        let ids = &cfg.config_perms[&cap];
        let kept: Vec<i64> = ids.iter().copied().filter(|r| roles.contains(r)).collect();
        if kept.len() != ids.len() {
            if kept.is_empty() {
                cfg.config_perms.shift_remove(&cap);
            } else {
                cfg.config_perms.insert(cap, kept);
            }
            changed = true;
        }
    }
    changed
}

// --- clubs -----------------------------------------------------------------------------

pub const CLUB_RANKS: [&str; 5] = ["president", "vice_president", "officer", "captain", "member"];
pub const MAX_CLUBS_PER_GUILD: u64 = 3;

pub fn rank_label(rank: &str) -> &'static str {
    match rank {
        "president" => "President",
        "vice_president" => "Vice-President",
        "officer" => "Officer",
        "captain" => "Captain",
        _ => "Member",
    }
}

/// `promote_rank`: the nearest linked rank above, never president.
pub fn promote_rank(current: &str, linked: &HashSet<String>) -> Option<&'static str> {
    let cur = CLUB_RANKS.iter().position(|r| *r == current)?;
    (1..cur).rev().map(|i| CLUB_RANKS[i]).find(|r| linked.contains(*r))
}

/// `demote_rank`: the nearest linked rank below.
pub fn demote_rank(current: &str, linked: &HashSet<String>) -> Option<&'static str> {
    let cur = CLUB_RANKS.iter().position(|r| *r == current)?;
    (cur + 1..CLUB_RANKS.len()).map(|i| CLUB_RANKS[i]).find(|r| linked.contains(*r))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Club {
    pub id: Option<ObjectId>,
    pub guild_id: i64,
    pub name: String,
    pub public: bool,
    pub description: Option<String>,
    pub banner_url: Option<String>,
    pub avatar_url: Option<String>,
    pub discord_url: Option<String>,
    pub website_url: Option<String>,
    pub role_links: IndexMap<String, i64>,
    pub updated_by: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Club {
    pub fn new(guild_id: i64, name: &str, updated_by: Option<i64>, now: DateTime<Utc>) -> Club {
        Club {
            id: None,
            guild_id,
            name: name.into(),
            public: false,
            description: None,
            banner_url: None,
            avatar_url: None,
            discord_url: None,
            website_url: None,
            role_links: IndexMap::new(),
            updated_by,
            created_at: now,
            updated_at: now,
        }
    }

    pub fn from_doc(d: &Document, now: DateTime<Utc>) -> anyhow::Result<Club> {
        let mut role_links = IndexMap::new();
        if let Some(r) = get_doc(d, "role_links")? {
            for k in r.keys() {
                role_links.insert(k.clone(), req_int(r, k)?);
            }
        }
        Ok(Club {
            id: d.get_object_id("_id").ok(),
            guild_id: req_int(d, "guild_id")?,
            name: get_str(d, "name")?.ok_or_else(|| anyhow::anyhow!("name: missing"))?,
            public: get_bool(d, "public", false)?,
            description: get_str(d, "description")?,
            banner_url: get_str(d, "banner_url")?,
            avatar_url: get_str(d, "avatar_url")?,
            discord_url: get_str(d, "discord_url")?,
            website_url: get_str(d, "website_url")?,
            role_links,
            updated_by: get_int(d, "updated_by")?,
            created_at: get_dt(d, "created_at")?.unwrap_or(now),
            updated_at: get_dt(d, "updated_at")?.unwrap_or(now),
        })
    }

    pub fn to_doc(&self) -> Document {
        let mut d = Document::new();
        if let Some(id) = self.id {
            d.insert("_id", id);
        }
        d.insert("guild_id", int_bson(self.guild_id));
        d.insert("name", &self.name);
        d.insert("public", self.public);
        d.insert("description", opt_str_bson(&self.description));
        d.insert("banner_url", opt_str_bson(&self.banner_url));
        d.insert("avatar_url", opt_str_bson(&self.avatar_url));
        d.insert("discord_url", opt_str_bson(&self.discord_url));
        d.insert("website_url", opt_str_bson(&self.website_url));
        let mut r = Document::new();
        for (k, v) in &self.role_links {
            r.insert(k.clone(), int_bson(*v));
        }
        d.insert("role_links", r);
        d.insert("updated_by", opt_int(self.updated_by));
        d.insert("created_at", Bson::DateTime(dt::to_bson(self.created_at)));
        d.insert("updated_at", Bson::DateTime(dt::to_bson(self.updated_at)));
        d
    }
}

/// `reconcile_club`: drop rank links to deleted roles.
pub fn reconcile_club(club: &mut Club, roles: &HashSet<i64>) -> bool {
    let before = club.role_links.len();
    club.role_links.retain(|_, r| roles.contains(r));
    club.role_links.len() != before
}
