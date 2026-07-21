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

from app import i18n
from app.bot.announcements import ANNOUNCEMENT_TYPES, TYPES_BY_KEY
from app.bot.models import GuildConfig, TrackedAnnouncement
from app.core.config import settings
from app.core.utils import countdown_bucket, utcnow
from app.discord import embed_contexts
from app.embed_templates import render_template

logger = logging.getLogger("kiwi.bot.announcer")


def _image_embed_dict(kind: str, token: str, lang: str = "en") -> dict:
    """A bare image embed pointing at the API's per-kind announcement banner. ``token``
    is the cache-buster (countdown-scaled) so Discord refetches when it changes.
    ``lang`` selects the localized render (omitted for English so existing URLs +
    the per-(kind,minute) image cache are unchanged)."""
    suffix = f"&lang={lang}" if lang != "en" else ""
    # announce.png is an API-rendered PNG, so it must point at the ASSET host
    # (api.aallyn.net once the website is split out), NOT the website host - Discord
    # fetches embed images server-side and the web container doesn't serve it.
    return {"image": {"url": f"{settings.asset_url}/announce.png?kind={kind}&v={token}{suffix}"}}


def _refresh_token(expires_at: int | None, now: int) -> str:
    """The image ``?v`` token = the DISPLAYED countdown bucket (``countdown_bucket``),
    so Discord refetches and we re-edit EXACTLY when the shown value changes - "16h"
    holds for the whole hour (one edit/hour), per-minute only under 1h, per-day
    beyond. Sharing the bucket with the image renderer keeps the token and the drawn
    text from drifting (no re-edit while the banner is visually identical)."""
    unit, val = countdown_bucket(expires_at, now)
    return f"{unit}{val}"

# Serializes announcement passes (event-driven vs poll backstop) so they never
# read the same last_anchor and double-post.
_lock = asyncio.Lock()


async def _build_embed(build_fn, lang: str = "en") -> dict:
    """Embed builders are a mix of sync and async; normalise to a dict. Sets the
    i18n context language first so the builder's ``t(...)`` calls localize."""
    i18n.set_current_language(lang)
    result = build_fn()
    return await result if inspect.isawaitable(result) else result


async def _design_file(image_design_id: str | None):
    """Render an Image Studio design to a ``discord.File`` (fresh per post, so live data
    in the image is current and never URL-cached), or None on any failure."""
    if not image_design_id:
        return None
    try:
        from io import BytesIO

        from app.embed_templates import EMBED_IMAGE_ATTACHMENT
        from app.images import service as images
        png = await images.render_public(image_design_id)
        if png:
            return discord.File(BytesIO(png), filename=EMBED_IMAGE_ATTACHMENT)
    except Exception:
        logger.warning("announce image render failed (%s)", image_design_id, exc_info=True)
    return None


