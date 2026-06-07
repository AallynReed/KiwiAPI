"""Pure tests for the leaderboards parser + the static helpers in models.

The DB-side (insert_dump / list_entries / etc.) needs Mongo and lives in
integration tests; the parser is pure regex + dataclass shaping so it tests
without any fixtures.
"""

from app.trove.leaderboards.models import (
    contest_type_for,
    is_player_board,
    reset_kind,
)
from app.trove.leaderboards.parser import parse_dump


# --- parser ----------------------------------------------------------------


def _line(name_id, category_id, uuid, name, category, entries) -> str:
    """Stitch a single source line in the bot's cfg format."""
    return f"{name_id}${category_id}${uuid} = {name}${category}##{entries}"


def _entries(*triples: tuple[int, str, float | int]) -> str:
    """Stitch the ``|``-joined per-rank entry string."""
    return "|".join(f"{r};{p};{s}" for r, p, s in triples)


def test_parse_dump_returns_one_board_per_line():
    text = "\n".join([
        _line("LB_A", "Cat_A", 100, "Board A", "Stats",
              _entries((1, "Alpha", 5000), (2, "Bravo", 4500))),
        _line("LB_B", "Cat_B", 200, "Board B", "Builds",
              _entries((1, "Charlie", 99.5))),
    ])
    boards = parse_dump(text)
    assert [b.uuid for b in boards] == [100, 200]
    assert boards[0].name == "Board A"
    assert boards[0].entries[0].player_name == "Alpha"
    assert boards[0].entries[0].rank == 1
    assert boards[0].entries[0].score == 5000.0
    assert boards[1].entries[0].score == 99.5  # float scores round-trip


def test_parse_dump_drops_favorites_category():
    text = _line("LB_F", "Cat_Fav", 9, "Faves", "FAVORITES",
                 _entries((1, "Alpha", 1)))
    assert parse_dump(text) == []
    # Case-insensitive — the source enforces .upper() match.
    text = _line("LB_F", "Cat_Fav", 9, "Faves", "Favorites",
                 _entries((1, "Alpha", 1)))
    assert parse_dump(text) == []


def test_parse_dump_dedupes_by_uuid():
    text = "\n".join([
        _line("LB_A", "Cat_A", 100, "Board A", "Stats", _entries((1, "Alpha", 5000))),
        _line("LB_A", "Cat_A", 100, "Board A v2", "Stats", _entries((1, "Beta", 9999))),
    ])
    boards = parse_dump(text)
    assert len(boards) == 1
    # First sighting wins.
    assert boards[0].name == "Board A"
    assert boards[0].entries[0].player_name == "Alpha"


def test_parse_dump_skips_malformed_entries_keeps_good_ones():
    bad_entries = "|".join([
        "1;Alpha;5000",        # ok
        "bogus",               # no semicolons — skipped
        "2;Bravo;",            # empty score — skipped (float() fails)
        "3;Charlie;3000",      # ok
        ";;;",                 # garbage — skipped
        "4;Delta;notanumber",  # non-numeric score — skipped
        "5;Echo;1500",         # ok
    ])
    text = _line("LB_A", "Cat_A", 100, "Board A", "Stats", bad_entries)
    boards = parse_dump(text)
    assert [(e.rank, e.player_name) for e in boards[0].entries] == [
        (1, "Alpha"), (3, "Charlie"), (5, "Echo"),
    ]


def test_parse_dump_ignores_unrelated_lines():
    # Real dumps have other key=value lines / comments mixed in. The parser
    # only matches the board shape and ignores everything else.
    text = "\n".join([
        "# header comment",
        "done = true",
        "some.other.setting = 42",
        _line("LB_A", "Cat_A", 100, "Board A", "Stats", _entries((1, "Alpha", 5000))),
        "trailing.junk = 1",
    ])
    boards = parse_dump(text)
    assert len(boards) == 1 and boards[0].uuid == 100


def test_parse_dump_returns_empty_for_empty_text():
    assert parse_dump("") == []
    assert parse_dump("\n\n\n") == []


def test_parse_dump_handles_score_zero_and_negative_zero():
    text = _line("LB_A", "Cat_A", 100, "Board A", "Stats",
                 _entries((1, "Alpha", 0), (2, "Bravo", 0.0)))
    boards = parse_dump(text)
    assert [e.score for e in boards[0].entries] == [0.0, 0.0]


# --- model helpers ---------------------------------------------------------


def test_reset_kind_lookup():
    # Daily set
    assert reset_kind(32000) == "daily"
    # Weekly set (sample from the delves range)
    assert reset_kind(2001) == "weekly"
    assert reset_kind(5000) == "weekly"
    # Unknown
    assert reset_kind(999999) == "default"


def test_is_player_board():
    assert is_player_board(1) is True
    # Known server-tally boards
    assert is_player_board(1100) is False
    assert is_player_board(21012) is False


def test_contest_type_for():
    assert contest_type_for("Leaderboard_Category_Contests_Daily") == "daily"
    assert contest_type_for("Leaderboard_Category_Contests") == "weekly"
    assert contest_type_for("Leaderboard_Category_Builds") is None
    assert contest_type_for("") is None


# --- archive cutoff helper -------------------------------------------------
# Drives the X-RateLimit-Archive-* throttle on anchors older than the
# configured threshold. The exact day count is a config knob — these tests
# pin the relative-to-now semantics, not the threshold value.


def test_is_archive_query_uses_configured_threshold():
    from datetime import UTC, datetime, timedelta

    from app.core.config import settings
    from app.trove.leaderboards.service import (
        archive_query_cutoff,
        is_archive_query,
    )

    days = settings.leaderboards_archive_query_threshold_days
    now = int(datetime.now(UTC).timestamp())
    cutoff = archive_query_cutoff()

    # The cutoff sits roughly `days` ago (within a couple-second slop for the
    # two now() calls — once here, once inside the helper).
    expected = int((datetime.now(UTC) - timedelta(days=days)).timestamp())
    assert abs(cutoff - expected) <= 2

    # A recent anchor (yesterday) is NOT an archive query.
    yesterday = now - 86400
    assert is_archive_query(yesterday) is False

    # An anchor 1 day past the threshold IS an archive query.
    past_cutoff = now - (days + 1) * 86400
    assert is_archive_query(past_cutoff) is True
