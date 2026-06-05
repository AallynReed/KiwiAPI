from datetime import UTC, datetime

import pytest

from app.trove import misc

# --- Third-party software --------------------------------------------------


def test_modding_software_categories():
    data = misc.modding_software()
    keys = {c["key"] for c in data["categories"]}
    assert {"blueprints", "vfx", "ui", "sound", "textures"} <= keys
    assert data["count"] == len(data["categories"])
    blueprints = next(c for c in data["categories"] if c["key"] == "blueprints")
    tools = {t["name"] for t in blueprints["software"]}
    assert "MagicaVoxel" in tools
    magica = next(t for t in blueprints["software"] if t["name"] == "MagicaVoxel")
    assert magica["free"] is True and magica["url"].startswith("http")


# --- Time converter --------------------------------------------------------


def test_timezones_list():
    tz = misc.timezones()
    ids = {z["id"] for z in tz["items"]}
    assert "trove" in ids and "UTC" in ids and "Asia/Tokyo" in ids
    assert tz["count"] == len(tz["items"])


def test_convert_utc_wall_clock():
    out = misc.convert_time("2026-06-05T00:00:00", "UTC", None)
    # 2026-06-05T00:00:00Z
    assert out["unix"] == int(datetime(2026, 6, 5, tzinfo=UTC).timestamp())
    assert out["iso_utc"].startswith("2026-06-05T00:00:00")
    trove = next(z for z in out["zones"] if z["id"] == "trove")
    # Trove time = UTC - 11h → the previous day 13:00.
    assert trove["time"] == "13:00:00"
    assert any(d["code"] == f"<t:{out['unix']}:f>" for d in out["discord"])
    assert len(out["discord"]) == 7


def test_convert_trove_wall_clock_adds_offset():
    # A wall clock entered as Trove time is 11h ahead of the same UTC wall clock.
    trove = misc.convert_time("2026-06-05T00:00:00", "trove", None)
    utc = misc.convert_time("2026-06-05T00:00:00", "UTC", None)
    assert trove["unix"] == utc["unix"] + 11 * 3600


def test_convert_iana_timezone():
    # 12:00 in Tokyo (UTC+9) is 03:00 UTC.
    out = misc.convert_time("2026-06-05T12:00:00", "Asia/Tokyo", None)
    assert out["iso_utc"].startswith("2026-06-05T03:00:00")


def test_convert_explicit_unix_ignores_timezone():
    out = misc.convert_time(None, "UTC", 1_717_596_600)
    assert out["unix"] == 1_717_596_600


def test_convert_rejects_bad_input():
    with pytest.raises(misc.MiscError):
        misc.convert_time(None, "UTC", None)  # neither datetime nor unix
    with pytest.raises(misc.MiscError):
        misc.convert_time("2026-06-05T00:00", "Not/AZone", None)
    with pytest.raises(misc.MiscError):
        misc.convert_time("not-a-date", "UTC", None)


def test_time_now_renders_all_zones():
    fixed = datetime(2026, 6, 5, 12, 0, tzinfo=UTC)
    out = misc.time_now(now=fixed)
    assert out["unix"] == int(fixed.timestamp())
    assert len(out["zones"]) == len(misc.TIMEZONES)
    utc_zone = next(z for z in out["zones"] if z["id"] == "UTC")
    assert utc_zone["time"] == "12:00:00"
