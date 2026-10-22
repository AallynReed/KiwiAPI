"""Golden fixtures for kiwi-server's leaderboard ingest + maintenance port.

The parser runs in pure Python (kiwi_native off) over hand-picked and fuzzed
dumps; the anchor, guard and insert_dump composition run against fake stores
with a fixed clock; _process_leaderboard_dump runs with its collaborators
recorded; the routes are driven through ``app.main.app`` at the ASGI level with
the gate and auth dependencies overridden and the stores faked. The Rust test
(src/lb_ingest/tests.rs) replays all of it.

Run from the repo root with the production-pinned venv:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_lb_ingest_fixtures.py
"""

from __future__ import annotations

import asyncio
import base64
import datetime as dt
import json
import logging
import os
import random
import sys
import types
import urllib.parse
import warnings
from pathlib import Path, PurePosixPath

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "lb_ingest" / "golden.json"
os.chdir(ROOT)
sys.path.insert(0, str(ROOT))

import bson  # noqa: E402
from beanie import init_beanie  # noqa: E402
from beanie.odm.utils import init as beanie_init  # noqa: E402
from beanie.odm.utils.dump import get_dict  # noqa: E402
from pymongo import AsyncMongoClient  # noqa: E402

from app.admin import ingest_log  # noqa: E402
from app.admin import runtime_config as rc  # noqa: E402
from app.admin.ingest_log import IngestLogEntry  # noqa: E402
from app.auth.models import User  # noqa: E402
from app.tokens.models import ApiToken  # noqa: E402
from app.trove.leaderboards import backlog, parser, pg_store, service  # noqa: E402

UTC = dt.timezone.utc
NOW = 1_761_130_000          # 2025-10-22 10:46:40 UTC (before the 11:00 reset)
USER_ID = "64b000000000000000000001"
LOG_ID = "64b0000000000000000000aa"


def fl(x: float) -> str:
    return repr(x)


# --- parser ---------------------------------------------------------------------

BASE = (
    "; comment\n"
    "Board_A$Leaderboard_Category_Weekly$12 = Trove Mastery$Weekly##1;Bob;100|2;Al ice;99.5|3;Zoë;7\n"
    "Board_A$Leaderboard_Category_Contests$12 = Trove Mastery$Contests##1;Bob;100\n"
    "Board_B$Fav$13 = Fav board$FAVORITES##1;x;1\n"
    "Board_C$Daily$14 = Daily$Daily##1;a;1e3| |bad|4;b;-2|5;c;nope\r\n"
    "Board_D$Leaderboard_Category_Contests_Daily$15 = Overlay only$Contests##1;q;1\n"
)

EDGE_DUMPS = [
    "",
    "\n\n",
    BASE,
    "A$B$1 = N$C##1²;x;5|2;y;6\n",
    "A$B$١٢ = N$C##١;x;1|٢;y;٣.٥|3;z;1_000|4;w; 7 |5;v;\u00a08\u3000|6;u;\x1c9\n",
    "A$B$7 = N$ favorıteſ ##1;x;1\nA$B$8 = N$Real##1;x;inf|2;y;-nan|3;z;Infinity|4;q;1e400|5;r;-0\n",
    "A$Leaderboard_Category_Contests$9 = N$C##1;x;1\nA$Leaderboard_Category_Contests_Daily$9 = N$C##1;x;1\n",
    "A$B$10 = N$C##1;x;1\nA$Leaderboard_Category_Contests_Daily$10 = N$C##1;x;1\nA$Leaderboard_Category_Contests$10 = N$C##1;x;1\n",
    "A$B$11 = N$C#1;x;1\nA$B$11 = N$C###1;x;1\n",
    "A\r$B$12 = N$C##1;x;1\nA$B$12 =  N$C##1;x;1\nA$B$012 = N$C##2;x;1\n",
    "A$B$13 = N$C##1;x;1;2|1;;1|;x;1|1;x|1;x;1_|1;x;_1|1;x;1__0|1;x;0x10|1;x;1e|1;x;.|1;x;+.5|1;x;5.\n",
    "A$B$14 = N$C##" + "|".join(f"{i};p{i};{i * 1.5}" for i in range(1, 40)) + "\n",
    "A$B$15 = N$C##1;\u0000x;1|2;y\u0000;2\u0000\n",
    "A$B$16 = N$C##" + "1" * 30 + ";big;1\n",
    "A$B$" + "9" * 25 + " = N$C##1;x;1\nA$B$17 = N$C##1;x;1\n",
    "A$B$18 = N$C##١\u0660;x;1\n",
    "A$B$19 = N$C\u2028X##1;x;1\nA$B$20 = N$C##1;x;1\u2029|2;y;2\u0085\n",
    "A$B$21 = N$C$D##1;x;1\nA$$22 = N$C##1;x;1\n$B$23 = N$C##1;x;1\nA$B$24 = $C##1;x;1\nA$B$25 = N$##1;x;1\n",
    "A$B$26 = N$C##\n",
    "A$B$27 = N$C##\r\n",
]


def board_json(b) -> dict:
    return {
        "name_id": b.name_id, "category_id": b.category_id, "uuid": b.uuid, "name": b.name,
        "category": b.category, "contest": b.contest,
        "entries": [[e.rank, e.player_name, fl(e.score)] for e in b.entries],
    }


