"""Pure helpers for the leaderboards scope.

The leaderboards data store is PostgreSQL (see ``pg_store.py`` / ``pg_schema.py``):
``board`` / ``board_contest`` / ``player`` / ``entry`` (partitioned by anchor) /
``activity_estimate``. This module no longer defines any Mongo documents - only
the cadence/board-classification helpers that the parser, service, detection and
admin layers share. ``reset_kind`` resolution from a per-board override now lives
in ``pg_store._effective_reset_kind`` (operates on a plain override string).

``anchor`` everywhere is a unix-seconds int (the dump's "as-of" Trove-time
anchor), NOT a datetime - reads filter by exact equality on the day's reset.
"""


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
    """Return one of ``"daily"`` / ``"weekly"`` / ``"default"`` for a board uuid.

    This is the HARDCODED FALLBACK only - production code should prefer the
    per-board override (``board.reset_kind_override`` in PG, resolved by
    ``pg_store._effective_reset_kind``) which the admin panel can edit at runtime.
    """
    if uuid in _DAILY_RESET_UUIDS:
        return "daily"
    if uuid in _WEEKLY_RESET_UUIDS:
        return "weekly"
    return "default"


# Reset cadences valid in the per-board override (stored on ``board`` in PG).
# Resolution against the hardcoded fallback above is done by
# ``pg_store._effective_reset_kind(override, uuid)``.
RESET_KIND_VALUES = ("daily", "weekly", "none")


def is_lifetime_kind(rk: str) -> bool:
    """True for cadences that accumulate forever rather than resetting.

    ``"default"`` is the implicit lifetime tag (any board not in the
    hardcoded daily/weekly sets); ``"none"`` is the explicit one the
    admin sets via the override. Cheater detection on these boards
    skips score-outlier + rank-gap because rank-1 will always look
    like an outlier on a 5-year-old stat - only velocity (score change
    over time) is a valid signal.
    """
    return rk in ("default", "none")


# Boards that aren't a leaderboard of PLAYERS (e.g. server-level tallies).
# Kept as a set so future additions are one-line.
_NON_PLAYER_BOARDS = {1100, 21012}


def is_player_board(uuid: int) -> bool:
    return uuid not in _NON_PLAYER_BOARDS


# Contest-overlay detection (Leaderboard_Category_Contests[_Daily]) lives in
# parser.py's ``contest_type_for``; the parser folds it into ParsedBoard.contest
# so a board keeps its real category and carries a per-dump contest flag.
