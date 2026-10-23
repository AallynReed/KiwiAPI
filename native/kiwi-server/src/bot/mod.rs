//! The gateway bot (`app/bot/runner.py` and what it drives): event-driven and
//! minute-aligned announcements, the self-editing live board, the market
//! watch, tracked message cleanup/refresh, reconcile on channel/role deletes,
//! avatar sync and presence stats. `kiwi-bot` runs it; Python's
//! `app.bot.runner` must not run at the same time.
//!
//! Everything below the gateway goes through [`Backend`] (stores, service
//! reads, Discord REST), so the tests drive it with fakes.

pub mod announcements;
pub mod announcer;
pub mod cache;
pub mod gateway;
pub mod live;
pub mod liveboard;
pub mod market_watch;
pub mod message;
pub mod presence;
pub mod rest;
pub mod runner;
pub mod store;

#[cfg(test)]
mod rest_tests;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod tests;

use std::{future::Future, sync::Arc};

use anyhow::Result;
use mongodb::bson::{DateTime, Document, oid::ObjectId};
use serde_json::Value;

use crate::discord_common::{i18n::Catalogs, sources::Sources};

/// A Discord REST failure, as discord.py raises it.
#[derive(Clone, Debug, PartialEq)]
pub enum RestError {
    NotFound,
    Forbidden,
    /// Any other HTTP error status (`HTTPException`).
    Http(u16),
    /// A transport failure (`OSError`, timeouts), which no handler catches.
    Transport(String),
}

impl RestError {
    /// The HTTP error a handler branches on; a transport error propagates.
    pub fn http(self) -> Result<RestError> {
        match self {
            RestError::Transport(e) => Err(anyhow::anyhow!("discord transport: {e}")),
            other => Ok(other),
        }
    }
}

/// The tracked-announcement queries.
#[derive(Clone, Debug)]
pub enum TrackedQuery {
    GuildKind(i64, String),
    /// `expires_at <= now`.
    Expired(i64),
    Refreshing,
}

pub trait Backend: Sources + 'static {
    fn guild_configs(&self) -> impl Future<Output = Result<Vec<Document>>> + Send;
    fn guild_config(&self, guild_id: i64) -> impl Future<Output = Result<Option<Document>>> + Send;
    /// Beanie `save()`: `$set` of the whole document, upserting.
    fn save(&self, coll: &'static str, id: ObjectId, doc: Document) -> impl Future<Output = Result<()>> + Send;
    fn insert(&self, coll: &'static str, doc: Document) -> impl Future<Output = Result<ObjectId>> + Send;
    fn delete(&self, coll: &'static str, id: ObjectId) -> impl Future<Output = Result<()>> + Send;
    fn tracked(&self, q: TrackedQuery) -> impl Future<Output = Result<Vec<Document>>> + Send;
    fn site_user_by_discord(&self, discord_id: i64) -> impl Future<Output = Result<Option<Document>>> + Send;
    fn record_presence(&self, guilds: i64, members: i64) -> impl Future<Output = Result<()>> + Send;

    /// `GET /channels/{id}` (`fetch_channel`).
    fn fetch_channel(&self, id: i64) -> impl Future<Output = Result<(), RestError>> + Send;
    /// `POST /channels/{id}/messages`; the new message id.
    fn send_message(
        &self,
        channel: i64,
        payload: Value,
        png: Option<Vec<u8>>,
    ) -> impl Future<Output = Result<i64, RestError>> + Send;
    fn edit_message(
        &self,
        channel: i64,
        message: i64,
        payload: Value,
    ) -> impl Future<Output = Result<(), RestError>> + Send;
    fn delete_message(&self, channel: i64, message: i64) -> impl Future<Output = Result<(), RestError>> + Send;

    /// `images.render_public(design_id)`, `None` on any failure.
    fn render_design(&self, design_id: &str) -> impl Future<Output = Option<Vec<u8>>> + Send;
}

pub struct Bot<B> {
    pub b: B,
    pub cache: Arc<cache::Cache>,
    pub i18n: &'static Catalogs,
    /// `settings.asset_url`.
    pub asset: String,
    announce_lock: tokio::sync::Mutex<()>,
    board_lock: tokio::sync::Mutex<()>,
    market_lock: tokio::sync::Mutex<()>,
}

impl<B: Backend> Bot<B> {
    pub fn new(b: B, cache: Arc<cache::Cache>, i18n: &'static Catalogs, asset: String) -> Self {
        Bot {
            b,
            cache,
            i18n,
            asset,
            announce_lock: Default::default(),
            board_lock: Default::default(),
            market_lock: Default::default(),
        }
    }

    pub fn now_secs(&self) -> i64 {
        self.b.now_us().div_euclid(1_000_000)
    }

    /// `utcnow()` as it reads back from Mongo.
    pub fn now_bson(&self) -> DateTime {
        DateTime::from_millis(self.b.now_us().div_euclid(1000))
    }
}
