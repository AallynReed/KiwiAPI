"""Golden fixtures for kiwi-server's leaderboards warmer port.

Drives the real orchestration in app/trove/leaderboards/detection.py
(_warm_all, _persisted_pass_age, _warmup_loop, _maybe_tier_cold_partitions),
backlog.reingest and backlog._read_text, and the master routes that drive the
warmer (/warm, /delete-anchor, /reingest-backlog) through ``app.main.app``,
with every collaborator faked and recorded. src/lb_warmer/tests.rs replays it.

Run from the repo root with the production-pinned venv:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_lb_warmer_fixtures.py
"""

from __future__ import annotations

import asyncio
import base64
import gzip
import json
import logging
import os
import sys
import tempfile
import types
import urllib.parse
import warnings
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "lb_warmer" / "golden.json"
os.chdir(ROOT)
sys.path.insert(0, str(ROOT))

import bson  # noqa: E402
from beanie import init_beanie  # noqa: E402
from beanie.odm.utils import init as beanie_init  # noqa: E402
from pymongo import AsyncMongoClient  # noqa: E402

from app.admin import runtime_config as rc  # noqa: E402
from app.admin.ingest_log import IngestLogEntry  # noqa: E402
from app.auth.models import User  # noqa: E402
from app.core import config as core_config  # noqa: E402
from app.tokens.models import ApiToken  # noqa: E402
from app.trove.leaderboards import (  # noqa: E402
    activity,
    backlog,
    cache,
    class_activity,
    detection,
    duplicates,
    pg_store,
    renames,
    service,
)

USER_ID = "64b000000000000000000001"
TRACE: list[str] = []


class Boom(Exception):
    pass


class Patch:
    """Set attributes for one scenario and put every one back afterwards."""

    def __init__(self):
        self.saved = []

    def __call__(self, obj, name, value):
        self.saved.append((obj, name, getattr(obj, name)))
        setattr(obj, name, value)

    def undo(self):
        for obj, name, value in reversed(self.saved):
            setattr(obj, name, value)
        self.saved.clear()


class Clock:
    def __init__(self, t: float):
        self.t = t

    def time(self) -> float:
        return self.t


# --- _warm_all ----------------------------------------------------------------------

WARM_SCENARIOS = [
    {"name": "all ok", "detect": {"anchor": 1_761_130_800, "players": []}},
    {"name": "detect fails, latest publishes", "fail": ["detect"], "latest": [1_761_127_200]},
    {"name": "detect without anchor", "detect": {"anchor": None}, "latest": [1_761_127_200]},
    {"name": "detect missing key", "detect": {"players": []}, "latest": []},
    {"name": "detect anchor zero", "detect": {"anchor": 0}},
    {"name": "every soft step fails",
     "fail": ["detect", "activity", "class_activity", "renames", "duplicates", "warm_board_histories",
              "warm_days", "tier_cold"], "latest": [5]},
    {"name": "cache.warm fails", "fail": ["cache.warm"], "detect": {"anchor": 9}},
    {"name": "latest fails", "fail": ["detect", "latest"]},
    {"name": "no postgres", "pg": False, "detect": {"anchor": 9}},
    {"name": "cold tier too recent", "last_cold": 1_761_130_000.5 - 86_399.9, "detect": {"anchor": 9}},
    {"name": "cold tier exactly a day", "last_cold": 1_761_130_000.5 - 86_400.0, "detect": {"anchor": 9}},
    {"name": "tier setting text", "after_days": " 12 ", "detect": {"anchor": 9}},
    {"name": "tier setting float", "after_days": 3.9, "detect": {"anchor": 9}},
    {"name": "tier setting junk", "after_days": "3.9", "detect": {"anchor": 9}},
    {"name": "tier setting fails", "fail": ["tier_setting"], "detect": {"anchor": 9}},
    {"name": "tier moves", "tier": {"cold_tablespace": True, "moved": ["entry_p_1", "entry_p_2"], "moved_bytes": 3},
     "detect": {"anchor": 9}},
    {"name": "tier without cold", "tier": {"cold_tablespace": False, "moved": [], "moved_bytes": 0},
     "detect": {"anchor": 9}},
]


