//! The Discord gateway connection (twilight: identify, heartbeat, resume and
//! reconnect), feeding the [`Cache`](super::cache::Cache) and the handlers
//! discord.py dispatched: `on_ready`, `on_user_update`,
//! `on_guild_channel_delete`, `on_guild_role_delete`.

use std::{collections::HashSet, sync::Arc, time::Duration};

use tokio::{sync::watch, time::Instant};
use twilight_gateway::{CloseFrame, Event, EventTypeFlags, Intents, Shard, ShardId, StreamExt};
use twilight_model::guild::Guild;

use super::{Backend, Bot, cache};

/// discord.py's `guild_ready_timeout`.
const GUILD_READY_TIMEOUT: Duration = Duration::from_secs(2);
/// `PrivilegedIntentsRequired`.
const DISALLOWED_INTENTS: u16 = 4014;

pub enum Exit {
    Stopped,
    DisallowedIntents,
    Closed(Option<u16>),
}

fn wanted() -> EventTypeFlags {
    EventTypeFlags::READY
        | EventTypeFlags::GUILD_CREATE
        | EventTypeFlags::GUILD_DELETE
        | EventTypeFlags::CHANNEL_CREATE
        | EventTypeFlags::CHANNEL_DELETE
        | EventTypeFlags::ROLE_CREATE
        | EventTypeFlags::ROLE_DELETE
        | EventTypeFlags::MEMBER_ADD
        | EventTypeFlags::MEMBER_REMOVE
        | EventTypeFlags::MEMBER_UPDATE
}

fn guild_entry(g: &Guild) -> cache::Guild {
    cache::Guild {
        member_count: g.member_count.map(|c| c as i64),
        channels: g.channels.iter().map(|c| c.id.get() as i64).collect(),
        roles: g.roles.iter().map(|r| r.id.get() as i64).collect(),
    }
}

