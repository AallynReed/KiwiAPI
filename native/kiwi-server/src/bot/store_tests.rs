//! The golden scenarios against a real Mongo, opt-in: each creates and drops
//! its own `kiwi_bot_test_*` database on MONGO_URI. Run
//! `cargo test -p kiwi-server bot_mongo_replay -- --ignored`.

use std::sync::atomic::Ordering;

use anyhow::Result;
use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use mongodb::{
    Database,
    bson::{Document, doc, oid::ObjectId},
};
use serde_json::Value;

use super::{
    Backend, RestError, TrackedQuery, store,
    tests::{Fake, check_db, ext_doc, golden, make_bot, replay},
};
use crate::{discord_common::sources::Sources, feeds::store::News};

/// [`store`] on a real database; sources, Discord and ids from the fake.
struct MongoReplay {
    db: Database,
    fake: Fake,
}

impl Sources for MongoReplay {
    fn now_us(&self) -> i64 {
        self.fake.now_us()
    }
    async fn challenge(&self) -> Result<Value> {
        self.fake.challenge().await
    }
    async fn chaos(&self) -> Result<Value> {
        self.fake.chaos().await
    }
    async fn luxion(&self) -> Result<Value> {
        self.fake.luxion().await
    }
    async fn status(&self) -> Value {
        self.fake.status().await
    }
    fn server_time(&self) -> Value {
        self.fake.server_time()
    }
    fn biome_rotation(&self) -> Result<Value> {
        self.fake.biome_rotation()
    }
    fn wild_mana(&self) -> Result<Value> {
        self.fake.wild_mana()
    }
    fn stampy(&self) -> Result<Value> {
        self.fake.stampy()
    }
    fn corruxion(&self) -> Value {
        self.fake.corruxion()
    }
    fn fluxion(&self) -> Value {
        self.fake.fluxion()
    }
    fn daily_buff(&self) -> Value {
        self.fake.daily_buff()
    }
    fn weekly_buff(&self) -> Value {
        self.fake.weekly_buff()
    }
    async fn news(&self, limit: usize) -> Result<Vec<News>> {
        self.fake.news(limit).await
    }
    async fn newest_open_giveaway(&self) -> Result<Option<DateTime<Utc>>> {
        self.fake.newest_open_giveaway().await
    }
    async fn giveaways(&self) -> Result<[Vec<Value>; 3]> {
        self.fake.giveaways().await
    }
    async fn latest_update(&self) -> Result<Option<Value>> {
        self.fake.latest_update().await
    }
    async fn internal_get(&self, path: &str) -> Option<Value> {
        self.fake.internal_get(path).await
    }
}

impl Backend for MongoReplay {
    async fn guild_configs(&self) -> Result<Vec<Document>> {
        store::guild_configs(&self.db).await
    }
    async fn guild_config(&self, guild_id: i64) -> Result<Option<Document>> {
        store::guild_config(&self.db, guild_id).await
    }
    async fn save(&self, coll: &'static str, id: ObjectId, d: Document) -> Result<()> {
        store::save(&self.db, coll, id, d).await
    }
    async fn insert(&self, coll: &'static str, fields: Document) -> Result<ObjectId> {
        let n = self.fake.next_id.fetch_add(1, Ordering::SeqCst) + 1;
        let id = ObjectId::parse_str(format!("66000000000000000000{n:04x}"))?;
        store::insert(&self.db, coll, id, fields).await
    }
    async fn delete(&self, coll: &'static str, id: ObjectId) -> Result<()> {
        store::delete(&self.db, coll, id).await
    }
    async fn tracked(&self, q: TrackedQuery) -> Result<Vec<Document>> {
        store::tracked(&self.db, q).await
    }
    async fn site_user_by_discord(&self, discord_id: i64) -> Result<Option<Document>> {
        store::site_user_by_discord(&self.db, discord_id).await
    }
    async fn record_presence(&self, guilds: i64, members: i64) -> Result<()> {
        store::record_presence(&self.db, guilds, members, self.fake.now_bson()).await
    }
    async fn fetch_channel(&self, id: i64) -> Result<(), RestError> {
        self.fake.fetch_channel(id).await
    }
    async fn send_message(&self, channel: i64, payload: Value, png: Option<Vec<u8>>) -> Result<i64, RestError> {
        self.fake.send_message(channel, payload, png).await
    }
    async fn edit_message(&self, channel: i64, message: i64, payload: Value) -> Result<(), RestError> {
        self.fake.edit_message(channel, message, payload).await
    }
    async fn delete_message(&self, channel: i64, message: i64) -> Result<(), RestError> {
        self.fake.delete_message(channel, message).await
    }
    async fn render_design(&self, design_id: &str) -> Option<Vec<u8>> {
        self.fake.render_design(design_id).await
    }
}

#[tokio::test]
#[ignore]
async fn bot_mongo_replay() {
    let uri = std::env::var("MONGO_URI").expect("MONGO_URI");
    let client = mongodb::Client::with_uri_str(&uri).await.unwrap();
    for (i, s) in golden()["scenarios"].as_array().unwrap().iter().enumerate() {
        let db = client.database(&format!("kiwi_bot_test_{}_{i}", std::process::id()));
        db.drop().await.unwrap();
        for (coll, docs) in s["seed"].as_object().unwrap() {
            let docs: Vec<Document> = docs.as_array().unwrap().iter().map(ext_doc).collect();
            if !docs.is_empty() {
                db.collection::<Document>(coll).insert_many(docs).await.unwrap();
            }
        }
        let bot = make_bot(MongoReplay { db: db.clone(), fake: Fake::scenario(s) }, s);
        replay(&bot, &bot.b.fake, s).await;
        for coll in s["db"].as_object().unwrap().keys() {
            let docs: Vec<Document> =
                db.collection::<Document>(coll).find(doc! {}).await.unwrap().try_collect().await.unwrap();
            check_db(s, coll, docs);
        }
        db.drop().await.unwrap();
    }
}
