"""Pure tests for the Chaos Chest: the weekly window math, payload normalization,
and the cached-item + window merge. (The relay/fetch is integration-tested.)"""

from datetime import datetime, timedelta, timezone

from app.trove import chaos, server_time

UTC = timezone.utc
# FIRST_FLUXION (2023-07-18, trove frame) + 11h offset = the real-UTC weekly anchor.
_BASE = datetime(2023, 7, 18, 11, 0, tzinfo=UTC)


# --- weekly window ----------------------------------------------------------

def test_chaos_window_first_interval():
    now = datetime(2023, 7, 20, 12, 0, tzinfo=UTC)  # inside the first window
    w = server_time.chaos_chest_window(now)
    assert w["starts_at"] == int(_BASE.timestamp())
    assert w["ends_at"] == int((_BASE + timedelta(days=7)).timestamp())
    assert w["starts_at"] <= int(now.timestamp()) < w["ends_at"]


def test_chaos_window_advances_weekly():
    now = datetime(2023, 7, 26, 12, 0, tzinfo=UTC)  # the next window
    w = server_time.chaos_chest_window(now)
    assert w["starts_at"] == int((_BASE + timedelta(days=7)).timestamp())
    assert w["ends_at"] == int((_BASE + timedelta(days=14)).timestamp())


# --- payload normalization --------------------------------------------------

def test_normalize_cleans_paths_and_ints():
    n = chaos.normalize({
        "name": "Shadow Dragon",
        "identifier": "Prefabs\\Collections\\X",
        "blueprint": "Blueprints/Foo.BLUEPRINT",
        "start": "100", "end": 200,
    })
    assert n == {
        "name": "Shadow Dragon",
        "identifier": "Prefabs/Collections/X",
        "blueprint": "blueprints/foo.blueprint",
        "start": 100, "end": 200,
    }


def test_normalize_handles_wrappers_and_rejects_empty():
    assert chaos.normalize({"data": {"name": "Wrapped"}})["name"] == "Wrapped"
    assert chaos.normalize([{"name": "Listed"}])["name"] == "Listed"
    assert chaos.normalize([]) is None
    assert chaos.normalize({"name": ""}) is None     # no usable name
    assert chaos.normalize("nonsense") is None
    assert chaos.normalize({"name": "X", "start": "bad"})["start"] is None


# --- cached item + window merge ---------------------------------------------

def _now() -> datetime:
    return datetime(2023, 7, 20, 12, 0, tzinfo=UTC)


def test_build_uses_item_window_when_current():
    now = _now()
    ts = int(now.timestamp())
    cached = {"name": "Shadow Dragon", "identifier": "x", "blueprint": "y",
              "start": ts - 100, "end": ts + 100}
    out = chaos.build_response(cached, now, now)
    assert out["active"] is True
    assert out["item"] == {"name": "Shadow Dragon", "identifier": "x", "blueprint": "y"}
    assert out["starts_at"] == ts - 100 and out["ends_at"] == ts + 100
    assert out["seconds_remaining"] == 100
    assert out["fetched_at"] == now


def test_build_drops_stale_item_and_falls_back_to_computed_window():
    now = _now()
    ts = int(now.timestamp())
    cached = {"name": "Last Week", "start": ts - 1000, "end": ts - 10}  # already ended
    out = chaos.build_response(cached, now, now)
    assert out["item"] is None and out["fetched_at"] is None
    assert out["starts_at"] <= ts < out["ends_at"]   # computed window frames it
    assert out["active"] is True


def test_build_without_cache_still_returns_window():
    now = _now()
    ts = int(now.timestamp())
    out = chaos.build_response(None, None, now)
    assert out["item"] is None
    assert out["starts_at"] <= ts < out["ends_at"]
    assert out["active"] is True
