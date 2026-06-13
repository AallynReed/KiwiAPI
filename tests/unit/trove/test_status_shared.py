"""Cross-process status sharing.

The prober runs only in the API process and caches its snapshot in-process; it also
mirrors it to Redis so the bot process (no prober) can read live status for the live
board + the status announcement via ``get_status_shared``.
"""
import asyncio

import app.trove.status as st


class _FakeRedis:
    def __init__(self):
        self.store = {}

    async def set(self, k, v, ex=None):
        self.store[k] = v

    async def get(self, k):
        return self.store.get(k)


def test_shared_status_round_trips_through_redis(monkeypatch):
    fake = _FakeRedis()
    monkeypatch.setattr(st, "get_redis", lambda: fake)
    monkeypatch.setattr(st, "_state", None)        # simulate the bot (no prober here)
    snap = {
        "overall": "maintenance",
        "auth": {"online": True},
        "environments": {"eu": {"status": "online", "game": {"latency_ms": 95}}},
        "checked_at": 1700000000,
    }
    asyncio.run(st._publish_snapshot(snap))         # API mirrors it
    shared = asyncio.run(st.get_status_shared())     # bot reads it
    assert shared["overall"] == "maintenance"
    assert shared["environments"]["eu"]["game"]["latency_ms"] == 95   # full detail survives


def test_shared_status_falls_back_to_unknown_without_redis(monkeypatch):
    monkeypatch.setattr(st, "get_redis", lambda: None)
    monkeypatch.setattr(st, "_state", None)
    shared = asyncio.run(st.get_status_shared())
    assert shared["overall"] == "unknown"            # graceful, no crash


def test_shared_status_prefers_in_process_cache(monkeypatch):
    # In the API process _state is populated -> never needs Redis.
    monkeypatch.setattr(st, "_state",
                        {"overall": "online", "environments": {}, "auth": None, "checked_at": 1})

    def _boom():
        raise AssertionError("get_redis must not be called when _state is set")

    monkeypatch.setattr(st, "get_redis", _boom)
    assert asyncio.run(st.get_status_shared())["overall"] == "online"