def maybe_fail(step: str, sc: dict):
    if step in sc.get("fail", []):
        raise Boom(step)


async def run_warm_all(sc: dict) -> dict:
    p = Patch()
    TRACE.clear()
    clock = Clock(sc.get("now", 1_761_130_000.5))

    def step(name, result=None):
        async def fn(*a, **k):
            TRACE.append(name)
            maybe_fail(name, sc)
            return result
        return fn

    async def detect_fake(*, force=False):
        TRACE.append("detect")
        assert force is True
        maybe_fail("detect", sc)
        return sc.get("detect", {"anchor": 1})

    async def estimate(*, force=False):
        TRACE.append("activity")
        assert force is True
        maybe_fail("activity", sc)
        return {}

    async def class_estimate(*, force=False):
        TRACE.append("class_activity")
        assert force is True
        maybe_fail("class_activity", sc)
        return {}

    async def latest(limit=60, *, include_archive=True, since=None):
        TRACE.append(f"latest {limit} {include_archive}")
        maybe_fail("latest", sc)
        return list(sc.get("latest", []))

    async def set_ready(anchor):
        TRACE.append(f"set_ready_anchor {anchor}")

    async def bhist(anchor):
        TRACE.append(f"warm_board_histories {anchor}")
        maybe_fail("warm_board_histories", sc)
        return 4

    async def get_setting(key):
        TRACE.append(f"setting {key}")
        if key == "leaderboards_pg_tier_after_days":
            maybe_fail("tier_setting", sc)
            return sc.get("after_days", 7)
        raise AssertionError(key)

    async def tier(after_days, now, limit=None):
        TRACE.append(f"tier_cold {after_days} {now} {limit}")
        maybe_fail("tier_cold", sc)
        return sc.get("tier", {"cold_tablespace": True, "moved": [], "moved_bytes": 0})

    p(detection, "detect_possible_cheaters", detect_fake)
    p(activity, "estimate_active_players", estimate)
    p(class_activity, "estimate_class_activity", class_estimate)
    p(renames, "detect_latest", step("renames", {}))
    p(duplicates, "detect_latest", step("duplicates", {}))
    p(cache, "warm", step("cache.warm"))
    p(cache, "set_ready_anchor", set_ready)
    p(cache, "warm_board_histories", bhist)
    p(cache, "warm_days", step("warm_days"))
    p(service, "list_timestamps", latest)
    p(rc, "get_setting", get_setting)
    p(pg_store, "tier_cold_partitions", tier)
    p(detection, "time", clock)
    p(detection, "_last_cold_tier_ts", sc.get("last_cold", 0.0))
    p(core_config, "settings", types.SimpleNamespace(postgres_enabled=sc.get("pg", True)))
    raised = None
    try:
        await detection._warm_all()
    except Boom as exc:
        raised = str(exc)
    last_cold = detection._last_cold_tier_ts
    p.undo()
    return {**sc, "now": clock.t, "trace": list(TRACE), "raised": raised, "last_cold_after": last_cold}


# --- _persisted_pass_age ----------------------------------------------------------------

