//! `app/giveaways/models.py`: the four Beanie documents, read the way Beanie
//! validates them (model defaults for missing fields, anything else invalid)
//! and written field by field in model order, as `insert()` / `save()` do.

use chrono::{DateTime, Utc};
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};

use crate::{auth::pyutil::bson_int, dt};

pub const VAULT_ITEMS: &str = "vault_items";
pub const PRIZE_CODES: &str = "prize_codes";
pub const GIVEAWAYS: &str = "giveaways";
pub const ENTRIES: &str = "giveaway_entries";

/// A stored document the model rejects (a 500 in Python).
#[derive(Debug)]
pub struct Invalid(pub String);

impl std::fmt::Display for Invalid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "document failed validation: {}", self.0)
    }
}

impl std::error::Error for Invalid {}

type R<T> = Result<T, Invalid>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodeStatus {
    Available,
    Reserved,
    Awarded,
}

impl CodeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            CodeStatus::Available => "available",
            CodeStatus::Reserved => "reserved",
            CodeStatus::Awarded => "awarded",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "available" => CodeStatus::Available,
            "reserved" => CodeStatus::Reserved,
            "awarded" => CodeStatus::Awarded,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GiveawayStatus {
    Scheduled,
    Open,
    Drawn,
    Closed,
    Cancelled,
}

impl GiveawayStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            GiveawayStatus::Scheduled => "scheduled",
            GiveawayStatus::Open => "open",
            GiveawayStatus::Drawn => "drawn",
            GiveawayStatus::Closed => "closed",
            GiveawayStatus::Cancelled => "cancelled",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "scheduled" => GiveawayStatus::Scheduled,
            "open" => GiveawayStatus::Open,
            "drawn" => GiveawayStatus::Drawn,
            "closed" => GiveawayStatus::Closed,
            "cancelled" => GiveawayStatus::Cancelled,
            _ => return None,
        })
    }
}

fn bad<T>(key: &str, v: &Bson) -> R<T> {
    Err(Invalid(format!("{key}: unexpected {v:?}")))
}

fn missing<T>(key: &str) -> R<T> {
    Err(Invalid(format!("{key}: missing")))
}

fn id(d: &Document) -> R<ObjectId> {
    match d.get("_id") {
        Some(Bson::ObjectId(o)) => Ok(*o),
        Some(Bson::String(s)) => ObjectId::parse_str(s).map_err(|_| Invalid(format!("_id: {s:?}"))),
        Some(v) => bad("_id", v),
        None => missing("_id"),
    }
}

fn oid_of(key: &str, v: &Bson) -> R<ObjectId> {
    match v {
        Bson::ObjectId(o) => Ok(*o),
        Bson::String(s) => ObjectId::parse_str(s).map_err(|_| Invalid(format!("{key}: {s:?}"))),
        _ => bad(key, v),
    }
}

fn oid(d: &Document, key: &str) -> R<ObjectId> {
    match d.get(key) {
        Some(v) => oid_of(key, v),
        None => missing(key),
    }
}

fn opt_oid(d: &Document, key: &str) -> R<Option<ObjectId>> {
    match d.get(key) {
        None | Some(Bson::Null) => Ok(None),
        Some(v) => oid_of(key, v).map(Some),
    }
}

fn s(d: &Document, key: &str) -> R<String> {
    match d.get(key) {
        Some(Bson::String(v)) => Ok(v.clone()),
        Some(v) => bad(key, v),
        None => missing(key),
    }
}

fn opt_s(d: &Document, key: &str) -> R<Option<String>> {
    match d.get(key) {
        None | Some(Bson::Null) => Ok(None),
        Some(Bson::String(v)) => Ok(Some(v.clone())),
        Some(v) => bad(key, v),
    }
}

fn dt_of(key: &str, v: &Bson) -> R<DateTime<Utc>> {
    match v {
        Bson::DateTime(b) => Ok(dt::from_bson(*b)),
        _ => bad(key, v),
    }
}

fn datetime(d: &Document, key: &str) -> R<DateTime<Utc>> {
    match d.get(key) {
        Some(v) => dt_of(key, v),
        None => missing(key),
    }
}

/// `datetime = Field(default_factory=utcnow)`.
fn datetime_or(d: &Document, key: &str, now: DateTime<Utc>) -> R<DateTime<Utc>> {
    match d.get(key) {
        Some(v) => dt_of(key, v),
        None => Ok(now),
    }
}

