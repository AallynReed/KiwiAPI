"""Golden fixtures for kiwi-server's cheater detection and views recompute.

Runs the REAL detection pipeline (app/trove/leaderboards/detection.py), cache
module and routes against an in-memory capture database (pg_store functions,
Redis and runtime_config faked; every semantics the fakes implement is mirrored
by the Rust FakeStore in src/cheaters/tests.rs) and records the payloads,
Redis writes and HTTP replies.

Run from the repo root with the production-pinned venv:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_cheaters_fixtures.py
"""

from __future__ import annotations

import asyncio
import fnmatch
import json
import logging
import math
import os
import random
import sys
import warnings
from datetime import UTC, datetime
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "cheaters" / "golden.json"
os.chdir(ROOT)
sys.path.insert(0, str(ROOT))

from fastapi import FastAPI  # noqa: E402
from fastapi.testclient import TestClient  # noqa: E402

from app.admin import runtime_config  # noqa: E402
from app.core.dependencies import AccessContext, require_master_ingest  # noqa: E402
from app.core.errors import register_error_handlers  # noqa: E402
from app.core.features import require_leaderboards_enabled  # noqa: E402
from app.site import router as site_router_mod  # noqa: E402
from app.trove import router as trove_router  # noqa: E402
from app.trove.leaderboards import cache as lb_cache  # noqa: E402
from app.trove.leaderboards import detection  # noqa: E402
from app.trove.leaderboards import models  # noqa: E402
from app.trove.leaderboards import pg_store  # noqa: E402
from app.trove.leaderboards import service as lb_service  # noqa: E402

assert detection._native is None, "fixtures come from the pure-Python path"

# ---------------------------------------------------------------------------
# Fakes (mirrored by src/cheaters/tests.rs)

STATE: dict = {}


class FakeClock:
    @staticmethod
    def time() -> float:
        return STATE["now"]


class FakeDatetime(datetime):
    @classmethod
    def now(cls, tz=None):
        return datetime.fromtimestamp(STATE["now"], tz)


detection.time = FakeClock
lb_service.datetime = FakeDatetime


async def fake_setting(key: str):
    value = STATE["overrides"].get(key, runtime_config.REGISTRY[key].default)
    STATE["settings"][key] = value
    return value


runtime_config.get_setting = fake_setting


class FakeRedis:
    def __init__(self, data: dict) -> None:
        self.data = data  # key -> [value, ex]

    async def get(self, key):
        v = self.data.get(key)
        return None if v is None else v[0]

    async def set(self, key, value, ex=None):
        self.data[key] = [value, ex]

    async def scan_iter(self, match=None):
        for k in list(self.data):
            if match is None or fnmatch.fnmatchcase(k, match):
                yield k

    async def delete(self, *keys):
        for k in keys:
            self.data.pop(k, None)


lb_cache.get_redis = lambda: STATE["redis"]


def db() -> dict:
    return STATE["db"]


