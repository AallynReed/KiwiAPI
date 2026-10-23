//! The bot's Mongo reads and writes, shaped like Beanie's: whole-document
//! `$set` saves (upserting), inserts with the id first, natural-order finds.

use anyhow::{Context, Result};
use futures::TryStreamExt;
use mongodb::{
    Database,
    bson::{DateTime, Document, doc, oid::ObjectId},
};

use super::{TrackedQuery, presence::SITE_USERS};
use crate::{
    discord_common::models::{GUILD_CONFIGS, TRACKED},
    outbound::int_bson,
};

async fn find(db: &Database, coll: &str, filter: Document) -> Result<Vec<Document>> {
    Ok(db.collection::<Document>(coll).find(filter).await?.try_collect().await?)
}

pub async fn guild_configs(db: &Database) -> Result<Vec<Document>> {
    find(db, GUILD_CONFIGS, doc! {}).await
}

pub async fn guild_config(db: &Database, guild_id: i64) -> Result<Option<Document>> {
    Ok(db.collection::<Document>(GUILD_CONFIGS).find_one(doc! {"guild_id": int_bson(guild_id)}).await?)
}

pub async fn save(db: &Database, coll: &str, id: ObjectId, d: Document) -> Result<()> {
    db.collection::<Document>(coll).update_one(doc! {"_id": id}, doc! {"$set": d}).upsert(true).await?;
    Ok(())
}

pub async fn insert(db: &Database, coll: &str, id: ObjectId, fields: Document) -> Result<ObjectId> {
    let mut d = doc! {"_id": id};
    d.extend(fields);
    db.collection::<Document>(coll).insert_one(d).await?;
    Ok(id)
}

pub async fn delete(db: &Database, coll: &str, id: ObjectId) -> Result<()> {
    db.collection::<Document>(coll).delete_one(doc! {"_id": id}).await?;
    Ok(())
}

pub async fn tracked(db: &Database, q: TrackedQuery) -> Result<Vec<Document>> {
    let filter = match q {
        TrackedQuery::GuildKind(g, k) => doc! {"guild_id": int_bson(g), "kind": k},
        TrackedQuery::Expired(now) => doc! {"expires_at": {"$lte": int_bson(now)}},
        TrackedQuery::Refreshing => doc! {"refresh": true},
    };
    find(db, TRACKED, filter).await
}

pub async fn site_user_by_discord(db: &Database, discord_id: i64) -> Result<Option<Document>> {
    Ok(db.collection::<Document>(SITE_USERS).find_one(doc! {"discord_id": int_bson(discord_id)}).await?)
}

/// `stats.record_presence`: the `bot_presence` singleton.
pub async fn record_presence(db: &Database, guilds: i64, members: i64, now: DateTime) -> Result<()> {
    db.collection::<Document>("bot_presence")
        .update_one(
            doc! {"_id": "current"},
            doc! {"$set": {"guild_count": int_bson(guilds), "member_count": int_bson(members), "updated_at": now}},
        )
        .upsert(true)
        .await
        .context("bot_presence")?;
    Ok(())
}
