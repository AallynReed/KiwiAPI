//! Everything `app/main.py`'s lifespan does before serving, so a fresh
//! environment needs no Python: `init_db`, Redis PING, the Postgres schema,
//! the bootstrap admin, the first-boot seeds and the owner-handle and
//! takedown backfills. Every step is idempotent (Python may run it too); a
//! failing step is logged and the next one still runs.

mod indexes;
pub mod mongo;
pub mod pg;
#[cfg(test)]
mod tests;

use std::{collections::HashSet, path::Path};

use futures::TryStreamExt;
use mongodb::bson::{Document, doc, oid::ObjectId};

use crate::{
    auth::{
        models::{SITE_USERS, User},
        passwords,
    },
    dt, gamedata,
    market::store::{INTEREST, InterestItem},
    portal::store as users,
    routes::leaf::supporters::{self, Supporter},
    state::AppState,
};

pub const DEFAULT_SECRET: &str = "CHANGE_ME_insecure_dev_secret_do_not_use_in_production";
pub const DEFAULT_SUPPORTERS: [&str; 7] = ["IINikstarII", "Nao373", "Wahoo", "boryzje", "nz", "Grainus", "Tues"];

pub async fn run(state: &AppState) {
    let s = state.settings();
    if s.secret_key == DEFAULT_SECRET {
        tracing::warn!(
            "SECRET_KEY is the insecure default - set a strong SECRET_KEY in production (sessions are forgeable otherwise)."
        );
    }
    if s.captcha_secret.as_deref().unwrap_or_default().is_empty() {
        tracing::warn!("CAPTCHA_SECRET is unset - signup captcha is DISABLED (dev mode).");
    }
    let db = &state.inner.db;
    let reachable = match &db.mongo {
        Some(m) => match m.run_command(doc! {"ping": 1}).await {
            Ok(_) => Some(m),
            Err(e) => {
                tracing::error!("mongo unreachable, startup skips it: {e}");
                None
            }
        },
        None => None,
    };
    if let Some(m) = reachable {
        mongo::init_db(&mongo::Live(m), s).await;
    }
    if let Some(mut redis) = db.redis.clone()
        && let Err(e) = redis::cmd("PING").query_async::<String>(&mut redis).await
    {
        tracing::error!("redis: PING failed: {e}");
    }
    if let Some(pool) = &db.pg {
        pg::init(pool).await;
    }
    if reachable.is_none() {
        return;
    }
    if let Err(e) = bootstrap_admin(state).await {
        tracing::error!("bootstrap admin failed: {e}");
    }
    if let Err(e) = seed_interest_items_if_empty(state).await {
        tracing::error!("market: interest item seed failed: {e}");
    }
    if let Err(e) = seed_supporters_if_empty(state).await {
        tracing::error!("supporters: seed failed: {e}");
    }
    if let Err(e) = backfill_owner_handles(state).await {
        tracing::error!("mods_hub: owner_handle backfill failed: {e}");
    }
    if let Err(e) = crate::mods_moderation::service::backfill_takedown_artifacts(state).await {
        tracing::error!("takedown artifact backfill failed: {e}");
    }
    if let Err(e) = crate::modpacks::service::backfill_handles(state).await {
        tracing::error!("modpacks: owner_handle backfill failed: {e}");
    }
}

/// `bootstrap_admin`: ADMIN_EMAIL/ADMIN_PASSWORD are authoritative; the
/// account is created, or promoted to superuser with its password reset.
pub async fn bootstrap_admin(state: &AppState) -> anyhow::Result<()> {
    let s = state.settings();
    let (Some(email), Some(password)) =
        (s.admin_email.as_deref().filter(|v| !v.is_empty()), s.admin_password.as_deref().filter(|v| !v.is_empty()))
    else {
        return Ok(());
    };
    let email = email.to_lowercase();
    let found = users::user_by_email(state, &email).await.map_err(|e| anyhow::anyhow!("{}", e.error))?;
    let Some(mut user) = found else {
        let email = match kiwi_core::email_validator::validate(&email) {
            kiwi_core::email_validator::Email::Valid(normalized) => normalized,
            kiwi_core::email_validator::Email::Invalid(reason) => anyhow::bail!("ADMIN_EMAIL: {reason}"),
        };
        let hashed = passwords::hash_password_blocking(password.to_owned()).await;
        let user = admin_user(ObjectId::new(), email, hashed, dt::now());
        users::insert_user(state, &user).await.map_err(|e| anyhow::anyhow!("{}", e.error))?;
        tracing::warn!("Bootstrapped admin account {}", user.email);
        return Ok(());
    };
    let mut changed = false;
    if !user.is_superuser {
        user.is_superuser = true;
        changed = true;
        tracing::warn!("Promoted existing account {email} to superuser");
    }
    if !passwords::verify_password_blocking(password.to_owned(), user.hashed_password.clone()).await {
        user.hashed_password = passwords::hash_password_blocking(password.to_owned()).await;
        changed = true;
        tracing::warn!("Reset admin password for {email} to match ADMIN_PASSWORD");
    }
    if changed {
        user.updated_at = dt::to_bson(dt::now());
        users::save_user(state, &user).await.map_err(|e| anyhow::anyhow!("{}", e.error))?;
    }
    Ok(())
}