def run_parse(text: str):
    saved = parser._native
    parser._native = None
    try:
        return {"boards": [board_json(b) for b in parser.parse_dump(text)]}
    except Exception as exc:  # noqa: BLE001
        return {"error": str(exc)}
    finally:
        parser._native = saved


def parse_section() -> list:
    texts = list(EDGE_DUMPS)
    rng = random.Random(7)
    alphabet = ["|", ";", "$", "#", "\n", "\r", " ", "=", "1", "a", "١", "²", "\xa0", "\x1c", "e", ".", "-", "_",
                "ı", "ſ", "٣", "\u3000", "\x00", "9"]
    for _ in range(600):
        chars = list(BASE)
        for _ in range(rng.randrange(1, 10)):
            chars[rng.randrange(len(chars))] = rng.choice(alphabet)
        texts.append("".join(chars))
    texts.append("Board_A$B$12 = N$C##1;" + "x;" * 0 + "1" * 4301 + "x;1\n")
    texts.append("Board_A$B$12 = N$C##" + "1" * 4301 + ";x;1\n")
    texts.append("Board_A$B$" + "1" * 4301 + " = N$C##1;x;1\n")
    return [{"text": t, **run_parse(t)} for t in texts]


# --- anchors, boundaries, verdicts -----------------------------------------------

class FixedDatetime(dt.datetime):
    now_ts = NOW

    @classmethod
    def now(cls, tz=None):
        return dt.datetime.fromtimestamp(cls.now_ts, tz or UTC)


def with_now(ts: int):
    FixedDatetime.now_ts = ts
    service.datetime = FixedDatetime


def normalize_section() -> list:
    out = []
    day = 86400
    midnight = NOW - NOW % day
    stamps = [None, -5, 0, 1, NOW, NOW + 59, NOW + 299, NOW + 300, NOW + 359, NOW + 360, NOW + 3600,
              NOW - 14 * day, NOW - 14 * day - 60, NOW - 14 * day + 60, NOW - 30 * day, midnight, midnight + 59,
              midnight + 60, midnight - day, midnight - 20 * day, 1_577_836_800, 30_000_000_000]
    for now in (NOW, NOW + 13, midnight, midnight + 11 * 3600):
        with_now(now)
        for ts in stamps:
            for backfill in (False, True):
                try:
                    got = service.normalize_timestamp(ts, allow_backfill=backfill)
                except Exception as exc:  # noqa: BLE001
                    got = {"error": f"{type(exc).__name__}: {exc}"}
                out.append({"now": now, "ts": ts, "backfill": backfill, "result": got})
        out.append({"now": now, "anchor": service._trove_day_anchor()})
    return out


def boundaries_section() -> list:
    out = []
    monday_11 = 1_760_958_000
    points = [monday_11 - 86400 * 3 - 1, monday_11 - 1, monday_11, monday_11 + 1, monday_11 + 3600 * 13,
              monday_11 + 86400 * 6 + 39_599, monday_11 + 86400 * 7, monday_11 + 86400 * 15]
    for kind in ("daily", "weekly", "default", "none"):
        for a in points:
            for b in points:
                out.append({"kind": kind, "start": a, "end": b,
                            "result": service.reset_boundaries_for_kind(kind, a, b)})
    return out


def verdict_section() -> list:
    cases = [
        ({1: 500, 2: 10, 3: 200}, {1: 100, 3: 200}, 0, 0.5),
        ({1: 500, 2: 10, 3: 200}, {1: 100, 3: 200}, 1, 0.0),
        ({1: 100, 2: 100}, {1: 50, 2: 49}, 5, 0.5),
        ({1: 99, 2: 1000}, {1: 0, 2: 125}, 0, 0.125),
        ({1: 1000}, {1: 334}, 0, 0.335),
        ({1: 1000, 4: 1, 5: 1, 6: 1}, {7: 1}, 2, 0.005),
        ({}, {1: 5}, 0, 0.5),
        ({1: 200}, {1: 200}, 0, 1.0),
        ({1: 200}, {1: 199}, 0, 1.0),
        ({1: 200, 9: 300}, {1: 1, 9: 1}, 0, 2.5),
        ({1: 200}, {1: 1}, 0, -0.5),
    ]
    out = []
    for prev, cur, mm, frac in cases:
        v = service._completeness_verdict(cur, prev, max_missing=mm, collapse_frac=frac)
        out.append({"prev": [[k, n] for k, n in prev.items()], "cur": [[k, n] for k, n in cur.items()],
                    "max_missing": mm, "collapse_frac": frac, "result": v})
    return out


# --- insert_dump against fake stores -------------------------------------------------

def dump_of(boards: dict[int, int], overlay: bool = False) -> str:
    lines = []
    for uuid, n in boards.items():
        ents = "|".join(f"{i};P{uuid}x{i};{1000 - i}" for i in range(1, n + 1))
        lines.append(f"LB_{uuid}$Leaderboard_Category_Stats${uuid} = Board {uuid}$STATS##{ents}")
        if overlay:
            lines.append(f"LB_{uuid}$Leaderboard_Category_Contests${uuid} = Board {uuid}$CONTESTS##{ents}")
    return "\n".join(lines) + "\n"


