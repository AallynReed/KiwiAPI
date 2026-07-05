"""DM delivery: a durable per-worker queue + the Discord DM itself.

Mirrors ``app/webhooks/delivery.py`` (same enqueue-from-the-bus / BRPOP-consumer
shape and the exactly-once guarantee from ``bus.publish``), except delivery opens
a DM channel via the bot token and posts there instead of POSTing a webhook URL.
Runs entirely in the API process - no gateway/bot-container involvement.

Two entry points:
  - ``enqueue(payload)`` for bus-driven events (challenge / corruxion / fluxion /
    game_update).
  - ``check_market(prices)`` for the market watchlist, called from the market
    ingest with ``{item_name: cheapest_price_each}`` for the items just updated.
"""

from __future__ import annotations

import asyncio
import logging
import time

from app.bot import discord_rest
from app.core.config import settings
from app.core.delivery_health import record_delivery
from app.core.features import DM_SUBS_FLAG, is_enabled
from app.core.queue_worker import RedisListConsumer, enqueue_or_inline
from app.core.redis import get_redis
from app.dm_subs import embeds
from app.dm_subs.models import (
    MARKET_NOTIFY_COOLDOWN,
    MAX_CONSECUTIVE_FAILURES,
    DmSubscription,
)

logger = logging.getLogger("kiwi.dmsubs")

# Bus-driven event types this feature delivers (market_watch is handled by
# check_market, not the bus).
_BUS_EVENTS = ("challenge", "corruxion", "fluxion", "game_update")


# ── enqueue (called from the event bus) ──────────────────────────────────────

async def enqueue(payload: dict) -> None:
    """Queue one bus event for DM fan-out. No-ops unless DM subs are enabled and
    the type is one we deliver."""
    await enqueue_or_inline(
        payload,
        deliverable_types=_BUS_EVENTS,
        flag=DM_SUBS_FLAG,
        queue=settings.dm_subs_queue,
        deliver=_deliver,
        get_redis=get_redis,
        is_enabled=is_enabled,
        logger=logger,
        log_name="dm-sub",
    )


# ── bus-event delivery ───────────────────────────────────────────────────────

def _matches_filters(sub: DmSubscription, event_type: str, data: dict) -> bool:
    """Whether a subscription's per-event filters admit this event."""
    if event_type == "challenge":
        wanted = (sub.filters or {}).get("challenge_types") or []
        if wanted:
            from app.trove.captures import classify_challenge
            return classify_challenge(data.get("name")) in wanted
    return True


async def _deliver(payload: dict) -> None:
    event_type = payload.get("type")
    data = payload.get("data") or {}
    body = embeds.build(event_type, data)
    if body is None:
        return
    try:
        subs = await DmSubscription.find(
            DmSubscription.active == True,                       # noqa: E712
            {"events": event_type},
        ).to_list()
    except Exception:
        logger.warning("dm-sub lookup failed (%s)", event_type, exc_info=True)
        return
    targets = [s for s in subs if _matches_filters(s, event_type, data)]
    if not targets:
        return
    await asyncio.gather(*(_deliver_one(s, body) for s in targets), return_exceptions=True)


async def _deliver_one(sub: DmSubscription, body: dict) -> bool:
    ok, status, error = await discord_rest.send_dm(sub.owner_discord_id, body)
    _record(sub, ok, status, error)
    try:
        await sub.save()
    except Exception:
        logger.warning("dm-sub bookkeeping save failed", exc_info=True)
    return ok


def _record(sub: DmSubscription, ok: bool, status: int | None, error: str | None) -> None:
    # 403 = user doesn't share a server / blocks DMs; won't fix itself - disable.
    record_delivery(
        sub, ok, status, error,
        permanent_statuses=(403,),
        permanent_reason="The bot can't DM you (share a server with it and allow DMs).",
        auto_disable_reason="Auto-disabled after {failures} failed DMs.",
        max_failures=MAX_CONSECUTIVE_FAILURES,
    )


# ── market watchlist delivery ────────────────────────────────────────────────

async def check_market(prices: dict[str, float]) -> None:
    """Given ``{item_name: cheapest_price_each}`` for items just ingested, DM any
    subscription whose watchlist threshold is now met (with a per-item cooldown so
    a standing deal doesn't re-fire every hour)."""
    if not prices:
        return
    if not await is_enabled(DM_SUBS_FLAG):
        return
    try:
        subs = await DmSubscription.find(
            DmSubscription.active == True,                       # noqa: E712
            {"events": "market_watch"},
        ).to_list()
    except Exception:
        logger.warning("dm-sub market lookup failed", exc_info=True)
        return
    now = int(time.time())
    for sub in subs:
        watch = (sub.filters or {}).get("watch") or []
        fired = False
        for w in watch:
            name = w.get("name")
            thr = w.get("max_price_each")
            if not name or thr is None or name not in prices:
                continue
            price = prices[name]
            if price > thr:
                continue
            last = (sub.watch_state or {}).get(name, 0)
            if now - last < MARKET_NOTIFY_COOLDOWN:
                continue
            body = embeds.build("market_watch",
                                {"name": name, "price_each": price, "max_price_each": thr})
            if body is None:
                continue
            ok, status, error = await discord_rest.send_dm(sub.owner_discord_id, body)
            _record(sub, ok, status, error)
            if ok:
                sub.watch_state[name] = now
                fired = True
            elif status == 403 or not sub.active:
                break            # disabled - stop hitting this user
        if fired or sub.consecutive_failures or not sub.active:
            try:
                await sub.save()
            except Exception:
                logger.warning("dm-sub market save failed", exc_info=True)


# ── per-worker consumer loop ─────────────────────────────────────────────────

_worker = RedisListConsumer(
    get_redis=get_redis,
    queue=settings.dm_subs_queue,
    handler=_deliver,
    logger=logger,
    log_name="dm-sub",
)


def start_dm_delivery() -> None:
    _worker.start()


async def stop_dm_delivery() -> None:
    await _worker.stop()
