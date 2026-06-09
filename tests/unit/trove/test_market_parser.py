"""Pure tests for the market dump parser and the UUID-v1 → created_at decoder.

The DB-side (insert_dump / list_listings / etc.) lives in integration tests
since it needs Mongo; this module covers just the regex/parse logic.
"""

from datetime import UTC, datetime, timedelta
from uuid import UUID, uuid1

from app.trove.market.parser import (
    MAX_REASONABLE_PRICE,
    listing_created_at,
    parse_dump,
)


def _line(uid: UUID, name: str, type_: str, stack: int, price: int) -> str:
    return f"{uid};{name};{type_};{stack};{price}"


def test_parse_dump_returns_one_per_line():
    a, b = uuid1(), uuid1()
    text = "\n".join([
        _line(a, "Glim", "Material", 100, 5000),
        _line(b, "Faerie Dust", "", 50, 12500),
    ])
    listings = parse_dump(text)
    assert [l.name for l in listings] == ["Glim", "Faerie Dust"]
    assert listings[0].id == a and listings[1].id == b
    assert listings[0].stack == 100 and listings[0].price == 5000
    assert listings[0].price_each == 50.0
    # Empty type field maps to None
    assert listings[0].type == "Material"
    assert listings[1].type is None


def test_parse_dump_dedupes_by_uuid():
    a = uuid1()
    text = "\n".join([
        _line(a, "Glim", "Material", 100, 5000),
        _line(a, "Glim", "Material", 100, 4000),   # same uuid, lower price
    ])
    listings = parse_dump(text)
    assert len(listings) == 1
    assert listings[0].price == 5000   # first sighting wins


def test_parse_dump_drops_unreasonable_prices():
    a = uuid1()
    # Above the cap → dropped
    text = _line(a, "Glim", "Material", 1, MAX_REASONABLE_PRICE + 1)
    assert parse_dump(text) == []
    # At the cap → kept
    text = _line(a, "Glim", "Material", 1, MAX_REASONABLE_PRICE)
    assert len(parse_dump(text)) == 1


def test_parse_dump_drops_zero_or_negative_stacks_and_prices():
    a, b = uuid1(), uuid1()
    text = "\n".join([
        _line(a, "Glim", "Material", 0, 5000),    # stack=0 → drop
        _line(b, "Glim", "Material", 1, 0),       # price=0 → drop
    ])
    assert parse_dump(text) == []


def test_parse_dump_skips_invalid_uuids():
    text = "\n".join([
        "not-a-uuid;Glim;Material;100;5000",
        "00000000-0000-0000-0000-000000000000;Glim;Material;100;5000",
    ])
    # The all-zeros UUID is valid UUID-syntax-wise but its decoded "created_at"
    # is bogus (year 1582). The parser still accepts it - filtering by reasonable
    # created_at is the caller's job (e.g. hide_expired in the read endpoint).
    listings = parse_dump(text)
    assert len(listings) == 1


def test_parse_dump_handles_unicode_item_names():
    a = uuid1()
    text = _line(a, "Glímmer ✨ Test", "", 1, 100)
    listings = parse_dump(text)
    assert listings[0].name == "Glímmer ✨ Test"


def test_parse_dump_ignores_unrelated_lines():
    a = uuid1()
    text = "\n".join([
        "# header comment",
        "done = true",
        _line(a, "Glim", "Material", 100, 5000),
        "some.other = config",
    ])
    listings = parse_dump(text)
    assert len(listings) == 1 and listings[0].name == "Glim"


def test_parse_dump_price_each_rounded_to_three_decimals():
    a = uuid1()
    text = _line(a, "Glim", "", 7, 10000)   # 10000 / 7 = 1428.571428...
    listings = parse_dump(text)
    assert listings[0].price_each == 1428.571


def test_parse_dump_empty_text():
    assert parse_dump("") == []
    assert parse_dump("\n\n") == []


# --- UUID v1 decoder -------------------------------------------------------


def test_listing_created_at_recent_uuid_is_close_to_now():
    """A freshly-minted UUID v1 should decode to ≈ now()."""
    before = int(datetime.now(UTC).timestamp())
    uid = uuid1()
    after = int(datetime.now(UTC).timestamp())
    decoded = listing_created_at(uid)
    # Allow a 5-second window for slow CI clocks.
    assert before - 5 <= decoded <= after + 5


def test_listing_created_at_known_value():
    """Decoded timestamp matches an independently-computed datetime."""
    uid = uuid1()
    expected_dt = (
        datetime(1582, 10, 15, tzinfo=UTC) + timedelta(microseconds=uid.time / 10)
    ).replace(microsecond=0)
    assert listing_created_at(uid) == int(expected_dt.timestamp())