pub fn admin_user(id: ObjectId, email: String, hashed_password: String, now: chrono::DateTime<chrono::Utc>) -> User {
    User {
        id,
        email,
        hashed_password,
        display_name: Some("Administrator".into()),
        is_active: true,
        is_verified: true,
        is_superuser: true,
        roles: Vec::new(),
        token_version: 0,
        github_id: None,
        email_bounced: false,
        created_at: dt::to_bson(now),
        updated_at: dt::to_bson(now),
        last_login_at: None,
    }
}

/// The bundled seed list without repeats: Python's `insert_many` of the raw
/// list stops at its duplicate name (unique index) and fails the boot.
pub fn interest_seed(names: &[String]) -> Vec<&str> {
    let mut seen = HashSet::new();
    names.iter().map(String::as_str).filter(|n| seen.insert(*n)).collect()
}

/// `seed_interest_items_if_empty`.
pub async fn seed_interest_items_if_empty(state: &AppState) -> anyhow::Result<usize> {
    let coll = state.mongo().collection::<Document>(INTEREST);
    if coll.count_documents(doc! {}).await? > 0 {
        return Ok(0);
    }
    let raw = std::fs::read_to_string(Path::new(gamedata::BASELINE_DIR).join("market_items.json"))?;
    let names: Vec<String> = serde_json::from_str(&raw)?;
    let names = interest_seed(&names);
    if names.is_empty() {
        return Ok(0);
    }
    let docs: Vec<Document> = names
        .iter()
        .map(|n| InterestItem { name: (*n).to_owned(), added_by: None, added_at: dt::now() }.to_doc(ObjectId::new()))
        .collect();
    coll.insert_many(docs).await?;
    crate::routes::misc::invalidate_interest_cache();
    tracing::info!("market: seeded {} interest items from gamedata/market_items.json", names.len());
    Ok(names.len())
}

/// `seed_supporters_if_empty`: one insert per name, in list order.
pub async fn seed_supporters_if_empty(state: &AppState) -> anyhow::Result<usize> {
    let coll = state.mongo().collection::<Document>(supporters::COLLECTION);
    if coll.find_one(doc! {}).await?.is_some() {
        return Ok(0);
    }
    for name in DEFAULT_SUPPORTERS {
        let row = Supporter { name: name.to_owned(), added_by: None, created_at: dt::now() };
        coll.insert_one(row.to_doc(ObjectId::new())).await?;
    }
    Ok(DEFAULT_SUPPORTERS.len())
}

/// `mods_hub.backfill_owner_handles`: resync each owned mod's `owner_handle`
/// to its owner's current username.
pub async fn backfill_owner_handles(state: &AppState) -> anyhow::Result<usize> {
    let coll = state.mongo().collection::<Document>(crate::mods_moderation::service::MOD_PROJECTS);
    let pipeline = [
        doc! {"$lookup": {"from": SITE_USERS, "localField": "owner_id", "foreignField": "_id", "as": "_owner"}},
        doc! {"$unwind": "$_owner"},
        doc! {"$match": {"$expr": {"$ne": ["$owner_handle", "$_owner.username"]}}},
        doc! {"$project": {"_id": 1, "username": "$_owner.username"}},
    ];
    let rows: Vec<Document> = coll.aggregate(pipeline).await?.try_collect().await?;
    for row in &rows {
        let (Some(id), Some(name)) = (row.get("_id"), row.get("username")) else { continue };
        coll.update_one(doc! {"_id": id.clone()}, doc! {"$set": {"owner_handle": name.clone()}}).await?;
    }
    if !rows.is_empty() {
        tracing::info!("mods_hub: resynced owner_handle on {} project(s)", rows.len());
    }
    Ok(rows.len())
}