NOW = 1_761_130_000.5
PROBE_SCENARIOS = [
    {"name": "no ready anchor", "ready": None},
    {"name": "no snapshot", "ready": 100, "persisted": None},
    {"name": "empty snapshot", "ready": 100, "persisted": {}},
    {"name": "fresh", "ready": 100, "persisted": {"computed_at": NOW - 100}, "latest": [100]},
    {"name": "fresh int", "ready": 100, "persisted": {"computed_at": int(NOW) - 1799}, "latest": [100]},
    {"name": "newer capture", "ready": 100, "persisted": {"computed_at": NOW - 100}, "latest": [101]},
    {"name": "older latest", "ready": 100, "persisted": {"computed_at": NOW - 100}, "latest": [99]},
    {"name": "no captures", "ready": 100, "persisted": {"computed_at": NOW - 100}, "latest": []},
    {"name": "latest fails", "ready": 100, "persisted": {"computed_at": NOW - 100}, "latest": "fail"},
    {"name": "ttl fails", "ready": 100, "persisted": {"computed_at": NOW - 100}, "latest": [100], "ttl": "fail"},
    {"name": "ttl text", "ready": 100, "persisted": {"computed_at": NOW - 100}, "latest": [100], "ttl": " 120 "},
    {"name": "ttl junk", "ready": 100, "persisted": {"computed_at": NOW - 100}, "latest": [100], "ttl": "abc"},
    {"name": "ttl nan", "ready": 100, "persisted": {"computed_at": NOW - 100}, "latest": [100], "ttl": "nan"},
    {"name": "ttl inf", "ready": 100, "persisted": {"computed_at": NOW - 9e9}, "latest": [100], "ttl": "inf"},
    {"name": "ttl bool", "ready": 100, "persisted": {"computed_at": NOW - 0.5}, "latest": [100], "ttl": True},
    {"name": "stale", "ready": 100, "persisted": {"computed_at": NOW - 1800}, "latest": [100]},
    {"name": "just inside", "ready": 100, "persisted": {"computed_at": NOW - 1799.75}, "latest": [100]},
    {"name": "future", "ready": 100, "persisted": {"computed_at": NOW + 5}, "latest": [100]},
    {"name": "now exactly", "ready": 100, "persisted": {"computed_at": NOW}, "latest": [100]},
    {"name": "missing computed_at", "ready": 100, "persisted": {"anchor": 100}, "latest": [100]},
    {"name": "null computed_at", "ready": 100, "persisted": {"computed_at": None}, "latest": [100]},
    {"name": "zero computed_at", "ready": 100, "persisted": {"computed_at": 0}, "latest": [100]},
    {"name": "text computed_at", "ready": 100, "persisted": {"computed_at": f" {NOW - 60} "}, "latest": [100]},
    {"name": "underscore computed_at", "ready": 100, "persisted": {"computed_at": "1_761_129_000"}, "latest": [100]},
    {"name": "junk computed_at", "ready": 100, "persisted": {"computed_at": "soon"}, "latest": [100]},
    {"name": "empty text computed_at", "ready": 100, "persisted": {"computed_at": ""}, "latest": [100]},
    {"name": "inf computed_at", "ready": 100, "persisted": {"computed_at": "inf"}, "latest": [100]},
    {"name": "bool computed_at", "ready": 100, "persisted": {"computed_at": True}, "latest": [100],
     "ttl": 2e9},
    {"name": "list computed_at", "ready": 100, "persisted": {"computed_at": [1]}, "latest": [100]},
    {"name": "empty list computed_at", "ready": 100, "persisted": {"computed_at": []}, "latest": [100],
     "ttl": 2e9},
    {"name": "dict computed_at", "ready": 100, "persisted": {"computed_at": {"a": 1}}, "latest": [100]},
    {"name": "list snapshot", "ready": 100, "persisted": [1], "latest": [100]},
    {"name": "text snapshot", "ready": 100, "persisted": "x", "latest": [100]},
    {"name": "number snapshot", "ready": 100, "persisted": 5, "latest": [100]},
    {"name": "zero snapshot", "ready": 100, "persisted": 0, "latest": [100]},
    {"name": "false snapshot", "ready": 100, "persisted": False, "latest": [100]},
    {"name": "true snapshot", "ready": 100, "persisted": True, "latest": [100]},
]


async def run_probe(sc: dict) -> dict:
    p = Patch()
    TRACE.clear()

    async def ready():
        TRACE.append("ready")
        return sc.get("ready")

    async def get_cheaters(anchor):
        TRACE.append(f"persisted {anchor}")
        return sc.get("persisted")

    async def latest(limit=60, *, include_archive=True, since=None):
        TRACE.append(f"latest {limit} {include_archive}")
        if sc.get("latest") == "fail":
            raise Boom("latest")
        return list(sc.get("latest", []))

    async def get_setting(key):
        TRACE.append(f"setting {key}")
        if sc.get("ttl") == "fail":
            raise Boom("ttl")
        return sc.get("ttl", 1800)

    p(cache, "get_ready_anchor", ready)
    p(cache, "get_cheaters", get_cheaters)
    p(service, "list_timestamps", latest)
    p(rc, "get_setting", get_setting)
    p(detection, "time", Clock(NOW))
    got = await detection._persisted_pass_age()
    p.undo()
    return {**sc, "now": NOW, "trace": list(TRACE), "result": None if got is None else repr(got)}


