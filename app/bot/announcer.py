"""Announcement runner.

Posts each enabled announcement type to every guild that configured it, once per
new anchor (edge-triggered per guild via ``AnnouncementSetting.last_anchor``).

Two entry points, both serialized by ``_lock`` so an event-driven run and the
poll backstop can never double-post:
- ``run_announcement_type(bot, key)`` - fired by the bot the instant a matching
  Redis event arrives (``app/bot/runner.py``); does just that one type.
- ``run_all_announcements(bot)`` - the 1-minute safety-net loop; does every type,
  catching anything the event path missed (Redis down, missed event, or the
  state-change types that have no scheduler: status / news / giveaways / activity).

Each type's anchor + embed is computed at most once per pass and fanned out to all
pending guilds; a type with no new anchor, or no guild waiting for it, is skipped
without building its embed.
"""
import asyncio
import inspect
import logging
import time

import discord

from app.bot.announcements import ANNOUNCEMENT_TYPES, TYPES_BY_KEY
from app.bot.models import GuildConfig, TrackedAnnouncement
from app.core.utils import utcnow
from app.discord.embeds import SITE

logger = logging.getLogger("kiwi.bot.announcer")


def _image_embed_dict(kind: str, token: str) -> dict:
    """A bare image embed pointing at the API's per-kind announcement banner. ``token``
    is the cache-buster (countdown-scaled) so Discord refetches when it changes."""
    return {"image": {"url": f"{SITE}/announce.png?kind={kind}&v={token}"}}


