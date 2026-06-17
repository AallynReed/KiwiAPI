"""Marketplace watch - alert a guild when a watched item hits its target price.

Per-guild config lives in ``GuildConfig.market_watch`` (one channel + ping roles +
a watchlist of items, each with an optional max price-each). The check runs when
the hourly market ingest publishes a ``market`` event: app/trove/router.py emits
it, app/bot/runner.py routes it here.

Edge-triggered per item via ``MarketWatchItem.last_alert_sig`` (the cheapest price
we last alerted for) so a steady deal isn't re-posted every refresh - but a new
low, or a price that left the deal zone and came back, does re-alert. The market
data is in Mongo (``market_listings``), which the bot process has, so no HTTP hop.
"""
import asyncio
import logging

import discord

from app import i18n

logger = logging.getLogger("kiwi.bot.market_watch")

# Serialize runs so two market events in quick succession can't double-post.
_lock = asyncio.Lock()


def match_item(summary: dict | None, max_price_each: float | None) -> str | None:
    """The alert signature when a watched item qualifies, else ``None``.

    Qualifies when it has at least one active listing and (no max set, or the
    cheapest per-unit price is at/under the max). The signature is the rounded
    cheapest price, so re-alerts fire only when that number changes."""
    if not summary or not summary.get("count"):
        return None
    cheapest = summary.get("min_each")
    if cheapest is None:
        return None
    if max_price_each is not None and float(cheapest) > float(max_price_each):
        return None
    return str(round(float(cheapest)))


async def _run(bot: discord.Client) -> None:
    from app.bot.announcer import _post
    from app.bot.models import GuildConfig
    from app.core.utils import utcnow
    from app.discord import embeds
    from app.trove.market import service as market_service

    configs = await GuildConfig.find_all().to_list()
    for cfg in configs:
        mw = getattr(cfg, "market_watch", None)
        if mw is None or not (mw.enabled and mw.channel_id and not mw.channel_missing and mw.items):
            continue
        lang = i18n.normalize_lang(getattr(cfg, "language", None))
        dirty = False
        for item in mw.items:
            try:
                summary = await market_service.item_summary(item.name)
            except Exception:
                logger.warning("market_watch: summary failed for %r", item.name, exc_info=True)
                continue
            sig = match_item(summary, item.max_price_each)
            if sig is None:
                # Left the deal zone - reset so re-entering re-alerts.
                if item.last_alert_sig is not None:
                    item.last_alert_sig = None
                    dirty = True
                continue
            if sig == item.last_alert_sig:
                continue                                  # same cheapest - already alerted
            i18n.set_current_language(lang)
            embed = discord.Embed.from_dict(
                embeds.market_watch_embed(item.name, summary, item.max_price_each))
            # MarketWatch has channel_id + ping_role_ids, so the announcer's _post
            # (which only reads those) works as-is.
            message_id = await _post(bot, cfg.guild_id, mw, embed, "market_watch")
            if message_id is not None:
                item.last_alert_sig = sig
                dirty = True
        if dirty:
            cfg.updated_at = utcnow()
            await cfg.save()


async def run_market_watch(bot: discord.Client) -> None:
    """Check every guild's watchlist and alert on newly-qualifying items."""
    async with _lock:
        await _run(bot)
