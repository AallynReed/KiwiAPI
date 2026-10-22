"""Golden fixtures for kiwi-server's trove-status routes and prober, from Python.

Drives the REAL misc_router endpoints (feature gate and public scope overridden,
the app's error handlers) with fixed prober snapshots and fake status segments,
records what the prober writes to Redis, and runs the real probe helpers against
local test servers. src/routes/trove_status/tests.rs replays every case.

Run from the repo root with the production-pinned venv:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_trove_status_fixtures.py
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import random
import socket
import sys
import threading
import types
import warnings
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "trove_status" / "fixtures.json"
os.chdir(ROOT)
sys.path.insert(0, str(ROOT))

from fastapi import FastAPI  # noqa: E402
from fastapi.testclient import TestClient  # noqa: E402

import app.trove.models as models  # noqa: E402
import app.trove.status as st  # noqa: E402
from app.core.config import settings  # noqa: E402
from app.core.dependencies import AccessContext  # noqa: E402
from app.core.errors import register_error_handlers  # noqa: E402
from app.core.features import require_server_status_enabled  # noqa: E402
from app.trove import router as trove_router  # noqa: E402

api = FastAPI()
register_error_handlers(api)
api.include_router(trove_router.misc_router)
api.dependency_overrides[require_server_status_enabled] = lambda: None
api.dependency_overrides[trove_router._MISC_PUBLIC.dependency] = lambda: AccessContext(None, None, None)
client = TestClient(api, raise_server_exceptions=False)
REAL_TIME = st.time


class FakeRedis:
    def __init__(self) -> None:
        self.calls: list = []

    async def set(self, key, value, ex=None):
        self.calls.append([key, value, ex])


def reply(r) -> dict:
    return {"status": r.status_code, "cache_control": r.headers.get("cache-control"), "body": r.text}


def game(online, host="h.example", port=6560, latency=12.3, error=None, probe=None, opener=None, keys=None):
    g = {"online": online, "host": host, "port": port, "latency_ms": latency, "error": error}
    if probe is not None:
        g["probe"] = probe
    if opener is not None or probe == "glsserver":
        g["opener"] = opener
    return g


def env(status, g):
    return {"status": status, "online": g["online"], "game": g}


def snap(overall, auth, eu, us, pts, checked_at=1760000000):
    return {"overall": overall, "auth": auth, "environments": {"eu": eu, "us": us, "pts": pts},
            "checked_at": checked_at}


AUTH_UP = {"online": True, "http_status": 401, "latency_ms": 143.7, "error": None}
SNAPSHOTS = [
    snap("online", AUTH_UP,
         env("online", game(True, "ams-c12-b05.ams.triongames.com", latency=1512.4, probe="glsserver", opener="random")),
         env("online", game(True, "dal-c35-b05.dal.triongames.com", latency=1630.0, probe="glsserver", opener="random")),
         env("online", game(True, "auth-pcpts01.trovegame.com", latency=95.1, probe="tcp"))),
    snap("down", AUTH_UP,
         env("down", game(False, latency=88.0, error="glsserver_dropped", probe="glsserver", opener="replay")),
         env("online", game(True, latency=1501.9, probe="tcp_fallback")),
         env("down", game(False, latency=8000.4, error="TimeoutError"))),
    snap("down", {"online": False, "http_status": None, "latency_ms": 8004.2, "error": "ConnectTimeout"},
         env("down", game(False, latency=21.5, error="ConnectionRefusedError")),
         env("down", game(False, "", 0, 0.0, "not_configured")),
         env("down", game(False, latency=3.0, error="gaierror"))),
    snap("down", {"online": False, "http_status": 404, "latency_ms": 0.1, "error": "login_route_unavailable"},
         env("down", game(True, latency=1e16, probe="tcp")),
         env("down", game(True, latency=1e-05, probe="tcp")),
         env("down", game(True, "h\u00f4st-\u65e5\u672c", latency=123456789.5, probe="tcp"))),
    snap("maintenance", {"online": False, "http_status": 503, "latency_ms": 0.0, "error": "HTTP 503"},
         env("maintenance", game(False, latency=2.5e-07, error="OSError")),
         env("online", game(True, latency=0.30000000000000004, probe="tcp")),
         env("online", game(True, latency=1.5e300, probe="tcp")), checked_at=0),
    {"overall": "unknown", "auth": None,
     "environments": {e: {"status": "unknown", "online": False, "game": None} for e in ("eu", "us", "pts")},
     "checked_at": None},
    {"overall": "online", "auth": {}, "environments": {}, "checked_at": 5, "local_network": False},
    {"overall": "bogus", "auth": None, "environments": {}, "checked_at": 1},
    {"overall": "online", "auth": [], "environments": {}, "checked_at": 1},
    {"overall": "online", "auth": None, "environments": {}, "checked_at": 1.0},
    {"overall": "online", "auth": None, "environments": {}, "checked_at": 1.5},
    {"overall": "online", "auth": None, "environments": {}},
]


def current_cases() -> list:
    out = []
    st._state = None
    out.append({"redis": None, "reply": reply(client.get("/v1/misc/trove-status"))})
    for s in SNAPSHOTS:
        st._state = s
        r = client.get("/v1/misc/trove-status")
        fake = FakeRedis()
        st.get_redis = lambda fake=fake: fake
        asyncio.run(st._publish_snapshot(s))
        assert fake.calls[0][0] == "kiwi:status:current" and fake.calls[0][2] == 600
        out.append({"redis": fake.calls[0][1], "reply": reply(r)})
    st._state = None
    return out


# ---------------------------------------------------------------------------
# history

class _EnvField:
    def __eq__(self, other):  # noqa: D105 - TroveStatusEvent.env == env
        return ("env", other)


class _Query:
    def __init__(self, rows):
        self.rows = rows

    def sort(self, key):
        assert key == "+started_at"
        return _Query(sorted(self.rows, key=lambda r: r.started_at))

    async def to_list(self):
        return list(self.rows)


ROWS: list = []


class FakeEvent:
    env = _EnvField()

    @classmethod
    def find(cls, cond):
        return _Query([r for r in ROWS if r.env == cond[1]])


def row(env, status, online, started_at, ended_at=None) -> dict:
    return {"env": env, "status": status, "online": online, "started_at": started_at, "ended_at": ended_at}


def history_reply(rows, now, query):
    ROWS[:] = [types.SimpleNamespace(**r) for r in rows]
    st.time = types.SimpleNamespace(time=lambda: now + 0.75)
    try:
        return reply(client.get("/v1/misc/trove-status/history" + query))
    finally:
        st.time = REAL_TIME


NOW = 1760000000
DAY = 86400
FIXED_ROWS = [
    row("eu", "online", True, NOW - 100 * DAY, NOW - 40 * DAY),
    row("eu", "down", False, NOW - 40 * DAY, NOW - 31 * DAY),
    row("eu", "online", True, NOW - 31 * DAY, NOW - 29 * DAY - 3600),
    row("eu", "maintenance", False, NOW - 29 * DAY - 3600, NOW - 29 * DAY),
    row("eu", "online", True, NOW - 29 * DAY, NOW - 2 * DAY),
    row("eu", "down", False, NOW - 2 * DAY, NOW - 2 * DAY + 754),
    row("eu", "online", True, NOW - 2 * DAY + 754, NOW - 3600),
    row("eu", "down", True, NOW - 3600, NOW - 3600),
    row("eu", "down", False, NOW - 3600, None),
    row("eu", "online", True, NOW + 50, None),
    row("eu", "online", True, NOW - 5, NOW - 10),
    row("us", "down", False, NOW - 10 * DAY, None),
    row("pts", "online", True, NOW - 7 * DAY, NOW - 1 * DAY),
    row("pts", "down", False, NOW - 1 * DAY, None),
]
QUERIES = [
    "", "?env=live", "?env=pts", "?days=1", "?days=7", "?days=90", "?env=pts&days=1", "?days=30&env=live",
    "?env=live&env=pts", "?env=pts&env=live", "?days=+5", "?days=05", "?days=%207", "?days=2&unknown=1",
    "?env=eu", "?env=us", "?env=LIVE", "?env=", "?env=live%0A", "?env=%20live",
    "?days=0", "?days=91", "?days=-5", "?days=abc", "?days=1.5", "?days=2.0", "?days=", "?days=1e3",
    "?days=5_0", "?days=%EF%BC%97", "?days=99999999999999999999999", "?env=eu&days=0", "?days=0&env=eu",
    "?env=live&days=x&days=3",
]


def history_cases() -> list:
    out = []
    for q in QUERIES:
        out.append({"rows": FIXED_ROWS, "now": NOW, "query": q, "reply": history_reply(FIXED_ROWS, NOW, q)})
    out.append({"rows": [], "now": NOW, "query": "", "reply": history_reply([], NOW, "")})
    rng = random.Random(20261022)
    statuses = ["online", "online", "online", "down", "maintenance"]
    for _ in range(160):
        now = rng.randrange(1_600_000_000, 2_100_000_000)
        days = rng.randint(1, 90)
        t = now - rng.randrange(0, 120 * DAY)
        rows = []
        for i in range(rng.randint(0, 9)):
            dur = rng.choice([rng.randrange(1, 600), rng.randrange(1, 5 * DAY), rng.randrange(1, 60 * DAY)])
            status = rng.choice(statuses)
            last = i == 8 or rng.random() < 0.12
            rows.append(row("eu", status, status == "online", t, None if last else t + dur))
            if last:
                break
            t += dur
        q = f"?days={days}"
        out.append({"rows": rows, "now": now, "query": q, "reply": history_reply(rows, now, q)})
    return out


# ---------------------------------------------------------------------------
# prober pieces

def auth_cases() -> list:
    cases = [
        (401, {"X-Trionworlds-Error": "InvalidCredentials"}, ""),
        (200, {}, "Signature: abc\n"),
        (404, {}, "404 page not found"),
        (404, {"x-trionworlds-session": "1"}, ""),
        (500, {}, "oops"),
        (503, {}, "Signature:"),
        (302, {"Location": "/x"}, ""),
        (429, {}, ""),
        (200, {"X-Trion": "no"}, "signature: lower"),
        (599, {}, ""),
    ]
    out = []
    for status, headers, body in cases:
        online, error = st._auth_is_reachable(status, headers, body)
        out.append({"status": status, "headers": list(headers), "body": body, "online": online, "error": error})
    return out


def verdict_cases() -> dict:
    envs = [
        {}, {"eu": {"status": "online"}}, {"pts": {"status": "down"}},
        {"eu": {"status": "online"}, "us": {"status": "online"}, "pts": {"status": "down"}},
        {"eu": {"status": "online"}, "us": {"status": "down"}},
        {"eu": {"status": "unknown"}, "us": {"status": "online"}},
    ]
    return {
        "overall": [[list(e.items()), st._overall(e)] for e in envs],
        "verdict": [[a, g, st._verdict(a, g)] for a in (True, False) for g in (True, False)],
    }


async def _anchors(spec: str):
    seen = []

    async def fake(host, port, timeout):
        seen.append([host, port])
        return False

    orig = st._anchor_reachable
    st._anchor_reachable = fake
    try:
        up = await st._local_network_is_up(spec, 1.0)
    finally:
        st._anchor_reachable = orig
    return {"spec": spec, "anchors": seen, "up_when_none_answer": up}


ANCHOR_SPECS = [
    "1.1.1.1:443,9.9.9.9:443,8.8.8.8:443", "", " , ,", "host", ":443", "a:b", "a:-1", " a.example:80 ,b:x,c:0",
    "x:1:2", "[::1]:443", "h:99999", "h:+1",
]


def fromhex_cases() -> list:
    out = []
    for s in ["", "20", "2000000000", " 20 00 ", "2 0", "zz", "abc", "AbCd", "20\t00", "20\n00", "\u00a020"]:
        try:
            out.append([s, bytes.fromhex(s.strip()).hex()])
        except ValueError:
            out.append([s, None])
    return out


class _AuthHandler(BaseHTTPRequestHandler):
    routes: dict = {}
    seen: list = []

    def do_POST(self):  # noqa: N802
        n = int(self.headers.get("content-length") or 0)
        type(self).seen.append({"path": self.path, "ua": self.headers.get("user-agent"),
                                "ctype": self.headers.get("content-type"), "body": self.rfile.read(n).decode()})
        status, headers, body = self.routes[self.path]
        data = body.encode()
        self.send_response(status)
        for k, v in headers.items():
            self.send_header(k, v)
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *a):
        pass


AUTH_ROUTES = {
    "/rejected": (401, {"X-Trionworlds-Error": "InvalidCredentials"}, "denied"),
    "/ticket": (200, {}, "Signature: zzz"),
    "/gone": (404, {}, "404 page not found"),
    "/broken": (502, {}, "bad gateway"),
    "/teapot": (418, {}, ""),
}


def free_port() -> int:
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


def auth_probe_cases() -> list:
    _AuthHandler.routes = AUTH_ROUTES
    srv = HTTPServer(("127.0.0.1", 0), _AuthHandler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{srv.server_address[1]}"
    out = []
    try:
        for path in list(AUTH_ROUTES) + ["refused"]:
            url = f"http://127.0.0.1:{free_port()}/x" if path == "refused" else base + path
            settings.trove_status_auth_url = url
            r = asyncio.run(st._probe_auth())
            r["latency_ms"] = "<masked>"
            out.append({"path": path, "result": r})
    finally:
        srv.shutdown()
    return out, _AuthHandler.seen[:1]


async def _game_cases() -> list:
    async def hold(reader, writer):
        await reader.read(64)
        await asyncio.sleep(1.0)
        writer.close()

    async def drop(reader, writer):
        await reader.read(64)
        writer.write(b"\x15" + b"\x00" * 20)
        await writer.drain()
        writer.close()

    async def chatty(reader, writer):
        await reader.read(64)
        writer.write(b"x" * 300)
        await writer.drain()
        writer.close()

    async def stream(reader, writer):
        await reader.read(64)
        try:
            writer.write(b"y" * 40000)
            await writer.drain()
            await asyncio.sleep(1.0)
        except Exception:
            pass
        writer.close()

    servers = {}
    for name, cb in [("hold", hold), ("drop", drop), ("chatty", chatty), ("stream", stream)]:
        srv = await asyncio.start_server(cb, "127.0.0.1", 0)
        servers[name] = (srv, srv.sockets[0].getsockname()[1])
    closed = free_port()
    hex_ok = "2000000000" + "ab" * 32
    plan = [
        ("not_configured_host", "", 6560, {}),
        ("not_configured_port", "127.0.0.1", 0, {}),
        ("refused", "127.0.0.1", closed, {}),
        ("connect_only", "127.0.0.1", servers["hold"][1], {"deep": False, "hello_hex": hex_ok}),
        ("empty_hello", "127.0.0.1", servers["hold"][1], {"deep": True, "hello_hex": ""}),
        ("bad_hex", "127.0.0.1", servers["hold"][1], {"deep": True, "hello_hex": "zz"}),
        ("blank_hex", "127.0.0.1", servers["hold"][1], {"deep": True, "hello_hex": "   "}),
        ("held_random", "127.0.0.1", servers["hold"][1], {"deep": True, "hello_hex": "x", "random_opener": True}),
        ("held_replay", "127.0.0.1", servers["hold"][1], {"deep": True, "hello_hex": hex_ok}),
        ("dropped", "127.0.0.1", servers["drop"][1], {"deep": True, "hello_hex": hex_ok}),
        ("chatty_close", "127.0.0.1", servers["chatty"][1], {"deep": True, "hello_hex": hex_ok}),
        ("streaming", "127.0.0.1", servers["stream"][1], {"deep": True, "hello_hex": hex_ok}),
        ("dns", "nonexistent.invalid", 6560, {}),
    ]
    out = []
    for name, host, port, kw in plan:
        r = await st._probe_game(host, port, hold_seconds=0.3, **kw)
        if r["error"] != "not_configured":
            r["latency_ms"] = "<masked>"
        if port not in (0, 6560):
            r["port"] = "<port>"
        out.append({"name": name, "result": r})
    for srv, _ in servers.values():
        srv.close()
    return out


def main() -> None:
    models.TroveStatusEvent = FakeEvent
    auth_probe, seen = auth_probe_cases()
    anchors = [asyncio.run(_anchors(s)) for s in ANCHOR_SPECS]
    fixtures = {
        "current": current_cases(),
        "history": history_cases(),
        "auth_reachable": auth_cases(),
        "rollup": verdict_cases(),
        "anchors": anchors,
        "fromhex": fromhex_cases(),
        "auth_probe": auth_probe,
        "auth_request": seen,
        "game_probe": asyncio.run(_game_cases()),
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(fixtures, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    print(f"wrote {OUT.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
