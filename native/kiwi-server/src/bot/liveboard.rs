//! `app/bot/liveboard.py`: one "Trove Now" message per guild, edited in place
//! (re-posted when it was deleted).

use anyhow::Result;
use serde_json::{Value, json};

use super::{Backend, Bot, RestError, message};
use crate::discord_common::{
    embeds::{Obj, live_board_embed},
    i18n::normalize_lang,
    models::GuildConfig,
};

/// The events whose data the board shows.
pub const BOARD_EVENTS: [&str; 7] =
    ["challenge", "chaos", "corruxion", "fluxion", "longshade", "daily_bonuses", "server_status"];

impl<B: Backend> Bot<B> {
    /// `_sync_board`: true when `cfg` changed (the caller saves).
    async fn sync_board(&self, cfg: &mut GuildConfig, embed: &Obj) -> Result<bool> {
        let channel = cfg.live_board.channel_id.unwrap_or_default();
        let reachable = self.cache.has_channel(channel)
            || match self.b.fetch_channel(channel).await {
                Ok(()) => true,
                Err(e) => {
                    e.http()?;
                    false
                }
            };
        if !reachable {
            tracing::warn!("liveboard: channel {channel} unreachable (guild {})", cfg.guild_id);
            return Ok(false);
        }
        if let Some(mid) = cfg.live_board.message_id {
            match self.b.edit_message(channel, mid, message::edit_payload(embed)).await {
                Ok(()) => return Ok(false),
                Err(e) => match e.http()? {
                    RestError::NotFound => cfg.live_board.message_id = None,
                    RestError::Forbidden => {
                        tracing::warn!("liveboard: missing edit perms in guild {}", cfg.guild_id);
                        return Ok(false);
                    }
                    _ => {
                        tracing::warn!("liveboard: edit failed in guild {}", cfg.guild_id);
                        return Ok(false);
                    }
                },
            }
        }
        let payload = message::send_payload(None, embed, None, false);
        match self.b.send_message(channel, payload, None).await {
            Ok(id) => {
                cfg.live_board.message_id = Some(id);
                Ok(true)
            }
            Err(e) => {
                match e.http()? {
                    RestError::Forbidden => {
                        tracing::warn!("liveboard: missing send perms in guild {} channel {channel}", cfg.guild_id)
                    }
                    _ => tracing::warn!("liveboard: send failed in guild {}", cfg.guild_id),
                }
                Ok(false)
            }
        }
    }

    /// `refresh_boards`: bring every enabled board up to date, one embed per
    /// guild language.
    pub async fn refresh_boards(&self) -> Result<()> {
        let _g = self.board_lock.lock().await;
        let now = self.now_bson();
        let mut configs = Vec::new();
        for d in self.b.guild_configs().await? {
            configs.push(GuildConfig::from_doc(&d, now)?);
        }
        let boards: Vec<&mut GuildConfig> = configs
            .iter_mut()
            .filter(|c| {
                c.live_board.enabled && c.live_board.channel_id.is_some_and(|c| c != 0) && !c.live_board.channel_missing
            })
            .collect();
        if boards.is_empty() {
            return Ok(());
        }
        let mut embeds: Vec<(&'static str, Obj)> = Vec::new();
        for cfg in boards {
            let lang = normalize_lang(Some(cfg.language.as_str()));
            if !embeds.iter().any(|(l, _)| *l == lang) {
                let snap = self.b.status().await;
                let overall = snap.get("overall").cloned().unwrap_or_else(|| json!("unknown"));
                let embed = live_board_embed(&self.i18n.tr(Some(lang)), &overall, self.b.now_us(), &self.asset);
                embeds.push((lang, embed));
            }
            let embed = embeds.iter().find(|(l, _)| *l == lang).map(|(_, e)| e.clone()).unwrap_or_default();
            if self.sync_board(cfg, &embed).await? {
                cfg.updated_at = self.now_bson();
                self.save_config(cfg).await?;
            }
        }
        Ok(())
    }
}

pub fn is_board_event(event: &Value) -> bool {
    event.as_str().is_some_and(|e| BOARD_EVENTS.contains(&e))
}