SETTINGS = {
    "capture_completeness_enabled": True, "capture_min_prev_boards": 2,
    "capture_max_missing_boards": 1, "capture_collapse_frac": 0.5,
}

INSERT_CASES = [
    # name, text, timestamp, backfill, now, prev_anchor, prev_counts, overrides, settings patch
    ("empty", "junk\n", None, False, NOW, None, {}, {}, {}),
    ("cold start", dump_of({1: 3, 2: 2}), None, False, NOW, None, {}, {}, {}),
    ("explicit ts", dump_of({1: 3, 2: 2}), NOW - 600, False, NOW, NOW - 4200, {1: 3, 2: 2}, {}, {}),
    ("future ts", dump_of({1: 3, 2: 2}), NOW + 7200, False, NOW, None, {}, {}, {}),
    ("old ts", dump_of({1: 3}), NOW - 20 * 86400, False, NOW, None, {}, {}, {}),
    ("old ts backfill", dump_of({1: 3}), NOW - 20 * 86400, True, NOW, None, {}, {}, {}),
    ("midnight alias", dump_of({1: 3}), NOW - NOW % 86400, False, NOW, None, {}, {}, {}),
    ("missing boards", dump_of({1: 3}), NOW - 600, False, NOW, NOW - 4200, {1: 3, 2: 2, 3: 4}, {}, {}),
    ("missing ok", dump_of({1: 3, 2: 2}), NOW - 600, False, NOW, NOW - 4200, {1: 3, 2: 2, 3: 4}, {}, {}),
    ("collapsed", dump_of({1: 30, 2: 2}), NOW - 600, False, NOW, NOW - 4200, {1: 200, 2: 2}, {}, {}),
    ("thin baseline", dump_of({1: 3}), NOW - 600, False, NOW, NOW - 4200, {1: 3, 2: 2, 3: 4},
     {}, {"capture_min_prev_boards": 4}),
    ("guard off", dump_of({1: 3}), NOW - 600, False, NOW, NOW - 4200, {1: 3, 2: 2, 3: 4},
     {}, {"capture_completeness_enabled": False}),
    ("backfill skips guard", dump_of({1: 3}), NOW - 600, True, NOW, NOW - 4200, {1: 3, 2: 2, 3: 4}, {}, {}),
    ("reset crossed lifetime only", dump_of({1: 3}), NOW + 1200, False, NOW + 1000, NOW - 600,
     {1: 3, 2: 2, 3: 4}, {2: "none", 3: "none"}, {}),
    ("reset crossed no lifetime", dump_of({1: 3}), NOW + 1200, False, NOW + 1000, NOW - 600,
     {1: 3, 2: 2, 3: 4}, {2: "daily"}, {}),
    ("reset crossed collapse", dump_of({1: 3, 2: 10}), NOW + 1200, False, NOW + 1000, NOW - 600,
     {1: 3, 2: 300}, {2: "none", 1: "none"}, {}),
    ("contest overlay", dump_of({1: 3, 2: 2}, overlay=True), None, False, NOW, None, {}, {}, {}),
    ("rank too long", "A$B$1 = N$C##" + "1" * 4301 + ";x;1\n", None, False, NOW, None, {}, {}, {}),
]


def insert_section() -> list:
    out = []
    for name, text, ts, backfill, now, prev, prev_counts, overrides, patch in INSERT_CASES:
        with_now(now)
        settings = {**SETTINGS, **patch}
        calls: list = []

        async def get_setting(key, _s=settings):
            return _s[key]

        async def previous_anchor_before(anchor, _p=prev):
            calls.append(["previous_anchor_before", anchor])
            return _p

        async def board_counts_at(anchor, _c=prev_counts):
            calls.append(["board_counts_at", anchor])
            return dict(_c)

        async def board_kinds(uuids, _o=overrides):
            calls.append(["board_kinds", sorted(uuids)])
            return {u: pg_store._effective_reset_kind(_o.get(u), u) for u in uuids}

        async def write_snapshot(boards, anchor):
            calls.append(["write_snapshot", anchor, [b.uuid for b in boards]])
            return {"boards": len(boards), "entries": sum(len(b.entries) for b in boards), "created_at": anchor,
                    "cleared_before_insert": 5, "archived_old": 0}

        rc.get_setting = get_setting
        pg_store.previous_anchor_before = previous_anchor_before
        pg_store.board_counts_at = board_counts_at
        pg_store.board_kinds = board_kinds
        pg_store.write_snapshot = write_snapshot
        saved = parser._native
        parser._native = None
        try:
            res = asyncio.run(service.insert_dump(text, timestamp=ts, allow_backfill=backfill))
            res = {"summary": res}
        except Exception as exc:  # noqa: BLE001
            res = {"error": str(exc)}
        finally:
            parser._native = saved
        out.append({"name": name, "text": text, "timestamp": ts, "backfill": backfill, "now": now,
                    "prev_anchor": prev, "prev_counts": [[k, n] for k, n in prev_counts.items()],
                    "overrides": [[k, v] for k, v in overrides.items()], "settings": settings,
                    "calls": calls, **res})
    return out


# --- _process_leaderboard_dump -------------------------------------------------------

