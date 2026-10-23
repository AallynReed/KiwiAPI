//! The gateway-driven writes: presence stats (`app/bot/stats.record_presence`),
//! the linked account's avatar hash (`app/bot/avatar_sync`) and reconcile on
//! a deleted channel or role (`KiwiBot._reconcile_guild`).

use std::collections::HashSet;

use anyhow::Result;

use super::{Backend, Bot};
use crate::{
    auth::models::SiteUser,
    discord_common::{models::GuildConfig, reconcile::reconcile},
};

pub const SITE_USERS: &str = "site_users";

impl<B: Backend> Bot<B> {
    /// `_record_presence`: guild count and the members they hold.
    pub async fn record_presence(&self) {
        let (guilds, members) = self.cache.presence();
        if let Err(e) = self.b.record_presence(guilds, members).await {
            tracing::warn!("stats: presence write failed: {e:#}");
        }
    }

    /// `sync_avatar`: mirror the hash onto the linked account; true when a row
    /// changed.
    pub async fn sync_avatar(&self, discord_id: i64, avatar: Option<String>) -> Result<bool> {
        let Some(d) = self.b.site_user_by_discord(discord_id).await? else { return Ok(false) };
        let mut user: SiteUser = mongodb::bson::from_document(d)?;
        if user.discord_avatar == avatar {
            return Ok(false);
        }
        user.discord_avatar = avatar;
        user.updated_at = self.now_bson();
        let doc = crate::admin::docs::site_user_doc(&user).map_err(|e| anyhow::anyhow!("{}", e.body()))?;
        self.b.save(SITE_USERS, user.id, doc).await?;
        tracing::info!("avatar sync: refreshed cached avatar for discord_id={discord_id}");
        Ok(true)
    }

    /// `_reconcile_guild` against the guild's live channel and role ids.
    pub async fn reconcile_guild(&self, guild_id: i64, channels: &HashSet<i64>, roles: &HashSet<i64>) -> Result<()> {
        let Some(d) = self.b.guild_config(guild_id).await? else { return Ok(()) };
        let mut cfg = GuildConfig::from_doc(&d, self.now_bson())?;
        let mut dirty = cfg.migrate_legacy();
        dirty = reconcile(&mut cfg, channels, roles) || dirty;
        if dirty {
            cfg.updated_at = self.now_bson();
            self.save_config(&cfg).await?;
            tracing::info!("reconciled guild {guild_id} after a channel/role change");
        }
        Ok(())
    }
}