# --- _warmup_loop -----------------------------------------------------------------------

LOOP_SCENARIOS = [
    {"name": "boot pass then wakes",
     "iterations": [{"probe": None, "work": 250.25, "ttl": 1800, "woken": True},
                    {"work": 12.5, "ttl": 1800, "woken": False},
                    {"work": 1790.0, "ttl": 1800, "woken": False}]},
    {"name": "resumed boot",
     "iterations": [{"probe": 600.5, "ttl": 1800, "woken": False},
                    {"work": 40.0, "ttl": 1800, "woken": False}]},
    {"name": "resumed old",
     "iterations": [{"probe": 1799.0, "ttl": 1800, "woken": False}]},
    {"name": "failures and ttl fallbacks",
     "iterations": [{"probe": None, "work": 3.0, "fail": True, "ttl": "fail", "woken": False},
                    {"work": 3.0, "ttl": "120", "woken": False},
                    {"work": 3.0, "ttl": "nan", "woken": False},
                    {"work": 3.0, "ttl": 10, "woken": True},
                    {"work": 0.0, "ttl": True, "woken": False},
                    {"work": 0.0, "ttl": "junk", "woken": False},
                    {"work": 1.0, "ttl": "inf", "woken": True}]},
]


async def run_loop(sc: dict) -> dict:
    p = Patch()
    TRACE.clear()
    clock = Clock(1_761_130_000.5)
    its = sc["iterations"]
    state = {"i": 0}

    async def fake_sleep(delay):
        TRACE.append(f"sleep {delay!r}")

    async def fake_wait_for(aw, timeout):
        aw.close()
        TRACE.append(f"wait {timeout!r}")
        it = its[state["i"]]
        state["i"] += 1
        if state["i"] >= len(its):
            raise asyncio.CancelledError
        if not it["woken"]:
            raise TimeoutError

    async def probe():
        TRACE.append("probe")
        return its[state["i"]].get("probe")

    async def warm_all():
        it = its[state["i"]]
        TRACE.append("pass")
        clock.t += it.get("work", 0.0)
        if it.get("fail"):
            raise Boom("pass")

    async def get_setting(key):
        assert key == "cheaters_cache_ttl_seconds"
        ttl = its[state["i"]].get("ttl", 1800)
        TRACE.append("ttl")
        if ttl == "fail":
            raise Boom("ttl")
        return ttl

    fake_asyncio = types.SimpleNamespace(
        Event=asyncio.Event, sleep=fake_sleep, wait_for=fake_wait_for, CancelledError=asyncio.CancelledError,
    )
    p(detection, "asyncio", fake_asyncio)
    p(detection, "_persisted_pass_age", probe)
    p(detection, "_warm_all", warm_all)
    p(detection, "time", clock)
    p(rc, "get_setting", get_setting)
    try:
        await detection._warmup_loop()
    except asyncio.CancelledError:
        pass
    p.undo()
    detection._wake_event = None
    return {**sc, "trace": list(TRACE)}


# --- backlog: _read_text and reingest -----------------------------------------------------

READ_CASES = [
    ("plain.cfg", b"A$B$1 = N$C##1;x;1\r\n2;y;2\rZ\n"),
    ("bad.cfg", b"caf\xc3\xa9 \xff\xfe \xe2\x82 end\r\r\n"),
    ("bom.cfg", b"\xef\xbb\xbfA$B$1 = N$C##1;x;1\n"),
    ("split.cfg.gz", None),
    ("multi.cfg.gz", None),
    ("lone-cr.gz", None),
    ("empty.cfg", b""),
]


def read_section() -> list:
    out = []
    with tempfile.TemporaryDirectory() as d:
        for name, data in READ_CASES:
            if name == "split.cfg.gz":
                data = gzip.compress(b"\xe2\x82\xac\r" * 20_000 + b"\n tail \xc3", mtime=0)
            elif name == "multi.cfg.gz":
                data = gzip.compress(b"one\r\n", mtime=0) + gzip.compress(b"two\r", mtime=0)
            elif name == "lone-cr.gz":
                data = gzip.compress(b"x\ry\r\nz\n\r", mtime=0)
            path = Path(d) / name
            path.write_bytes(data)
            text = backlog._read_text(path)
            out.append({"name": name, "data_b64": base64.b64encode(data).decode(),
                        "text_b64": base64.b64encode(text.encode("utf-8", "surrogatepass")).decode()})
    return out