def bson_hex(doc: dict) -> str:
    return bson.encode(doc).hex()


PROCESS_CASES = [
    ("stored", {"boards": 3, "entries": 70000, "created_at": 1_761_130_800, "cleared_before_insert": 12,
                "archived_old": 0}, None, 120, True),
    ("stored no warm", {"boards": 3, "entries": 7, "created_at": 1_761_130_800, "cleared_before_insert": 0,
                        "archived_old": 0}, 1_761_130_800, 120, False),
    ("stored fallback", {"boards": 3, "entries": 7, "created_at": 1_761_044_400, "cleared_before_insert": 0,
                         "archived_old": 0, "rejected_timestamp": 99_999_999_999}, 99_999_999_999, 3_000_000_000, True),
    ("empty", {"boards": 0, "entries": 0, "created_at": None}, None, 5, True),
    ("skipped", {"skipped": True, "created_at": 1_761_130_800, "boards": 2, "entries": 9,
                 "reason": "2 board(s) missing vs previous capture", "missing_boards": [3, 40000],
                 "collapsed_boards": [], "prev_anchor": 1_761_127_200, "prev_boards": 4, "cur_boards": 2},
     -3, 77, True),
    ("failed", RuntimeError("boom " + "é" * 400), 1_761_130_800, 10, True),
    ("failed no ts", ValueError("Exceeds the limit (4300 digits) for integer string conversion"), None, 10, False),
]


def process_section(trove_router) -> list:
    from app.events import bus as events_bus
    from app.trove.leaderboards import cache as lb_cache
    from app.trove.leaderboards import detection

    user = types.SimpleNamespace(id=bson.ObjectId(USER_ID), email="master@aallyn.net")
    out = []
    for name, result, ts, nbytes, warm in PROCESS_CASES:
        calls: list = []

        async def insert_dump(text, *, timestamp=None, allow_backfill=False, _r=result):
            if isinstance(_r, Exception):
                raise _r
            return dict(_r)

        async def record(*, endpoint, user, token=None, summary=None, success=True, error=None):
            calls.append(["log", endpoint, success, error, bson_hex({"summary": summary or {}})])

        async def invalidate_anchor(anchor):
            calls.append(["invalidate", anchor])

        async def save(anchor, text):
            calls.append(["save", anchor])

        async def publish(event_type, signature, data):
            calls.append(["publish", event_type, signature, data])
            return True

        trove_router.leaderboards_service.insert_dump = insert_dump
        ingest_log.record = record
        lb_cache.invalidate_anchor = invalidate_anchor
        backlog.save = save
        events_bus.publish = publish
        detection.trigger_warmer = lambda: calls.append(["warm"])
        got = asyncio.run(trove_router._process_leaderboard_dump(
            "dump", ts, user, None, nbytes, allow_backfill=False, warm=warm))
        out.append({"name": name, "timestamp": ts, "nbytes": nbytes, "warm": warm,
                    "result": None if isinstance(result, Exception) else result,
                    "error": str(result) if isinstance(result, Exception) else None,
                    "returned": got, "calls": calls})
    return out


async def log_doc_section() -> list:
    real_insert = IngestLogEntry.insert
    captured: list = []

    async def fake_insert(self, *a, **k):
        d = get_dict(self, to_db=True, keep_nulls=self.get_settings().keep_nulls)
        captured.append(bson_hex({"_id": bson.ObjectId(LOG_ID), **d}))
        return self

    class Fixed(dt.datetime):
        @classmethod
        def now(cls, tz=None):
            return dt.datetime(2025, 10, 22, 11, 2, 3, 456789, tzinfo=UTC)

    IngestLogEntry.insert = fake_insert
    ingest_log.datetime = Fixed
    user = User.model_construct(id=bson.ObjectId(USER_ID), email="master@aallyn.net")
    token = ApiToken.model_construct(name="capture bot")
    cases = [
        ({"boards": 3, "entries": 7, "anchor": 1_761_130_800, "cleared_before_insert": 0, "archived_old": 0,
          "rejected_timestamp": None, "bytes": 120}, True, None, token),
        ({"rejected": True, "anchor": 1_761_130_800, "missing_boards": [1, 3_000_000_000]}, False,
         "rejected incomplete capture: 1 board(s) missing vs previous capture", None),
        ({"bytes": 1, "anchor": None}, False, "boom", token),
    ]
    out = []
    for summary, success, error, tok in cases:
        captured.clear()
        await ingest_log.record(endpoint="/v1/leaderboards/insert", user=user, token=tok, summary=summary,
                                success=success, error=error)
        out.append({"summary": bson_hex({"summary": summary}), "success": success, "error": error,
                    "token_name": tok.name if tok else None, "bson": captured[0]})
    IngestLogEntry.insert = real_insert
    ingest_log.datetime = dt.datetime
    return out


# --- backlog --------------------------------------------------------------------------

