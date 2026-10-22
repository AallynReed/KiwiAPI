//! The durable email outbox (`app/core/email_outbox.py`): [`queue_email`]
//! writes `email_outbox` documents exactly as Python does, and [`worker`]
//! (RUST_OWNS `email_outbox`) claims and delivers them through [`mailer`].

pub mod mailer;
pub mod worker;

#[cfg(test)]
pub(crate) mod tests;

use chrono::{DateTime, Utc};
use kiwi_core::mail::{Brand, Notice};
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};

use crate::{auth::models::USERS, config::Settings, dt, state::AppState};

pub const OUTBOX: &str = "email_outbox";

/// `OutboxEmail` as Beanie writes it: every field, model order, explicit nulls.
#[derive(Clone, Debug, PartialEq)]
pub struct OutboxEmail {
    pub id: ObjectId,
    pub to: String,
    pub subject: String,
    pub text: String,
    pub html: Option<String>,
    pub status: String,
    pub attempts: i64,
    pub last_error: Option<String>,
    pub next_attempt_at: DateTime<Utc>,
    pub claimed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub sent_at: Option<DateTime<Utc>>,
}

fn opt_str(v: &Option<String>) -> Bson {
    v.as_ref().map_or(Bson::Null, |s| Bson::String(s.clone()))
}

fn opt_dt(v: &Option<DateTime<Utc>>) -> Bson {
    v.map_or(Bson::Null, |d| Bson::DateTime(dt::to_bson(d)))
}

fn get_dt(d: &Document, key: &str) -> Option<DateTime<Utc>> {
    match d.get(key) {
        Some(Bson::DateTime(b)) => Some(dt::from_bson(*b)),
        _ => None,
    }
}

fn get_str(d: &Document, key: &str) -> Option<String> {
    d.get_str(key).ok().map(str::to_owned)
}

impl OutboxEmail {
    pub fn new(to: &str, subject: &str, text: &str, html: Option<&str>) -> Self {
        let next_attempt_at = dt::now();
        OutboxEmail {
            id: ObjectId::new(),
            to: to.to_owned(),
            subject: subject.to_owned(),
            text: text.to_owned(),
            html: html.map(str::to_owned),
            status: "pending".into(),
            attempts: 0,
            last_error: None,
            next_attempt_at,
            claimed_at: None,
            created_at: dt::now(),
            sent_at: None,
        }
    }

    pub fn to_doc(&self) -> Document {
        doc! {
            "_id": self.id,
            "to": &self.to,
            "subject": &self.subject,
            "text": &self.text,
            "html": opt_str(&self.html),
            "status": &self.status,
            "attempts": crate::auth::pyutil::bson_int(self.attempts),
            "last_error": opt_str(&self.last_error),
            "next_attempt_at": dt::to_bson(self.next_attempt_at),
            "claimed_at": opt_dt(&self.claimed_at),
            "created_at": dt::to_bson(self.created_at),
            "sent_at": opt_dt(&self.sent_at),
        }
    }

    /// A stored document; `None` where Beanie's validation would raise.
    pub fn from_doc(d: &Document) -> Option<Self> {
        let attempts = match d.get("attempts") {
            Some(Bson::Int32(n)) => i64::from(*n),
            Some(Bson::Int64(n)) => *n,
            None => 0,
            _ => return None,
        };
        Some(OutboxEmail {
            id: d.get_object_id("_id").ok()?,
            to: get_str(d, "to")?,
            subject: get_str(d, "subject")?,
            text: get_str(d, "text")?,
            html: get_str(d, "html"),
            status: get_str(d, "status").unwrap_or_else(|| "pending".into()),
            attempts,
            last_error: get_str(d, "last_error"),
            next_attempt_at: get_dt(d, "next_attempt_at")?,
            claimed_at: get_dt(d, "claimed_at"),
            created_at: get_dt(d, "created_at")?,
            sent_at: get_dt(d, "sent_at"),
        })
    }
}

pub fn brand(settings: &Settings) -> Brand<'_> {
    Brand { app_name: &settings.app_name, dev_url: &settings.dev_url, docs_url: &settings.docs_url }
}

/// `queue_email`: a no-op without SMTP, skipped for an address that already
/// hard-bounced, else one pending outbox document.
pub async fn queue_email(
    state: &AppState,
    to: &str,
    subject: &str,
    text: &str,
    html: Option<&str>,
) -> anyhow::Result<()> {
    if !state.settings().email_enabled() {
        tracing::warn!("SMTP not configured - not queueing email to {to} ({subject})");
        return Ok(());
    }
    let db = state.mongo();
    let bounced =
        db.collection::<Document>(USERS).find_one(doc! {"email": to.to_lowercase(), "email_bounced": true}).await?;
    if bounced.is_some() {
        tracing::warn!("Skipping email to bounced address {to} ({subject})");
        return Ok(());
    }
    db.collection::<Document>(OUTBOX).insert_one(OutboxEmail::new(to, subject, text, html).to_doc()).await?;
    Ok(())
}

/// `_queue` of `app/auth/emails.py`: the text and branded HTML parts of a notice.
pub async fn queue_notice(state: &AppState, to: &str, notice: &Notice) -> anyhow::Result<()> {
    let html = notice.html(&brand(state.settings()));
    queue_email(state, to, &notice.subject, &notice.text(), Some(&html)).await
}