def _refresh_token(expires_at: int | None, now: int) -> str:
    """The image ``?v`` token, scaled to the countdown so we don't edit constantly:
    per-minute under 1h to expiry, per-hour under a day, per-day beyond."""
    remaining = (expires_at - now) if expires_at else 0
    if not expires_at or remaining <= 3600:
        return str(now // 60)
    if remaining <= 86400:
        return str(now // 3600)
    return str(now // 86400)

# Serializes announcement passes (event-driven vs poll backstop) so they never
# read the same last_anchor and double-post.
_lock = asyncio.Lock()


async def _build_embed(build_fn) -> dict:
    """Embed builders are a mix of sync and async; normalise to a dict."""
    result = build_fn()
    return await result if inspect.isawaitable(result) else result


async def _post(bot: discord.Client, guild_id: int, setting, embed: discord.Embed,
                key: str) -> int | None:
    """Send one announcement (with an optional role ping). Returns the sent message
    id, or None if it didn't go out."""
    channel = bot.get_channel(setting.channel_id)
    if channel is None:
        try:
            channel = await bot.fetch_channel(setting.channel_id)
        except (discord.NotFound, discord.Forbidden, discord.HTTPException):
            logger.warning("announce %s: channel %s unreachable (guild %s)",
                           key, setting.channel_id, guild_id)
            return None

    content = None
    allowed = discord.AllowedMentions.none()
    if setting.ping_role_ids:
        content = " ".join(f"<@&{r}>" for r in setting.ping_role_ids)
        # Only allow the roles we intend to ping (never @everyone / users).
        allowed = discord.AllowedMentions(roles=[discord.Object(id=r) for r in setting.ping_role_ids])

    try:
        msg = await channel.send(content=content, embed=embed, allowed_mentions=allowed)
    except discord.Forbidden:
        logger.warning("announce %s: missing send perms in guild %s channel %s",
                       key, guild_id, setting.channel_id)
        return None
    except discord.HTTPException:
        logger.exception("announce %s: send failed in guild %s", key, guild_id)
        return None
    logger.info("announced %s to guild %s", key, guild_id)
    return msg.id


async def _delete_message(bot: discord.Client, channel_id: int, message_id: int) -> bool:
    """Delete a message. Returns True when the tracking record should be DROPPED -
    i.e. the message is gone (deleted by us or someone else) or there's no point
    retrying (channel gone / no perms); False only on a transient error."""
    channel = bot.get_channel(channel_id)
    if channel is None:
        try:
            channel = await bot.fetch_channel(channel_id)
        except discord.NotFound:
            return True                       # channel gone -> message gone
        except (discord.Forbidden, discord.HTTPException):
            return False                      # transient / unreachable -> retry
    try:
        await channel.get_partial_message(message_id).delete()
        return True
    except discord.NotFound:
        return True                           # already deleted by someone else
    except discord.Forbidden:
        logger.warning("cleanup: can't delete message %s in channel %s", message_id, channel_id)
        return True                           # give up - stop tracking it
    except discord.HTTPException:
        logger.warning("cleanup: delete failed for %s", message_id, exc_info=True)
        return False                          # transient - retry next tick


async def _edit_message(bot: discord.Client, channel_id: int, message_id: int,
                        embed: discord.Embed) -> str:
    """Edit a message's embed. Returns "ok", "gone" (drop the record), or "retry"."""
    channel = bot.get_channel(channel_id)
    if channel is None:
        try:
            channel = await bot.fetch_channel(channel_id)
        except discord.NotFound:
            return "gone"
        except (discord.Forbidden, discord.HTTPException):
            return "retry"
    try:
        await channel.get_partial_message(message_id).edit(embed=embed)
        return "ok"
    except discord.NotFound:
        return "gone"
    except discord.Forbidden:
        return "gone"
    except discord.HTTPException:
        logger.warning("refresh: edit failed for %s", message_id, exc_info=True)
        return "retry"


async def _track(bot: discord.Client, guild_id: int, channel_id: int, message_id: int,
                 atype, anchor: str, expires: int | None, token: str) -> None:
    """Record a posted announcement for later cleanup/refresh, superseding any prior
    message of the same kind in this guild so only the current one ever stays."""
    prior = await TrackedAnnouncement.find(
        TrackedAnnouncement.guild_id == guild_id, TrackedAnnouncement.kind == atype.key,
    ).to_list()
    for p in prior:
        await _delete_message(bot, p.channel_id, p.message_id)
        await p.delete()                      # the new message is the source of truth now

    await TrackedAnnouncement(
        guild_id=guild_id, channel_id=channel_id, message_id=message_id,
        kind=atype.key, anchor=anchor, expires_at=expires,
        refresh=atype.expiry is not None,     # has a countdown -> refresh it; status has none
        refresh_v=token,
    ).insert()


async def cleanup_tracked(bot: discord.Client) -> None:
    """Delete announcements whose occurrence has ended, then drop their records.
    Self-cleaning + restart-safe: the records live in Mongo and this re-reads them
    each tick, so a restart just resumes. (The Mongo $lte query naturally skips the
    supersede-only rows whose expires_at is null.)"""
    now = int(utcnow().timestamp())
    try:
        expired = await TrackedAnnouncement.find(TrackedAnnouncement.expires_at <= now).to_list()
    except Exception:
        logger.warning("cleanup: query failed", exc_info=True)
        return
    for t in expired:
        if t.expires_at is None:          # belt-and-suspenders: never sweep supersede-only rows
            continue
        try:
            if await _delete_message(bot, t.channel_id, t.message_id):
                await t.delete()
        except Exception:
            logger.warning("cleanup: failed for message %s", t.message_id, exc_info=True)


async def _load_configs() -> list[GuildConfig]:
    """All guild configs, with any legacy challenge-only config folded in once."""
    configs = await GuildConfig.find_all().to_list()
    for cfg in configs:
        if cfg.migrate_legacy():
            await cfg.save()
    return configs


async def _announce_one(bot: discord.Client, atype, configs: list[GuildConfig]) -> None:
    """Post ``atype`` to every guild that's enabled it and hasn't seen this anchor."""
    candidates = [
        c for c in configs
        if (s := c.announcements.get(atype.key)) and s.enabled
        and s.channel_id and not s.channel_missing
    ]
    if not candidates:
        return
    try:
        anchor = await atype.current_anchor()
    except Exception:
        logger.warning("anchor failed for %s", atype.key, exc_info=True)
        return
    if anchor is None:
        return
    pending = [c for c in candidates if c.announcements[atype.key].last_anchor != anchor]
    if not pending:
        return
    # Managed types post a generated image (the API renders + Redis-caches it per
    # minute); everything else posts its rich text embed. Build the embed once and
    # fan it out - every guild references the same per-(kind,minute) image URL.
    expires: int | None = None
    token = ""
    try:
        if atype.auto_manage:
            if atype.expiry is not None:
                expires = await atype.expiry()
            token = _refresh_token(expires, int(time.time()))
            embed = discord.Embed.from_dict(_image_embed_dict(atype.key, token))
        else:
            embed = discord.Embed.from_dict(await _build_embed(atype.build_embed))
    except Exception:
        logger.warning("embed build failed for %s", atype.key, exc_info=True)
        return
    for cfg in pending:
        setting = cfg.announcements[atype.key]
        message_id = await _post(bot, cfg.guild_id, setting, embed, atype.key)
        if message_id is not None:
            setting.last_anchor = anchor
            cfg.updated_at = utcnow()
            await cfg.save()
            if atype.auto_manage:
                await _track(bot, cfg.guild_id, setting.channel_id, message_id,
                             atype, anchor, expires, token)


async def run_announcement_type(bot: discord.Client, key: str) -> None:
    """Fire a single announcement type (event-driven path)."""
    atype = TYPES_BY_KEY.get(key)
    if atype is None:
        return
    async with _lock:
        await _announce_one(bot, atype, await _load_configs())


async def run_all_announcements(bot: discord.Client) -> None:
    """Fire every announcement type (poll-backstop path)."""
    async with _lock:
        configs = await _load_configs()
        if not any(s.enabled and s.channel_id and not s.channel_missing
                   for c in configs for s in c.announcements.values()):
            return
        for atype in ANNOUNCEMENT_TYPES:
            await _announce_one(bot, atype, configs)


async def refresh_tracked(bot: discord.Client) -> None:
    """Re-edit live image announcements so their baked-in "ends in X" stays accurate.
    Edits only when the countdown bucket changes (per ``_refresh_token``) - so a
    far-off merchant is touched ~daily, not every minute. Restart-safe (Mongo)."""
    now = int(utcnow().timestamp())
    try:
        records = await TrackedAnnouncement.find(TrackedAnnouncement.refresh == True).to_list()  # noqa: E712
    except Exception:
        logger.warning("refresh: query failed", exc_info=True)
        return
    for t in records:
        if t.expires_at and t.expires_at <= now:
            continue                          # expired - cleanup_tracked deletes it
        token = _refresh_token(t.expires_at, now)
        if token == t.refresh_v:
            continue                          # displayed countdown unchanged - skip the edit
        embed = discord.Embed.from_dict(_image_embed_dict(t.kind, token))
        result = await _edit_message(bot, t.channel_id, t.message_id, embed)
        if result == "ok":
            t.refresh_v = token
            await t.save()
        elif result == "gone":
            await t.delete()