def backlog_section() -> dict:
    names = ["1700000000.cfg.gz", "1700000000.cfg", "x1700000000y", "170000000.cfg", "12345678901234.cfg",
             "1577836799.cfg", "1577836800.cfg", "2051222400.cfg", "2051222401.cfg", "17000000001.cfg",
             "abc", "١٧٠٠٠٠٠٠٠٠.cfg", "12_1700000000", "99999999 1700000000", "1700000000\u00b2"]
    dirs = ["/data/backlog", "/data/backlog/", "data/backlog", "./data//backlog/.", "", ".", "/", "//x", "///x",
            "a/../b"]
    return {
        "anchor_of": [[n, backlog._anchor_of(n)] for n in names],
        "dir": [[d, str(PurePosixPath(d) / "leaderboards")] for d in dirs],
    }


def event_section() -> list:
    out = []
    for data in ({"anchor": 1_761_130_800, "boards": 3, "entries": 7}, {"anchor": 1, "boards": 0, "entries": 0}):
        payload = {"type": "leaderboard", "data": data, "ts": 1_761_130_123}
        out.append({"data": data, "ts": 1_761_130_123, "payload": json.dumps(payload, default=str)})
    return out


# --- routes -------------------------------------------------------------------------

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


def multipart(parts: list[tuple[str, str | None, bytes, str | None]], boundary: str = "XyZkiwi") -> bytes:
    out = b""
    for name, filename, data, ctype in parts:
        out += f"--{boundary}\r\n".encode()
        disp = f'Content-Disposition: form-data; name="{name}"'
        if filename is not None:
            disp += f'; filename="{filename}"'
        out += disp.encode() + b"\r\n"
        if ctype:
            out += f"Content-Type: {ctype}\r\n".encode()
        out += b"\r\n" + data + b"\r\n"
    return out + f"--{boundary}--\r\n".encode()


MP = "multipart/form-data; boundary=XyZkiwi"
DUMP = dump_of({1: 2}).encode()

INSERT_HTTP = [
    # name, query, content-type, body, fake summary for sync ("fail" = None)
    ("ok async", "", MP, multipart([("file", "LeaderBot.cfg", DUMP, "text/plain")]), None),
    ("ok sync stored", "?sync=true&timestamp=1761130800", MP,
     multipart([("file", "LeaderBot.cfg", DUMP, None)]),
     {"boards": 1, "entries": 2, "created_at": 1_761_130_800, "cleared_before_insert": 0, "archived_old": 0}),
    ("ok sync empty", "?sync=1", MP, multipart([("file", "a", b"junk", None)]),
     {"boards": 0, "entries": 0, "created_at": None}),
    ("ok sync skipped", "?sync=yes&warm=false", MP, multipart([("file", "a", DUMP, None)]),
     {"skipped": True, "created_at": 1_761_130_800, "boards": 1, "entries": 2,
      "reason": "1 board(s) missing vs previous capture; 1 board(s) collapsed below 50% of previous",
      "missing_boards": [2], "collapsed_boards": [1], "prev_anchor": 1, "prev_boards": 2, "cur_boards": 1}),
    ("ok sync failed", "?sync=on&backfill=true", MP, multipart([("file", "a", DUMP, None)]), "fail"),
    ("non-utf8", "?sync=true", MP, multipart([("file", "a", b"A$B$1 = N$C##1;\xff\xfe;1\n", None)]),
     {"boards": 1, "entries": 1, "created_at": 5, "cleared_before_insert": 0, "archived_old": 0}),
    ("empty file", "", MP, multipart([("file", "a", b"", None)]), None),
    ("no file", "", MP, multipart([("other", None, b"x", None)]), None),
    ("text field", "", MP, multipart([("file", None, b"hello", None)]), None),
    ("empty text field", "", MP, multipart([("file", None, b"", None)]), None),
    ("last file wins", "", MP, multipart([("file", "a", b"", None), ("file", None, b"txt", None)]), None),
    ("text then file", "?sync=true", MP, multipart([("file", None, b"txt", None), ("file", "b", DUMP, None)]),
     {"boards": 1, "entries": 2, "created_at": 7, "cleared_before_insert": 3, "archived_old": 0}),
    ("bad query", "?timestamp=abc&backfill=maybe&sync=2&warm=", MP, multipart([("other", None, b"x", None)]), None),
    ("bad ts float", "?timestamp=1.5", MP, multipart([("file", "a", DUMP, None)]), None),
    ("ts float zero", "?timestamp=17.0&sync=true", MP, multipart([("file", "a", DUMP, None)]),
     {"boards": 1, "entries": 2, "created_at": 17, "cleared_before_insert": 0, "archived_old": 0}),
    ("empty ts", "?timestamp=", MP, multipart([("file", "a", DUMP, None)]), None),
    ("no body", "", None, b"", None),
    ("json body", "", "application/json", b'{"file": "x"}', None),
    ("urlencoded", "", "application/x-www-form-urlencoded", b"file=abc", None),
    ("urlencoded empty", "", "application/x-www-form-urlencoded", b"file=", None),
    ("no boundary", "", "multipart/form-data", b"--x\r\n", None),
    ("garbage multipart", "", MP, b"garbage", None),
]


def install_route_fakes(trove_router) -> dict:
    state: dict = {"summary": None}

    async def process(text, timestamp, user, token, nbytes, *, allow_backfill=False, warm=True):
        state.setdefault("seen", []).append([text, timestamp, nbytes, allow_backfill, warm])
        s = state["summary"]
        return None if s == "fail" else s

    trove_router._process_leaderboard_dump = process
    return state


