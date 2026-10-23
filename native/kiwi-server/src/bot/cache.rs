//! The slice of discord.py's connection state the bot reads: which channels
//! it can see (`get_channel`), each guild's channel and role ids (reconcile),
//! member counts (presence) and the last avatar seen per user (avatar sync).

use std::{
    collections::{HashMap, HashSet},
    sync::RwLock,
};

#[derive(Clone, Debug, Default)]
pub struct Guild {
    pub member_count: Option<i64>,
    pub channels: HashSet<i64>,
    pub roles: HashSet<i64>,
}

#[derive(Default)]
pub struct Cache {
    guilds: RwLock<HashMap<i64, Guild>>,
    avatars: RwLock<HashMap<i64, Option<String>>>,
}

impl Cache {
    fn write(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<i64, Guild>> {
        self.guilds.write().unwrap_or_else(|e| e.into_inner())
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<i64, Guild>> {
        self.guilds.read().unwrap_or_else(|e| e.into_inner())
    }

    /// `bot.get_channel(id) is not None`.
    pub fn has_channel(&self, id: i64) -> bool {
        self.read().values().any(|g| g.channels.contains(&id))
    }

    /// READY: every guild starts out unavailable.
    pub fn reset(&self, guild_ids: impl IntoIterator<Item = i64>) {
        let mut g = self.write();
        g.clear();
        for id in guild_ids {
            g.insert(id, Guild::default());
        }
    }

    pub fn upsert_guild(&self, id: i64, guild: Guild) {
        self.write().insert(id, guild);
    }

    pub fn update_guild(&self, id: i64, f: impl FnOnce(&mut Guild)) {
        if let Some(g) = self.write().get_mut(&id) {
            f(g);
        }
    }

    pub fn remove_guild(&self, id: i64) {
        self.write().remove(&id);
    }

    /// `(len(guilds), sum(g.member_count or 0))`.
    pub fn presence(&self) -> (i64, i64) {
        let g = self.read();
        (g.len() as i64, g.values().map(|g| g.member_count.unwrap_or(0)).sum())
    }

    /// The live channel and role ids of a cached guild.
    pub fn guild_ids(&self, id: i64) -> Option<(HashSet<i64>, HashSet<i64>)> {
        self.read().get(&id).map(|g| (g.channels.clone(), g.roles.clone()))
    }

    /// Records `avatar` for `user`; true when it differs from the last one seen
    /// (or none was).
    pub fn avatar_changed(&self, user: i64, avatar: Option<String>) -> bool {
        let mut a = self.avatars.write().unwrap_or_else(|e| e.into_inner());
        match a.insert(user, avatar.clone()) {
            Some(prev) => prev != avatar,
            None => true,
        }
    }

    /// Records `avatar` without reporting a change (a member joining).
    pub fn seen_avatar(&self, user: i64, avatar: Option<String>) {
        self.avatars.write().unwrap_or_else(|e| e.into_inner()).insert(user, avatar);
    }
}
