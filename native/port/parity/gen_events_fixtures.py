"""Golden fixtures for kiwi-server's events port (app/events), captured from Python.

The real source functions, bus.publish, the scheduler loop and the SSE route
generator run against fake stores, a fake Redis and a pinned clock.

Run from the repo root with the production-pinned venv:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_events_fixtures.py
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import random
import sys
from contextlib import asynccontextmanager
from datetime import datetime, timedelta, timezone
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "events" / "golden.json"
os.chdir(ROOT)
sys.path.insert(0, str(ROOT))

from app.core import features  # noqa: E402
from app.core.config import settings  # noqa: E402
from app.core.errors import APIError  # noqa: E402
from app.events import bus, scheduler  # noqa: E402
from app.events import router as r  # noqa: E402
from app.events import sources as src  # noqa: E402
from app.trove import captures, chaos, rotations, server_time, status  # noqa: E402

UTC = timezone.utc
EPOCH = datetime(1970, 1, 1, tzinfo=UTC)


def us_of(dt: datetime) -> int:
    return (dt - EPOCH) // timedelta(microseconds=1)


def dt_of(us: int) -> datetime:
    return EPOCH + timedelta(microseconds=us)


def ms_dt(ms: int) -> datetime:
    return EPOCH + timedelta(milliseconds=ms)


def short(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()[:16]


def dumps(v) -> str:
    return json.dumps(v, default=str)


def run(coro):
    return asyncio.run(coro)


class Clock:
    now = datetime(2026, 10, 22, 12, 34, 56, 123456, tzinfo=UTC)


server_time.real_utc_now = lambda: Clock.now
rotations.real_utc_now = lambda: Clock.now
src.time = SimpleNamespace(time=lambda: Clock.now.timestamp())
bus.utcnow = lambda: Clock.now
scheduler.utcnow = lambda: Clock.now


async def source_data(kind: str):
    return await bus._source_data(src.SOURCES_BY_TYPE[kind])


def record(kind: str) -> dict:
    s = src.SOURCES_BY_TYPE[kind]
    data = run(source_data(kind))
    out = {"data": dumps(data), "sig": s.sig_fn(data)}
    if s.next_at_fn is not None:
        out["next"] = s.next_at_fn()
    return out


# --- time-driven sources ------------------------------------------------------------

TIMED = [s.type for s in src.SCHEDULED_SOURCES]


def instants() -> list[datetime]:
    out: set[datetime] = set()
    seed = datetime(2026, 10, 22, 12, 34, 56, 123456, tzinfo=UTC)
    marks: list[int] = []
    for fn in (server_time.corruxion, server_time.fluxion):
        res = fn(seed)
        marks += [res["starts_at"], res["ends_at"]] + [w["starts_at"] for w in res["schedule"][:3]]
        marks += [w["ends_at"] for w in res["schedule"][:2]]
    for feed in (rotations.biome_rotation(seed), rotations.stampy(seed), rotations.wild_mana(seed)):
        marks += [w["starts_at"] for w in feed["upcoming"][:3]] + [w["ends_at"] for w in feed["upcoming"][:3]]
        if feed["current"]:
            marks += [feed["current"]["starts_at"], feed["current"]["ends_at"]]
    days = [datetime(2026, 10, 19, 11, tzinfo=UTC) + timedelta(days=i) for i in range(8)]
    marks += [int(d.timestamp()) for d in days]
    for m in marks:
        for delta in (-1_000_001, -1, 0, 1, 999_999, 1_000_000, 2_000_000):
            out.add(EPOCH + timedelta(seconds=m, microseconds=delta))
    rng = random.Random(20261022)
    lo, hi = us_of(datetime(2024, 7, 1, tzinfo=UTC)), us_of(datetime(2029, 1, 1, tzinfo=UTC))
    for i in range(120):
        us = rng.randrange(lo, hi)
        if i % 3 == 0:
            us -= us % 10**6
        out.add(dt_of(us))
    return sorted(out)


def scheduler_delay(kind: str) -> int:
    """One pass of the real ``_run_source``: the sleep it asks for."""
    got = {}

    async def publish_source(_source):
        return False

    async def sleep(delay):
        got["delay"] = delay
        raise asyncio.CancelledError

    scheduler.bus = SimpleNamespace(publish_source=publish_source)
    scheduler.asyncio = SimpleNamespace(sleep=sleep, CancelledError=asyncio.CancelledError)
    try:
        run(scheduler._run_source(src.SOURCES_BY_TYPE[kind]))
    except asyncio.CancelledError:
        pass
    return got["delay"]


def timed_sweep() -> tuple[list, dict]:
    rows, full = [], {}
    keep = {us_of(datetime(2026, 10, 22, 12, 34, 56, 123456, tzinfo=UTC)),
            us_of(datetime(2026, 10, 24, 11, 0, 0, 1, tzinfo=UTC))}
    for now in instants():
        Clock.now = now
        row = {"us": us_of(now)}
        texts = {}
        for kind in TIMED:
            rec = record(kind)
            texts[kind] = rec["data"]
            row[kind] = {"h": short(rec["data"]), "sig": rec["sig"], "next": rec["next"],
                         "delay": scheduler_delay(kind)}
        rows.append(row)
        if row["us"] in keep:
            full[str(row["us"])] = texts
    for k in keep:
        if str(k) not in full:
            Clock.now = dt_of(k)
            full[str(k)] = {kind: record(kind)["data"] for kind in TIMED}
    return rows, full


# --- insert-driven sources over fake stores --------------------------------------

def challenge_cases() -> list[dict]:
    out = []
    names = [None, "Cursed Vale", "RAMPAGE ALERT!", " Collection Challenge ", "Ünïcode \"q\" \\ dungeon", ""]
    instants_ = [datetime(2026, 10, 23, 11, 35, 1, 5, tzinfo=UTC), datetime(2026, 10, 22, 12, 34, 56, tzinfo=UTC),
                 datetime(2026, 10, 24, 10, 59, 59, 999999, tzinfo=UTC), datetime(2026, 10, 24, 11, 19, 59, tzinfo=UTC)]
    for now in instants_:
        for name in names:
            for cap_ms in (1792000000000, 1792000000777):
                doc = None if name is None else SimpleNamespace(name=name, captured_at=ms_dt(cap_ms))

                class FakeChallengeCapture:
                    window_anchor = 0

                    @staticmethod
                    async def find_one(*_a, doc=doc, **_k):
                        return doc

                captures.ChallengeCapture = FakeChallengeCapture
                Clock.now = now
                rec = record("challenge")
                out.append({"now_us": us_of(now), "doc": None if doc is None else [name, cap_ms], **rec})
                if doc is None:
                    break
    return out


class FakeFeedCache:
    feed = "feed"
    doc = None

    @classmethod
    async def find_one(cls, *_a, **_k):
        return cls.doc


def chaos_cases() -> list[dict]:
    chaos.FeedCache = FakeFeedCache
    now0 = datetime(2026, 10, 22, 12, 34, 56, 123456, tzinfo=UTC)
    week = server_time.chaos_chest_window(now0)
    feeds = [
        None,
        {"items": [], "fetched_ms": 1792000000123},
        {"items": [{"name": "Shadow Chest", "identifier": "item/a/b", "blueprint": "item/x y.blueprint",
                    "start": week["starts_at"], "end": week["ends_at"]}], "fetched_ms": 1792000000123},
        {"items": [{"name": "Old Item", "identifier": None, "blueprint": None, "start": 0,
                    "end": week["starts_at"]}], "fetched_ms": 1792000000000},
        {"items": [{"name": "Ünï Chest", "identifier": None, "blueprint": "", "start": None, "end": None}],
         "fetched_ms": 1792000000500},
    ]
    caps = [None, {"name": "Captured Egg", "captured_ms": 1792050000777}]
    out = []
    for cap in caps:
        for feed in feeds:
            for res in (None, "item/collections/mount_dragon~1.blueprint"):
                for now in (now0, datetime(2026, 10, 20, 11, tzinfo=UTC)):
                    async def get_capture(anchor, cap=cap):
                        if cap is None:
                            return None
                        return SimpleNamespace(name=cap["name"], captured_at=ms_dt(cap["captured_ms"]))

                    async def resolve(name, res=res):
                        return res

                    captures.get_chaos_chest_for_week = get_capture
                    chaos.resolve_item_blueprint = resolve
                    FakeFeedCache.doc = None if feed is None else SimpleNamespace(
                        items=feed["items"], fetched_at=ms_dt(feed["fetched_ms"]))
                    Clock.now = now
                    out.append({"capture": cap, "feed": feed, "resolved": res, "now_us": us_of(now),
                                **record("chaos")})
    return out


def luxion_cases() -> list[dict]:
    runs = [
        (1785754800, 1785770525123, 1785780000999, None),
        (1784631600, 1784646000500, 1785140000000, 1785150000000),
        (1760094000, 1760094000000, 1760100000000, None),
    ]
    out = []

    async def none():
        return None

    captures.get_latest_luxion = none
    Clock.now = datetime(2026, 10, 22, 12, tzinfo=UTC)
    out.append({"doc": None, "now_us": us_of(Clock.now), **record("luxion")})
    for anchor, first_ms, last_ms, ended_ms in runs:
        doc = SimpleNamespace(started_at=anchor, first_seen_at=ms_dt(first_ms), last_seen_at=ms_dt(last_ms),
                              ended_at=None if ended_ms is None else ms_dt(ended_ms))

        async def latest(doc=doc):
            return doc

        captures.get_latest_luxion = latest
        from app.trove import luxion
        sched = luxion.schedule_for(anchor, doc.first_seen_at, doc.ended_at)
        probes = {anchor - 1, first_ms // 1000 - 1, first_ms // 1000, first_ms // 1000 + 1, anchor + 7 * 86400}
        for w in sched[:3]:
            probes |= {w["starts_at"] - 1, w["starts_at"], w["ends_at"], w["ends_at"] + 3600}
        for p in sorted(probes):
            for frac in (0, 999999):
                Clock.now = EPOCH + timedelta(seconds=p, microseconds=frac)
                out.append({"doc": [anchor, first_ms, last_ms, ended_ms], "now_us": us_of(Clock.now),
                            **record("luxion")})
    return out


def status_cases() -> list[dict]:
    snapshots = [
        None,
        {"overall": "online", "auth": {"online": True, "latency_ms": 12.5, "status": 200, "error": None},
         "environments": {"eu": {"status": "online", "online": True, "game": {"online": True, "latency_ms": 0.1}},
                          "us": {"status": "maintenance", "online": False, "game": None},
                          "pts": {"status": "unknown", "online": False, "game": None}},
         "checked_at": 1792000000},
        {"overall": "unknown", "auth": None, "environments": {}, "checked_at": 1792000001},
        {"overall": "partial", "checked_at": 1e16, "note": "Ünï \u2028"},
        {"auth": None},
    ]
    out = []
    for snap in snapshots:
        status._state = snap
        out.append({"snapshot": None if snap is None else dumps(snap), **record("server_status")})
    status._state = None
    return out


def news_cases() -> list[dict]:
    from app.trove import news
    out = []
    items = [None, ["Patch notes: Ünï", "https://trovegame.com/x?a=1&b=2", 1792000000123],
             ["No date", "https://trovegame.com/y", None], ["Empty url", "", 1792000000000]]
    for it in items:
        async def latest(limit, it=it):
            if it is None:
                return []
            return [SimpleNamespace(title=it[0], url=it[1], published_at=None if it[2] is None else ms_dt(it[2]))]

        news.latest_news = latest
        out.append({"item": it, **record("trove_news")})
    return out


def giveaway_cases() -> list[dict]:
    from bson import ObjectId

    from app.giveaways import models
    out = []
    for g in (None, ["65f0c0ffee0000000000abcd", "Dragon ⚡ \"giveaway\"", 1792000000999]):
        class Query:
            def __init__(self, g):
                self.g = g

            def sort(self, *_a):
                return self

            def limit(self, *_a):
                return self

            async def to_list(self):
                if self.g is None:
                    return []
                return [SimpleNamespace(id=ObjectId(self.g[0]), title=self.g[1], created_at=ms_dt(self.g[2]))]

        class FakeGiveaway:
            status = "status"

            @staticmethod
            def find(*_a, g=g, **_k):
                return Query(g)

        models.Giveaway = FakeGiveaway
        out.append({"doc": g, **record("giveaways")})
    return out


def game_update_cases() -> list[dict]:
    from app.trove.updates import read
    out = []
    versions = [None, ["live-us", 412, "a20.5", 12, 340, 2, 5_000_000_000, 1792000000456],
                ["live-us", 3, "Ünï", 0, 0, 0, 0, None]]
    for v in versions:
        async def latest(branch, v=v):
            if v is None:
                return None
            return SimpleNamespace(branch=v[0], ordinal=v[1], version_tag=v[2], files_added=v[3],
                                   files_modified=v[4], files_removed=v[5], bytes_added=v[6],
                                   completed_at=None if v[7] is None else ms_dt(v[7]))

        read.latest_version = latest
        out.append({"version": v, **record("game_update")})
    return out


# --- the bus ------------------------------------------------------------------------

class FakeRedis:
    def __init__(self):
        self.store: dict[str, str] = {}
        self.calls: list[list] = []

    async def set(self, key, value, get=False):
        self.calls.append(["SET", key, value, "GET"])
        prev = self.store.get(key)
        self.store[key] = value
        return prev

    async def publish(self, channel, message):
        self.calls.append(["PUBLISH", channel, message])

    async def lpush(self, queue, value):
        self.calls.append(["LPUSH", queue, value])


def publish_cases() -> list[dict]:
    from app.dm_subs import delivery as dm
    from app.webhooks import delivery as wh
    redis = FakeRedis()
    bus.get_redis = lambda: redis
    wh.get_redis = lambda: redis
    dm.get_redis = lambda: redis
    flags = {}

    async def is_enabled(flag):
        return flags[flag]

    wh.is_enabled = is_enabled
    dm.is_enabled = is_enabled
    Clock.now = datetime(2026, 10, 22, 12, 34, 56, 999999, tzinfo=UTC)
    data = {"starts_at": 1792000000, "name": "Ünï", "captured_at": ms_dt(1792000000777), "x": 0.1, "n": None}
    steps = [
        ("challenge", "1792000000:Ünï", data, True, True),
        ("challenge", "1792000000:Ünï", data, True, True),
        ("challenge", "1792003600:B", {"a": [1, 2.5]}, False, True),
        ("corruxion", "1792000000", {"active": True}, True, True),
        ("corruxion", "1792000000", {"active": True}, True, True),
        ("fluxion", "1", {}, True, False),
        ("game_update", "live-us:4", {"version": None}, True, True),
        ("mod_release", "m1", {"k": "v"}, True, True),
        ("leaderboard", "1792000000", {"anchor": 1792000000}, True, True),
        ("trove_news", "https://x", {"item": None}, True, True),
    ]
    out = []
    for kind, sig, d, webhooks, dms in steps:
        flags[features.WEBHOOKS_FLAG] = webhooks
        flags[features.DM_SUBS_FLAG] = dms
        redis.calls = []
        emitted = run(bus.publish(kind, sig, d))
        out.append({"type": kind, "sig": sig, "data": dumps(d), "webhooks": webhooks, "dm": dms,
                    "emitted": emitted, "calls": redis.calls})
    return out


# --- SSE frames and the route generator ------------------------------------------

FRAME_PAYLOADS = [
    {"type": "challenge", "data": {"name": "Ünï \u2028 \"q\" \\", "x": 1e16, "y": 0.1, "z": -0.0}, "ts": 1792000000},
    {"type": "mod_release", "data": {"nested": [[], {}, [1, {"a": None}]], "emoji": "\U0001F409"}, "ts": 0},
    {"type": 5, "data": None},
    {"type": None},
    {"type": True},
    {"type": ["a", 1]},
    {"type": {"k": "v"}},
    {"type": "a\nb"},
    {"data": 1},
    ["type"],
    "type",
    5,
    None,
]

RAW_MESSAGES = [
    '{"type": "chaos", "data": {"item": {"name": "X"}}, "ts": 1}',
    '{"type":"compact","data":{"f":1.0,"g":1e5,"h":12345678901234567890},"ts":2}',
    '{"type": "dup", "type": "second", "ts": 3}',
    '{"ts": 4}',
    '[1, 2]',
    '"just a string"',
    'not json',
    '',
    '{"type": "uni", "data": "\\u00e9\\ud83d\\ude00"}',
    '\ufeff{"type": "bom"}',
]


def frames() -> dict:
    out = {"payloads": [], "messages": []}
    for p in FRAME_PAYLOADS:
        try:
            out["payloads"].append([p, r._sse(p)])
        except Exception as e:  # noqa: BLE001
            out["payloads"].append([p, type(e).__name__])
    for raw in RAW_MESSAGES:
        try:
            payload = json.loads(raw)
        except (ValueError, TypeError):
            out["messages"].append([raw, "skip"])
            continue
        try:
            out["messages"].append([raw, r._sse(payload)])
        except Exception as e:  # noqa: BLE001
            out["messages"].append([raw, type(e).__name__])
    return out


TIMEOUT = object()


def stream_case(snapshot: list, script: list, heartbeat: int) -> dict:
    """Drive the real route generator: ``script`` items are delivered by the
    queue one per wait (TIMEOUT makes that wait time out); the client
    disconnects once the script is used up."""
    waits = []
    script = list(script)

    class Queue:
        async def get(self):
            return script.pop(0)

    async def snap():
        return snapshot

    @asynccontextmanager
    async def subscribe():
        yield Queue()

    async def wait_for(aw, timeout):
        waits.append(timeout)
        if script and script[0] is TIMEOUT:
            script.pop(0)
            aw.close()
            raise TimeoutError
        return await aw

    class Request:
        async def is_disconnected(self):
            return not script

    r.bus = SimpleNamespace(current_snapshot=snap, subscribe=subscribe, connection_count=lambda: 0)
    r.asyncio = SimpleNamespace(wait_for=wait_for)
    settings.events_heartbeat_seconds = heartbeat

    async def go():
        resp = await r.stream(Request(), ctx=None)
        chunks, error = [], None
        try:
            async for c in resp.body_iterator:
                chunks.append(c)
        except Exception as e:  # noqa: BLE001
            error = type(e).__name__
        return resp, chunks, error

    resp, chunks, error = run(go())
    return {"status": resp.status_code,
            "headers": [[k.decode("latin-1"), v.decode("latin-1")] for k, v in resp.raw_headers],
            "body": "".join(chunks), "error": error, "waits": waits}


def stream_cases() -> dict:
    a = {"type": "challenge", "data": {"name": "Ünï", "captured_at": "2026-10-22 11:00:00.777000+00:00"}, "ts": 1}
    b = {"type": "server_status", "data": {"overall": "online", "lat": 0.25}, "ts": 1}
    live1 = {"type": "leaderboard", "data": {"anchor": 1792000000}, "ts": 2}
    live2 = {"type": "mod_release", "data": {"t": "\u00e9\u2028\U0001F409"}, "ts": 3}
    out = {
        "full": {"snapshot": [a, b], "live": [live1, None, live2], **stream_case([a, b], [live1, TIMEOUT, live2], 20)},
        "empty": {"snapshot": [], "live": [None, live1], **stream_case([], [TIMEOUT, live1], 3)},
        "bad": {"snapshot": [a], "live": [live1, {"data": 1}], **stream_case([a], [live1, {"data": 1}], 7)},
    }

    r.bus = SimpleNamespace(connection_count=lambda: settings.events_max_connections)
    try:
        run(r.stream(None, ctx=None))
    except APIError as e:
        out["capacity"] = {"status": e.status_code, "code": str(e.code), "message": e.message}
    return out


def main() -> None:
    rows, full = timed_sweep()
    golden = {
        "timed": rows,
        "timed_full": full,
        "challenge": challenge_cases(),
        "chaos": chaos_cases(),
        "luxion": luxion_cases(),
        "server_status": status_cases(),
        "trove_news": news_cases(),
        "giveaways": giveaway_cases(),
        "game_update": game_update_cases(),
        "publish": publish_cases(),
        "frames": frames(),
        "stream": stream_cases(),
        "settings": {"api_url": settings.api_url, "events_channel": settings.events_channel,
                     "webhooks_queue": settings.webhooks_queue, "dm_subs_queue": settings.dm_subs_queue},
        "sources": [[s.type, s.next_at_fn is not None] for s in src.SOURCES],
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(golden, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes, {len(rows)} instants)")


if __name__ == "__main__":
    main()
