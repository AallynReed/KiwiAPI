//! Discord code the gateway bot and the slash-command side share: i18n, the
//! announcement embed builders and contexts, the service reads behind them,
//! and the guild config models with their reconcile.

pub mod contexts;
pub mod embeds;
pub mod i18n;
pub mod models;
pub mod pyfmt;
pub mod reconcile;
pub mod sources;

/// `DISCORD_HOME_GUILD_ID`: the server Fox Hearth runs from its own config.
pub fn home_guild(s: &crate::config::Settings) -> Option<i64> {
    s.discord_home_guild_id.as_deref().and_then(|v| v.trim().parse().ok())
}
