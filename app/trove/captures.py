"""Insert + read helpers for the bot-captured rotation data.

Two rotations, same shape:

- **Chaos chest** - one captured item per weekly window (Tue 11:00 UTC anchor).
  Used by ``chaos.get_chaos_chest`` as the primary source; the Trovesaurus
  relay stays as a fallback when the bot hasn't run yet for the current week.

- **Hourly challenge** - one captured name per 20-minute active window. Two
  cadences: hourly on most days, half-hourly on trove Fridays. See
  ``server_time.challenge_window`` for the window-anchor logic.

Inserts are anchor-keyed upserts: re-submitting the same dump on the same
window converges instead of duplicating. The bot just sends ``{name}``; the
server computes the anchor from real-UTC now.
"""

from __future__ import annotations

import logging
from datetime import datetime

from pymongo.errors import DuplicateKeyError

from app.core.pagination import paginate
from app.core.utils import utcnow
from app.trove import server_time
from app.trove.models import ChallengeCapture, ChaosChestCapture, LuxionAppearance

logger = logging.getLogger("kiwi.trove.captures")

# A Luxion run ends at the daily reset LUXION_RUN_DAYS after the day it started
# on (run length + the 27h cycle grid the windows sit on live in app.trove.luxion).
# A sighting inside a live run refreshes it; a sighting after the run has elapsed
# is a brand-new appearance (the next ~4-week cycle).


# --- Chaos chest -----------------------------------------------------------


async def insert_chaos_chest(
    name: str, *, now: datetime | None = None,
) -> tuple[ChaosChestCapture, bool]:
    """Persist one chaos-chest capture for the current weekly window. Upsert
    by ``week_anchor`` - re-submitting the same week replaces the row.

    Returns ``(doc, was_new)`` so the caller can tell first-sighting from
    re-submit without an extra query."""
    name = name.strip()
    if not name:
        raise ValueError("name is empty")
    window = server_time.chaos_chest_window(now)
    week_anchor = window["starts_at"]
    captured_at = utcnow()

    doc = await ChaosChestCapture.find_one(ChaosChestCapture.week_anchor == week_anchor)
    if doc is None:
        doc = await ChaosChestCapture(
            week_anchor=week_anchor, name=name, captured_at=captured_at,
        ).insert()
        logger.info("chaos chest captured: week=%d name=%r (new)", week_anchor, name)
        return doc, True
    doc.name = name
    doc.captured_at = captured_at
    await doc.save()
    logger.info("chaos chest captured: week=%d name=%r (refreshed)", week_anchor, name)
    return doc, False


async def get_chaos_chest_for_week(week_anchor: int) -> ChaosChestCapture | None:
    return await ChaosChestCapture.find_one(ChaosChestCapture.week_anchor == week_anchor)


async def list_chaos_chest_history(
    *, limit: int = 50, offset: int = 0,
) -> tuple[list[ChaosChestCapture], int]:
    return await paginate(
        ChaosChestCapture.find_all(), sort="-week_anchor", limit=limit, offset=offset,
    )


# --- Luxion merchant -------------------------------------------------------


async def insert_luxion(
    *, now: datetime | None = None,
) -> tuple[LuxionAppearance, bool]:
    """Record a Luxion sighting → the current run, creating it only on the FIRST
    signal.

    The bot re-reports every hour Luxion is up. The first sighting opens the run -
    it is a direct read of the live welcome screen, so the run is live from that
    instant (``first_seen_at``); ``started_at`` records the trove-DAY it fell on
    (00:00 = 11:00 UTC), which is what the run's END is measured from. The 3-hour
    merchant windows inside it come off the global 27h grid, not off the daily
    reset - see ``luxion.next_grid_slot``. Every later sighting *inside* the run
    just refreshes ``last_seen_at`` on the SAME row - we never start a second
    appearance while one is live. Only once the run has elapsed does a new signal
    open the next one.

    Returns ``(doc, was_new)``. The bot sends no body - the server infers the
    anchor from now."""
    from app.trove import luxion

    real = now or utcnow()
    now_ts = int(real.timestamp())
    anchor = server_time.current_daily_reset(real)

    latest = await LuxionAppearance.find_all().sort("-started_at").first_or_none()
    if latest is not None and now_ts < luxion.run_bounds(latest.started_at)[1]:
        # Still inside the live run - same appearance, just refresh last-seen.
        latest.last_seen_at = real
        await latest.save()
        logger.info("luxion sighting: run start=%d (refreshed)", latest.started_at)
        return latest, False

    try:
        doc = await LuxionAppearance(
            started_at=anchor, first_seen_at=real, last_seen_at=real,
        ).insert()
    except DuplicateKeyError:
        # Race: a concurrent first-signal already opened this run's row (the
        # unique index on started_at is the hard backstop that keeps us from ever
        # starting two appearances for the same day). Fold this one into it.
        existing = await LuxionAppearance.find_one(
            LuxionAppearance.started_at == anchor
        )
        if existing is not None:
            existing.last_seen_at = real
            await existing.save()
            logger.info("luxion sighting: run start=%d (raced, refreshed)", anchor)
            return existing, False
        raise
    logger.info("luxion sighting: run start=%d (new appearance)", anchor)
    return doc, True


async def get_latest_luxion() -> LuxionAppearance | None:
    """The most recent Luxion appearance (regardless of whether it's still live)."""
    return await LuxionAppearance.find_all().sort("-started_at").first_or_none()