async def http_section(app, trove_router) -> dict:
    from app.admin import router as admin_router
    from app.core.dependencies import IngestAuth, get_current_superuser, require_master_ingest
    from app.core.features import require_leaderboards_enabled

    user = types.SimpleNamespace(id=bson.ObjectId(USER_ID), email="master@aallyn.net", is_superuser=True)
    app.dependency_overrides[require_leaderboards_enabled] = lambda: None
    app.dependency_overrides[require_master_ingest] = lambda: IngestAuth(user=user, token=None)
    app.dependency_overrides[get_current_superuser] = lambda: user
    fake = install_route_fakes(trove_router)
    out: dict = {"insert": [], "other": []}

    async def go(method, url, ctype, body, rid, extra=()):
        headers = [(b"x-request-id", rid.encode())] + list(extra)
        if ctype:
            headers.append((b"content-type", ctype.encode()))
        status, rh, raw = await call(app, method, url, headers, body)
        get = lambda k: next((v for n, v in rh if n == k), None)  # noqa: E731
        return {"status": status, "allow": get("allow"), "response_type": get("content-type"),
                "response": raw.decode("utf-8")}

    for i, (name, query, ctype, body, summary) in enumerate(INSERT_HTTP):
        fake["summary"] = summary
        fake["seen"] = []
        rid = f"parity-lbi-{i:03d}"
        res = await go("POST", "/v1/leaderboards/insert" + query, ctype, body, rid)
        out["insert"].append({"name": name, "query": query, "content_type": ctype,
                              "body_b64": base64.b64encode(body).decode(), "summary": summary,
                              "request_id": rid, "seen": fake["seen"], **res})

    # reset
    reset_summaries = [
        {"hot_entries_deleted": 123456, "archive_entries_deleted": 0, "activity_estimates_deleted": 7,
         "boards_deleted": 0, "dropped_boards": False, "redis_keys_cleared": 3},
        {"hot_entries_deleted": -1, "archive_entries_deleted": 0, "activity_estimates_deleted": 0,
         "boards_deleted": 212, "dropped_boards": True, "redis_keys_cleared": 0},
    ]
    logged: list = []

    async def record(**kw):
        logged.append(bson_hex({"summary": kw.get("summary") or {}}))

    trove_router.ingest_log.record = record
    for i, (q, s) in enumerate([("", reset_summaries[0]), ("?drop_boards=true", reset_summaries[1]),
                                ("?drop_boards=nah", reset_summaries[0])]):
        async def reset_all(*, drop_boards=False, _s=s):
            return dict(_s)
        trove_router.leaderboards_service.reset_all = reset_all
        logged.clear()
        res = await go("POST", "/v1/leaderboards/reset" + q, None, b"", f"parity-lbr-{i}")
        out["other"].append({"name": f"reset {q}", "kind": "reset", "query": q, "fake": s,
                             "logged": list(logged), **res})

    # reingest-status
    statuses = [
        None,
        {"running": True, "total": 3, "done": 1, "ok": 1, "failed": 0, "started_at": 1_761_130_000,
         "finished_at": None, "last_anchor": 1_700_000_000, "clear_first": False, "errors": []},
        {"running": False, "phase": "done", "errors": [{"anchor": 1, "error": "bad ünicode"}],
         "backlog_files": 99},
    ]
    for i, st in enumerate(statuses):
        async def get_status(_s=st):
            return dict(_s) if _s is not None else {"running": False}
        backlog.get_status = get_status
        backlog.info = lambda: {"backlog_dir": "/data/backlog/leaderboards", "backlog_dir_exists": True,
                                "backlog_files": 4}
        res = await go("GET", "/v1/leaderboards/reingest-status", None, b"", f"parity-lbs-{i}")
        out["other"].append({"name": f"reingest-status {i}", "kind": "reingest_status",
                             "redis": None if st is None else json.dumps(st), **res})

    # admin boards
    rows = [
        {"uuid": 2001, "name": "Delve", "name_id": "LB_D", "category": "STATS", "category_id": "C1",
         "reset_kind_override": None},
        {"uuid": 32000, "name": "Leviathans", "name_id": "LB_L", "category": "STATS", "category_id": "C1",
         "reset_kind_override": "none"},
        {"uuid": 1, "name": "Trove Mastery", "name_id": "LB_T", "category": "MASTERY", "category_id": "C2",
         "reset_kind_override": "weekly"},
        {"uuid": 5, "name": "Zeta", "name_id": "LB_Z", "category": "ÉLITE", "category_id": "C3",
         "reset_kind_override": "bogus"},
        {"uuid": 3, "name": "Delve", "name_id": "LB_D3", "category": "STATS", "category_id": "C1",
         "reset_kind_override": "daily"},
        {"uuid": 4, "name": "delve", "name_id": "LB_d", "category": "STATS", "category_id": "C1",
         "reset_kind_override": None},
    ]

    async def admin_list_boards():
        return [dict(r) for r in rows]

    pg_store.admin_list_boards = admin_list_boards
    res = await go("GET", "/admin/leaderboards/boards", None, b"", "parity-lba-0")
    out["other"].append({"name": "admin boards", "kind": "admin_boards", "rows": rows, **res})

    async def set_reset_kind_override(uuid, value):
        for r in rows:
            if r["uuid"] == uuid:
                return {**r, "reset_kind_override": value}
        return None

    pg_store.set_reset_kind_override = set_reset_kind_override
    import app.trove.leaderboards.detection as detection
    detection.trigger_warmer = lambda: None
    detection.invalidate_cache = lambda: None
    patches = [
        ("2001", "application/json", b'{"reset_kind_override": "none"}'),
        ("2001", "application/json", b'{"reset_kind_override": null}'),
        ("2001", "application/json", b"{}"),
        ("32000", "application/json", b'{"reset_kind_override": "weekly", "extra": 1}'),
        ("999", "application/json", b'{"reset_kind_override": "daily"}'),
        ("2001", "application/json", b'{"reset_kind_override": "Daily"}'),
        ("2001", "application/json", b'{"reset_kind_override": "it\'s"}'),
        ("2001", "application/json", b'{"reset_kind_override": 5}'),
        ("abc", "application/json", b'{"reset_kind_override": 5}'),
        ("abc", "application/json", b'[]'),
        ("1.0", "application/json", b'{"reset_kind_override": "weekly"}'),
        ("%31", "application/json", b'{"reset_kind_override": "weekly"}'),
        ("2001", None, b""),
        ("2001", "application/json", b"{bad"),
        ("2001", "text/plain", b'{"reset_kind_override": "none"}'),
        ("99999999999", "application/json", b'{"reset_kind_override": "zz"}'),
    ]
    for i, (uuid, ctype, body) in enumerate(patches):
        res = await go("PATCH", f"/admin/leaderboards/boards/{uuid}", ctype, body, f"parity-lbp-{i}")
        out["other"].append({"name": f"patch {uuid} {body!r}", "kind": "patch", "uuid": uuid,
                             "content_type": ctype, "body_b64": base64.b64encode(body).decode(), **res})

    # ingest log
    t0 = dt.datetime(2025, 10, 22, 11, 2, 3, 456000, tzinfo=UTC)
    docs = [
        {"_id": bson.ObjectId(), "endpoint": "/v1/leaderboards/insert", "timestamp": t0,
         "user_id": bson.ObjectId(USER_ID), "user_email": "master@aallyn.net", "success": True,
         "summary": {"boards": 3, "entries": bson.Int64(7), "anchor": None, "ratio": 0.5, "big": 1e16,
                     "nested": {"list": [1, "a", None, True]}},
         "error": None, "auth_via": "token", "token_name": "bot"},
        {"_id": bson.ObjectId(), "endpoint": "/v1/market/insert", "timestamp": t0.replace(microsecond=0),
         "user_id": bson.ObjectId(USER_ID), "user_email": "m@x", "success": False, "summary": {},
         "error": "rejected ünicode"},
    ]

    class FakeFind:
        def __init__(self, items):
            self.items = items

        def sort(self, *a, **k):
            return self

        def limit(self, n):
            self.n = n
            return self

        async def to_list(self):
            return self.items[: self.n]

    seen_q: list = []

    def find(q, *a, **k):
        seen_q.append(dict(q))
        return FakeFind([IngestLogEntry.model_validate(d) for d in docs])

    IngestLogEntry.find = staticmethod(find)
    for i, q in enumerate(["", "?limit=1", "?limit=0", "?limit=201&endpoint=", "?endpoint=/v1/market/insert",
                           "?limit=x"]):
        seen_q.clear()
        res = await go("GET", "/admin/ingest/log" + q, None, b"", f"parity-lbl-{i}")
        out["other"].append({"name": f"ingest log {q}", "kind": "ingest_log", "query": q,
                             "docs": [bson_hex(d) for d in docs], "filter": list(seen_q), **res})

    # job statuses + starts
    admin_router._agg_rebuild_status.update(running=False, captures=0)
    res = await go("GET", "/admin/leaderboards/rebuild-player-agg/status", None, b"", "parity-lbj-0")
    out["other"].append({"name": "rebuild status", "kind": "agg_status", **res})
    admin_router._last_rise_status.update(running=False, captures=12, first_anchor=1, anchor=9, rows=3,
                                          started_at=5, finished_at=6, error="x")
    res = await go("GET", "/admin/leaderboards/backfill-last-rise/status", None, b"", "parity-lbj-1")
    out["other"].append({"name": "last rise status", "kind": "agg_status",
                         "job": dict(admin_router._last_rise_status), **res})
    trove_router.settings.postgres_dsn = "postgresql://x"
    admin_router._agg_rebuild_status["running"] = True
    for i, path in enumerate(["/admin/leaderboards/rebuild-player-agg"]):
        res = await go("POST", path, None, b"", f"parity-lbj-2{i}")
        out["other"].append({"name": "rebuild busy", "kind": "busy", "path": path, **res})
    admin_router._agg_rebuild_status["running"] = False
    admin_router._last_rise_status["running"] = True
    res = await go("POST", "/admin/leaderboards/backfill-last-rise", None, b"", "parity-lbj-3")
    out["other"].append({"name": "last rise busy", "kind": "busy", **res})
    admin_router._last_rise_status["running"] = False
    admin_router._cold_tier_status["running"] = True
    res = await go("POST", "/admin/leaderboards/tier-cold", None, b"", "parity-lbj-4")
    out["other"].append({"name": "cold busy", "kind": "busy", **res})
    admin_router._cold_tier_status.update(running=False, moved=["entry_p_1"], moved_bytes=10, finished_at=7)

    # tier status layout from partitions
    parts = [
        {"oid": 1, "relname": "entry_p_1760958000", "tablespace": "pg_default", "bytes": 100},
        {"oid": 2, "relname": "entry_p_1761044400", "tablespace": "pg_default", "bytes": 250},
        {"oid": 3, "relname": "entry_p_1760000000", "tablespace": "cold", "bytes": 1000},
        {"oid": 4, "relname": "entry_default", "tablespace": "pg_default", "bytes": 1},
        {"oid": 5, "relname": "entry_p_1700000000", "tablespace": "other", "bytes": 5},
    ]
    settings_obj = trove_router.settings

    async def tier_get_setting(key):
        return 2

    for i, cold in enumerate([True, False]):
        async def exists(_c=cold):
            return _c

        class Con:
            async def fetch(self, *a, **k):
                return parts

        class Acq:
            async def __aenter__(self):
                return Con()

            async def __aexit__(self, *a):
                return False

        pg_store.cold_tablespace_exists = exists
        pg_store.acquire = lambda: Acq()
        rc.get_setting = tier_get_setting
        settings_obj.postgres_dsn = "postgresql://x"
        import time as _time
        real_time = _time.time
        _time.time = lambda: 1_761_130_000.5
        try:
            res = await go("GET", "/admin/leaderboards/tier-cold/status", None, b"", f"parity-lbt-{i}")
        finally:
            _time.time = real_time
        out["other"].append({"name": f"tier status cold={cold}", "kind": "tier_status", "cold": cold,
                             "parts": parts, "now": 1_761_130_000, "after_days": 2, **res})
    settings_obj.postgres_dsn = None
    for path in ["/admin/leaderboards/rebuild-player-agg", "/admin/leaderboards/tier-cold"]:
        res = await go("POST", path, None, b"", "parity-lbj-9")
        out["other"].append({"name": f"no pg {path}", "kind": "no_pg", "path": path, **res})
    res = await go("GET", "/admin/leaderboards/tier-cold/status", None, b"", "parity-lbj-10")
    out["other"].append({"name": "tier status no pg", "kind": "tier_no_pg",
                         "job": dict(admin_router._cold_tier_status), **res})

    # wrong methods
    for i, (method, path) in enumerate([
        ("GET", "/v1/leaderboards/insert"), ("PUT", "/v1/leaderboards/reset"),
        ("POST", "/v1/leaderboards/reingest-status"), ("POST", "/admin/leaderboards/boards"),
        ("GET", "/admin/leaderboards/boards/1"), ("DELETE", "/admin/ingest/log"),
        ("GET", "/admin/leaderboards/tier-cold"), ("POST", "/admin/leaderboards/tier-cold/status"),
        ("HEAD", "/v1/leaderboards/reset"),
    ]):
        res = await go(method, path, None, b"", f"parity-lbm-{i}")
        out["other"].append({"name": f"{method} {path}", "kind": "method", "method": method, "path": path, **res})
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
    data["parse"] = parse_section()
    data["normalize"] = normalize_section()
    data["boundaries"] = boundaries_section()
    data["verdicts"] = verdict_section()
    data["log_docs"] = await log_doc_section()
    data["backlog"] = backlog_section()
    data["events"] = event_section()
    from app.main import app
    from app.trove import router as trove_router
    data["http"] = await http_section(app, trove_router)
    OUT.parent.mkdir(parents=True, exist_ok=True)
    tmp = {k: v for k, v in data.items()}
    OUT.write_text(json.dumps(tmp, ensure_ascii=True, indent=1) + "\n", encoding="utf-8")
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes) with Python {data['python']}")


