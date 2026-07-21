"""Kiwi gateway bot (discord.py) - a separate always-on process.

Coexists with the HTTP-interactions slash commands: Discord routes slash commands
to the interactions endpoint, while this gateway connection handles proactive work:
- announcements, driven two ways:
    * **event-driven** - subscribes to the same Redis events channel the API
      publishes to (captures on insert, rotations on schedule); the instant an
      event arrives it posts that one announcement type. Near-real-time.
    * **clock-aligned minute loop** - at :00 sharp every minute: delete ended
      announcements, re-edit live image countdowns + the board, run the post backstop.
      Aligning to the wall clock keeps the countdowns from drifting. (The board is
      also refreshed instantly on board-relevant events.)
- keeping per-guild config in sync with Discord's live state - when a channel or
  role a guild configured is deleted, we reconcile immediately.
- mirroring a linked user's Discord avatar change into their SiteUser (on_user_update),
  so the website's cached avatar hash stays fresh between logins (app/bot/avatar_sync).

Shares the API's Mongo (``GuildConfig``, ``SiteUser``) and Redis. Runs on the default
(Guilds) intent plus the members privileged intent for the avatar sync above - if that
intent isn't granted in the Developer Portal, main() falls back to running without it.
Run: ``python -m app.bot.runner`` (the compose ``bot`` service).
"""
import asyncio
import json
import logging
from datetime import timedelta

import discord

from app.bot import liveboard, stats
from app.bot.announcements import ANNOUNCED_CHALLENGE_CATEGORIES
from app.bot.announcer import (
    cleanup_tracked,
    refresh_tracked,
    run_all_announcements,
    run_announcement_type,
)
from app.bot.avatar_sync import sync_avatar
from app.bot.models import GuildConfig
from app.bot.reconcile import reconcile
from app.core.config import settings
from app.core.database import close_db, init_db
from app.core.redis import close_redis, get_redis, init_redis
from app.core.utils import utcnow

logger = logging.getLogger("kiwi.bot")

# Redis event type -> announcement registry key(s). Rotation event types already
# match their announcement key; only challenge/chaos use the SSE-legacy names. The
# single "challenge" event fans out to every per-category challenge type (only the
# live category's anchor actually fires).
_EVENT_TO_ANNOUNCEMENT: dict[str, str | tuple[str, ...]] = {
    # Derived from the offered categories so a hidden one (racing/target) never maps
    # to a non-existent announcement type.
    "challenge": tuple(f"challenge_{cat}" for cat in ANNOUNCED_CHALLENGE_CATEGORIES),
    "chaos": "chaos_chest",
    "luxion": "luxion",
    "corruxion": "corruxion",
    "fluxion": "fluxion",
    "longshade": "longshade",
    "wild_mana": "wild_mana",
    "stampy": "stampy",
    "daily_bonuses": "daily_bonuses",
    "activity": "activity",
    "server_status": "server_status",
    "trove_news": "trove_news",
    "giveaways": "giveaways",
    "game_update": "game_update",
}


def _seconds_until(second: int) -> float:
    """Seconds until the next HH:MM:<second> (UTC), so a loop can align to the clock."""
    now = utcnow()
    target = now.replace(second=second, microsecond=0)
    if target <= now:
        target += timedelta(minutes=1)
    return max(0.0, (target - now).total_seconds())


def _build_intents(members: bool) -> discord.Intents:
    # Default intents (Guilds) already cover everything the announcer/reconciler
    # need - sending messages, reading guild structure, channel/role events. The
    # members privileged intent is added only for the Discord avatar auto-sync
    # (on_user_update); it requires the "Server Members Intent" toggle in the
    # Developer Portal, and main() drops it gracefully if that's not granted.
    intents = discord.Intents.default()
    if members:
        intents.members = True
    return intents


