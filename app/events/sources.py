"""Live event sources - the catalog of things published to the Redis events
channel (``settings.events_channel``). Both the API's SSE endpoint
(``app/events/router.py``) and the Discord bot (``app/bot/runner.py``) subscribe
and react to the same events.

Two kinds of source:

- **insert-driven** (``challenge``, ``chaos``): published by the ingest endpoints
  the moment a capture lands (``app/trove/router.py``). ``next_at`` is ``None`` -
  nothing to schedule; the insert is the trigger.
- **time-driven** (``corruxion``, ``fluxion``, ``longshade``, ``wild_mana``,
  ``stampy``, ``daily_bonuses``): published by the scheduler
  (``app/events/scheduler.py``), which sleeps until ``next_at`` (the next
  occurrence boundary) instead of polling, then publishes the rolled-over state.

Each source exposes:
- ``data_fn()``   -> the SSE payload dict (the same shape the matching endpoint
  serves). May be sync or async.
- ``sig_fn(data)``-> the exactly-once signature: a string that changes when there's
  something new, or ``None`` when there's nothing to announce (gap window, merchant
  away, no capture yet). ``publish`` only emits when it changes, so running the
  scheduler in every API worker is safe.
- ``next_at_fn()``-> unix seconds of the next boundary (time-driven), or ``None``.

Lazy imports keep this module import-light and cycle-free.
"""
from __future__ import annotations

import time
from collections.abc import Awaitable, Callable
from dataclasses import dataclass


@dataclass(frozen=True)
class EventSource:
    type: str
    data_fn: Callable[[], dict | Awaitable[dict]]
    sig_fn: Callable[[dict], str | None]
    next_at_fn: Callable[[], int | None] | None   # None => insert-driven


# ── insert-driven: hourly challenge + chaos chest ───────────────────────────

async def _challenge_data() -> dict:
    from app.trove.captures import get_current_challenge
    return await get_current_challenge()


def _challenge_sig(d: dict) -> str | None:
    return f"{d.get('starts_at')}:{d.get('name')}" if d.get("name") else None


async def _chaos_data() -> dict:
    from app.trove.chaos import get_chaos_chest
    return await get_chaos_chest()


def _chaos_sig(d: dict) -> str | None:
    item = d.get("item") or {}
    return f"{d.get('starts_at')}:{item.get('name')}" if item.get("name") else None


# ── time-driven: merchants ──────────────────────────────────────────────────

def _corruxion_data() -> dict:
    from app.trove.server_time import corruxion
    return corruxion()


def _fluxion_data() -> dict:
    from app.trove.server_time import fluxion
    return fluxion()


def _merchant_sig(d: dict) -> str | None:
    # Announce only while the merchant is here; the gap publishes data (for SSE)
    # but no signature, so the bot doesn't post.
    return str(d["starts_at"]) if d.get("active") else None


def _merchant_next(d: dict) -> int:
    # Active -> next boundary is the departure; away -> the next arrival.
    return d["ends_at"] if d.get("active") else d["starts_at"]


def _corruxion_next() -> int:
    return _merchant_next(_corruxion_data())


def _fluxion_next() -> int:
    return _merchant_next(_fluxion_data())


# ── time-driven: biome rotations ────────────────────────────────────────────

def _longshade_data() -> dict:
    from app.trove.rotations import biome_rotation
    return biome_rotation()


def _wild_mana_data() -> dict:
    from app.trove.rotations import wild_mana
    return wild_mana()


def _rotation_sig(d: dict) -> str | None:
    cur = d.get("current") or {}
    return str(cur["starts_at"]) if cur.get("starts_at") else None


def _rotation_next(d: dict) -> int | None:
    return (d.get("current") or {}).get("ends_at")


def _longshade_next() -> int | None:
    return _rotation_next(_longshade_data())


def _wild_mana_next() -> int | None:
    return _rotation_next(_wild_mana_data())


# ── time-driven: stampy (48h event with gaps) ───────────────────────────────

def _stampy_data() -> dict:
    from app.trove.rotations import stampy
    return stampy()


def _stampy_sig(d: dict) -> str | None:
    cur = d.get("current")
    now = int(time.time())
    return str(cur["starts_at"]) if cur and cur["starts_at"] <= now < cur["ends_at"] else None


def _stampy_next() -> int:
    cur = _stampy_data().get("current")
    if not cur:
        return int(time.time()) + 3600        # nothing scheduled - recheck in an hour
    now = int(time.time())
    return cur["starts_at"] if now < cur["starts_at"] else cur["ends_at"]


# ── time-driven: daily bonuses (fires each daily reset) ─────────────────────

