"""Tests for the yearly calendar. The strongest check is cross-validation: every
calendar entry must line up with its dedicated rotation endpoint (same anchors),
and invasion must be absent. Pure + deterministic via a fixed `now`."""

from datetime import datetime, timedelta, timezone

from app.trove import calendar, rotations, server_time

UTC = timezone.utc
NOW = datetime(2026, 6, 5, 12, 0, tzinfo=UTC)
NOW_TS = int(NOW.timestamp())


def test_window_and_structure():
    cal = calendar.yearly_calendar(NOW)
    assert cal["starts_at"] == int((NOW - timedelta(days=365)).timestamp())
    assert cal["ends_at"] == int((NOW + timedelta(days=365)).timestamp())
    assert cal["generated_at"] == NOW_TS
    assert cal["count"] == len(cal["events"])
    # flat list, sorted by start, every entry inside the window
    starts = [e["starts_at"] for e in cal["events"]]
    assert starts == sorted(starts)
    assert all(e["ends_at"] > cal["starts_at"] and e["starts_at"] < cal["ends_at"] for e in cal["events"])


def test_all_expected_types_present_and_no_invasion():
    types = {e["type"] for e in calendar.yearly_calendar(NOW)["events"]}
    assert {"weekly_buff", "corruxion", "fluxion",
            "gardening_2", "gardening_3", "stampy", "mana"} <= types
    assert "invasion" not in types  # invasion is excluded project-wide


def test_weekly_buff_matches_endpoint():
    cal = calendar.yearly_calendar(NOW)
    current = server_time.weekly_buffs(NOW)["current"]
    live = [e for e in cal["events"]
            if e["type"] == "weekly_buff" and e["starts_at"] <= NOW_TS < e["ends_at"]]
    assert len(live) == 1
    assert live[0]["name"] == current["name"]


def test_corruxion_matches_endpoint():
    cal = calendar.yearly_calendar(NOW)
    corr = server_time.corruxion(NOW)
    starts = {e["starts_at"] for e in cal["events"] if e["type"] == "corruxion"}
    # the current/next Corruxion window (always within ±1y) appears in the calendar
    assert corr["starts_at"] in starts


def test_fluxion_has_both_phases():
    flux = [e for e in calendar.yearly_calendar(NOW)["events"] if e["type"] == "fluxion"]
    states = {e["state"] for e in flux}
    assert states == {"voting", "selling"}
    assert all(e["color"] for e in flux)


def test_mana_matches_endpoint():
    cal = calendar.yearly_calendar(NOW)
    cur = rotations.wild_mana(NOW)["current"]
    mana = [e for e in cal["events"] if e["type"] == "mana" and e["starts_at"] == cur["starts_at"]]
    assert len(mana) == 1
    assert [b["name"] for b in mana[0]["biomes"]] == [b["name"] for b in cur["biomes"]]
    assert all(b["icon"] for b in mana[0]["biomes"])


def test_stampy_entries_have_one_biome():
    stampy = [e for e in calendar.yearly_calendar(NOW)["events"] if e["type"] == "stampy"]
    assert stampy
    assert all(len(e["biomes"]) == 1 and e["biomes"][0]["name"] for e in stampy)
