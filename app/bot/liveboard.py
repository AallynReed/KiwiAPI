"""Self-updating "Trove Now" board.

Each guild that enables it gets ONE message the bot keeps current by EDITING it in
place - never re-posting. It's refreshed the instant a board-relevant event lands
(``runner._subscribe_events``) AND once a minute on the :00 clock loop
(``runner._minute_loop``), so its countdowns stay synced to the wall clock. If the
message was deleted we re-post; if the channel was deleted the reconcile path flags
it (and we skip).
"""
import asyncio
import logging

import discord

from app import i18n
from app.bot.models import GuildConfig
from app.core.utils import utcnow
from app.discord.embeds import live_board_embed

logger = logging.getLogger("kiwi.bot.liveboard")

# The event types whose data the board shows - only these trigger an instant refresh.
BOARD_EVENTS = frozenset(
    {"challenge", "chaos", "corruxion", "fluxion", "longshade", "daily_bonuses", "server_status"}
)

_lock = asyncio.Lock()


async def _resolve_channel(bot: discord.Client, channel_id: int):
    channel = bot.get_channel(channel_id)
    if channel is None:
        try:
            channel = await bot.fetch_channel(channel_id)
        except (discord.NotFound, discord.Forbidden, discord.HTTPException):
            return None
    return channel


async def _sync_board(bot: discord.Client, cfg: GuildConfig, embed: discord.Embed) -> bool:
    """Post or edit one guild's board. Returns True if cfg changed (caller saves)."""
    board = cfg.live_board
    channel = await _resolve_channel(bot, board.channel_id)
    if channel is None:
        logger.warning("liveboard: channel %s unreachable (guild %s)",
                       board.channel_id, cfg.guild_id)
        return False

    if board.message_id is not None:
        try:
            await channel.get_partial_message(board.message_id).edit(embed=embed)
            return False
        except discord.NotFound:
            board.message_id = None          # message was deleted - re-post below
        except discord.Forbidden:
            logger.warning("liveboard: missing edit perms in guild %s", cfg.guild_id)
            return False
        except discord.HTTPException:
            logger.warning("liveboard: edit failed in guild %s", cfg.guild_id, exc_info=True)
            return False

    try:
        msg = await channel.send(embed=embed)
    except discord.Forbidden:
        logger.warning("liveboard: missing send perms in guild %s channel %s",
                       cfg.guild_id, board.channel_id)
        return False
    except discord.HTTPException:
        logger.warning("liveboard: send failed in guild %s", cfg.guild_id, exc_info=True)
        return False
    board.message_id = msg.id
    return True


async def refresh_boards(bot: discord.Client) -> None:
    """Update every enabled live board to the current state. The board is an image
    (localized server-side) wrapped in a thin embed; build ONE embed per distinct
    guild language and reuse it, so a French server's board points at the French
    render (and stays French on every edit), not English."""
    async with _lock:
        configs = await GuildConfig.find_all().to_list()
        boards = [
            c for c in configs
            if c.live_board.enabled and c.live_board.channel_id and not c.live_board.channel_missing
        ]
        if not boards:
            return
        embeds_by_lang: dict[str, discord.Embed] = {}
        for cfg in boards:
            lang = i18n.normalize_lang(getattr(cfg, "language", None))
            if lang not in embeds_by_lang:
                i18n.set_current_language(lang)   # live_board_embed reads it for the &lang image URL
                try:
                    embeds_by_lang[lang] = discord.Embed.from_dict(await live_board_embed())
                except Exception:
                    logger.warning("liveboard: embed build failed (%s)", lang, exc_info=True)
                    continue
            embed = embeds_by_lang.get(lang)
            if embed is not None and await _sync_board(bot, cfg, embed):
                cfg.updated_at = utcnow()
                await cfg.save()
