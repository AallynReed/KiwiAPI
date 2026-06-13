"""Reconcile a GuildConfig against the guild's LIVE Discord state.

The dashboard's database "dynamically links" to Discord: when a channel or role a
guild's config points at disappears, we react instead of silently keeping a
dangling id. One pure function, two callers:

- the gateway bot's ``on_guild_channel_delete`` / ``on_guild_role_delete``
  handlers (app/bot/runner.py) react the instant Discord tells us; and
- the dashboard read path (app/bot/router.py) re-checks on every load, as a
  safety net for changes that happened while the bot was down.

Policy (mirrors what the dashboard surfaces to the user):
- **Deleted channel** -> flag the announcement ``channel_missing`` and disable it,
  but KEEP the row, so the dashboard can shout "this channel was deleted" rather
  than silently dropping the user's setup. Re-pointing to a live channel clears it.
- **Deleted role** -> remove it everywhere (ping targets + capability grants). A
  role that no longer exists is just noise; clean it up.
"""
from __future__ import annotations

from collections.abc import Iterable

from app.bot.models import Club, GuildConfig


def reconcile(
    cfg: GuildConfig,
    live_channel_ids: Iterable[int],
    live_role_ids: Iterable[int],
) -> bool:
    """Prune/flag dangling references in ``cfg`` against the live id sets.

    Returns True if anything changed (the caller persists)."""
    channels = set(live_channel_ids)
    roles = set(live_role_ids)
    changed = False

    for setting in cfg.announcements.values():
        if setting.channel_id is not None and setting.channel_id not in channels:
            # Channel gone -> disable + flag (kept visible for the user).
            if setting.enabled or not setting.channel_missing:
                setting.enabled = False
                setting.channel_missing = True
                changed = True
        elif setting.channel_missing and setting.channel_id in channels:
            # Re-pointed to a live channel since we flagged it.
            setting.channel_missing = False
            changed = True

        if setting.ping_role_ids:                   # drop any deleted ping roles
            kept = [r for r in setting.ping_role_ids if r in roles]
            if len(kept) != len(setting.ping_role_ids):
                setting.ping_role_ids = kept
                changed = True

    # Live board: same channel policy as an announcement.
    board = cfg.live_board
    if board.channel_id is not None and board.channel_id not in channels:
        if board.enabled or not board.channel_missing:
            board.enabled = False
            board.channel_missing = True
            board.message_id = None                 # old message is gone with the channel
            changed = True
    elif board.channel_missing and board.channel_id in channels:
        board.channel_missing = False
        changed = True

    # Capability grants to deleted roles -> drop them (clean up empty caps).
    for cap, role_ids in list(cfg.config_perms.items()):
        kept = [r for r in role_ids if r in roles]
        if len(kept) != len(role_ids):
            if kept:
                cfg.config_perms[cap] = kept
            else:
                del cfg.config_perms[cap]
            changed = True

    return changed


def reconcile_club(club: Club, live_role_ids: Iterable[int]) -> bool:
    """Drop any club rank->role link pointing at a role that no longer exists.
    Checked on every load of the Clubs interface (no events). Returns True if
    anything changed (the caller persists)."""
    roles = set(live_role_ids)
    changed = False
    for rank, role_id in list(club.role_links.items()):
        if role_id not in roles:
            del club.role_links[rank]
            changed = True
    return changed