async def _post(bot: discord.Client, guild_id: int, setting, embed: discord.Embed,
                key: str, extra_content: str | None = None,
                image_design_id: str | None = None) -> int | None:
    """Send one announcement (with an optional role ping + custom message text +
    optional rendered image attachment). Returns the sent message id, or None."""
    channel = bot.get_channel(setting.channel_id)
    if channel is None:
        try:
            channel = await bot.fetch_channel(setting.channel_id)
        except (discord.NotFound, discord.Forbidden, discord.HTTPException):
            logger.warning("announce %s: channel %s unreachable (guild %s)",
                           key, setting.channel_id, guild_id)
            return None

    parts = []
    allowed = discord.AllowedMentions.none()
    if setting.ping_role_ids:
        parts.append(" ".join(f"<@&{r}>" for r in setting.ping_role_ids))
        # Only allow the roles we intend to ping (never @everyone / users).
        allowed = discord.AllowedMentions(roles=[discord.Object(id=r) for r in setting.ping_role_ids])
    if extra_content:                         # the custom template's message text
        parts.append(extra_content)
    content = "\n".join(parts)[:2000] or None

    file = await _design_file(image_design_id)
    try:
        msg = await channel.send(content=content, embed=embed, file=file, allowed_mentions=allowed)
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
                 atype, anchor: str, expires: int | None, token: str,
                 custom: bool = False) -> None:
    """Record a posted announcement for later cleanup/refresh, superseding any prior
    message of the same kind in this guild so only the current one ever stays.

    ``custom`` (a user's rich template) embeds use live Discord ``<t:>`` timestamps, so
    they never need the image re-edit - we still supersede + auto-delete them, just
    skip the refresh."""
    prior = await TrackedAnnouncement.find(
        TrackedAnnouncement.guild_id == guild_id, TrackedAnnouncement.kind == atype.key,
    ).to_list()
    for p in prior:
        await _delete_message(bot, p.channel_id, p.message_id)
        await p.delete()                      # the new message is the source of truth now

    await TrackedAnnouncement(
        guild_id=guild_id, channel_id=channel_id, message_id=message_id,
        kind=atype.key, anchor=anchor, expires_at=expires,
        # has a countdown -> refresh it; status has none; custom embeds self-update.
        refresh=(atype.expiry is not None) and not custom,
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
    # minute); everything else posts its rich text embed. Build ONE embed per
    # distinct guild language among the pending guilds and fan each out to the
    # guilds that speak it (image types reference a per-(kind,minute,lang) URL).
    langs = {i18n.normalize_lang(getattr(c, "language", None)) for c in pending}
    expires: int | None = None
    token = ""
    embeds_by_lang: dict[str, discord.Embed] = {}
    try:
        if atype.auto_manage:
            if atype.expiry is not None:
                expires = await atype.expiry()
            token = _refresh_token(expires, int(time.time()))
            for lang in langs:
                embeds_by_lang[lang] = discord.Embed.from_dict(
                    _image_embed_dict(atype.key, token, lang))
        else:
            for lang in langs:
                embeds_by_lang[lang] = discord.Embed.from_dict(
                    await _build_embed(atype.build_embed, lang))
    except Exception:
        logger.warning("embed build failed for %s", atype.key, exc_info=True)
        return

    # Per-language live variable context, built lazily only when a pending guild has a
    # custom template. Resilient per lang (a failure just falls that lang back to the
    # default embed) so it can never block the un-customized guilds.
    contexts_by_lang: dict[str, dict] = {}
    if embed_contexts.has_customization(atype.key) and any(
            (s := c.announcements[atype.key]).template and s.template.enabled for c in pending):
        for lang in langs:
            try:
                i18n.set_current_language(lang)
                contexts_by_lang[lang] = await embed_contexts.context(atype.key)
            except Exception:
                logger.warning("context build failed for %s (%s)", atype.key, lang, exc_info=True)

    for cfg in pending:
        setting = cfg.announcements[atype.key]
        lang = i18n.normalize_lang(getattr(cfg, "language", None))
        custom = bool(setting.template and setting.template.enabled and lang in contexts_by_lang)
        extra_content = None
        design_id = None
        if custom:
            ctx = contexts_by_lang[lang]
            ed, extra_content = render_template(
                setting.template, ctx, default_image_url=ctx.get("image_url"))
            embed = discord.Embed.from_dict(ed)
            if setting.template.show_image:
                design_id = setting.template.image_design_id
        else:
            embed = embeds_by_lang[lang]
        message_id = await _post(bot, cfg.guild_id, setting, embed, atype.key,
                                 extra_content, design_id)
        if message_id is not None:
            setting.last_anchor = anchor
            cfg.updated_at = utcnow()
            await cfg.save()
            if atype.auto_manage:
                await _track(bot, cfg.guild_id, setting.channel_id, message_id,
                             atype, anchor, expires, token, custom=custom)


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
        # Keep the guild's language on the re-edit, or the refresh reverts the
        # image to English a minute after posting.
        lang = await i18n.guild_language(t.guild_id)
        embed = discord.Embed.from_dict(_image_embed_dict(t.kind, token, lang))
        result = await _edit_message(bot, t.channel_id, t.message_id, embed)
        if result == "ok":
            t.refresh_v = token
            await t.save()
        elif result == "gone":
            await t.delete()
