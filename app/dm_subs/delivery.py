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
import json
import logging
import time

from app.bot import discord_rest
from app.core.config import settings
from app.core.features import DM_SUBS_FLAG, is_enabled
from app.core.redis import get_redis
from app.core.utils import utcnow
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

_worker: asyncio.Task | None = None


# ── enqueue (called from the event bus) ──────────────────────────────────────

async def enqueue(payload: dict) -> None:
    """Queue one bus event for DM fan-out. No-ops unless DM subs are enabled and
    the type is one we deliver."""
    if payload.get("type") not in _BUS_EVENTS:
        return
    if not await is_enabled(DM_SUBS_FLAG):
        return
    redis = get_redis()
    if redis is None:
        asyncio.create_task(_deliver(payload))
        return
    try:
        await redis.lpush(settings.dm_subs_queue, json.dumps(payload, default=str))
    except Exception:
        logger.warning("dm-sub enqueue failed", exc_info=True)


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
    sub.last_status = status
    sub.updated_at = utcnow()
    if ok:
        sub.consecutive_failures = 0
        sub.last_error = None
        sub.last_delivered_at = utcnow()
        return
    # 403 = user doesn't share a server / blocks DMs; won't fix itself - disable.
    if status == 403:
        sub.active = False
        sub.last_error = error
        sub.disabled_reason = "The bot can't DM you (share a server with it and allow DMs)."
        return
    sub.consecutive_failures += 1
    sub.last_error = error
    if sub.consecutive_failures >= MAX_CONSECUTIVE_FAILURES:
        sub.active = False
        sub.disabled_reason = f"Auto-disabled after {sub.consecutive_failures} failed DMs."


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

async def _consume() -> None:
    redis = get_redis()
    if redis is None:
        return
    while True:
        try:
            item = await redis.brpop([settings.dm_subs_queue], timeout=5)
        except asyncio.CancelledError:
            raise
        except Exception:
            logger.warning("dm-sub BRPOP failed", exc_info=True)
            await asyncio.sleep(1)
            continue
        if item is None:
            continue
        try:
            _key, raw = item
            await _deliver(json.loads(raw))
        except asyncio.CancelledError:
            raise
        except Exception:
            logger.warning("dm-sub delivery failed", exc_info=True)


def start_dm_delivery() -> None:
    global _worker
    if _worker is None:
        _worker = asyncio.create_task(_consume())


async def stop_dm_delivery() -> None:
    global _worker
    if _worker is not None:
        _worker.cancel()
        try:
            await _worker
        except asyncio.CancelledError:
            pass
    _worker = None
