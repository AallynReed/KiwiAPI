"""Golden fixtures for kiwi-server's leaderboards read routes (src/lbread), from Python.

Drives the REAL /v1/leaderboards and /site/leaderboards read routes (the scope
dependencies overridden, the real feature gates, the app's error handlers) over a
fake pg_store whose every call and answer is recorded, a fake Redis, fixed runtime
settings and a frozen clock. src/lbread/tests.rs replays each case through the same
Rust code the handlers run and compares status, headers, body, store calls and Redis
writes. Pure helpers (reset boundaries, reset zeros, timestamps, mastery, health)
get their own tables.

Run from the repo root with the production-pinned venv:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_lbread_fixtures.py
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import random
import sys
import tempfile
import warnings
from datetime import UTC, datetime
from pathlib import Path
from types import SimpleNamespace

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "lbread" / "fixtures.json"
os.chdir(ROOT)
sys.path.insert(0, str(ROOT))

from fastapi import FastAPI  # noqa: E402
from fastapi.testclient import TestClient  # noqa: E402

import app.site.router as site_router  # noqa: E402
import app.trove.router as trove_router  # noqa: E402
from app.admin import runtime_config  # noqa: E402
from app.core.config import settings as app_settings  # noqa: E402
from app.core.dependencies import AccessContext  # noqa: E402
from app.core.errors import register_error_handlers  # noqa: E402
from app.core.ratelimit import RateLimitInfo  # noqa: E402
from app.core.features import require_leaderboards_enabled  # noqa: E402
from app.trove.leaderboards import cache, mastery, pg_store, service  # noqa: E402
from app.trove.leaderboards.models import is_player_board  # noqa: E402

DAY = 86400
NOW = datetime(2025, 10, 22, 14, 30, 17, 654321, tzinfo=UTC)
D0 = int(datetime(2025, 10, 22, 11, tzinfo=UTC).timestamp())
TOKEN_ID = "64f0c0ffee0000000000abcd"

# --- fake Postgres ----------------------------------------------------------

BOARDS = [
    (1, "Leaderboard_TroveMastery", "Trove Mastery", "Leaderboard_Category_Mastery", "Mastery", None),
    (20, "Leaderboard_GeodeMastery", "Geode Mastery", "Leaderboard_Category_Mastery", "Mastery", None),
    (1000, "Leaderboard_PR_Knight", "Knight Power Rank", "Leaderboard_Category_PowerRank", "Power Rank", None),
    (1003, "Leaderboard_PR_IceSage", "Ice Sage Power Rank", "Leaderboard_Category_PowerRank", "Power Rank", None),
    (1016, "Leaderboard_PR_Solarion", "Solarion Power Rank", "Leaderboard_Category_PowerRank", "Power Rank", "weekly"),
    (4000, "Leaderboard_Effort_Knight", "Knight Effort", "Leaderboard_Category_Effort", "Effort", None),
    (32000, "Leaderboard_Leviathans", "Leviathans", "Leaderboard_Category_Daily", "Daily", None),
    (1100, "Leaderboard_ClubPR", "Club Power Rank", "Leaderboard_Category_Clubs", "Clubs", None),
    (2001, "Leaderboard_DelveWins", "Delve Wins \u00e9t\u00e9", "Leaderboard_Category_Delves", "Delves", "none"),
    (50000, "Leaderboard_\u65e5\u672c", "\u00dcn\u00efc\u00f8d\u00e9 \u65e5\u672c \"q\"", "Leaderboard_Category_Z",
     "Zeta \\ cat", "monthly"),
    (777, "Leaderboard_Empty", "Never Captured", "Leaderboard_Category_Z", "Zeta \\ cat", None),
]
# Python's str.lower() and the DB's name_lower disagree for some of these on purpose.
PLAYERS = [
    (1, "Alpha", "alpha"), (2, "bravo", "bravo"), (3, "Charlie_99", "charlie_99"),
    (4, "\u03a3\u038a\u03a3\u03a5\u03a6\u039f\u03a3", "\u03c3\u03af\u03c3\u03c5\u03c6\u03bf\u03c2"),
    (5, "\u0130stanbul", "i\u0307stanbul"), (6, "\u1e9eIG", "\u00dfig"), (7, "Delta", "delta"),
    (8, "echo", "echo"), (9, "Foxtrot", "foxtrot"), (10, "golf\u00e9", "golf\u00e9"),
    (11, "Hotel", "hotel"), (12, "India", "india"), (13, "Juliet", "juliet"),
    (14, "KILO", "kilo"), (15, "Lima", "lima"), (16, "\u039c\u0399\u039a\u03a1\u039f\u03a3", "\u03bc\u03b9\u03ba\u03c1\u03bf\u03c3"),
]
NAME = {pid: n for pid, n, _ in PLAYERS}
LOWER = {pid: lo for pid, _, lo in PLAYERS}

ANCHORS = sorted({
    D0 + 3 * 3600, D0 + 3600, D0,
    D0 - DAY + 20 * 3600, D0 - DAY + 5 * 3600, D0 - DAY,
    D0 - 2 * DAY + 7 * 3600, D0 - 2 * DAY,
    D0 - 3 * DAY + 9 * 3600,
    D0 - 5 * DAY, D0 - 8 * DAY, D0 - 12 * DAY, D0 - 20 * DAY,
}, reverse=True)
TRICKY = [1e16, 2.5e16, 0.1 + 0.2, 1e-05, 123456789.125, 7.0, 0.0, 5e-324, 1.7976931348623157e308]


def build_entries() -> list[tuple[int, int, int, float, int]]:
    rng = random.Random(20251022)
    rows: list[tuple[int, int, int, float, int]] = []
    for uuid, *_ in BOARDS:
        if uuid == 777:
            continue
        base = {1: 4_000_000.0, 20: 2_500_000.0, 1000: 41_000.0, 1003: 39_500.0, 1016: 52_000.0,
                4000: 90_000.0, 32000: 12_000.0, 1100: 300_000.0, 2001: 50.0, 50000: 3.0}[uuid]
        for k, anchor in enumerate(sorted(ANCHORS)):
            if uuid == 1003 and anchor < D0:
                continue                      # only today's captures: no prior snapshot
            if uuid == 32000 and anchor not in (D0, D0 - DAY + 5 * 3600, D0 - 3 * DAY + 9 * 3600):
                continue
            pids = list(range(1, 17))
            rng.shuffle(pids)
            pids = pids[: 9 if uuid != 1100 else 5]
            if uuid == 1000 and anchor == D0 + 3 * 3600:
                pids = [16, 4, 5, 6, 1, 2, 3, 7, 8]
            scores = sorted((base * (1 + 0.02 * k) * rng.uniform(0.2, 1.0) for _ in pids), reverse=True)
            if uuid == 50000:
                scores = sorted(TRICKY[: len(pids)], reverse=True)
            if uuid == 2001 and k % 3 == 0:
                scores[-1] = 0.0
            for rank, (pid, score) in enumerate(zip(pids, scores), start=1):
                digits = rng.choice([0, 1, 3])
                rows.append((uuid, anchor, rank, score if uuid == 50000 else float(round(score, digits)), pid))
            if uuid in (1000, 4000) and anchor >= D0 - 2 * DAY:
                # Trove's dump listing one name twice on a board.
                rows.append((uuid, anchor, len(pids) + 1, 11.5 + k, pids[0]))
    # Rank-1 ties broken by the lowest player id.
    rows.append((20, D0 + 3 * 3600, 1, 9_999_999_999.0, 2))
    rows.append((1003, D0 + 3 * 3600, 1, 61_234.5, 3))
    return rows


ENTRIES = build_entries()
CONTESTS = [(1000, D0 - DAY, "daily"), (1000, D0 - 8 * DAY, "weekly"), (1, D0, "daily")]
OVERRIDE = {u: o for u, *_, o in BOARDS}


def board_dict(b) -> dict:
    uuid, name_id, name, category_id, category, override = b
    return {"uuid": uuid, "name_id": name_id, "name": name, "category_id": category_id,
            "category": category, "reset_kind_override": override}


CALLS: list = []


def recorded(fn):
    async def wrapper(*args, **kwargs):
        ret = await fn(*args, **kwargs)
        CALLS.append([fn.__name__, [*args, *kwargs.values()], ret])
        return ret
    wrapper.__name__ = fn.__name__
    return wrapper


@recorded
async def list_timestamps(limit=60, since=None):
    out = sorted({a for _, a, *_ in ENTRIES if since is None or a >= since}, reverse=True)
    return out[:limit]


@recorded
async def list_days(limit=40):
    days: dict[int, int] = {}
    for _, a, *_ in ENTRIES:
        k = pg_store.trove_day_start(a)
        days[k] = max(days.get(k, a), a)
    return sorted(days.values(), reverse=True)[:limit]


@recorded
async def list_boards_at(anchor):
    present = {u for u, a, *_ in ENTRIES if a == anchor}
    contest = {u: t for u, a, t in CONTESTS if a == anchor}
    rows = [board_dict(b) for b in BOARDS if b[0] in present]
    rows.sort(key=lambda r: (r["category"], r["name"]))
    return [
        {"uuid": r["uuid"], "name_id": r["name_id"], "name": r["name"],
         "category_id": r["category_id"], "category": r["category"],
         "contest_type": contest.get(r["uuid"]),
         "reset_kind": pg_store._effective_reset_kind(r["reset_kind_override"], r["uuid"]),
         "reset_kind_override": r["reset_kind_override"],
         "player_board": is_player_board(r["uuid"])}
        for r in rows
    ]


@recorded
async def get_board(uuid):
    b = next((b for b in BOARDS if b[0] == uuid), None)
    if b is None:
        return None
    r = board_dict(b)
    return {
        "uuid": r["uuid"], "name_id": r["name_id"], "name": r["name"],
        "category_id": r["category_id"], "category": r["category"],
        "reset_kind": pg_store._effective_reset_kind(r["reset_kind_override"], r["uuid"]),
        "reset_kind_override": r["reset_kind_override"],
        "player_board": is_player_board(r["uuid"]),
        "contests": [{"time": a, "type": t} for u, a, t in sorted(CONTESTS, key=lambda c: c[1]) if u == uuid],
    }


@recorded
async def latest_anchor_for_board(uuid):
    return max((a for u, a, *_ in ENTRIES if u == uuid), default=None)


@recorded
async def top_entries_for_boards(uuids, anchor):
    out = {}
    for uuid in uuids:
        rows = sorted((e for e in ENTRIES if e[0] == uuid and e[1] == anchor), key=lambda e: (e[2], e[4]))
        if rows:
            u, a, _, score, pid = rows[0]
            out[uuid] = {"player_name": NAME[pid], "score": score, "anchor": a}
    return out


@recorded
async def list_entries(uuid, anchor, *, limit, offset):
    rows = sorted((e for e in ENTRIES if e[0] == uuid and e[1] == anchor), key=lambda e: e[2])
    items = [{"player_name": NAME[pid], "rank": rank, "score": score} for _, _, rank, score, pid in rows]
    return items[offset:offset + limit], len(rows)


@recorded
async def previous_day_anchor(uuid, anchor):
    day_start = pg_store.trove_day_start(anchor)
    return max((a for u, a, *_ in ENTRIES if u == uuid and a < day_start), default=None)


@recorded
async def board_reset_kind(uuid):
    return pg_store._effective_reset_kind(OVERRIDE.get(uuid), uuid)


@recorded
async def prev_rows_for_players(uuid, prev_anchor, names):
    if not names:
        return {}
    lowered = [n.lower() for n in names]
    return {LOWER[pid]: {"rank": rank, "score": score}
            for u, a, rank, score, pid in ENTRIES
            if u == uuid and a == prev_anchor and LOWER[pid] in lowered}


@recorded
async def board_top_series(uuid, window_start, top):
    anchors = sorted({a for u, a, *_ in ENTRIES if u == uuid and a >= window_start})
    if not anchors:
        return [], None, [], []
    latest = anchors[-1]
    top_rows = sorted((e for e in ENTRIES if e[0] == uuid and e[1] == latest), key=lambda e: e[2])[:top]
    if not top_rows:
        return anchors, latest, [], []
    ids = [e[4] for e in top_rows]
    series = [{"player_id": pid, "created_at": a, "rank": rank, "score": score}
              for u, a, rank, score, pid in reversed(ENTRIES)
              if u == uuid and a >= window_start and pid in ids]
    meta = [{"player_id": e[4], "player_name": NAME[e[4]], "rank": e[2]} for e in top_rows]
    return anchors, latest, meta, series


for f in (list_timestamps, list_days, list_boards_at, get_board, latest_anchor_for_board,
          top_entries_for_boards, list_entries, previous_day_anchor, board_reset_kind,
          prev_rows_for_players, board_top_series):
    setattr(pg_store, f.__name__, f)


async def _never(*a, **k):
    raise AssertionError("unexpected pg_store call")

for name in ("player_rows", "player_rows_window", "board_meta", "board_kinds"):
    setattr(pg_store, name, _never)

# --- fake Redis, settings, clock --------------------------------------------


class FakeRedis:
    def __init__(self, store: dict[str, str], down: bool = False) -> None:
        self.store = dict(store)
        self.down = down
        self.sets: list = []

    async def get(self, key):
        if self.down:
            raise ConnectionError("redis down")
        return self.store.get(key)

    async def set(self, key, value, ex=None):
        if self.down:
            raise ConnectionError("redis down")
        self.store[key] = value
        self.sets.append([key, value, ex])


REDIS: FakeRedis | None = None
cache.get_redis = lambda: REDIS

BASE_SETTINGS = {
    "leaderboards_hot_retention_days": 3,
    "leaderboards_anon_retention_days": 7,
    "leaderboards_archive_query_threshold_days": 3,
    "leaderboards_archive_rate_limit_max": 10,
    "leaderboards_archive_rate_limit_window_seconds": 60,
    "feature_cheater_detection_enabled": True,
    "feature_alt_clusters_enabled": False,
    "feature_leaderboard_renames_enabled": True,
    "feature_leaderboard_duplicates_enabled": 0,
}
SETTINGS: dict = {}


async def fake_get_setting(key):
    if key in SETTINGS:
        v = SETTINGS[key]
        if isinstance(v, Exception):
            raise v
        return v
    if key.startswith("feature_"):
        return True
    raise KeyError(key)


runtime_config.get_setting = fake_get_setting


class FakeDT(datetime):
    @classmethod
    def now(cls, tz=None):
        return NOW


service.datetime = FakeDT
trove_router.utcnow = lambda: NOW
site_router.time = SimpleNamespace(time=lambda: NOW.timestamp())

ARCHIVE_INFO = RateLimitInfo(limit=10, remaining=7, reset=datetime(2025, 10, 22, 14, 31, 0, tzinfo=UTC))
ARCHIVE_KEYS: list = []


async def fake_check_rate_limit(key, max_requests, window_seconds):
    ARCHIVE_KEYS.append([key, max_requests, window_seconds])
    return ARCHIVE_INFO


trove_router.check_rate_limit = fake_check_rate_limit

ICON_DIR = Path(tempfile.mkdtemp())
ICON_META: dict = {}


async def fake_file_meta(branch, path):
    CALLS.append(["get_file_meta", [branch, path], ICON_META.get(path)])
    return ICON_META.get(path)


import app.trove.updates.read as updates_read  # noqa: E402

updates_read.get_file_meta = fake_file_meta
app_settings.trove_update_store_dir = str(ICON_DIR)

api = FastAPI()
register_error_handlers(api)
api.include_router(trove_router.leaderboards_router, dependencies=[__import__("fastapi").Depends(require_leaderboards_enabled)])
api.include_router(site_router.router)
api.dependency_overrides[trove_router._LB.dependency] = lambda: SimpleNamespace(token=SimpleNamespace(id=TOKEN_ID))
api.dependency_overrides[trove_router._LB_PUBLIC.dependency] = lambda: AccessContext(None, None, None)
client = TestClient(api, raise_server_exceptions=False)

KEEP_HEADERS = ("content-type", "cache-control", "x-ratelimit-archive-limit", "x-ratelimit-archive-remaining",
                "x-ratelimit-archive-reset")


def pyjson(v) -> str:
    return json.dumps(v)


def run_case(path: str, *, settings: dict | None = None, redis: dict | None = None, redis_mode: str = "up",
             now: datetime | None = None) -> dict:
    global REDIS, NOW
    SETTINGS.clear()
    SETTINGS.update(BASE_SETTINGS)
    SETTINGS.update(settings or {})
    CALLS.clear()
    ARCHIVE_KEYS.clear()
    NOW = now or datetime(2025, 10, 22, 14, 30, 17, 654321, tzinfo=UTC)
    REDIS = None if redis_mode == "none" else FakeRedis(redis or {}, down=redis_mode == "down")
    r = client.get(path)
    is_png = r.headers.get("content-type") == "image/png"
    return {
        "path": path,
        "settings": {k: {"error": str(v)} if isinstance(v, Exception) else v for k, v in SETTINGS.items()},
        "now_us": round(NOW.timestamp() * 1_000_000),
        "redis_mode": redis_mode,
        "redis_before": redis or {},
        "calls": json.loads(json.dumps(CALLS)),
        "archive_keys": list(ARCHIVE_KEYS),
        "redis_sets": [] if REDIS is None else REDIS.sets,
        "status": r.status_code,
        "headers": {k: r.headers[k] for k in KEEP_HEADERS if k in r.headers},
        "body": r.content.hex() if is_png else r.text,
    }


A = D0 + 3 * 3600
CASES = []


def case(path, **kw):
    CASES.append(run_case(path, **kw))


# /v1
case("/v1/leaderboards/timestamps")
case("/v1/leaderboards/timestamps?limit=3")
case("/v1/leaderboards/timestamps?limit=0")
case("/v1/leaderboards/timestamps?limit=abc&limit=366")
case("/v1/leaderboards/timestamps?limit=365")
case(f"/v1/leaderboards?created_at={A}")
case(f"/v1/leaderboards?created_at={D0 - 11 * 3600}")
case(f"/v1/leaderboards?created_at={D0 - DAY + 59}")
case(f"/v1/leaderboards?created_at={D0 - 5 * DAY}")
case(f"/v1/leaderboards?created_at={D0 - 8 * DAY}")
case("/v1/leaderboards")
case("/v1/leaderboards?created_at=0")
case("/v1/leaderboards?created_at=-5")
case(f"/v1/leaderboards?created_at={int(NOW.timestamp()) + 3600}")
case(f"/v1/leaderboards?created_at={D0 - 20 * DAY}")
case("/v1/leaderboards?created_at=1000000000000")
case("/v1/leaderboards?created_at=1.5")
case(f"/v1/leaderboards?created_at={A}", settings={"feature_leaderboards_enabled": False})
case(f"/v1/leaderboards?created_at={A}", settings={"leaderboards_archive_query_threshold_days": 0})
for uuid in ("1", "1000", "0004000", "50000", "999", "1100"):
    case(f"/v1/leaderboards/{uuid}")
for uuid, at in ((1000, A), (1000, D0 - DAY + 20 * 3600), (4000, D0 - 2 * DAY + 7 * 3600), (4000, A),
                 (32000, D0), (1003, A), (50000, A), (2001, A), (1100, A), (77777, A), (1, D0)):
    case(f"/v1/leaderboards/{uuid}/entries?created_at={at}")
case(f"/v1/leaderboards/1000/entries?created_at={A}&limit=2&offset=1")
case(f"/v1/leaderboards/1000/entries?created_at={A}&limit=5&offset=8")
case(f"/v1/leaderboards/1000/entries?created_at={D0 - 5 * DAY}&limit=3")
case("/v1/leaderboards/1000/entries?limit=1001&offset=-1")
case("/v1/leaderboards/1000/entries?created_at=x&limit=0")
case(f"/v1/leaderboards/1000/entries?created_at={D0 - 20 * DAY}")
for uuid, q in ((1000, ""), (4000, "?days=7&top=3"), (32000, "?days=10"), (2001, "?days=30&top=20"),
                (50000, "?top=2"), (777, ""), (1003, "?days=1&top=1"), (1, "?days=2")):
    case(f"/v1/leaderboards/{uuid}/history{q}")
case("/v1/leaderboards/1000/history?days=0&top=21")
case("/v1/leaderboards/1000/history", redis={
    "lb:cache:bhist:1000:7:5": pyjson({"uuid": 1000, "days": 7, "window_start": 1, "window_end": 2,
                                       "anchors": [1, 2], "series": [{"player_name": "Z\u00e9", "current_rank": 1,
                                       "points": [{"created_at": 1, "rank": 1, "score": 1e16, "synthetic": False},
                                                  {"created_at": 2, "rank": 2, "score": 3, "synthetic": True},
                                                  {"created_at": 3.0, "rank": 1, "score": 1e-05},
                                                  {"created_at": 4, "rank": 1, "score": 1.5e-07}]}]})})
case("/v1/leaderboards/4000/history", redis_mode="none")
case("/v1/leaderboards/4000/history", redis_mode="down")
case("/v1/leaderboards/4000/history", settings={"leaderboards_hot_retention_days": RuntimeError("boom")})
for uuid in (1000, 4000, 32000, 1003, 2001, 50000, 1100, 777, 999):
    case(f"/v1/leaderboards/{uuid}/health")
case("/v1/leaderboards/records")
case("/v1/leaderboards/records", redis={"lb:cache:ready_anchor": str(D0)})
case("/v1/leaderboards/records", redis={"lb:cache:ready_anchor": "true", "lb:cache:timestamps": pyjson([A, D0 + 3600])})
case("/v1/leaderboards/records", redis={"lb:cache:ready_anchor": "\"x\"", "lb:cache:timestamps": pyjson([D0 - DAY])})
case("/v1/leaderboards/records", redis={"lb:cache:timestamps": "[]"})
case("/v1/leaderboards/records", redis={"lb:cache:timestamps": pyjson([D0 - 20 * DAY])})
case("/v1/leaderboards/records", redis={"lb:cache:timestamps": "not json"})

# /site
case("/site/leaderboards/config")
case("/site/leaderboards/config", settings={"leaderboards_anon_retention_days": 0,
                                            "leaderboards_hot_retention_days": 14,
                                            "feature_cheater_detection_enabled": "",
                                            "feature_alt_clusters_enabled": "yes"})
case("/site/leaderboards/timestamps")
case("/site/leaderboards/timestamps?limit=2", redis={"lb:cache:ready_anchor": str(D0 - DAY + 20 * 3600)})
case("/site/leaderboards/timestamps?limit=4", redis={"lb:cache:timestamps": pyjson([9, 8, 7, 6, 5, 4]),
                                                     "lb:cache:ready_anchor": "7"})
case("/site/leaderboards/timestamps?limit=366")
case("/site/leaderboards/timestamps", redis_mode="down")
case("/site/leaderboards/timestamps", settings={"feature_leaderboards_enabled": False})
case("/site/leaderboards/days")
case("/site/leaderboards/days", settings={"leaderboards_anon_retention_days": 30})
case("/site/leaderboards/days", redis={"lb:cache:days": pyjson([D0, D0 - DAY, D0 - 6 * DAY, D0 - 7 * DAY + 1])})
case("/site/leaderboards/days", settings={"leaderboards_anon_retention_days": -3})
case(f"/site/leaderboards/boards?created_at={A}")
case(f"/site/leaderboards/boards?created_at={D0 - 5 * DAY}")
case(f"/site/leaderboards/boards?created_at={D0 - 8 * DAY}")
case(f"/site/leaderboards/boards?created_at={A + 10 * DAY}")
case("/site/leaderboards/boards")
case(f"/site/leaderboards/boards?created_at={A}", redis={f"lb:cache:boards:{A}": pyjson([{"uuid": 5, "x": 1.0}])})
for uuid, at, q in ((1000, A, ""), (4000, D0 - 2 * DAY + 7 * 3600, ""), (1003, A, ""), (50000, A, "&limit=3"), (50000, A, "&limit=9"),
                    (1000, A, "&limit=2&offset=7"), (77777, A, ""), (1000, D0 - 8 * DAY, "")):
    case(f"/site/leaderboards/{uuid}/entries?created_at={at}{q}")
case(f"/site/leaderboards/1000/entries?created_at={A}", redis={
    f"lb:cache:entries:{A}:1000:0:100": pyjson({"items": [{"player_name": "x", "rank": 1, "score": 2.0}], "total": 9})})
case(f"/site/leaderboards/1000/entries?created_at={A}&limit=0&offset=-2")
case("/site/leaderboards/0001000/history?days=3")
case("/site/leaderboards/4000/history")
case("/site/leaderboards/1000/history?top=0")
case("/site/leaderboards/1000/history", redis={"lb:cache:bhist:1000:7:5": "{\"cached\": [1.5, 1e+16, \"\u00e9\"]}"})
for uuid in (1000, 2001, 777, 50000):
    case(f"/site/leaderboards/{uuid}/health")
case("/site/leaderboards/records")
case("/site/leaderboards/records", redis={"lb:cache:timestamps": "[]"})

# board icons
blob = bytes(range(256)) * 3
sha = "ab" + "0" * 62
(ICON_DIR / "objects" / "ab").mkdir(parents=True)
(ICON_DIR / "objects" / "ab" / sha).write_bytes(blob)
ICON_META.update({
    "ui/leaderboard_icons/icon_paragon_knight.png": {"content_sha256": sha},
    "ui/leaderboard_icons/icon_empty.png": {"content_sha256": ""},
    "ui/leaderboard_icons/icon_missing_blob.png": {"content_sha256": "cd" + "1" * 62},
})
ICONS = []
for name in ("icon_paragon_knight", "icon_empty", "icon_missing_blob", "icon_unknown", "Icon_Upper", "a-b",
             "icon%5Fparagon_knight", "%C3%A9"):
    c = run_case(f"/site/leaderboards/board-icon/{name}")
    c["calls"] = [x[:2] + [None if x[2] is None else x[2]["content_sha256"]] for x in c["calls"]]
    ICONS.append(c)


# --- pure helpers -----------------------------------------------------------

def pure() -> dict:
    rng = random.Random(7)
    bounds = []
    for kind in ("daily", "weekly", "default", "none"):
        for _ in range(60):
            t0 = rng.randrange(1_600_000_000, 1_800_000_000)
            t1 = t0 + rng.choice([-5, 0, 1, 3600, 39600, DAY - 1, DAY, 3 * DAY, 8 * DAY, 15 * DAY])
            bounds.append([kind, t0, t1, service.reset_boundaries_for_kind(kind, t0, t1)])
    t = D0 - 2 * DAY + 3600
    bounds.append(["weekly", t, t + 22 * 3600, service.reset_boundaries_for_kind("weekly", t, t + 22 * 3600)])
    zeros = []
    for kind in ("daily", "weekly", "default"):
        for _ in range(25):
            n = rng.randrange(0, 6)
            t = rng.randrange(1_700_000_000, 1_760_000_000)
            pts = []
            for _ in range(n):
                t += rng.choice([1800, 3600, 39600, DAY, 3 * DAY])
                pts.append({"created_at": t, "rank": rng.randrange(1, 50), "score": rng.choice([0.0, 12.5, 1e16, 3.0])})
            zeros.append([kind, pts, service._inject_reset_zeros(pts, kind)])
    stamps = []
    now_s = int(NOW.timestamp())
    for ts in (0, -1, 1, D0, D0 - 11 * 3600, D0 - 11 * 3600 + 59, D0 + 61, now_s + 299, now_s + 300, now_s + 360,
               now_s - 14 * DAY, now_s - 14 * DAY - 60, now_s - 14 * DAY + 59, 32503680000, 86400 * 3):
        for backfill in (False, True):
            stamps.append([ts, backfill, service.normalize_timestamp(ts, allow_backfill=backfill)])
    levels = [[p, list(mastery.level_from_points(p))] for p in
              [-3, 0, 1, 24, 25, 26, 100, 125, 126, 1000, 29_000, 29_025, 31_000, 1_000_000, 10**9, 10**12]]
    health = []
    for _ in range(40):
        n = rng.randrange(0, 12)
        comparable = rng.random() < 0.7
        items = []
        for _ in range(n):
            it = {"score": rng.choice([0.0, 1.0, 3.5, 1e16, rng.uniform(0, 1e6), 12345.678])}
            if comparable:
                new = rng.random() < 0.3
                prev = None if new else rng.choice([0.0, 2.0, rng.uniform(1, 1e6)])
                it.update(is_new=new, prev_score=prev,
                          score_delta=None if new else it["score"] - prev)
            items.append(it)
        items.sort(key=lambda it: -it["score"])
        health.append([items, comparable, service.compute_board_health(items, comparable=comparable)])
    per_day = []
    for _ in range(10):
        anchors = [rng.randrange(1_700_000_000, 1_700_000_000 + 9 * DAY) for _ in range(rng.randrange(0, 20))]
        days = rng.randrange(1, 6)
        per_day.append([anchors, days, cache._latest_per_day(anchors, days)])
    return {"bounds": bounds, "zeros": zeros, "stamps": stamps, "now": now_s, "levels": levels,
            "health": health, "per_day": per_day}


OUT.parent.mkdir(parents=True, exist_ok=True)
OUT.write_text(json.dumps({"token_id": TOKEN_ID, "cases": CASES, "icons": ICONS, "pure": pure()},
                          indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
print(f"wrote {OUT} ({len(CASES)} cases, {len(ICONS)} icon cases)")
