"""Parser for the raw LeaderBot.cfg dump the bot POSTs.

The bot writes a config block per board:

    <name_id>$<category_id>$<uuid> = <name>$<category>##<entries>

where ``<entries>`` is a ``|``-separated list of ``rank;player;score`` triples.
A full dump is a sequence of those lines, plus comments / unrelated key=value
lines we ignore.

This module is pure (no I/O, no DB) so it can be unit-tested cheaply.
"""

from __future__ import annotations

import logging
import re
from typing import NamedTuple

logger = logging.getLogger("kiwi.trove.leaderboards.parser")

# Captures: name_id$category_id$uuid = name$category##entries
# Greedy bits restricted so ``$`` and ``##`` aren't swallowed across fields.
_BOARD_RE = re.compile(
    r"^(?P<name_id>[^\$\r\n]+)\$"
    r"(?P<category_id>[^\$\r\n]+)\$"
    r"(?P<uuid>\d+) = "
    r"(?P<name>[^\$\r\n]+)\$"
    r"(?P<category>[^#\r\n]+)##"
    r"(?P<entries>.+)$",
    re.MULTILINE,
)

# rank;player_name;score. Rank is any integer: the old source script capped it
# at 4 digits, but boards now run to ~20k entries, so a 4-digit limit silently
# dropped every rank >= 10000 (the whole reason a 20k board only stored ~5-10k).
# Player names and scores allow any non-``;`` chars (display names can include
# unicode + spaces; scores can be ints or floats).
_ENTRY_RE = re.compile(r"^(?P<rank>\d+);(?P<player>[^;]+);(?P<score>[^;]+)$")


# Source-side category ids that mark a CONTEST overlay rather than a real
# category. When a leaderboard is a contest this week the bot dumps it TWICE -
# once under its real category and once under one of these, with IDENTICAL
# standings. "Contests" is NOT a category; it's a per-week rotation of what gets
# awarded. We keep the real category and fold the overlay into a per-dump
# ``contest`` flag. (This is purely the contest LABEL - a board's reset cadence
# is admin-controlled via reset_kind_override, never inferred from here.)
_CONTEST_CATEGORY_IDS = {
    "Leaderboard_Category_Contests_Daily": "daily",
    "Leaderboard_Category_Contests": "weekly",
}


def contest_type_for(category_id: str) -> str | None:
    """Contest kind ("weekly"/"daily") for a source category id, else None."""
    return _CONTEST_CATEGORY_IDS.get(category_id)


class ParsedBoard(NamedTuple):
    name_id: str
    category_id: str
    uuid: int
    name: str
    category: str
    entries: list["ParsedEntry"]
    # This dump's contest window for the board: "weekly" / "daily" / None.
    # Inferred from the contest overlay line (above); shown as a UI marker.
    contest: str | None = None


class ParsedEntry(NamedTuple):
    rank: int
    player_name: str
    score: float


def _parse_entries(raw: str) -> list[ParsedEntry]:
    """Split the ``|``-joined entry string and parse each ``rank;player;score``
    triple. Bad rows are dropped (don't fail the whole board on a single weird
    line).

    Uses ``str.split(';')`` rather than a regex match per entry - on a ~20k-row
    board (and ~730k rows across a full dump) the split is meaningfully cheaper
    and the format is rigid: rank is digits, player has no ``;`` (so exactly two
    separators), score is an int/float. Equivalent output to the old ``_ENTRY_RE``
    on real dumps (verified byte-identical)."""
    out: list[ParsedEntry] = []
    append = out.append
    for chunk in raw.split("|"):
        chunk = chunk.strip()
        if not chunk:
            continue
        parts = chunk.split(";")
        if len(parts) != 3:
            continue
        rank_s, player, score_s = parts
        if not rank_s.isdigit():
            continue
        try:
            score = float(score_s)
        except ValueError:
            continue
        append(ParsedEntry(rank=int(rank_s), player_name=player, score=score))
    return out


def parse_dump(text: str) -> list[ParsedBoard]:
    """Parse a full LeaderBot.cfg dump into a list of ``ParsedBoard``.

    A board is emitted ONCE per uuid using its REAL category. When a board is a
    contest this week the bot dumps it twice - under the real category AND under
    a contest overlay (``Leaderboard_Category_Contests`` / ``_Daily``) with
    identical standings; we keep the real line and fold the overlay into the
    board's ``contest`` flag, so "Contests" is never stored as a category.
    Boards under the ``FAVORITES`` UI grouping are dropped.
    """
    # We can't decide a board until we've seen all its lines, because the real
    # line and the contest overlay can appear in either order.
    real: dict[int, "re.Match[str]"] = {}   # uuid -> real-category line (first wins)
    contest: dict[int, str] = {}            # uuid -> "weekly" | "daily"
    order: list[int] = []                   # uuids in first-seen order
    for m in _BOARD_RE.finditer(text):
        if m["category"].strip().upper() == "FAVORITES":
            continue
        try:
            uuid = int(m["uuid"])
        except ValueError:
            continue
        if uuid not in order:
            order.append(uuid)
        ctype = contest_type_for(m["category_id"])
        if ctype is not None:
            contest.setdefault(uuid, ctype)   # overlay -> flag only, not a board
        elif uuid not in real:
            real[uuid] = m                    # real-category line; first wins

    boards: list[ParsedBoard] = []
    for uuid in order:
        m = real.get(uuid)
        if m is None:
            # Contest overlay with no real-category counterpart - shouldn't
            # happen (every contest overlays a real board). Drop it rather than
            # invent a "Contests" category, and log so it surfaces if it does.
            logger.warning(
                "leaderboards: contest-only board uuid=%d dropped (no real category line)",
                uuid,
            )
            continue
        boards.append(ParsedBoard(
            name_id=m["name_id"],
            category_id=m["category_id"],
            uuid=uuid,
            name=m["name"],
            category=m["category"].strip(),
            entries=_parse_entries(m["entries"]),
            contest=contest.get(uuid),
        ))
    return boards
