//! `app/bot/reconcile.reconcile`: a deleted channel flags and disables what
//! pointed at it (the row stays visible); a deleted role is dropped everywhere.

use std::collections::HashSet;

use super::models::GuildConfig;

/// True when `cfg` changed (the caller saves).
pub fn reconcile(cfg: &mut GuildConfig, channels: &HashSet<i64>, roles: &HashSet<i64>) -> bool {
    let mut changed = false;
    for s in cfg.announcements.values_mut() {
        match s.channel_id {
            Some(c) if !channels.contains(&c) => {
                if s.enabled || !s.channel_missing {
                    s.enabled = false;
                    s.channel_missing = true;
                    changed = true;
                }
            }
            c if s.channel_missing && c.is_some_and(|c| channels.contains(&c)) => {
                s.channel_missing = false;
                changed = true;
            }
            _ => {}
        }
        if !s.ping_role_ids.is_empty() {
            let kept: Vec<i64> = s.ping_role_ids.iter().copied().filter(|r| roles.contains(r)).collect();
            if kept.len() != s.ping_role_ids.len() {
                s.ping_role_ids = kept;
                changed = true;
            }
        }
    }
    let board = &mut cfg.live_board;
    match board.channel_id {
        Some(c) if !channels.contains(&c) => {
            if board.enabled || !board.channel_missing {
                board.enabled = false;
                board.channel_missing = true;
                board.message_id = None;
                changed = true;
            }
        }
        c if board.channel_missing && c.is_some_and(|c| channels.contains(&c)) => {
            board.channel_missing = false;
            changed = true;
        }
        _ => {}
    }
    let caps: Vec<String> = cfg.config_perms.keys().cloned().collect();
    for cap in caps {
        let ids = &cfg.config_perms[&cap];
        let kept: Vec<i64> = ids.iter().copied().filter(|r| roles.contains(r)).collect();
        if kept.len() != ids.len() {
            if kept.is_empty() {
                cfg.config_perms.shift_remove(&cap);
            } else {
                cfg.config_perms.insert(cap, kept);
            }
            changed = true;
        }
    }
    changed
}
