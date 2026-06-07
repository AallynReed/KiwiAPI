"""Beanie documents for the leaderboards scope.

The dump from the bot is one big text blob; we explode it into two collections:

- ``Leaderboard``       — per-board metadata (one doc per ``uuid``). Includes the
  list of contests it has appeared in (daily/weekly contest cycles).
- ``LeaderboardEntry``  — per-(board, timestamp, rank) row. The hot read path is
  "show the top-N of board X at timestamp T", so we index on
  ``(leaderboard, created_at, rank)``.

``created_at`` is a unix-seconds int (the dump's "as-of" Trove-time anchor),
NOT a datetime — every read filters by exact equality on the day's reset, so
ints are simpler than UTC datetime round-trips.
"""

from beanie import Document
from pydantic import Field
from pymongo import ASCENDING, DESCENDING, IndexModel


# ---------------------------------------------------------------------------
# Static lookups copied from the old API so old behavior holds: which board
# uuids reset DAILY vs WEEKLY (everything else is "default"). The bot's category
# id (Leaderboard_Category_Contests / _Daily) tells us whether a given dump
# represents a CONTEST window for that board.

# Daily-reset uuids
_DAILY_RESET_UUIDS = {
    32000,  # Leviathans
}

# Weekly-reset uuids
_WEEKLY_RESET_UUIDS = {
    # Delves
    2001, 2002, 2004, 2011, 2013, 2014, 2021, 2024,
    2300, 2301, 2302, 2303, 2304, 2305, 2306, 2307, 2308, 2309, 2310, 2311, 2312, 2313, 2314, 2315, 2316, 2317,
    2400, 2401, 2402, 2403, 2404, 2405, 2406, 2407, 2408, 2409, 2410, 2411, 2412, 2413, 2414, 2415, 2416, 2417,
    # Effort
    4000, 4001, 4002, 4003, 4004, 4005, 4006, 4007, 4008, 4009, 4010, 4011, 4012, 4013, 4014, 4015, 4016, 4017,
    # Paragon
    5000, 5001, 5002, 5003, 5004, 5005, 5006, 5007, 5008, 5009, 5010, 5011, 5012, 5013, 5014, 5015, 5016, 5017,
    # Stats
    10004, 10009, 10012, 10019, 21004, 21005, 21012, 30001, 30002, 30003, 30004, 30005, 33001, 33002, 50000,
}


def reset_kind(uuid: int) -> str:
    """Return one of ``"daily"`` / ``"weekly"`` / ``"default"`` for a board uuid."""
    if uuid in _DAILY_RESET_UUIDS:
        return "daily"
    if uuid in _WEEKLY_RESET_UUIDS:
        return "weekly"
    return "default"


# Boards that aren't a leaderboard of PLAYERS (e.g. server-level tallies).
# Kept as a set so future additions are one-line.
_NON_PLAYER_BOARDS = {1100, 21012}


def is_player_board(uuid: int) -> bool:
    return uuid not in _NON_PLAYER_BOARDS


# Contest category-ids the bot emits. Anything else is a non-contest dump
# (the board is being polled but its current window isn't a Daily/Weekly contest).
_CONTEST_TYPES = {
    "Leaderboard_Category_Contests_Daily": "daily",
    "Leaderboard_Category_Contests": "weekly",
}


def contest_type_for(category_id: str) -> str | None:
    """If the source line marks this dump as a contest window, return its kind."""
    return _CONTEST_TYPES.get(category_id)


# ---------------------------------------------------------------------------


class Leaderboard(Document):
    """One Trove leaderboard's metadata. Created on first sighting, then upserted.

    ``contests`` is a small append-only list of ``{time, type}`` records — every
    timestamp where the board was dumped while flagged as a contest. The list is
    bounded by how often contests occur and is fine to keep in-document.
    """

    uuid: int                 # the game's leaderboard id — unique
    name_id: str              # source-side string id (e.g. "Leaderboard_Game_Stats")
    name: str                 # human-readable name
    category_id: str          # source-side category id
    category: str             # human-readable category
    # [{time: int (unix seconds), type: "daily" | "weekly"}, ...]
    contests: list[dict] = Field(default_factory=list)

    class Settings:
        name = "leaderboards"
        indexes = [
            IndexModel([("uuid", ASCENDING)], unique=True),
            IndexModel([("name_id", ASCENDING)]),
            IndexModel([("category_id", ASCENDING)]),
        ]


class LeaderboardEntry(Document):
    """HOT collection: one player's slot on one board at one dump.

    Holds the last ``leaderboards_hot_retention_days`` worth of entries (runtime
    tunable; default 3 days). Older rows are MOVED (not deleted) to
    ``LeaderboardEntryArchive`` at the tail of each insert, so the hot
    collection stays small enough for fast top-N reads while history is
    preserved.

    ``score`` is a float because some boards expose decimals (delve depth times,
    paragon multipliers); integer-only boards store as ``1234.0`` which JSON
    serialises identically.
    """

    player_name: str
    rank: int
    score: float
    leaderboard: int   # the board uuid this row belongs to
    created_at: int    # the dump's submit-time in unix seconds (the "as-of" anchor)

    class Settings:
        name = "leaderboard_entries"
        indexes = [
            # Hot path: top-N for a (board, timestamp), sorted by rank.
            IndexModel([("leaderboard", ASCENDING), ("created_at", ASCENDING), ("rank", ASCENDING)]),
            # Player-history lookups (cheaper than a full collection scan).
            IndexModel([("player_name", ASCENDING), ("created_at", DESCENDING)]),
            # Used by "what timestamps are available?".
            IndexModel([("created_at", DESCENDING)]),
        ]


class LeaderboardEntryArchive(Document):
    """COLD collection: same shape as ``LeaderboardEntry`` but for entries past
    the hot retention window. Read endpoints transparently fall through to here
    when a query asks for an anchor older than the hot cutoff.

    Indexes are tighter — the hot composite ``(leaderboard, created_at, rank)``
    is preserved so historical top-N reads stay O(index seek), but we drop the
    standalone ``(created_at desc)`` index (the hot version of timestamps stays
    authoritative; archive listing is union-deduped on the read side)."""

    player_name: str
    rank: int
    score: float
    leaderboard: int
    created_at: int

    class Settings:
        name = "leaderboard_entries_archive"
        indexes = [
            IndexModel([("leaderboard", ASCENDING), ("created_at", ASCENDING), ("rank", ASCENDING)]),
            IndexModel([("player_name", ASCENDING), ("created_at", DESCENDING)]),
        ]
