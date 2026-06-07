"""Parser for the raw LeaderBot.cfg dump the bot POSTs.

The bot writes a config block per board:

    <name_id>$<category_id>$<uuid> = <name>$<category>##<entries>

where ``<entries>`` is a ``|``-separated list of ``rank;player;score`` triples.
A full dump is a sequence of those lines, plus comments / unrelated key=value
lines we ignore.

This module is pure (no I/O, no DB) so it can be unit-tested cheaply.
"""

from __future__ import annotations

import re
from typing import NamedTuple

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

# rank;player_name;score  — rank caps at 4 digits like in the source script.
# Player names and scores are allowed any non-``;`` chars (display names can
# include unicode + spaces; scores can be ints or floats).
_ENTRY_RE = re.compile(r"^(?P<rank>\d{1,4});(?P<player>[^;]+);(?P<score>[^;]+)$")


class ParsedBoard(NamedTuple):
    name_id: str
    category_id: str
    uuid: int
    name: str
    category: str
    entries: list["ParsedEntry"]


class ParsedEntry(NamedTuple):
    rank: int
    player_name: str
    score: float


def _parse_entries(raw: str) -> list[ParsedEntry]:
    """Split the ``|``-joined entry string and parse each triple. Bad rows are
    dropped (don't fail the whole board on a single weird line)."""
    out: list[ParsedEntry] = []
    for chunk in raw.split("|"):
        chunk = chunk.strip()
        if not chunk:
            continue
        m = _ENTRY_RE.match(chunk)
        if not m:
            continue
        try:
            out.append(ParsedEntry(
                rank=int(m["rank"]),
                player_name=m["player"],
                score=float(m["score"]),
            ))
        except ValueError:
            continue
    return out


def parse_dump(text: str) -> list[ParsedBoard]:
    """Parse a full LeaderBot.cfg dump into a list of ``ParsedBoard``.

    De-dupes by uuid (the bot sometimes emits the same board twice — first sighting
    wins). Boards under the ``FAVORITES`` category are dropped (per the old
    ingestion: it's a UI grouping, not a real leaderboard).
    """
    boards: list[ParsedBoard] = []
    seen_uuids: set[int] = set()
    for m in _BOARD_RE.finditer(text):
        if m["category"].strip().upper() == "FAVORITES":
            continue
        try:
            uuid = int(m["uuid"])
        except ValueError:
            continue
        if uuid in seen_uuids:
            continue
        seen_uuids.add(uuid)
        boards.append(ParsedBoard(
            name_id=m["name_id"],
            category_id=m["category_id"],
            uuid=uuid,
            name=m["name"],
            category=m["category"].strip(),
            entries=_parse_entries(m["entries"]),
        ))
    return boards