class FakeRedis:
    def __init__(self, sc):
        self.sc = sc

    async def set(self, key, value, nx=False, ex=None):
        if nx:
            TRACE.append(f"SET {key} {value} NX EX {ex}")
            return self.sc.get("lock", True)
        TRACE.append(f"SET {key} {value} EX {ex}")
        if self.sc.get("status_fails"):
            raise Boom("set")
        return True

    async def expire(self, key, ttl):
        TRACE.append(f"EXPIRE {key} {ttl}")
        if self.sc.get("expire_fails"):
            raise Boom("expire")

    async def delete(self, key):
        TRACE.append(f"DEL {key}")
        if self.sc.get("del_fails"):
            raise Boom("del")


LONG = "x" * 150 + "é" * 60 + "tail"
REINGEST_SCENARIOS = [
    {"name": "three files", "files": [[1_700_000_000, "ok", 1_700_000_000], [1_700_003_600, "fail", LONG],
                                      [1_700_007_200, "read_fail", "[Errno 2] No such file or directory: 'x'"]]},
    {"name": "locked out", "lock": False, "files": [[1, "ok", 1]]},
    {"name": "no redis, clear first", "redis": False, "clear_first": True,
     "files": [[1_700_000_000, "ok", 1_699_999_000], [1_700_003_600, "ok", None]],
     "activity_fails": True},
    {"name": "reset fails", "clear_first": True, "reset_fails": True, "files": [[1, "ok", 1]]},
    {"name": "reset fails without redis", "redis": False, "clear_first": True, "reset_fails": True,
     "files": []},
    {"name": "errors capped", "files": [[1_700_000_000 + i, "fail", f"bad ünïcode {i} 💥"] for i in range(28)],
     "class_fails": True},
    {"name": "store hiccups", "status_fails": True, "expire_fails": True, "del_fails": True,
     "files": [[7, "ok", 7]]},
    {"name": "empty backlog", "files": []},
]


async def run_reingest(sc: dict) -> dict:
    p = Patch()
    TRACE.clear()
    files = [(a, Path(f"/data/backlog/leaderboards/{a}.cfg.gz")) for a, _, _ in sc["files"]]
    by_anchor = {a: (kind, v) for a, kind, v in sc["files"]}

    async def read_text_thread(fn, path):
        a = int(path.name.split(".")[0])
        TRACE.append(f"read {a}")
        kind, v = by_anchor[a]
        if kind == "read_fail":
            raise OSError(v)
        return f"dump {a}"

    async def insert_dump(text, *, timestamp=None, allow_backfill=False):
        TRACE.append(f"insert {text!r} {timestamp} {allow_backfill}")
        kind, v = by_anchor[timestamp]
        if kind == "fail":
            raise Boom(v)
        return {"boards": 1, "entries": 2, "created_at": v}

    async def reset_all(*, drop_boards=False):
        TRACE.append(f"reset_all {drop_boards}")
        if sc.get("reset_fails"):
            raise Boom("reset")
        return {}

    async def backfill(*, total_days=400, chunk_days=14, force=False, reset=False):
        TRACE.append(f"backfill_activity {total_days} {force} {reset}")
        if sc.get("activity_fails"):
            raise Boom("activity")
        return {}

    async def class_backfill(*, total_days=400, chunk_days=14, force=False, reset=False):
        TRACE.append(f"backfill_class {total_days} {force} {reset}")
        if sc.get("class_fails"):
            raise Boom("class")
        return {}

    def trigger():
        TRACE.append("trigger")

    redis = FakeRedis(sc) if sc.get("redis", True) else None
    fake_asyncio = types.SimpleNamespace(to_thread=read_text_thread)
    p(backlog, "get_redis", lambda: redis)
    p(backlog, "list_files", lambda: list(files))
    p(backlog, "asyncio", fake_asyncio)
    p(backlog, "time", Clock(1_761_130_000.75))
    p(backlog, "_local_status", {"running": False})
    p(service, "insert_dump", insert_dump)
    p(service, "reset_all", reset_all)
    p(activity, "backfill_history_chunked", backfill)
    p(class_activity, "backfill_class_history_chunked", class_backfill)
    p(detection, "trigger_warmer", trigger)
    raised = None
    try:
        await backlog.reingest(clear_first=sc.get("clear_first", False))
    except Boom as exc:
        raised = str(exc)
    local = json.dumps(backlog._local_status)
    p.undo()
    return {**sc, "trace": list(TRACE), "raised": raised, "local": local}