fn opt_datetime(d: &Document, key: &str) -> R<Option<DateTime<Utc>>> {
    match d.get(key) {
        None | Some(Bson::Null) => Ok(None),
        Some(v) => dt_of(key, v).map(Some),
    }
}

fn int(d: &Document, key: &str, default: i64) -> R<i64> {
    match d.get(key) {
        None => Ok(default),
        Some(Bson::Int32(i)) => Ok(i64::from(*i)),
        Some(Bson::Int64(i)) => Ok(*i),
        Some(Bson::Boolean(b)) => Ok(i64::from(*b)),
        Some(Bson::Double(f)) if f.is_finite() && f.fract() == 0.0 && f.abs() < 9.2e18 => Ok(*f as i64),
        Some(v) => bad(key, v),
    }
}

fn opt_bson<T>(v: Option<T>, f: impl FnOnce(T) -> Bson) -> Bson {
    v.map_or(Bson::Null, f)
}

fn bdt(t: DateTime<Utc>) -> Bson {
    Bson::DateTime(dt::to_bson(t))
}

#[derive(Clone, Debug, PartialEq)]
pub struct VaultItem {
    pub id: ObjectId,
    pub name: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl VaultItem {
    pub fn from_doc(d: &Document, now: DateTime<Utc>) -> R<Self> {
        Ok(VaultItem {
            id: id(d)?,
            name: s(d, "name")?,
            description: opt_s(d, "description")?,
            created_at: datetime_or(d, "created_at", now)?,
            updated_at: datetime_or(d, "updated_at", now)?,
        })
    }

    pub fn fields(&self) -> Document {
        doc! {
            "name": &self.name,
            "description": opt_bson(self.description.as_deref(), Bson::from),
            "created_at": bdt(self.created_at),
            "updated_at": bdt(self.updated_at),
        }
    }

    /// What `save()` reads back.
    pub fn round_trip(&mut self) {
        self.created_at = dt::mongo_round_trip(self.created_at);
        self.updated_at = dt::mongo_round_trip(self.updated_at);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PrizeCode {
    pub id: ObjectId,
    pub vault_item_id: ObjectId,
    pub code: String,
    pub status: CodeStatus,
    pub giveaway_id: Option<ObjectId>,
    pub awarded_to: Option<ObjectId>,
    pub awarded_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl PrizeCode {
    pub fn from_doc(d: &Document, now: DateTime<Utc>) -> R<Self> {
        let status = match d.get("status") {
            None => CodeStatus::Available,
            Some(Bson::String(v)) => {
                CodeStatus::parse(v).map_or_else(|| bad("status", &Bson::String(v.clone())), Ok)?
            }
            Some(v) => return bad("status", v),
        };
        Ok(PrizeCode {
            id: id(d)?,
            vault_item_id: oid(d, "vault_item_id")?,
            code: s(d, "code")?,
            status,
            giveaway_id: opt_oid(d, "giveaway_id")?,
            awarded_to: opt_oid(d, "awarded_to")?,
            awarded_at: opt_datetime(d, "awarded_at")?,
            created_at: datetime_or(d, "created_at", now)?,
        })
    }

    pub fn fields(&self) -> Document {
        doc! {
            "vault_item_id": self.vault_item_id,
            "code": &self.code,
            "status": self.status.as_str(),
            "giveaway_id": opt_bson(self.giveaway_id, Bson::ObjectId),
            "awarded_to": opt_bson(self.awarded_to, Bson::ObjectId),
            "awarded_at": opt_bson(self.awarded_at, bdt),
            "created_at": bdt(self.created_at),
        }
    }

    pub fn round_trip(&mut self) {
        self.awarded_at = self.awarded_at.map(dt::mongo_round_trip);
        self.created_at = dt::mongo_round_trip(self.created_at);
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Giveaway {
    pub id: ObjectId,
    pub title: String,
    pub description: Option<String>,
    pub prize_name: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub status: GiveawayStatus,
    pub vault_item_id: Option<ObjectId>,
    pub prize_code_id: Option<ObjectId>,
    pub winner_user_id: Option<ObjectId>,
    pub winner_username: Option<String>,
    pub drawn_at: Option<DateTime<Utc>>,
    pub entry_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Giveaway {
    pub fn from_doc(d: &Document, now: DateTime<Utc>) -> R<Self> {
        let status = match d.get("status") {
            None => GiveawayStatus::Scheduled,
            Some(Bson::String(v)) => {
                GiveawayStatus::parse(v).map_or_else(|| bad("status", &Bson::String(v.clone())), Ok)?
            }
            Some(v) => return bad("status", v),
        };
        Ok(Giveaway {
            id: id(d)?,
            title: s(d, "title")?,
            description: opt_s(d, "description")?,
            prize_name: s(d, "prize_name")?,
            starts_at: datetime(d, "starts_at")?,
            ends_at: datetime(d, "ends_at")?,
            status,
            vault_item_id: opt_oid(d, "vault_item_id")?,
            prize_code_id: opt_oid(d, "prize_code_id")?,
            winner_user_id: opt_oid(d, "winner_user_id")?,
            winner_username: opt_s(d, "winner_username")?,
            drawn_at: opt_datetime(d, "drawn_at")?,
            entry_count: int(d, "entry_count", 0)?,
            created_at: datetime_or(d, "created_at", now)?,
            updated_at: datetime_or(d, "updated_at", now)?,
        })
    }

    pub fn fields(&self) -> Document {
        doc! {
            "title": &self.title,
            "description": opt_bson(self.description.as_deref(), Bson::from),
            "prize_name": &self.prize_name,
            "starts_at": bdt(self.starts_at),
            "ends_at": bdt(self.ends_at),
            "status": self.status.as_str(),
            "vault_item_id": opt_bson(self.vault_item_id, Bson::ObjectId),
            "prize_code_id": opt_bson(self.prize_code_id, Bson::ObjectId),
            "winner_user_id": opt_bson(self.winner_user_id, Bson::ObjectId),
            "winner_username": opt_bson(self.winner_username.as_deref(), Bson::from),
            "drawn_at": opt_bson(self.drawn_at, bdt),
            "entry_count": bson_int(self.entry_count),
            "created_at": bdt(self.created_at),
            "updated_at": bdt(self.updated_at),
        }
    }

    pub fn round_trip(&mut self) {
        self.starts_at = dt::mongo_round_trip(self.starts_at);
        self.ends_at = dt::mongo_round_trip(self.ends_at);
        self.drawn_at = self.drawn_at.map(dt::mongo_round_trip);
        self.created_at = dt::mongo_round_trip(self.created_at);
        self.updated_at = dt::mongo_round_trip(self.updated_at);
    }

    pub fn finished(&self) -> bool {
        matches!(self.status, GiveawayStatus::Drawn | GiveawayStatus::Closed | GiveawayStatus::Cancelled)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: ObjectId,
    pub giveaway_id: ObjectId,
    pub site_user_id: ObjectId,
    pub username: String,
    pub entered_at: DateTime<Utc>,
}

impl Entry {
    pub fn from_doc(d: &Document, now: DateTime<Utc>) -> R<Self> {
        Ok(Entry {
            id: id(d)?,
            giveaway_id: oid(d, "giveaway_id")?,
            site_user_id: oid(d, "site_user_id")?,
            username: s(d, "username")?,
            entered_at: datetime_or(d, "entered_at", now)?,
        })
    }
}

/// A new entry as `GiveawayEntry(...).insert()` writes it.
pub fn entry_fields(giveaway_id: ObjectId, site_user_id: ObjectId, username: &str, now: DateTime<Utc>) -> Document {
    doc! {
        "giveaway_id": giveaway_id,
        "site_user_id": site_user_id,
        "username": username,
        "entered_at": bdt(now),
    }
}

/// `OutboxEmail(to=, subject=, text=, html=).insert()` (`app/core/email_outbox.py`).
pub fn outbox_fields(
    to: &str,
    subject: &str,
    text: &str,
    html: &str,
    next_attempt_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
) -> Document {
    doc! {
        "to": to,
        "subject": subject,
        "text": text,
        "html": html,
        "status": "pending",
        "attempts": 0_i32,
        "last_error": Bson::Null,
        "next_attempt_at": bdt(next_attempt_at),
        "claimed_at": Bson::Null,
        "created_at": bdt(created_at),
        "sent_at": Bson::Null,
    }
}