def trove_day_start(anchor: int) -> int:
    return ((anchor - 39600) // 86400) * 86400 + 39600


def player_ids() -> dict[str, int]:
    ids: dict[str, int] = {}
    for _, _, name, _, _ in db()["entries"]:
        ids.setdefault(name.lower(), len(ids) + 1)
    return ids


def canonical() -> dict[str, str]:
    out: dict[str, str] = {}
    for _, _, name, _, _ in db()["entries"]:
        out.setdefault(name.lower(), name)
    return out


async def f_list_timestamps(limit=60, *, include_archive=True, since=None):
    STATE["calls"].append(["list_timestamps", limit])
    return sorted({e[1] for e in db()["entries"]}, reverse=True)[:limit]


def _board_rows(anchor: int) -> list[dict]:
    present = {e[0] for e in db()["entries"] if e[1] == anchor}
    contests = {(c[0], c[1]): c[2] for c in db()["contests"]}
    rows = [b for b in db()["boards"] if b["uuid"] in present]
    rows.sort(key=lambda b: (b["category"], b["name"]))
    return [
        {
            "uuid": r["uuid"], "name_id": r["name_id"], "name": r["name"],
            "category_id": r["category_id"], "category": r["category"],
            "contest_type": contests.get((r["uuid"], anchor)),
            "reset_kind": pg_store._effective_reset_kind(r["reset_kind_override"], r["uuid"]),
            "reset_kind_override": r["reset_kind_override"],
            "player_board": models.is_player_board(r["uuid"]),
        }
        for r in rows
    ]


async def f_list_boards_at(anchor):
    return _board_rows(anchor)


async def f_entries_by_board_at(anchor, uuids=None):
    rows = [e for e in db()["entries"] if e[1] == anchor and (uuids is None or e[0] in uuids)]
    rows.sort(key=lambda e: (e[0], e[3]))
    out: dict[int, list[dict]] = {}
    for u, _, name, rank, score in rows:
        out.setdefault(u, []).append({"player_name": name, "rank": rank, "score": score})
    return out


async def f_hot_window_start():
    return STATE["hot_window_start"]


async def f_previous_captures_bulk(player_names, board_uuid, anchor, cycle_start):
    if not player_names:
        return {}
    lowered = {n.lower() for n in player_names}
    canon = canonical()
    who = {lo for lo in lowered if lo in canon}
    best: dict[str, tuple] = {}
    for u, a, name, _, score in db()["entries"]:
        lo = name.lower()
        if u != board_uuid or lo not in who or not (cycle_start <= a < anchor):
            continue
        if lo not in best or a > best[lo][1]:
            best[lo] = (score, a)
    return {canon[lo]: v for lo, v in best.items()}


async def f_comovement_series(board_uuid, player_names, since_anchor, until_anchor):
    if not player_names:
        return {}
    lowered = {n.lower() for n in player_names}
    canon = canonical()
    out: dict[str, dict[int, float]] = {}
    for u, a, name, _, score in db()["entries"]:
        if u == board_uuid and name.lower() in lowered and since_anchor <= a <= until_anchor:
            out.setdefault(canon[name.lower()], {})[a] = score
    return out


async def f_list_entries(uuid, anchor, *, limit, offset):
    rows = sorted((e for e in db()["entries"] if e[0] == uuid and e[1] == anchor), key=lambda e: e[3])
    canon = canonical()
    items = [{"player_name": canon[e[2].lower()], "rank": e[3], "score": e[4]} for e in rows[offset:offset + limit]]
    return items, len(rows)


async def f_previous_day_anchor(uuid, anchor):
    day = trove_day_start(anchor)
    xs = [e[1] for e in db()["entries"] if e[0] == uuid and e[1] < day]
    return max(xs) if xs else None


async def f_board_reset_kind(uuid):
    over = next((b["reset_kind_override"] for b in db()["boards"] if b["uuid"] == uuid), None)
    return pg_store._effective_reset_kind(over, uuid)


async def f_prev_rows_for_players(uuid, prev_anchor, names):
    if not names:
        return {}
    lowered = {n.lower() for n in names}
    out = {}
    for u, a, name, rank, score in db()["entries"]:
        if u == uuid and a == prev_anchor and name.lower() in lowered:
            out[name.lower()] = {"rank": rank, "score": score}
    return out


async def f_board_top_series(uuid, window_start, top):
    anchors = sorted({e[1] for e in db()["entries"] if e[0] == uuid and e[1] >= window_start})
    if not anchors:
        return [], None, [], []
    latest = anchors[-1]
    ids = player_ids()
    canon = canonical()
    at = sorted((e for e in db()["entries"] if e[0] == uuid and e[1] == latest), key=lambda e: e[3])[:top]
    if not at:
        return anchors, latest, [], []
    top_meta = [{"player_id": ids[e[2].lower()], "player_name": canon[e[2].lower()], "rank": e[3]} for e in at]
    pids = {m["player_id"] for m in top_meta}
    series = [
        {"player_id": ids[e[2].lower()], "created_at": e[1], "rank": e[3], "score": e[4]}
        for e in db()["entries"]
        if e[0] == uuid and e[1] >= window_start and ids[e[2].lower()] in pids
    ]
    return anchors, latest, top_meta, series


lb_service.list_timestamps = f_list_timestamps
lb_service.list_boards_at = f_list_boards_at
lb_service.entries_by_board_at = f_entries_by_board_at
lb_service.hot_window_start = f_hot_window_start
pg_store.previous_captures_bulk = f_previous_captures_bulk
pg_store.comovement_series = f_comovement_series
pg_store.list_entries = f_list_entries
pg_store.previous_day_anchor = f_previous_day_anchor
pg_store.board_reset_kind = f_board_reset_kind
pg_store.prev_rows_for_players = f_prev_rows_for_players
pg_store.board_top_series = f_board_top_series

api = FastAPI()
register_error_handlers(api)
api.include_router(trove_router.leaderboards_router)
api.include_router(site_router_mod.router)
api.dependency_overrides[require_leaderboards_enabled] = lambda: None
api.dependency_overrides[trove_router._LB_PUBLIC.dependency] = lambda: AccessContext(None, None, None)
api.dependency_overrides[require_master_ingest] = lambda: None
api.dependency_overrides[site_router_mod._resolve_feature_flags] = lambda: None
client = TestClient(api, raise_server_exceptions=False)


def reply(r) -> dict:
    return {"status": r.status_code, "cache_control": r.headers.get("cache-control"), "body": r.text}


def begin(dbdata: dict, overrides: dict, now: float, redis: dict | None, hot: int = 0) -> None:
    detection.reset()
    STATE.clear()
    STATE.update(
        db=dbdata, overrides=overrides, now=now, settings={}, calls=[], hot_window_start=hot,
        redis=FakeRedis(redis) if redis is not None else None,
    )


DBS: dict[str, dict] = {}


def snapshot() -> dict:
    key = json.dumps(STATE["db"], sort_keys=True)
    name = next((n for n, d in DBS.items() if json.dumps(d, sort_keys=True) == key), None)
    if name is None:
        name = f"db{len(DBS)}"
        DBS[name] = STATE["db"]
    return {"settings": STATE["settings"], "db": name, "now": STATE["now"],
            "hot_window_start": STATE["hot_window_start"]}


# ---------------------------------------------------------------------------
# The capture database

WEEK = 1792407600  # Monday 2026-10-19 11:00 UTC
H = 3600


def board(uuid, name, category, *, name_id=None, category_id=None, override=None):
    return {"uuid": uuid, "name_id": name_id or f"LB_{uuid}", "name": name,
            "category_id": category_id or f"Cat_{category or 'none'}", "category": category,
            "reset_kind_override": override}


def build_db(anchor: int, rng: random.Random, *, weekly_series: bool = True) -> dict:
    boards = [
        board(4001, "Effort Knight", "Effort"),
        board(2001, "Delve Speed", "Delves"),
        board(1, "Trove Mastery", "Mastery"),
        board(1100, "Club Power", "Clubs"),
        board(50000, "Tiny Stat", "Stats"),
        board(4002, "Effort Lockstep", "Effort"),
        board(4006, "Effort Lockstep II", "effort"),
        board(4003, "Effort Bot", "Effort"),
        board(4004, "Effort Schedule", "Effort"),
        board(4005, "Effort Bots", "Effort"),
        board(60000, "", "", name_id="LB_Name_60000", category_id="Category_Misc"),
        board(60001, "Unicode Board", "Ünïcode"),
        board(60002, "Capped Board", "Classes", override="weekly"),
        board(60003, "Swarm Board", "Misc", override="none"),
    ]
    contests = [[4001, anchor, "Leaderboard_Category_Contests"]]
    entries: list[list] = []

    def add(u, a, name, rank, score):
        entries.append([u, a, name, rank, float(score)])

    # 4001: a rank-1 runaway (score outlier + rank gap + velocity) and peers with history.
    peers = [f"Knight{i:02d}" for i in range(2, 61)]
    for back in (2, 1):
        a = anchor - back * H
        add(4001, a, "Runaway", 1, 1000 + back)
        for i, p in enumerate(peers):
            add(4001, a, p, i + 2, 2500 - i * 20 - back * (10 + i % 7))
    add(4001, anchor, "Runaway", 1, 1_000_000)
    for i, p in enumerate(peers):
        add(4001, anchor, p, i + 2, 2500 - i * 20)
    # 2001: lower is better (delve times), rank 1 suspiciously fast.
    names = [f"Delver{i:02d}" for i in range(2, 41)]
    for back in (1,):
        a = anchor - back * H
        add(2001, a, "Speedy", 1, 700)
        for i, p in enumerate(names):
            add(2001, a, p, i + 2, 520 + i * 9 + 15 + i % 5)
    add(2001, anchor, "Speedy", 1, 1)
    for i, p in enumerate(names):
        add(2001, anchor, p, i + 2, 520 + i * 9)
    # 1: lifetime board with an alt family (anana*) and a non-ASCII family.
    fam = ["anana1", "anana2", "Anana3", "anana_4", "anana 5", "anan6"]
    for i, n in enumerate(fam):
        add(1, anchor, n, 30 + i, 50_000 + i * 3)
    lifers = [f"Veteran{chr(65 + i)}" for i in range(26)]
    for i, n in enumerate(lifers):
        add(1, anchor, n, i + 1, 900_000 - i * 9_000)
    # 1100 (admin-excluded club board) and 50000 (below the minimum size).
    for i in range(25):
        add(1100, anchor, f"Club{i}", i + 1, 5000 - i * 50)
    for i in range(5):
        add(50000, anchor, f"Tiny{i}", i + 1, 100 - i)
    # Week-long series: lockstep trio (4002 + 4006), a no-sleep bot (4003),
    # a shared schedule (4004), a lockstep family with a stem (4005).
    hours = list(range(0, (anchor - WEEK) // H + 1)) if weekly_series else [(anchor - WEEK) // H]
    crowd = [f"Grinder{i:02d}" for i in range(12)]
    trio = ["Zephyr", "Quill", "Marrow"]
    sched = ["Moonlit", "Larkspur", "Fernwood"]
    bots = ["bot1", "bot2", "bot3"]
    for h in hours:
        a = WEEK + h * H
        if a > anchor:
            break
        for u in (4002, 4006):
            for i, n in enumerate(trio):
                add(u, a, n, i + 1, 100_000 + h * 500 + u)
            for i, n in enumerate(crowd):
                add(u, a, n, i + 4, 50_000 + h * (17 + i * 3) + i)
        add(4003, a, "NoSleepBot", 1, 80_000 + h * 300)
        add(4003, a, "Human", 2, 70_000 + min(h, 20) * 100)
        for i in range(20):
            add(4003, a, f"Casual{i:02d}", i + 3, 60_000 - i * 1000 + (h // 9) * (i + 1))
        for i, n in enumerate(sched):
            add(4004, a, n, i + 1, 40_000 + i * 977 + min(max(h - 5, 0), 12) * (40 + i * 13))
        for i in range(18):
            add(4004, a, f"Walker{i:02d}", i + 4, 30_000 - i * 400)
        for i, n in enumerate(bots):
            add(4005, a, n, i + 1, 20_000 + h * 250 + i)
        for i in range(10):
            add(4005, a, f"Other{i}", i + 4, 10_000 - i * 100 + h * (i + 1))
    # 60000: empty name + category (fallbacks), lifetime.
    for i in range(22):
        add(60000, anchor, f"Plain{i:02d}", i + 1, 3000 - i * 30)
    # 60001: non-ASCII families.
    uni = ["Σίσυφος1", "ΣΊΣΥΦΟΣ2", "Σίσυφος٣", "ｆｕｌｌ１", "ｆｕｌｌ２", "ｆｕｌｌ３", "İstanbul7", "İstanbul8", "İstanbul9"]
    for i, n in enumerate(uni):
        add(60001, anchor, n, i + 1, 7000 + (i // 3) * 1000 + i % 3)
    for i in range(15):
        add(60001, anchor, f"Ünique{chr(97 + i)}", i + 10, 2000 - i * 50)
    # 60002: capped (top 5 tied) weekly board.
    for i in range(30):
        add(60002, anchor, f"Capper{i:02d}", i + 1, 59731 if i < 6 else 59731 - i * 37)
    # 60003: 65 near-identical accounts (members cap) on an explicit lifetime board.
    for i in range(65):
        add(60003, anchor, f"swarm{i + 1}", i + 1, 12_345 + (i % 3))
    return {"boards": boards, "contests": contests, "entries": entries}


ANCHOR = WEEK + 49 * H  # Wednesday 12:00 UTC: 50 captures since the reset
NOW = ANCHOR + 600 + 0.25


def run_compute(name: str, overrides: dict, *, anchor: int = ANCHOR, weekly_series: bool = True) -> dict:
    rng = random.Random(7)
    begin(build_db(anchor, rng, weekly_series=weekly_series), overrides, anchor + 600.25, {}, hot=anchor - 6 * 86400)
    loop = asyncio.new_event_loop()
    try:
        payload = loop.run_until_complete(detection.detect_possible_cheaters(force=True))
    finally:
        loop.close()
    writes = {k: v for k, v in STATE["redis"].data.items()}
    v1 = reply(client.get("/v1/leaderboards/cheaters"))
    site = reply(client.get("/site/leaderboards/cheaters"))
    case = snapshot()
    case.update(name=name, redis_writes=writes, v1=v1, site=site)
    return case


OFF = {"feature_cheater_detection_enabled": False, "feature_alt_clusters_enabled": False}


def compute_cases() -> list[dict]:
    on = {"feature_cheater_detection_enabled": True, "feature_alt_clusters_enabled": True,
          "cheaters_excluded_board_uuids": "1100, junk, 21012 ,, 7",
          "cheaters_cluster_excluded_board_uuids": " 1100,4006x"}
    return [
        run_compute("full", on),
        run_compute("cheaters_only", {**on, "feature_alt_clusters_enabled": False}),
        run_compute("clusters_only", {**on, "feature_cheater_detection_enabled": False}),
        run_compute("int_threshold", {**on, "cheaters_z_threshold": 4, "cheaters_velocity_multiplier": 3,
                                      "cheaters_min_board_size": 10, "cheaters_cluster_min_size": 2,
                                      "cheaters_comovement_min_matching_hours": 4,
                                      "cheaters_schedule_min_active_hours": 0}),
        run_compute("top_n_zero", {**on, "cheaters_comovement_candidate_top_n": 0}),
        run_compute("early_week", on, anchor=WEEK + 2 * H, weekly_series=True),
        run_compute("no_week", on, weekly_series=False),
        run_compute("disabled", OFF),
    ]


# ---------------------------------------------------------------------------
# Serving path

def run_serve(name: str, overrides: dict, redis: dict | None, *, with_db: bool = True,
              force: bool = False) -> dict:
    dbdata = SERVE_DB if with_db else {"boards": [], "contests": [], "entries": []}
    before = json.loads(json.dumps(redis)) if redis is not None else None
    begin(dbdata, overrides, NOW, redis)
    v1 = reply(client.get("/v1/leaderboards/cheaters"))
    site = reply(client.get("/site/leaderboards/cheaters"))
    case = snapshot()
    case.update(name=name, redis=before, v1=v1, site=site)
    return case


SERVE_DB = {"boards": [board(1, "Trove Mastery", "Mastery")], "contests": [],
            "entries": [[1, ANCHOR - H, "A", 1, 1.0], [1, ANCHOR, "A", 1, 2.0]]}


def serve_cases() -> list[dict]:
    on = {"feature_cheater_detection_enabled": True, "feature_alt_clusters_enabled": False}
    begin(build_db(ANCHOR, random.Random(1)), {"feature_cheater_detection_enabled": True,
                                               "feature_alt_clusters_enabled": True}, NOW, {})
    loop = asyncio.new_event_loop()
    try:
        stored = loop.run_until_complete(detection.detect_possible_cheaters(force=True))
    finally:
        loop.close()
    snap = json.dumps(stored)
    ready = json.dumps(ANCHOR)
    bad = json.dumps({**stored, "players": [{"player_name": 5}]})
    return [
        run_serve("disabled_ready", OFF, {"lb:cache:ready_anchor": [json.dumps(ANCHOR - H), 100]}),
        run_serve("disabled_no_ready", OFF, {}),
        run_serve("disabled_no_redis", OFF, None),
        run_serve("disabled_no_data", OFF, {}, with_db=False),
        run_serve("clusters_only_ready", {"feature_cheater_detection_enabled": False, "feature_alt_clusters_enabled": True},
                  {"lb:cache:ready_anchor": [ready, 100]}),
        run_serve("snapshot", on, {"lb:cache:ready_anchor": [ready, 100], f"lb:cache:cheaters:{ANCHOR}": [snap, 100]}),
        run_serve("snapshot_latest", on, {f"lb:cache:cheaters:{ANCHOR}": [snap, 100]}),
        run_serve("missing_snapshot", on, {"lb:cache:ready_anchor": [ready, 100]}),
        run_serve("null_snapshot", on, {"lb:cache:ready_anchor": [ready, 100], f"lb:cache:cheaters:{ANCHOR}": ["null", 100]}),
        run_serve("no_data", on, {}, with_db=False),
        run_serve("invalid_snapshot", on, {"lb:cache:ready_anchor": [ready, 100], f"lb:cache:cheaters:{ANCHOR}": [bad, 100]}),
        run_serve("ttl_float", {**on, "cheaters_cache_ttl_seconds": 90.9},
                  {"lb:cache:ready_anchor": [ready, 100], f"lb:cache:cheaters:{ANCHOR}": [snap, 100]}),
    ]


# ---------------------------------------------------------------------------
# Views recompute

def views_db() -> dict:
    boards = [
        board(32000, "Leviathans", "Daily"),
        board(4001, "Effort Knight", "Effort"),
        board(1, "Trove Mastery", "Mastery"),
        board(1100, "Club Power", "Clubs"),
    ]
    contests = [[4001, ANCHOR, "Leaderboard_Category_Contests"]]
    entries = []
    anchors = [ANCHOR - d * 86400 - h * H for d in range(5) for h in (0, 5, 13)]
    for a in sorted(anchors):
        k = (a - ANCHOR) // H
        for i, n in enumerate(["Levi", "Ahab", "Ishmael", "Queequeg", "Starbuck", "Stubb", "Flask"]):
            entries.append([32000, a, n, i + 1, float(max(0, 500 - i * 40 + k * 3))])
        for i, n in enumerate(["Runaway", "Knight02", "knight03", "Knight04"]):
            if a == ANCHOR and n == "Knight04":
                continue
            entries.append([4001, a, n, i + 1, float(4000 - i * 300 + k * 7) + 0.5])
        for i, n in enumerate(["VeteranA", "VeteranB", "Néo"]):
            entries.append([1, a, n, i + 1, float(900_000 - i * 9_000 + k)])
        if a == ANCHOR:
            entries.append([4001, a, "Newcomer", 5, 100.25])
            entries.append([1100, a, "SomeClub", 1, 5.0])
    return {"boards": boards, "contests": contests, "entries": entries}


def run_views(name: str, overrides: dict, redis: dict | None, dbdata: dict | None = None) -> dict:
    dbdata = dbdata if dbdata is not None else views_db()
    before = json.loads(json.dumps(redis)) if redis is not None else None
    begin(dbdata, overrides, NOW, redis)
    r = client.post("/v1/leaderboards/views/recompute")
    case = snapshot()
    case.update(name=name, redis=before, reply=reply(r),
                redis_after=STATE["redis"].data if STATE["redis"] is not None else None)
    return case


def views_cases() -> list[dict]:
    seeded = {
        "lb:cache:timestamps": ["[1, 2]", 10],
        "lb:cache:ready_anchor": ["5", 10],
        f"lb:cache:boards:{ANCHOR - 86400}": ["[]", 10],
        f"lb:cache:entries:{ANCHOR}:1:0:100": ["{}", 10],
        "lb:cache:bhist:1:7:5": ["{}", 10],
        f"lb:cache:cheaters:{ANCHOR}": ["{}", 10],
        "lb:cache:activity:1": ["{}", 10],
    }
    return [
        run_views("seeded", {}, seeded),
        run_views("retention_one", {"leaderboards_hot_retention_days": 1}, {}),
        run_views("no_redis", {}, None),
        run_views("empty_db", {}, {}, {"boards": [], "contests": [], "entries": []}),
    ]


# ---------------------------------------------------------------------------
# Pure helpers

def unit_cases() -> dict:
    rng = random.Random(11)
    sums = []
    for n in (1, 2, 3, 10, 57):
        for _ in range(6):
            xs = [rng.choice([rng.uniform(-1e6, 1e6), rng.random(), 1e16, -1e16, 0.1, 1 / 3]) for _ in range(n)]
            sums.append([xs, sum(xs)])
    nums = [0.0, -0.0, 1.0, 1234.0, 1234.5, 1234.567, -1234.567, 0.125, 0.375, 2.5, 99.95, 1e15, 1e16 + 2,
            123456789.125, 0.000123456, 1e-05, 0.5, -0.04, 7.0 / 3, 59731.0, 1e300]
    fmts = []
    for v in nums:
        fmts.append([v, detection._fmt(v) if math.isfinite(v) and abs(v) < 1e200 else None,
                     f"{v:.0f}", f"{v:.1f}", f"{v:.3g}",
                     detection._round(v) if abs(v) < 1e18 else None])
    names = ["anana17", " Player 001 ", "Dr4g0n", "abc_-", "Σίσυφος1", "ΣΊΣΥΦΟΣ2", "Σίσυφος٣", "ｆｕｌｌ１２",
             "İstanbul7", "Ünïcode\u00a0 9", "x\u2028#5", "ǅemal3", "ﬀ1", "Tom", "", "12", "@dm1n",
             "\u1c9012", "日本語２", "ΑΣ", "Ω\u0345a1"]
    stems = [[n, detection._name_stem(n)] for n in names]
    csvs = ["", "1100, 21012", " 1100,abc,, 7 ,+3,-2,1_000,٣٣,0x10, 4.5", "١٢"]
    excluded = [[c, sorted(detection._parse_excluded(c))] for c in csvs]
    before = []
    for a in (ANCHOR, WEEK, WEEK - 1, WEEK + 39600, 1792540800, 1792540799):
        for k in ("daily", "weekly", "default", "none", ""):
            before.append([a, k, detection._reset_boundary_before(a, k)])
    bounds = []
    for k in ("daily", "weekly", "default"):
        for t0, t1 in ((WEEK - 1, WEEK + 8 * 86400), (WEEK, WEEK), (WEEK + 5, WEEK + 3 * 86400 + 39600)):
            bounds.append([k, t0, t1, lb_service.reset_boundaries_for_kind(k, t0, t1)])
    return {"py_sum": sums, "fmt": fmts, "stems": stems, "excluded": excluded,
            "reset_before": before, "reset_boundaries": bounds}


def main() -> None:
    data = {"units": unit_cases(), "compute": compute_cases(), "serve": serve_cases(), "views": views_cases()}
    data["dbs"] = DBS
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(data, ensure_ascii=False, separators=(",", ":")) + "\n", encoding="utf-8")
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