async def list_luxion_history(
    *, limit: int = 50, offset: int = 0,
) -> tuple[list[LuxionAppearance], int]:
    return await paginate(
        LuxionAppearance.find_all(), sort="-started_at", limit=limit, offset=offset,
    )


async def list_luxion_starts() -> list[int]:
    """Every captured Luxion run-start anchor (unix seconds), newest first. Used to
    place recorded appearances on the yearly calendar (they can't be computed).
    Appearances are sparse (~one per 4 weeks), so fetching all is cheap."""
    docs = await LuxionAppearance.find_all().sort("-started_at").to_list()
    return [d.started_at for d in docs]


# --- Hourly challenge ------------------------------------------------------


# Every captured challenge falls into exactly one of five categories. The
# in-game name (verbatim from QuestLog.cfg) disambiguates them - these
# comparison strings are LITERAL and case-sensitive, exactly as the old
# RenewedTroveToolsAPI matched them. Changing any one of them breaks every
# existing consumer that relied on `challenge_type` from that API.
#
#   - ``"Collection Challenge"`` (literal) - chaos chest contributor
#   - ``"RAMPAGE ALERT!"``       (literal) - boss-rush event. The game DOES
#                                            ship the exclamation mark and
#                                            the all-caps "ALERT!" suffix;
#                                            matching plain "Rampage" misses
#                                            every real capture.
#   - ``"Racing Challenge"``     (literal) - racetrack rotation. The old API
#                                            matched the all-caps ``"RACING"``
#                                            instead, but the game actually
#                                            ships the title-case "X Challenge"
#                                            form, so the old enum value never
#                                            fired in practice (every real
#                                            racing capture silently became
#                                            "dungeon"). Fixed here.
#   - ``"Target Challenge"``     (literal) - target-shooting event. NOT in
#                                            the old API enum - added here
#                                            because the game ships it and
#                                            otherwise it'd silently bucket
#                                            into "dungeon" (wrong).
#   - anything else                        - biome-themed Dungeon Challenge
#                                            (``"Cursed Vale"``, ``"Permafrost"``, …),
#                                            INCLUDING the literal string
#                                            ``"DUNGEON"`` that the old API
#                                            also matched explicitly - the
#                                            fallthrough handles it the same way.
#
# The biome list drifts between Trove patches and Trovesaurus doesn't expose
# a canonical enum, so we fold "everything-not-special" into the dungeon
# bucket. Keeps the rule fork-proof against future biome additions.
def classify_challenge(name: str | None) -> str | None:
    """Return ``"collection" / "rampage" / "racing" / "target" / "dungeon"``
    for a captured name, or ``None`` when there's no name to classify
    (gap-window response)."""
    if name is None:
        return None
    n = name.strip()
    if not n:
        return None
    if n == "Collection Challenge":
        return "collection"
    if n == "RAMPAGE ALERT!":
        return "rampage"
    if n == "Racing Challenge":
        return "racing"
    if n == "Target Challenge":
        return "target"
    return "dungeon"


async def insert_challenge(
    name: str, *, now: datetime | None = None,
) -> tuple[ChallengeCapture, bool]:
    """Persist one challenge capture for the active 20-minute window. Upsert
    by ``window_anchor``. Returns ``(doc, was_new)``.

    No "no challenge" sentinel is stored - the bot only POSTs when it captured
    a real name (the OLD bot dropped ``challenge == "none"``); empty names
    raise here so a misfire surfaces instead of polluting history."""
    name = name.strip()
    if not name or name.lower() == "none":
        raise ValueError("challenge name is empty or 'none'")
    window = server_time.challenge_window(now)
    anchor = window["starts_at"]
    captured_at = utcnow()

    doc = await ChallengeCapture.find_one(ChallengeCapture.window_anchor == anchor)
    if doc is None:
        doc = await ChallengeCapture(
            window_anchor=anchor,
            window_ends_at=window["ends_at"],
            name=name,
            is_friday_window=window["is_friday_window"],
            captured_at=captured_at,
        ).insert()
        logger.info("challenge captured: window=%d name=%r (new)", anchor, name)
        return doc, True
    doc.name = name
    doc.captured_at = captured_at
    doc.window_ends_at = window["ends_at"]
    doc.is_friday_window = window["is_friday_window"]
    await doc.save()
    logger.info("challenge captured: window=%d name=%r (refreshed)", anchor, name)
    return doc, False


async def get_current_challenge() -> dict:
    """The active window + the captured name for it (if any).

    Always returns the window shape (start/end/active/is_friday/seconds_remaining);
    ``name`` and ``captured_at`` are populated only when the bot has captured
    this window. During a gap between challenges ``active`` is false but the
    most-recent window is still reported."""
    window = server_time.challenge_window()
    doc = await ChallengeCapture.find_one(
        ChallengeCapture.window_anchor == window["starts_at"]
    )
    name = doc.name if doc else None
    return {
        **window,
        "name": name,
        "type": classify_challenge(name),
        "captured_at": doc.captured_at if doc else None,
    }


async def list_challenge_history(
    *, limit: int = 50, offset: int = 0,
) -> tuple[list[ChallengeCapture], int]:
    return await paginate(
        ChallengeCapture.find_all(), sort="-window_anchor", limit=limit, offset=offset,
    )
