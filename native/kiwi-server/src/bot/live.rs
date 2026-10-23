//! The production [`Backend`]: the shared Mongo/Redis, the API over the
//! compose network, Discord REST.

use anyhow::Result;
use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use mongodb::{
    Collection,
    bson::{Bson, Document, doc, oid::ObjectId},
};
use serde_json::{Map, Value};

use super::{Backend, RestError, TrackedQuery, rest::Rest, store};
use crate::{
    discord_common::sources::{self, Sources},
    dt,
    feeds::store::News,
    giveaways::store::Mongo as GiveawayStore,
    og::data,
    routes::rotations::{bodies, gamedata::Gamedata},
    state::AppState,
};

pub struct Live {
    pub state: AppState,
    pub rest: Rest,
    http: reqwest::Client,
}

impl Live {
    pub fn new(state: AppState, token: &str) -> Self {
        let http = crate::http::client_builder().build().unwrap_or_else(|_| reqwest::Client::new());
        Live { state, rest: Rest::new(token), http }
    }

    fn coll(&self, name: &str) -> Collection<Document> {
        self.state.mongo().collection(name)
    }
}

fn now_us() -> i64 {
    dt::now().timestamp_micros()
}

impl Sources for Live {
    fn now_us(&self) -> i64 {
        now_us()
    }

    async fn challenge(&self) -> Result<Value> {
        data::challenge(&self.state, now_us()).await
    }

    async fn chaos(&self) -> Result<Value> {
        data::chaos_chest(&self.state, now_us()).await
    }

    async fn luxion(&self) -> Result<Value> {
        data::luxion(&self.state, now_us()).await
    }

    async fn status(&self) -> Value {
        data::status(&self.state).await
    }

    fn server_time(&self) -> Value {
        data::server_time()
    }

    fn biome_rotation(&self) -> Result<Value> {
        data::biome_rotation(now_us())
    }

    fn wild_mana(&self) -> Result<Value> {
        data::wild_mana(now_us())
    }

    fn stampy(&self) -> Result<Value> {
        data::stampy(now_us())
    }

    fn corruxion(&self) -> Value {
        bodies::corruxion(now_us())
    }

    fn fluxion(&self) -> Value {
        bodies::fluxion(now_us())
    }

    fn daily_buff(&self) -> Value {
        data::daily_buffs(now_us()).get("current").cloned().unwrap_or(Value::Object(Map::new()))
    }

    fn weekly_buff(&self) -> Value {
        let idx = kiwi_core::rotations::weekly_buff_index(now_us());
        Gamedata::get().weekly.get(&idx.to_string()).cloned().unwrap_or(Value::Object(Map::new()))
    }

    async fn news(&self, limit: usize) -> Result<Vec<News>> {
        let docs: Vec<Document> = self
            .coll("trove_news")
            .find(doc! {})
            .sort(doc! {"published_at": -1})
            .limit((limit * 3).max(limit) as i64)
            .await?
            .try_collect()
            .await?;
        let items = std::panic::catch_unwind(|| docs.iter().map(News::from_doc).collect::<Vec<_>>())
            .map_err(|_| anyhow::anyhow!("a trove_news document failed validation"))?;
        Ok(crate::feeds::routes::dedupe_news(items, limit))
    }

    async fn newest_open_giveaway(&self) -> Result<Option<DateTime<Utc>>> {
        let d = self.coll("giveaways").find_one(doc! {"status": "open"}).sort(doc! {"created_at": -1}).await?;
        Ok(match d.as_ref().and_then(|d| d.get("created_at")) {
            Some(Bson::DateTime(t)) => Some(dt::from_bson(*t)),
            Some(other) => anyhow::bail!("created_at: {other:?}"),
            None => None,
        })
    }

    async fn giveaways(&self) -> Result<[Vec<Value>; 3]> {
        let store = GiveawayStore { db: self.state.mongo(), email_enabled: false };
        let err = |e: crate::error::ApiError| anyhow::anyhow!("{}", e.body());
        Ok([
            crate::giveaways::service::list_ongoing(&store).await.map_err(err)?,
            crate::giveaways::service::list_upcoming(&store).await.map_err(err)?,
            crate::giveaways::service::list_ended(&store, 7).await.map_err(err)?,
        ])
    }

    async fn latest_update(&self) -> Result<Option<Value>> {
        let d = self
            .coll("update_versions")
            .find_one(doc! {"branch": "live-us", "status": "complete"})
            .sort(doc! {"ordinal": -1})
            .await?;
        Ok(d.map(|d| Bson::Document(d).into_relaxed_extjson()))
    }

    async fn internal_get(&self, path: &str) -> Option<Value> {
        sources::http_internal_get(&self.http, &self.state.settings().internal_api_url, path).await
    }
}

impl Backend for Live {
    async fn guild_configs(&self) -> Result<Vec<Document>> {
        store::guild_configs(self.state.mongo()).await
    }

    async fn guild_config(&self, guild_id: i64) -> Result<Option<Document>> {
        store::guild_config(self.state.mongo(), guild_id).await
    }

    async fn save(&self, coll: &'static str, id: ObjectId, d: Document) -> Result<()> {
        store::save(self.state.mongo(), coll, id, d).await
    }

    async fn insert(&self, coll: &'static str, fields: Document) -> Result<ObjectId> {
        store::insert(self.state.mongo(), coll, ObjectId::new(), fields).await
    }

    async fn delete(&self, coll: &'static str, id: ObjectId) -> Result<()> {
        store::delete(self.state.mongo(), coll, id).await
    }

    async fn tracked(&self, q: TrackedQuery) -> Result<Vec<Document>> {
        store::tracked(self.state.mongo(), q).await
    }

    async fn site_user_by_discord(&self, discord_id: i64) -> Result<Option<Document>> {
        store::site_user_by_discord(self.state.mongo(), discord_id).await
    }

    async fn record_presence(&self, guilds: i64, members: i64) -> Result<()> {
        store::record_presence(self.state.mongo(), guilds, members, dt::to_bson(dt::now())).await
    }

    async fn fetch_channel(&self, id: i64) -> Result<(), RestError> {
        self.rest.fetch_channel(id).await
    }

    async fn send_message(&self, channel: i64, payload: Value, png: Option<Vec<u8>>) -> Result<i64, RestError> {
        self.rest.send_message(channel, payload, png).await
    }

    async fn edit_message(&self, channel: i64, message: i64, payload: Value) -> Result<(), RestError> {
        self.rest.edit_message(channel, message, payload).await
    }

    async fn delete_message(&self, channel: i64, message: i64) -> Result<(), RestError> {
        self.rest.delete_message(channel, message).await
    }

    async fn render_design(&self, design_id: &str) -> Option<Vec<u8>> {
        match crate::images::service::render_public(&self.state, design_id, None).await {
            Ok(png) => png.map(|b| b.to_vec()),
            Err(e) => {
                tracing::warn!("announce image render failed ({design_id}): {}", e.body());
                None
            }
        }
    }
}