class KiwiBot(discord.Client):
    def __init__(self, members_intent: bool | None = None) -> None:
        use_members = (
            settings.bot_members_intent if members_intent is None else members_intent
        )
        super().__init__(intents=_build_intents(use_members))
        self._tasks: list[asyncio.Task] = []

    async def setup_hook(self) -> None:
        self._tasks = [
            asyncio.create_task(self._subscribe_events()),
            asyncio.create_task(self._minute_loop()),    # edits + deletes, aligned to :00
        ]

    async def on_ready(self) -> None:
        logger.info("Kiwi bot ready as %s - in %d guild(s)", self.user, len(self.guilds))
        await self._record_presence()

    async def _record_presence(self) -> None:
        """Report the bot's reach (servers + users it can see) for the master stats."""
        try:
            members = sum((g.member_count or 0) for g in self.guilds)
            await stats.record_presence(len(self.guilds), members)
        except Exception:
            logger.exception("presence stats write failed")

    async def close(self) -> None:
        for task in self._tasks:
            task.cancel()
        await super().close()

    async def _subscribe_events(self) -> None:
        """Subscribe to the Redis events channel and post the matching announcement
        the instant an event arrives. No-op (poll backstop only) without Redis."""
        await self.wait_until_ready()
        redis = get_redis()
        if redis is None:
            logger.warning("Redis not configured - announcements use the poll backstop only")
            return
        pubsub = redis.pubsub()
        await pubsub.subscribe(settings.events_channel)
        logger.info("subscribed to events channel %r", settings.events_channel)
        try:
            async for msg in pubsub.listen():
                if msg.get("type") != "message":
                    continue            # skip subscribe/unsubscribe confirmations
                try:
                    payload = json.loads(msg["data"])
                except (ValueError, TypeError):
                    continue
                event_type = payload.get("type")
                mapped = _EVENT_TO_ANNOUNCEMENT.get(event_type)
                keys = (mapped,) if isinstance(mapped, str) else (mapped or ())
                for key in keys:
                    try:
                        await run_announcement_type(self, key)
                    except Exception:
                        logger.exception("event-driven announce failed for %s", key)
                # The hourly market refresh drives the per-guild marketplace watch.
                if event_type == "market":
                    try:
                        from app.bot.market_watch import run_market_watch
                        await run_market_watch(self)
                    except Exception:
                        logger.exception("market_watch run failed")
                # Refresh the live board the instant a board-relevant event lands; the
                # :55 clock loop also refreshes it so the periodic countdowns stay synced.
                if event_type in liveboard.BOARD_EVENTS:
                    try:
                        await liveboard.refresh_boards(self)
                    except Exception:
                        logger.exception("live board refresh failed for %s", event_type)
        except asyncio.CancelledError:
            raise
        finally:
            try:
                await pubsub.unsubscribe(settings.events_channel)
                await pubsub.aclose()
            except Exception:
                pass

    # Clock-aligned minute loop. Everything periodic fires at HH:MM:00 sharp: deletes
    # land the instant an occurrence ends, and the image re-edits happen at the boundary
    # too (image generation is fast, so there's no need to pre-compute). Aligning to the
    # wall clock keeps the countdowns from drifting. (The live board is ALSO refreshed
    # instantly on events - see _subscribe_events.)

    async def _minute_loop(self) -> None:
        await self.wait_until_ready()
        while not self.is_closed():
            await asyncio.sleep(_seconds_until(0))
            if self.is_closed():
                break
            try:
                await cleanup_tracked(self)         # delete expired (sharp at :00)
            except Exception:
                logger.exception("announcement cleanup round failed")
            try:
                await refresh_tracked(self)         # re-edit live image countdowns
            except Exception:
                logger.exception("announcement refresh round failed")
            try:
                await liveboard.refresh_boards(self)   # also (re)posts newly-enabled boards
            except Exception:
                logger.exception("live board refresh round failed")
            try:
                await run_all_announcements(self)   # post backstop (catch missed events)
            except Exception:
                logger.exception("announcement backstop failed")
            await self._record_presence()           # keep server/user counts fresh

    # Dynamic linking to guild state. The cache reflects the post-deletion state by the
    # time these fire, so ``guild.channels`` / ``guild.roles`` are the live id sets to
    # reconcile against.

    async def on_user_update(self, before: discord.User, after: discord.User) -> None:
        """Mirror a Discord avatar change into the linked SiteUser (if any), so the
        website's cached hash stays fresh between logins. Fires only for users we
        share a guild with (needs the members intent). Username/discriminator-only
        changes are skipped so we don't touch the DB for edits we don't track."""
        if before.avatar == after.avatar:
            return
        try:
            await sync_avatar(after)
        except Exception:
            logger.exception("avatar sync failed for user %s", after.id)

    async def on_guild_channel_delete(self, channel: discord.abc.GuildChannel) -> None:
        await self._reconcile_guild(channel.guild)

    async def on_guild_role_delete(self, role: discord.Role) -> None:
        await self._reconcile_guild(role.guild)

    async def _reconcile_guild(self, guild: discord.Guild) -> None:
        try:
            cfg = await GuildConfig.find_one(GuildConfig.guild_id == guild.id)
            if cfg is None:
                return
            dirty = cfg.migrate_legacy()
            dirty = reconcile(cfg, {c.id for c in guild.channels},
                              {r.id for r in guild.roles}) or dirty
            if dirty:
                cfg.updated_at = utcnow()
                await cfg.save()
                logger.info("reconciled guild %s after a channel/role change", guild.id)
        except Exception:
            logger.exception("reconcile failed for guild %s", guild.id)


async def main() -> None:
    logging.basicConfig(
        level=logging.INFO,
        format="%(asctime)s %(levelname)s %(name)s: %(message)s",
    )
    if not settings.discord_bot_token:
        raise SystemExit("DISCORD_BOT_TOKEN is required to run the Kiwi bot.")
    await init_db()
    await init_redis()
    try:
        try:
            await KiwiBot().start(settings.discord_bot_token)
        except discord.PrivilegedIntentsRequired:
            # The members intent (for avatar auto-sync) isn't granted in the Discord
            # Developer Portal. Don't take the whole bot down for it - restart WITHOUT
            # the intent; avatars just keep refreshing at login as before.
            if not settings.bot_members_intent:
                raise  # not our doing - surface the real error
            logger.warning(
                "Server Members Intent is not enabled in the Discord Developer "
                "Portal - starting the bot without it (Discord avatar auto-sync is "
                "off; avatars still refresh on the user's next login). Enable the "
                "intent to turn it on, or set BOT_MEMBERS_INTENT=false to silence this."
            )
            await KiwiBot(members_intent=False).start(settings.discord_bot_token)
    finally:
        await close_redis()
        await close_db()


if __name__ == "__main__":
    asyncio.run(main())