def _daily_data() -> dict:
    from app.trove.server_time import daily_buffs
    return daily_buffs()


def _daily_sig(_d: dict) -> str | None:
    from app.trove.server_time import DAY, server_time
    return str(server_time()["daily_reset_at"] - DAY)


def _daily_next() -> int:
    from app.trove.server_time import server_time
    return server_time()["daily_reset_at"]


# ── time-driven: player-activity daily snapshot ─────────────────────────────
# Light payload (just the daily window) - the bot rebuilds the full estimate when
# it posts, so we don't run the heavy aggregation on every SSE connect.

def _activity_data() -> dict:
    from app.trove.server_time import DAY, server_time
    return {"window_start": server_time()["daily_reset_at"] - DAY}


def _activity_sig(d: dict) -> str | None:
    return str(d["window_start"])


def _activity_next() -> int:
    from app.trove.server_time import server_time
    return server_time()["daily_reset_at"]


# ── state-change driven (published by their producers, not scheduled) ───────

def _status_data() -> dict:
    from app.trove.status import get_status
    return get_status()


def _status_sig(d: dict) -> str | None:
    overall = d.get("overall", "unknown")
    return None if overall == "unknown" else f"status:{overall}"


async def _news_data() -> dict:
    from app.trove.news import latest_news
    items = await latest_news(1)
    if not items:
        return {"item": None}
    top = items[0]
    return {"item": {
        "title": top.title,
        "url": top.url,
        "published_at": top.published_at.isoformat() if top.published_at else None,
    }}


def _news_sig(d: dict) -> str | None:
    item = d.get("item")
    return item["url"] if item and item.get("url") else None


async def _giveaways_data() -> dict:
    from app.giveaways.models import Giveaway, GiveawayStatus
    newest = (
        await Giveaway.find(Giveaway.status == GiveawayStatus.open)
        .sort("-created_at").limit(1).to_list()
    )
    if not newest:
        return {"newest": None}
    g = newest[0]
    return {"newest": {"id": str(g.id), "title": g.title,
                       "created_at": int(g.created_at.timestamp())}}


def _giveaways_sig(d: dict) -> str | None:
    n = d.get("newest")
    return str(n["created_at"]) if n else None


# ── state-change driven: game updates (a new build is mirrored) ─────────────
# Scoped to the live (US) branch - the public "patch is live" signal. Published
# by the archiver when it finishes a version (app/trove/updates/repo.py); PTS
# builds don't move this source's signature, so they stay quiet here.

_GAME_UPDATE_BRANCH = "live-us"


async def _game_update_data() -> dict:
    from app.trove.updates import read as updates_read
    v = await updates_read.latest_version(_GAME_UPDATE_BRANCH)
    if v is None:
        return {"version": None}
    return {"version": {
        "branch": v.branch, "ordinal": v.ordinal, "version_tag": v.version_tag,
        "files_added": v.files_added, "files_modified": v.files_modified,
        "files_removed": v.files_removed, "bytes_added": v.bytes_added,
        "completed_at": v.completed_at.isoformat() if v.completed_at else None,
    }}


def _game_update_sig(d: dict) -> str | None:
    v = d.get("version")
    return f"{v['branch']}:{v['ordinal']}" if v else None


# ── registry ────────────────────────────────────────────────────────────────

SOURCES: tuple[EventSource, ...] = (
    EventSource("challenge", _challenge_data, _challenge_sig, None),
    EventSource("chaos", _chaos_data, _chaos_sig, None),
    EventSource("corruxion", _corruxion_data, _merchant_sig, _corruxion_next),
    EventSource("fluxion", _fluxion_data, _merchant_sig, _fluxion_next),
    EventSource("longshade", _longshade_data, _rotation_sig, _longshade_next),
    EventSource("wild_mana", _wild_mana_data, _rotation_sig, _wild_mana_next),
    EventSource("stampy", _stampy_data, _stampy_sig, _stampy_next),
    EventSource("daily_bonuses", _daily_data, _daily_sig, _daily_next),
    EventSource("activity", _activity_data, _activity_sig, _activity_next),
    # state-change driven (next_at None -> published by producers, see below):
    EventSource("server_status", _status_data, _status_sig, None),
    EventSource("trove_news", _news_data, _news_sig, None),
    EventSource("giveaways", _giveaways_data, _giveaways_sig, None),
    EventSource("game_update", _game_update_data, _game_update_sig, None),
)

SOURCES_BY_TYPE: dict[str, EventSource] = {s.type: s for s in SOURCES}
SCHEDULED_SOURCES: tuple[EventSource, ...] = tuple(s for s in SOURCES if s.next_at_fn is not None)