def sync_sections() -> dict:
    """insert_dump + _process_leaderboard_dump with their collaborators faked;
    every patched attribute is put back afterwards."""
    from app.events import bus as events_bus
    from app.trove import router as trove_router
    from app.trove.leaderboards import cache as lb_cache
    from app.trove.leaderboards import detection

    patched = [(rc, "get_setting"), (pg_store, "previous_anchor_before"), (pg_store, "board_counts_at"),
               (pg_store, "board_kinds"), (pg_store, "write_snapshot"), (service, "insert_dump"),
               (service, "datetime"), (ingest_log, "record"), (lb_cache, "invalidate_anchor"),
               (backlog, "save"), (events_bus, "publish"), (detection, "trigger_warmer")]
    saved = [(m, a, getattr(m, a)) for m, a in patched]
    try:
        return {"insert_dump": insert_section(), "process": process_section(trove_router)}
    finally:
        for m, a, v in saved:
            setattr(m, a, v)


if __name__ == "__main__":
    extra = sync_sections()
    asyncio.run(main())
    golden = json.loads(OUT.read_text(encoding="utf-8"))
    golden.update(extra)
    OUT.write_text(json.dumps(golden, ensure_ascii=True, indent=1) + "\n", encoding="utf-8")
    print("added insert_dump + process sections")