/// Runs one gateway session until shutdown or a fatal close. `ready` turns
/// true on the first `on_ready`.
pub async fn run<B: Backend>(
    bot: Arc<Bot<B>>,
    token: String,
    members: bool,
    ready: watch::Sender<bool>,
    mut shutdown: watch::Receiver<bool>,
) -> Exit {
    let mut intents = Intents::GUILDS;
    if members {
        intents |= Intents::GUILD_MEMBERS;
    }
    let mut shard = Shard::new(ShardId::ONE, token, intents);
    let mut waiting: Option<HashSet<i64>> = None;
    let mut deadline = Instant::now();
    let mut last_close = None;
    loop {
        let timer = tokio::time::sleep_until(deadline);
        tokio::select! {
            _ = shutdown.changed() => {
                shard.close(CloseFrame::NORMAL);
                return Exit::Stopped;
            }
            _ = timer, if waiting.is_some() => {
                waiting = None;
                on_ready(&bot, &ready);
            }
            ev = shard.next_event(wanted()) => {
                let event = match ev {
                    None => {
                        return if last_close == Some(DISALLOWED_INTENTS) {
                            Exit::DisallowedIntents
                        } else {
                            Exit::Closed(last_close)
                        };
                    }
                    Some(Err(e)) => {
                        tracing::warn!("gateway: {e}");
                        continue;
                    }
                    Some(Ok(event)) => event,
                };
                match event {
                    Event::GatewayClose(frame) => {
                        last_close = frame.map(|f| f.code);
                        if let Some(code) = last_close {
                            tracing::warn!("gateway closed with code {code}");
                        }
                    }
                    Event::Ready(r) => {
                        tracing::info!("Kiwi bot connected as {} - in {} guild(s)", r.user.name, r.guilds.len());
                        bot.cache.reset(r.guilds.iter().map(|g| g.id.get() as i64));
                        waiting = Some(r.guilds.iter().map(|g| g.id.get() as i64).collect());
                        deadline = Instant::now() + GUILD_READY_TIMEOUT;
                        if waiting.as_ref().is_some_and(HashSet::is_empty) {
                            waiting = None;
                            on_ready(&bot, &ready);
                        }
                    }
                    Event::GuildCreate(gc) => {
                        if let twilight_model::gateway::payload::incoming::GuildCreate::Available(g) = gc.as_ref() {
                            let id = g.id.get() as i64;
                            bot.cache.upsert_guild(id, guild_entry(g));
                            if let Some(w) = waiting.as_mut() {
                                w.remove(&id);
                                deadline = Instant::now() + GUILD_READY_TIMEOUT;
                                if w.is_empty() {
                                    waiting = None;
                                    on_ready(&bot, &ready);
                                }
                            }
                        }
                    }
                    Event::GuildDelete(gd) => {
                        if gd.unavailable != Some(true) {
                            bot.cache.remove_guild(gd.id.get() as i64);
                        }
                    }
                    Event::ChannelCreate(c) => {
                        if let Some(g) = c.guild_id {
                            let id = c.id.get() as i64;
                            bot.cache.update_guild(g.get() as i64, |e| {
                                e.channels.insert(id);
                            });
                        }
                    }
                    Event::ChannelDelete(c) => {
                        if let Some(g) = c.guild_id {
                            let (gid, id) = (g.get() as i64, c.id.get() as i64);
                            bot.cache.update_guild(gid, |e| {
                                e.channels.remove(&id);
                            });
                            reconcile(&bot, gid);
                        }
                    }
                    Event::RoleCreate(r) => {
                        let id = r.role.id.get() as i64;
                        bot.cache.update_guild(r.guild_id.get() as i64, |e| {
                            e.roles.insert(id);
                        });
                    }
                    Event::RoleDelete(r) => {
                        let (gid, id) = (r.guild_id.get() as i64, r.role_id.get() as i64);
                        bot.cache.update_guild(gid, |e| {
                            e.roles.remove(&id);
                        });
                        reconcile(&bot, gid);
                    }
                    Event::MemberAdd(m) => {
                        bot.cache.update_guild(m.guild_id.get() as i64, |e| {
                            if let Some(c) = e.member_count.as_mut() {
                                *c += 1;
                            }
                        });
                        let user = &m.member.user;
                        bot.cache.seen_avatar(user.id.get() as i64, user.avatar.map(|a| a.to_string()));
                    }
                    Event::MemberRemove(m) => {
                        bot.cache.update_guild(m.guild_id.get() as i64, |e| {
                            if let Some(c) = e.member_count.as_mut() {
                                *c -= 1;
                            }
                        });
                    }
                    Event::MemberUpdate(m) => {
                        let id = m.user.id.get() as i64;
                        let avatar = m.user.avatar.map(|a| a.to_string());
                        if bot.cache.avatar_changed(id, avatar.clone()) {
                            let bot = bot.clone();
                            tokio::spawn(async move {
                                if let Err(e) = bot.sync_avatar(id, avatar).await {
                                    tracing::error!("avatar sync failed for user {id}: {e:#}");
                                }
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

fn on_ready<B: Backend>(bot: &Arc<Bot<B>>, ready: &watch::Sender<bool>) {
    let (guilds, _) = bot.cache.presence();
    tracing::info!("Kiwi bot ready - in {guilds} guild(s)");
    ready.send_replace(true);
    let bot = bot.clone();
    tokio::spawn(async move { bot.record_presence().await });
}

fn reconcile<B: Backend>(bot: &Arc<Bot<B>>, guild_id: i64) {
    let Some((channels, roles)) = bot.cache.guild_ids(guild_id) else { return };
    let bot = bot.clone();
    tokio::spawn(async move {
        if let Err(e) = bot.reconcile_guild(guild_id, &channels, &roles).await {
            tracing::error!("reconcile failed for guild {guild_id}: {e:#}");
        }
    });
}