# --- routes -------------------------------------------------------------------------------

async def call(app, method: str, url: str, headers: list[tuple[bytes, bytes]], body: bytes):
    path, _, qs = url.partition("?")
    scope = {
        "type": "http", "asgi": {"version": "3.0"}, "http_version": "1.1", "method": method,
        "scheme": "http", "path": urllib.parse.unquote(path), "raw_path": path.encode(),
        "query_string": qs.encode(), "root_path": "",
        "headers": [(b"host", b"testserver")] + headers + [(b"content-length", str(len(body)).encode())],
        "client": ("127.0.0.1", 50000), "server": ("testserver", 80),
    }
    done = asyncio.Event()
    pending = [{"type": "http.request", "body": body, "more_body": False}]
    sent = []

    async def receive():
        if pending:
            return pending.pop(0)
        await done.wait()
        return {"type": "http.disconnect"}

    async def send(message):
        sent.append(message)
        if message["type"] == "http.response.body" and not message.get("more_body"):
            done.set()

    await app(scope, receive, send)
    start = next(m for m in sent if m["type"] == "http.response.start")
    raw = b"".join(m.get("body", b"") for m in sent if m["type"] == "http.response.body")
    return start["status"], [(k.decode("latin-1").lower(), v.decode("latin-1")) for k, v in start["headers"]], raw


def bson_hex(doc: dict) -> str:
    return bson.encode(doc).hex()


DELETE_CASES = [
    {"name": "missing anchor", "query": ""},
    {"name": "bad anchor", "query": "?anchor=abc&recompute=maybe"},
    {"name": "float anchor", "query": "?anchor=1.5"},
    {"name": "empty anchor", "query": "?anchor="},
    {"name": "float zero anchor", "query": "?anchor=1700000000.0", "deleted": 3},
    {"name": "bad recompute", "query": "?anchor=1700000000&recompute=maybe"},
    {"name": "deleted with recompute", "query": "?anchor=1700000000", "deleted": 123456},
    {"name": "deleted, recompute off", "query": "?anchor=1700000000&recompute=false", "deleted": 5},
    {"name": "nothing deleted", "query": "?anchor=1700000000&recompute=true", "deleted": 0},
    {"name": "estimate purge fails", "query": "?anchor=-5&recompute=1", "deleted": 2, "estimate_fails": True},
    {"name": "big count", "query": "?anchor=3000000000", "deleted": 3_000_000_000},
    {"name": "repeated anchor", "query": "?anchor=1&anchor=2", "deleted": 1},
]

REINGEST_HTTP = [
    {"name": "empty backlog", "query": "", "files": 0},
    {"name": "files", "query": "", "files": 3},
    {"name": "clear first", "query": "?clear_first=yes", "files": 2},
    {"name": "clear first off", "query": "?clear_first=0", "files": 1},
    {"name": "bad flag", "query": "?clear_first=perhaps", "files": 1},
    {"name": "empty flag", "query": "?clear_first=", "files": 1},
]


