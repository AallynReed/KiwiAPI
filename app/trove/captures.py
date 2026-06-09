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

from app.core.utils import utcnow
from app.trove import server_time
from app.trove.models import ChallengeCapture, ChaosChestCapture

logger = logging.getLogger("kiwi.trove.captures")


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
    query = ChaosChestCapture.find_all()
    total = await query.count()
    docs = await query.sort("-week_anchor").skip(offset).limit(limit).to_list()
    return docs, total


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
    query = ChallengeCapture.find_all()
    total = await query.count()
    docs = await query.sort("-window_anchor").skip(offset).limit(limit).to_list()
    return docs, total