async def http_section(app, trove_router) -> dict:
    from app.core.dependencies import IngestAuth, require_master_ingest
    from app.core.features import require_leaderboards_enabled

    user = types.SimpleNamespace(id=bson.ObjectId(USER_ID), email="master@aallyn.net", is_superuser=True)
    app.dependency_overrides[require_leaderboards_enabled] = lambda: None
    app.dependency_overrides[require_master_ingest] = lambda: IngestAuth(user=user, token=None)
    out: dict = {"warm": [], "delete": [], "reingest": [], "method": []}
    p = Patch()

    async def go(method, url, rid):
        status, rh, raw = await call(app, method, url, [(b"x-request-id", rid.encode())], b"")
        get = lambda k: next((v for n, v in rh if n == k), None)  # noqa: E731
        return {"status": status, "allow": get("allow"), "response_type": get("content-type"),
                "response": raw.decode("utf-8")}

    p(detection, "trigger_warmer", lambda: TRACE.append("trigger"))
    p(detection, "reset", lambda: TRACE.append("reset"))
    TRACE.clear()
    out["warm"].append({**await go("POST", "/v1/leaderboards/warm", "parity-lbw-0"), "trace": list(TRACE)})

    for i, c in enumerate(DELETE_CASES):
        TRACE.clear()
        backfills: list = []

        async def delete_anchor(anchor, _c=c):
            TRACE.append(f"delete_anchor {anchor}")
            return _c.get("deleted", 0)

        async def delete_estimate(anchor, _c=c):
            TRACE.append(f"delete_estimate {anchor}")
            if _c.get("estimate_fails"):
                raise Boom("estimate")

        async def invalidate_anchor(anchor):
            TRACE.append(f"invalidate_anchor {anchor}")

        async def record(**kw):
            TRACE.append(f"log {kw['endpoint']} {bson_hex({'summary': kw.get('summary') or {}})}")

        async def backfill(**kw):
            backfills.append(kw)

        p(pg_store, "delete_anchor", delete_anchor)
        p(pg_store, "delete_estimate", delete_estimate)
        p(cache, "invalidate_anchor", invalidate_anchor)
        p(trove_router.ingest_log, "record", record)
        p(activity, "backfill_history_chunked", backfill)
        res = await go("POST", "/v1/leaderboards/delete-anchor" + c["query"], f"parity-lbd-{i}")
        out["delete"].append({**c, **res, "trace": list(TRACE), "backfills": backfills})

    for i, c in enumerate(REINGEST_HTTP):
        TRACE.clear()
        started: list = []

        async def reingest(**kw):
            started.append(kw)

        p(backlog, "list_files", lambda _n=c["files"]: [(k, Path(f"/x/{k}.cfg")) for k in range(_n)])
        p(backlog, "reingest", reingest)
        res = await go("POST", "/v1/leaderboards/reingest-backlog" + c["query"], f"parity-lbb-{i}")
        out["reingest"].append({**c, **res, "started": started})

    for i, (method, path) in enumerate([
        ("GET", "/v1/leaderboards/warm"), ("HEAD", "/v1/leaderboards/warm"),
        ("GET", "/v1/leaderboards/delete-anchor?anchor=1"), ("PUT", "/v1/leaderboards/delete-anchor"),
        ("GET", "/v1/leaderboards/reingest-backlog"), ("DELETE", "/v1/leaderboards/reingest-backlog"),
    ]):
        res = await go(method, path, f"parity-lbm-{i}")
        out["method"].append({"method": method, "path": path, **res})
    p.undo()
    app.dependency_overrides.clear()
    return out


async def main() -> None:
    async def no_server(self):
        self._database_major_version = 7
        self._existing_collections = []

    beanie_init.Initializer._load_cached_info = no_server
    client = AsyncMongoClient("mongodb://127.0.0.1:1/", tz_aware=True, connect=False)
    await init_beanie(database=client["fixtures"], document_models=[IngestLogEntry, User, ApiToken],
                      skip_indexes=True)
    for key, spec in rc.REGISTRY.items():
        rc._CACHE[key] = (spec.default, float("inf"))
    data = {"python": sys.version.split()[0]}
    data["warm_all"] = [await run_warm_all(sc) for sc in WARM_SCENARIOS]
    data["probe"] = [await run_probe(sc) for sc in PROBE_SCENARIOS]
    data["loop"] = [await run_loop(sc) for sc in LOOP_SCENARIOS]
    data["read_text"] = read_section()
    data["reingest"] = [await run_reingest(sc) for sc in REINGEST_SCENARIOS]
    from app.main import app
    from app.trove import router as trove_router
    data["http"] = await http_section(app, trove_router)
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(data, ensure_ascii=True, indent=1) + "\n", encoding="utf-8")
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes) with Python {data['python']}")


if __name__ == "__main__":
    asyncio.run(main())
