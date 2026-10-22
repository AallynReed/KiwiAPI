"""Golden fixtures for kiwi-server's rename + duplicate-name detection, captured from Python.

The real ``renames.py`` / ``duplicates.py`` run over small in-memory worlds:
``pg_store``'s reads are replaced by functions with the SQL's semantics, its
upserts run for real against a recording connection (so the fixtures hold the
exact executemany parameter rows, double-encoded json included), Redis is a
recording fake and the clock is pinned. The Rust tests replay each world
through an in-memory ``Store`` and must produce the same writes, Redis traffic
and return values. Route bodies come from the real ``leaderboards_router`` and
site router (auth stubbed) over canned rows in their stored jsonb form.

Run from the repo root with the production-pinned venv:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_lb_names_fixtures.py
Writes native/kiwi-server/testdata/lb_names/golden.json.
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import logging
import os
import random
import sys
import warnings
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "lb_names" / "golden.json"
os.chdir(ROOT)
sys.path.insert(0, str(ROOT))
os.environ.setdefault("SECRET_KEY", "lb-names-fixture-secret")
warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)

assert sys.version_info[:2] == (3, 13), "run with the production-pinned 3.13 venv"

from fastapi import Depends, FastAPI  # noqa: E402
from fastapi.testclient import TestClient  # noqa: E402

import app.core.redis as core_redis  # noqa: E402
import app.site.router as SR  # noqa: E402
import app.trove.router as R  # noqa: E402
from app.admin import runtime_config as rc  # noqa: E402
from app.core import features  # noqa: E402
from app.core.dependencies import AccessContext, IngestAuth  # noqa: E402
from app.core.errors import register_error_handlers  # noqa: E402
from app.core.observability import add_request_context_middleware  # noqa: E402
from app.trove.leaderboards import duplicates, pg_store, renames  # noqa: E402
from app.trove.leaderboards.models import is_player_board, non_player_boards  # noqa: E402

RID = "fixture-rid"
CLOCK = 1760000000.75

# --- settings / flags ---------------------------------------------------------

SETTING_KEYS = [
    "renames_max_gap_seconds", "renames_min_boards", "renames_score_drift_pct", "renames_min_score",
    "renames_min_confidence", "renames_excluded_board_uuids", "renames_cache_ttl_seconds",
    "duplicates_lookback_days", "duplicates_min_boards", "duplicates_frozen_min_captures",
    "duplicates_max_names", "duplicates_cache_ttl_seconds",
    features.LEADERBOARDS_FLAG, features.RENAMES_FLAG, features.DUPLICATES_FLAG,
]
SETTINGS: dict = {}


def reset_settings(**over) -> dict:
    SETTINGS.clear()
    for k in SETTING_KEYS:
        SETTINGS[k] = rc._spec(k).default
    SETTINGS.update(over)
    return {k: SETTINGS[k] for k in SETTING_KEYS}


async def get_setting(key):
    if key in SETTINGS:
        return SETTINGS[key]
    return rc._spec(key).default


rc.get_setting = get_setting


class FakeTime:
    @staticmethod
    def time():
        return CLOCK


renames.time = FakeTime
duplicates.time = FakeTime

# --- recording stores -----------------------------------------------------------

WRITES: list = []
REDIS_OPS: list = []


def op_of(sql: str) -> str:
    if sql.startswith("INSERT INTO player_rename"):
        return "upsert_renames"
    if sql.startswith("INSERT INTO player_duplicate") and "'not_analysed'" in sql:
        return "upsert_duplicate_dates"
    if sql.startswith("INSERT INTO player_duplicate"):
        return "upsert_duplicates"
    if sql.startswith("DELETE FROM player_rename"):
        return "delete_renames"
    if sql.startswith("DELETE FROM player_duplicate"):
        return "delete_duplicates"
    raise AssertionError(sql)


class RecConn:
    async def executemany(self, sql, payload):
        rows = [list(r) for r in payload]
        if op_of(sql) == "upsert_duplicate_dates":
            rows.sort(key=json.dumps)  # a set's order in Python; sorted so reruns are stable
        WRITES.append({"op": op_of(sql), "rows": rows})

    async def execute(self, sql, *args):
        op = op_of(sql)
        WRITES.append({"op": op})
        return f"DELETE {WORLD.deleted[op]}"


@contextlib.asynccontextmanager
async def rec_acquire():
    yield RecConn()


pg_store.acquire = rec_acquire


class FakeRedis:
    def __init__(self, held=()):
        self.data = dict.fromkeys(held, "1")

    async def set(self, key, value, nx=False, ex=None):
        if nx:
            REDIS_OPS.append(["set_nx", key])
            if key in self.data:
                return None
        else:
            REDIS_OPS.append(["set", key, value, ex])
        self.data[key] = value
        return True

    async def get(self, key):
        REDIS_OPS.append(["get", key])
        return self.data.get(key)

    async def expire(self, key, ttl):
        REDIS_OPS.append(["expire", key, ttl])
        return True

    async def delete(self, key):
        REDIS_OPS.append(["delete", key])
        self.data.pop(key, None)
        return 1


REDIS: FakeRedis | None = None
core_redis.get_redis = lambda: REDIS

# --- the in-memory world (pg_store reads with the SQL's semantics) --------------


class World:
    def __init__(self, boards, players, entries, fail=(), deleted=None):
        self.boards = boards        # [[uuid, name_id, name, override]]
        self.players = players      # [[id, name]]
        self.entries = entries      # [[board, anchor, rank, score, player_id]]
        self.fail = set(fail)       # anchors whose capture / grouping raises
        self.deleted = deleted or {"delete_renames": 3, "delete_duplicates": 4}
        self.fail_anchor_list = False

    def names(self):
        return dict(self.players)

    def to_json(self):
        return {"boards": self.boards, "players": self.players, "entries": self.entries,
                "fail_anchors": sorted(self.fail), "fail_anchor_list": self.fail_anchor_list,
                "deleted": self.deleted}


WORLD: World


async def all_anchors_asc(*, since=None):
    if WORLD.fail_anchor_list:
        raise RuntimeError("anchor scan timed out")
    return sorted({e[1] for e in WORLD.entries})


async def list_timestamps(limit=60, since=None):
    return sorted({e[1] for e in WORLD.entries}, reverse=True)[:limit]


async def list_boards_at(anchor):
    present = {e[0] for e in WORLD.entries if e[1] == anchor}
    out = []
    for uuid, name_id, name, override in WORLD.boards:
        if uuid in present:
            out.append({"uuid": uuid, "name_id": name_id, "name": name, "category_id": "c", "category": "c",
                        "contest_type": None, "reset_kind": pg_store._effective_reset_kind(override, uuid),
                        "reset_kind_override": override, "player_board": is_player_board(uuid)})
    return out


async def anchor_maps(anchor, board_uuids):
    if anchor in WORLD.fail:
        raise RuntimeError(f"capture {anchor} failed")
    names = WORLD.names()
    out: dict = {}
    for b, a, _rank, score, pid in WORLD.entries:
        if a != anchor or b not in board_uuids or pid not in names:
            continue
        board = out.setdefault(b, {})
        prev = board.get(names[pid])
        if prev is None or score > prev:
            board[names[pid]] = score
    return out


def pg_canon(score):
    return 0.0 if score == 0 else score


async def duplicate_groups_at(anchor):
    if anchor in WORLD.fail:
        raise RuntimeError(f"grouping {anchor} failed")
    seen: dict = {}
    for b, a, rank, score, pid in WORLD.entries:
        if a == anchor:
            seen.setdefault((pid, b), set()).add((rank, pg_canon(score)))
    names = WORLD.names()
    return [{"name": names.get(pid, str(pid)), "board_uuid": b, "occurrences": len(v)}
            for (pid, b), v in sorted(seen.items()) if len(v) > 1]


async def player_rows_window(name, window_start, window_end=None):
    who = [(pid, n) for pid, n in WORLD.players if n.lower() == name.strip().lower()]
    if not who:
        return []
    pid, pname = who[0]
    rows = [{"player_name": pname, "rank": r, "score": s, "leaderboard": b, "created_at": a}
            for b, a, r, s, p in WORLD.entries
            if p == pid and a >= window_start and b not in non_player_boards()
            and (window_end is None or a <= window_end)]
    return rows


pg_store.all_anchors_asc = all_anchors_asc
pg_store.list_timestamps = list_timestamps
pg_store.list_boards_at = list_boards_at
pg_store.anchor_maps = anchor_maps
pg_store.duplicate_groups_at = duplicate_groups_at
pg_store.player_rows_window = player_rows_window

# --- worlds ------------------------------------------------------------------------

T0 = 1_759_000_000
H = 3600

RENAME_BOARDS = [
    [1, "TROVE_MASTERY", "Trove Mastery", None],
    [20, "GEODE_MASTERY", "Geode Mastery", None],
    [1000, "PR_0", "Power Rank Knight", None],
    [1001, "PR_1", "Power Rank Gunslinger", None],
    [1002, "PR_2", "", None],
    [10001, "STAT_10001", "", None],
    [10004, "STAT_10004", "Weekly Stat", None],
    [4000, "EFFORT_0", "Effort Knight", None],
    [1100, "CLUB_PR", "Club Power Rank", None],
    [77, "OVERRIDE_NONE", "Overridden Lifetime", "none"],
    [2, "OVERRIDE_WEEKLY", "Overridden Weekly", "weekly"],
    [31, "EXCLUDABLE", "Excludable", None],
    [32000, "LEVIATHAN", "Leviathans", None],
]
LIFETIME = [1, 20, 1000, 1001, 1002, 10001, 77, 31]
RESETTING = [10004, 4000, 1100, 2, 32000]


def rename_world(seed: int, anchors: list[int]) -> World:
    rng = random.Random(seed)
    players: list = []
    entries: list = []
    pid = 100

    def new_player(name):
        nonlocal pid
        pid += 1
        players.append([pid, name])
        return pid

    # The steady population: present everywhere, slowly grinding.
    steady = []
    for i in range(30):
        p = new_player(f"Steady{i:02d}")
        boards = rng.sample(LIFETIME, rng.randint(1, 5))
        base = {b: float(rng.choice([rng.randint(1000, 9_000_000), rng.randint(1000, 50_000) + 0.5])) for b in boards}
        steady.append((p, base))
    for k, a in enumerate(anchors):
        for p, base in steady:
            for b, s in base.items():
                entries.append([b, a, rng.randint(1, 500), s + k * rng.choice([0, 0, 5, 100]), p])
            entries.append([10004, a, rng.randint(1, 500), float(rng.randint(0, 5000)), p])

    def carry(old_name, new_name, at, scores, drift=None, extra_new=None, keep_old_reset=True):
        """old_name holds `scores` before anchor index `at`, new_name after."""
        o = new_player(old_name)
        n = new_player(new_name)
        for k, a in enumerate(anchors):
            who = o if k < at else n
            for b, s in scores.items():
                v = s
                if who == n and drift and b in drift:
                    v = s * (1 + drift[b])
                entries.append([b, a, rng.randint(1, 900), v, who])
            if keep_old_reset:
                entries.append([4000, a, rng.randint(1, 900), float(rng.randint(0, 900)), who])
            if extra_new and who == n:
                for b, s in extra_new.items():
                    entries.append([b, a, rng.randint(1, 900), s, who])
        return o, n

    carry("OldName", "NewName", 2, {1: 7_654_321.0, 20: 1_234_567.0, 1000: 45_678.25})
    carry("Drifter", "Drifted", 3, {1: 2_000_000.0, 1001: 30_000.0}, drift={1001: 0.01})
    carry("ÜberOld", "ΣΟΦΟΣ", 4, {20: 999_999.5, 10001: 77_777.0, 77: 4242.0})
    carry("Bigger", "Biggest", 5, {1: 3_000_000.0, 20: 3_000_000.0, 1002: 3_000_000.0, 31: 55_555.0})
    carry("TooDrifty", "Drifted2", 2, {1: 5_000_000.0, 20: 6_000_000.0}, drift={20: 0.05})
    carry("Dropper", "Dropped", 3, {1: 8_100_000.0, 20: 8_200_000.0}, drift={20: -0.02})
    carry("OneBoard", "OneBoard2", 2, {1: 6_543_210.0})
    carry("LowScore", "LowScore2", 2, {1: 500.0, 20: 600.0})
    carry("ExclOnly", "ExclOnly2", 3, {31: 9_999_999.0, 77: 8_888_888.0})
    carry("SixBoards", "SixBoardsNew", 6, {1: 6_100_000.0, 20: 6_200_000.0, 1000: 61_000.0, 1001: 62_000.0,
                                           1002: 63_000.0, 10001: 64_000.0})
    # Ambiguous: one vanisher, two identical appearers.
    o = new_player("AmbOld")
    n1 = new_player("AmbNewA")
    n2 = new_player("AmbNewB")
    for k, a in enumerate(anchors):
        for who in ([o] if k < 3 else [n1, n2]):
            entries.append([1, a, 5, 4_444_444.0, who])
            entries.append([20, a, 6, 5_555_555.0, who])
    # Not mutual: two vanishers, one appearer holding both fingerprints' scores.
    x1 = new_player("PairX1")
    x2 = new_player("PairX2")
    y = new_player("PairY")
    for k, a in enumerate(anchors):
        if k < 4:
            entries += [[1, a, 9, 7_000_001.0, x1], [20, a, 9, 7_000_002.0, x1], [1000, a, 9, 7_000_003.0, x1]]
            entries += [[1, a, 9, 7_000_001.0, x2], [20, a, 9, 7_000_002.0, x2]]
        else:
            entries += [[1, a, 9, 7_000_001.0, y], [20, a, 9, 7_000_002.0, y], [1000, a, 9, 7_000_003.0, y]]
    # Two spellings of one key share a fingerprint; a case change is no rename.
    s1 = new_player("Spacey ")
    s2 = new_player(" spacey")
    c1 = new_player("CaseMan")
    c2 = new_player("caseman")
    for k, a in enumerate(anchors):
        entries += [[1, a, 3, 6_000_000.0 + k, s1], [20, a, 3, 6_100_000.0, s2]]
        entries.append([1, a, 4, 5_900_000.0, c1 if k < 3 else c2])
        entries.append([1000, a, 4, 5_900_001.0, c1 if k < 3 else c2])
    # A vanish whose new name only shows up on a resetting board.
    carry("ResetOnly", "ResetOnly2", 4, {10004: 4_000_000.0, 4000: 5_000_000.0})
    # Doubled rows for one player keep the higher score.
    d = new_player("Doubled")
    dn = new_player("Doubled2")
    for k, a in enumerate(anchors):
        who = d if k < 5 else dn
        entries += [[1, a, 11, 1_111_111.0, who], [1, a, 12, 1_000_000.0, who], [20, a, 11, 2_222_222.0, who]]
    # A player id with no player row.
    for a in anchors:
        entries.append([1, a, 99, 3_333_333.0, 99999])
    return World([list(b) for b in RENAME_BOARDS], players, entries)


DUP_BOARDS = [
    [1, "TROVE_MASTERY", "Trove Mastery", None],
    [11, "STAT_11", "Coins Collected", None],
    [12, "STAT_12", "", None],
    [4000, "EFFORT_0", "Effort Knight", None],
    [1100, "CLUB_PR", "Club Power Rank", None],
]


def dup_world(seed: int, anchors: list[int]) -> World:
    rng = random.Random(seed)
    players: list = []
    entries: list = []
    pid = 500

    def new_player(name):
        nonlocal pid
        pid += 1
        players.append([pid, name])
        return pid

    for i in range(12):
        p = new_player(f"Solo{i:02d}")
        for k, a in enumerate(anchors):
            entries.append([1, a, rng.randint(1, 300), 10_000.0 + 7 * k + i, p])
    late = new_player("LateCom")
    twins = new_player("Twins")
    idle = new_player("Idle")
    triple = new_player("Triple")
    gone = new_player("GoneDup")
    rep = new_player("Repeat")
    club = new_player("ClubAndMe")
    wide = new_player("WideDup")
    for k, a in enumerate(anchors):
        entries += [[11, a, 3, 8_100_000.0, late], [11, a, 40, 2_600_000.0 + 250_000.0 * k, late]]
        entries += [[1, a, 7, 5_000_000.0 + 1_000.0 * k, twins], [1, a, 8, 4_900_000.0 + 1_500.0 * k, twins]]
        entries += [[12, a, 20, 777.0, idle], [12, a, 21, 776.0, idle]]
        entries += [[11, a, 1, 9e6 + k, triple], [11, a, 2, 9e6, triple], [11, a, 50, 1e6 + 10 * k, triple]]
        if k < len(anchors) // 2:
            entries += [[1, a, 30, 123_456.0, gone], [1, a, 31, 120_000.0 + k, gone]]
        entries += [[12, a, 9, 4242.0, rep], [12, a, 9, 4242.0, rep]]
        entries += [[1100, a, 2, 50_000.0, club], [1100, a, 5, 40_000.0 + k, club]]
        entries += [[1, a, 60, 2.5e6 + k, wide], [1, a, 61, 2.4e6, wide],
                    [11, a, 60, 3.5e6, wide], [11, a, 62, 3.4e6 + k * 2, wide],
                    [12, a, 3, 0.0, wide], [12, a, 4, -0.0, wide], [12, a, 5, 1.0, wide]]
        if k % 3 == 0:
            entries.append([4000, a, 1, 100.0 * k, late])
    rng.shuffle(entries)
    return World([list(b) for b in DUP_BOARDS], players, entries)


def churn_world(anchors: list[int], per_pair: int) -> World:
    """per_pair identities rename at every capture: one batch of 500 fills mid-walk."""
    players: list = []
    entries: list = []
    pid = 2000
    for j in range(per_pair):
        for k, a in enumerate(anchors):
            pid += 1
            players.append([pid, f"C{j:02d}_{k:02d}"])
            entries += [[1, a, j + 1, 1_000_000.0 * 1.05 ** j, pid], [20, a, j + 1, 2_000_000.0 * 1.07 ** j, pid]]
    return World([list(b) for b in RENAME_BOARDS], players, entries)


def many_dups_world(anchors: list[int], n: int) -> World:
    players: list = []
    entries: list = []
    for j in range(n):
        pid = 3000 + j
        players.append([pid, f"Dup{j:02d}"])
        for k, a in enumerate(anchors):
            entries += [[11, a, 2 * j + 1, 5e6 + j, pid], [11, a, 2 * j + 2, 1e6 + k * (j % 3), pid]]
    return World([list(b) for b in DUP_BOARDS], players, entries)


def gappy(n: int, start: int, gaps: list[int]) -> list[int]:
    out = [start]
    for i in range(n - 1):
        out.append(out[-1] + gaps[i % len(gaps)])
    return out


# --- scenarios ----------------------------------------------------------------------


WORLDS: dict = {}


def run_scenario(name, world, call, args, settings, redis_held=None):
    global WORLD, REDIS
    WORLD = world
    key = next((k for k, w in WORLDS.items() if w == world.to_json()), None)
    if key is None:
        key = f"w{len(WORLDS)}"
        WORLDS[key] = world.to_json()
    WRITES.clear()
    REDIS_OPS.clear()
    REDIS = None if redis_held is None else FakeRedis(redis_held)
    cfg = reset_settings(**settings)
    renames._last_scanned_pair = None
    duplicates._last_scanned_anchor = None
    renames._KEYS = None
    renames._KEY_INDEX.clear()
    fn = {"renames.backfill": renames.backfill, "renames.detect_latest": renames.detect_latest,
          "duplicates.backfill": duplicates.backfill, "duplicates.detect_latest": duplicates.detect_latest}[call]
    results = []
    error = None
    for kw in args:
        try:
            results.append(asyncio.run(fn(**kw)))
        except Exception as exc:  # noqa: BLE001
            error = f"{type(exc).__name__}: {exc}"
            results.append(None)
    return {"name": name, "call": call, "args": args, "settings": cfg, "world": key,
            "redis": redis_held is not None, "redis_held": sorted(redis_held or []), "clock": CLOCK,
            "results": results, "error": error, "writes": json.loads(json.dumps(WRITES)),
            "redis_ops": list(REDIS_OPS)}


def scenarios():
    out = []
    ra = gappy(9, T0, [6 * H, 6 * H, 10 * H, 6 * H, 1 * H, 7 * H, 5 * H, 6 * H])
    rw = rename_world(7, ra)
    out.append(run_scenario("renames_backfill_default", rw, "renames.backfill", [{}], {}, redis_held=[]))
    out.append(run_scenario(
        "renames_backfill_clear_tuned", rw, "renames.backfill", [{"clear_first": True}],
        {"renames_excluded_board_uuids": " 31, x ,77,, +2_0 ", "renames_min_boards": 3,
         "renames_score_drift_pct": 0.05, "renames_min_confidence": 0.6, "renames_min_score": 400.0},
        redis_held=[]))
    out.append(run_scenario(
        "renames_backfill_loose", rw, "renames.backfill", [{}],
        {"renames_min_boards": 1, "renames_score_drift_pct": 0.0, "renames_min_confidence": 0.0,
         "renames_max_gap_seconds": 40000}, redis_held=[]))
    out.append(run_scenario("renames_backfill_running", rw, "renames.backfill", [{}], {},
                            redis_held=["lb:renames:running"]))
    out.append(run_scenario("renames_backfill_no_redis", rw, "renames.backfill", [{"clear_first": True}], {}))
    failing = rename_world(7, ra)
    failing.fail = {ra[4], ra[8]}
    out.append(run_scenario("renames_backfill_pair_fails", failing, "renames.backfill", [{}], {}, redis_held=[]))
    broken = rename_world(7, ra)
    broken.fail_anchor_list = True
    out.append(run_scenario("renames_backfill_anchor_scan_fails", broken, "renames.backfill", [{}], {},
                            redis_held=[]))
    many = churn_world(gappy(56, T0, [3 * H]), 2)
    out.append(run_scenario("renames_backfill_long", many, "renames.backfill", [{}],
                            {"renames_min_score": 0.0}, redis_held=[]))
    out.append(run_scenario("renames_backfill_churn", churn_world(gappy(20, T0, [H]), 30), "renames.backfill",
                            [{}], {}, redis_held=[]))
    out.append(run_scenario("renames_detect_latest", rw, "renames.detect_latest",
                            [{"force": False}, {"force": False}, {"force": True}], {}))
    near = rename_world(7, gappy(6, T0, [6 * H, 6 * H, 6 * H, 6 * H, 2 * H]))
    out.append(run_scenario("renames_detect_latest_near", near, "renames.detect_latest", [{}, {}],
                            {"renames_min_boards": 1}))
    out.append(run_scenario("renames_detect_latest_gap", rename_world(7, gappy(3, T0, [6 * H, 9 * H])),
                            "renames.detect_latest", [{}, {}, {"force": True}], {}))
    out.append(run_scenario("renames_detect_latest_disabled", rw, "renames.detect_latest", [{}],
                            {features.RENAMES_FLAG: False}))
    out.append(run_scenario("renames_detect_latest_one_capture", rename_world(7, [T0]),
                            "renames.detect_latest", [{}], {}))
    weekly_only = rename_world(7, gappy(3, T0, [H]))
    weekly_only.boards = [b if b[0] in RESETTING else [b[0], b[1], b[2], "weekly"] for b in weekly_only.boards]
    out.append(run_scenario("renames_detect_latest_no_lifetime", weekly_only, "renames.detect_latest", [{}], {}))

    da = gappy(21, T0, [12 * H])
    dw = dup_world(3, da)
    out.append(run_scenario("duplicates_backfill_default", dw, "duplicates.backfill", [{}], {}, redis_held=[]))
    out.append(run_scenario(
        "duplicates_backfill_clear_tuned", dw, "duplicates.backfill", [{"clear_first": True}],
        {"duplicates_max_names": 2, "duplicates_min_boards": 2, "duplicates_frozen_min_captures": 3,
         "duplicates_lookback_days": 3}, redis_held=[]))
    out.append(run_scenario("duplicates_backfill_running", dw, "duplicates.backfill", [{}], {},
                            redis_held=["lb:duplicates:running"]))
    out.append(run_scenario("duplicates_backfill_no_redis", dw, "duplicates.backfill", [{}],
                            {"duplicates_max_names": -1}))
    dfail = dup_world(3, da)
    dfail.fail = {da[3], da[-1]}
    out.append(run_scenario("duplicates_backfill_anchor_fails", dfail, "duplicates.backfill", [{}], {},
                            redis_held=[]))
    dbroken = dup_world(3, da)
    dbroken.fail_anchor_list = True
    out.append(run_scenario("duplicates_backfill_anchor_scan_fails", dbroken, "duplicates.backfill", [{}], {},
                            redis_held=[]))
    dlong = many_dups_world(gappy(230, T0, [H]), 4)
    out.append(run_scenario("duplicates_backfill_long", dlong, "duplicates.backfill", [{}],
                            {"duplicates_max_names": 3}, redis_held=[]))
    out.append(run_scenario("duplicates_backfill_many", many_dups_world(gappy(8, T0, [6 * H]), 60),
                            "duplicates.backfill", [{}], {}, redis_held=[]))
    out.append(run_scenario("duplicates_detect_latest_many", many_dups_world(gappy(4, T0, [6 * H]), 7),
                            "duplicates.detect_latest", [{}], {"duplicates_max_names": 5}))
    out.append(run_scenario("duplicates_detect_latest", dw, "duplicates.detect_latest",
                            [{}, {}, {"force": True}], {}))
    out.append(run_scenario("duplicates_detect_latest_tuned", dw, "duplicates.detect_latest", [{}],
                            {"duplicates_min_boards": 2, "duplicates_max_names": 1,
                             "duplicates_frozen_min_captures": 50}))
    out.append(run_scenario("duplicates_detect_latest_disabled", dw, "duplicates.detect_latest", [{}],
                            {features.DUPLICATES_FLAG: False}))
    out.append(run_scenario("duplicates_detect_latest_empty", World([], [], []), "duplicates.detect_latest",
                            [{}], {}))
    return out


# --- pure: _match and split_series --------------------------------------------------


def match_cases():
    rng = random.Random(42)
    pool = ["al", "bo", "cy", "di", "ed", "fa", "gu", "hi", "Ïo", "ja"]
    boards = [1, 20, 1000, 1001, 1002]
    values = [0.0, 1000.0, 1000.5, 5000.0, 123_456.0, 2_000_000.0, -50.0]
    drifts = [0.0, 0.0, 0.004, 0.019, 0.021, -1e-10, -0.001, 0.5]
    cases = []
    for n in range(160):
        fp_a, fp_b = {}, {}
        for nm in rng.sample(pool, rng.randint(1, 6)):
            fp_a[nm] = {"display": nm.upper() if rng.random() < 0.3 else nm,
                        "boards": {b: rng.choice(values) for b in rng.sample(boards, rng.randint(1, 4))}}
        src = list(fp_a.items())
        for nm in rng.sample(pool, rng.randint(1, 6)):
            if nm in fp_a and rng.random() < 0.5:
                fp_b[nm] = fp_a[nm]
                continue
            if src and rng.random() < 0.7:
                _, base = rng.choice(src)
                bs = {b: s * (1 + rng.choice(drifts)) for b, s in base["boards"].items() if rng.random() < 0.85}
                if rng.random() < 0.3:
                    bs[rng.choice(boards)] = rng.choice(values)
            else:
                bs = {b: rng.choice(values) for b in rng.sample(boards, rng.randint(1, 4))}
            fp_b[nm + "2"] = {"display": nm + "2", "boards": bs}
        cfg = renames._Cfg(max_gap=27000, min_boards=rng.choice([1, 2, 2, 3, 0, -1]),
                           drift=rng.choice([0.02, 0.02, 0.05, 0.0, -0.01]),
                           min_score=1000.0, min_conf=rng.choice([0.5, 0.0, 0.8]),
                           board_names={1: "Trove Mastery", 20: "Geode"})
        events = renames._match(fp_a, fp_b, 100, 200 + n, cfg, 1_700_000_000 + n)
        cases.append({
            "fp_a": {k: {"display": v["display"], "boards": [[b, s] for b, s in v["boards"].items()]} for k, v in fp_a.items()},
            "fp_b": {k: {"display": v["display"], "boards": [[b, s] for b, s in v["boards"].items()]} for k, v in fp_b.items()},
            "from_anchor": 100, "to_anchor": 200 + n, "now": 1_700_000_000 + n,
            "cfg": {"min_boards": cfg.min_boards, "drift": cfg.drift, "min_conf": cfg.min_conf,
                    "board_names": [[b, s] for b, s in cfg.board_names.items()]},
            "rows": payload_rows(events),
        })
    return cases


def payload_rows(events):
    WRITES.clear()
    asyncio.run(pg_store.upsert_renames(events))
    return WRITES[0]["rows"] if WRITES else []


def split_cases():
    rng = random.Random(9)
    cases = []
    for n in range(140):
        k = rng.choice([1, 2, 2, 2, 3, 4, 6, 7])
        pts = []
        lines = [[rng.choice([0.0, 10.0, 5e5, 8.1e6]), rng.choice([0, 0, 1, 50, 2.5e5, -3.0])] for _ in range(k)]
        for a in range(rng.randint(1, 9)):
            for li, (base, rate) in enumerate(lines):
                if rng.random() < 0.15:
                    continue
                score = base + rate * a + rng.choice([0, 0, 0, 0.5, -7.0])
                pts.append({"created_at": 1000 + a * 3600, "rank": rng.randint(1, 4) + li, "score": score})
                if rng.random() < 0.08:
                    pts.append(dict(pts[-1]))
        rng.shuffle(pts)
        fmc = rng.choice([1, 3, 4, 50])
        split = duplicates.split_series(pts)
        summ = duplicates.summarise_series(split, frozen_min_captures=fmc)
        verdict = duplicates._verdict([{"series": summ}])
        text_args = [f"N{n}", rng.randint(1, 3), verdict, rng.randint(2, 4)]
        cases.append({
            "points": [[p["created_at"], p["rank"], p["score"]] for p in pts],
            "frozen_min_captures": fmc,
            "split": [[s["slot"], [[p["created_at"], p["rank"], p["score"]] for p in s["points"]]] for s in split],
            "summary": json.dumps(summ),
            "verdict": verdict,
            "text_args": text_args,
            "text": duplicates._summary_text(*text_args),
        })
    for v in ("case_only", "multi_live", "one_live", "all_idle"):
        for boards in (1, 2):
            args = [f"“Q” {v}", boards, v, 3]
            cases.append({"text_args": args, "text": duplicates._summary_text(*args)})
    return cases


# --- routes -------------------------------------------------------------------------


def jsonb_text(v) -> str:
    """Postgres jsonb output: object keys by (length, bytes), ', ' and ': '."""
    if isinstance(v, dict):
        items = sorted(v.items(), key=lambda kv: (len(kv[0].encode()), kv[0].encode()))
        return "{" + ", ".join(f"{json.dumps(k, ensure_ascii=False)}: {jsonb_text(x)}" for k, x in items) + "}"
    if isinstance(v, list):
        return "[" + ", ".join(jsonb_text(x) for x in v) + "]"
    return json.dumps(v, ensure_ascii=False)


def double(v) -> str:
    """What `$n::jsonb` with asyncpg's json.dumps codec stores for json.dumps(v)."""
    return jsonb_text(json.dumps(v))


ROUTE_RENAMES: list = []     # [id, from, to, from_anchor, to_anchor, conf, matched, evidence_text, mv, created_at]
ROUTE_DUPS: list = []        # [name, kind, verdict, boards, max_occ, spellings_text, first, last, evidence_text, mv, updated]
ROUTE_STATUS: dict = {}


def rename_record(r):
    keys = ["id", "from_name", "to_name", "from_anchor", "to_anchor", "confidence", "matched_boards",
            "evidence", "method_version", "created_at"]
    d = dict(zip(keys, r, strict=True))
    d["evidence"] = json.loads(d["evidence"])
    return pg_store._rename_row(d)


async def r_list_renames(*, limit, offset):
    rows = sorted(ROUTE_RENAMES, key=lambda r: (-r[4], -r[5], -r[0]))[offset:offset + limit]
    return [rename_record(r) for r in rows], len(ROUTE_RENAMES)


async def r_renames_for_name(name):
    lowered = name.strip().lower()
    rows = [r for r in ROUTE_RENAMES if r[1].strip().lower() == lowered or r[2].strip().lower() == lowered]
    rows.sort(key=lambda r: (-r[4], -r[0]))
    return [rename_record(r) for r in rows]


async def r_count_renames():
    return len(ROUTE_RENAMES)


def dup_record(r):
    keys = ["name", "kind", "verdict", "boards", "max_occurrences", "spellings", "first_anchor", "last_anchor",
            "evidence", "method_version", "updated_at"]
    d = dict(zip(keys, r, strict=True))
    d["spellings"] = json.loads(d["spellings"])
    d["evidence"] = json.loads(d["evidence"])
    return pg_store._duplicate_row(d)


async def r_list_duplicates(*, limit, offset, kind=None):
    rows = [r for r in ROUTE_DUPS if not kind or r[1] in (kind, "both")]
    latest = max((r[7] for r in ROUTE_DUPS), default=None)
    current = sum(1 for r in rows if r[7] == latest)
    page = sorted(rows, key=lambda r: (-r[7], -r[3], r[0].strip().lower()))[offset:offset + limit]
    return [dup_record(r) for r in page], len(rows), current


async def r_latest_duplicate_anchor():
    return max((r[7] for r in ROUTE_DUPS), default=None)


async def r_duplicate_for_name(name):
    for r in ROUTE_DUPS:
        if r[0].strip().lower() == name.strip().lower():
            return dup_record(r)
    return None


async def r_count_duplicates():
    return len(ROUTE_DUPS)


async def noop_backfill(*, clear_first=False):
    return None


def route_app():
    app = FastAPI()
    register_error_handlers(app)
    add_request_context_middleware(app)
    app.include_router(R.leaderboards_router, dependencies=[Depends(features.require_leaderboards_enabled)])
    app.include_router(SR.router)
    app.dependency_overrides[R._LB_PUBLIC.dependency] = lambda: AccessContext(user=None, token=None, ip=None)
    app.dependency_overrides[R._LB_MASTER.dependency] = lambda: IngestAuth(user=None, token=None)
    return TestClient(app, raise_server_exceptions=False)


def routes(scen):
    global REDIS
    pg_store.list_renames = r_list_renames
    pg_store.renames_for_name = r_renames_for_name
    pg_store.count_renames = r_count_renames
    pg_store.list_duplicates = r_list_duplicates
    pg_store.latest_duplicate_anchor = r_latest_duplicate_anchor
    pg_store.duplicate_for_name = r_duplicate_for_name
    pg_store.count_duplicates = r_count_duplicates
    renames.backfill = noop_backfill
    duplicates.backfill = noop_backfill

    # Stored rows: everything the scenarios wrote, plus odd shapes.
    rid = 0
    sources = {"renames_backfill_default", "renames_backfill_loose", "renames_detect_latest_near",
               "duplicates_backfill_default", "duplicates_backfill_clear_tuned", "duplicates_detect_latest_many"}
    scen = [s for s in scen if s["name"] in sources]
    for s in scen:
        for w in s["writes"]:
            if w["op"] == "upsert_renames":
                for row in w["rows"]:
                    rid += 1
                    ROUTE_RENAMES.append([rid, row[0], row[2], row[4], row[5], row[6], row[7],
                                          jsonb_text(row[8]), row[9], row[10]])
    seen_dup = set()
    for s in scen:
        for w in s["writes"]:
            if w["op"] == "upsert_duplicates":
                for row in w["rows"]:
                    if row[0] in seen_dup:
                        continue
                    seen_dup.add(row[0])
                    ROUTE_DUPS.append([row[1], row[2], row[3], row[4], row[5], jsonb_text(row[6]), row[7], row[8],
                                       jsonb_text(row[9]), row[10], row[11]])
    ROUTE_RENAMES.extend([
        [9001, "Chain A", "Chain B", T0, T0 + 10, 0.9, 2, jsonb_text({"b": [1, 2.5], "a": {"z": None}}), 1, 5],
        [9002, "chain b", "Chain C", T0 + 10, T0 + 20, 0.85, 3, double({}), 1, 6],
        [9003, "Chain C", " Chain D", T0 + 20, T0 + 30, 0.97, 3, jsonb_text("not json"), 1, 7],
        [9004, " chain d", "Chain E", T0 + 30, T0 + 40, 0.5, 2, jsonb_text(None), 1, 8],
        [9005, "Loop", "Chain A", T0 + 5, T0 + 10, 0.9, 2, double({"x": 1e-05, "y": 1e16, "z": 0.1}), 1, 9],
        [9006, "Same", "Same2", T0 + 10, T0 + 10, 0.9, 2, double([1, 2]), 1, 10],
    ] + [[9100 + i, f"Star{i}", "Hub", T0 + i, T0 + 50, 0.6, 2, double({"i": i}), 1, i] for i in range(45)])
    ROUTE_DUPS.extend([
        ["Robot", "case", "case_only", 3, 2, jsonb_text(["ROBOT", "Robot", "robot"]), T0, T0 + 99, jsonb_text({}), 1, T0],
        ["Mixed", "both", "one_live", 2, 3, jsonb_text(["MIXED", "Mixed"]), T0, T0 + 99, double({"lookback_days": 7}), 1, T0],
        ["Dated", "same_name", "not_analysed", 1, 2, jsonb_text([]), T0, T0 + 5, jsonb_text({}), 1, T0],
        ["Weird", "same_name", "all_idle", 1, 2, double(["x"]), T0, T0 + 5, jsonb_text("{bad"), 1, T0],
        ["NullSp", "same_name", "all_idle", 1, 2, double(None), T0, T0 + 5, double({"k": [1.5, 2]}), 1, T0],
    ])
    status_full = {"running": False, "total": 8, "done": 8, "detected": 4, "skipped_gap": 1, "started_at": 5,
                   "finished_at": 9, "last_anchor": 77, "clear_first": True, "lifetime_boards": 7,
                   "cleared": 3, "phase": "done"}
    client = route_app()
    out = []

    def call(name, path, flags=None, redis_status=None, method="GET"):
        global REDIS
        reset_settings(**(flags or {}))
        REDIS = FakeRedis()
        if redis_status is not None:
            REDIS.data.update(redis_status)
        renames._local_status = {"running": False}
        duplicates._local_status = {"running": False}
        r = client.request(method, path, headers={"X-Request-ID": RID})
        out.append({"name": name, "method": method, "path": path, "flags": dict(flags or {}),
                    "settings": {k: SETTINGS[k] for k in SETTING_KEYS},
                    "redis_status": redis_status, "status": r.status_code, "body": r.text,
                    "cache_control": r.headers.get("cache-control"),
                    "content_type": r.headers.get("content-type")})

    for pre in ("/v1/leaderboards", "/site/leaderboards"):
        call(f"{pre} renames", f"{pre}/renames")
        call(f"{pre} renames page", f"{pre}/renames?limit=3&offset=2&ignored=1")
        call(f"{pre} renames last", f"{pre}/renames?limit=200&offset=60")
        for q in ("limit=0", "limit=201&offset=-1", "limit=abc", "offset=1.5&limit=", "limit=1e2", "limit=+7&offset=%20003"):
            call(f"{pre} renames bad {q}", f"{pre}/renames?{q}")
        call(f"{pre} renames ttl", f"{pre}/renames?limit=1", flags={"renames_cache_ttl_seconds": 17})
        call(f"{pre} renames off", f"{pre}/renames", flags={features.RENAMES_FLAG: False})
        call(f"{pre} renames lb off", f"{pre}/renames", flags={features.LEADERBOARDS_FLAG: False})
        for nm in ("chain%20b", "CHAIN%20E", "Hub", "Star3", "nobody", "%C3%9CberOld", "%CE%A3%CE%9F%CE%A6%CE%9F%CE%A3",
                   "NewName", "Same", "%20loop", "a%25b", "x%FFy"):
            call(f"{pre} history {nm}", f"{pre}/renames/{nm}")
        call(f"{pre} history off", f"{pre}/renames/Hub", flags={features.RENAMES_FLAG: False})
        call(f"{pre} duplicates", f"{pre}/duplicates")
        call(f"{pre} duplicates page", f"{pre}/duplicates?limit=2&offset=1")
        for kind in ("same_name", "case", "both", "", "CASE"):
            call(f"{pre} duplicates kind {kind}", f"{pre}/duplicates?kind={kind}")
        call(f"{pre} duplicates bad", f"{pre}/duplicates?limit=0&kind=nope&offset=x")
        call(f"{pre} duplicates off", f"{pre}/duplicates", flags={features.DUPLICATES_FLAG: False})
        call(f"{pre} duplicates ttl", f"{pre}/duplicates?limit=1", flags={"duplicates_cache_ttl_seconds": 0})
        for nm in ("LateCom", "latecom", "Robot", "MIXED", "Dated", "Weird", "NullSp", "nobody", "%20Twins%20", "x%FFy"):
            call(f"{pre} dup lookup {nm}", f"{pre}/duplicates/{nm}")
        call(f"{pre} dup lookup off", f"{pre}/duplicates/LateCom", flags={features.DUPLICATES_FLAG: False})

    call("renames status empty", "/v1/leaderboards/renames-backfill-status")
    call("renames status", "/v1/leaderboards/renames-backfill-status",
         redis_status={"lb:renames:status": json.dumps(status_full)})
    call("renames status recorded twice", "/v1/leaderboards/renames-backfill-status",
         redis_status={"lb:renames:status": json.dumps({"recorded_renames": 1, "running": True})})
    call("renames status bad json", "/v1/leaderboards/renames-backfill-status",
         redis_status={"lb:renames:status": "{nope"})
    call("duplicates status", "/v1/leaderboards/duplicates-backfill-status",
         redis_status={"lb:duplicates:status": json.dumps({"running": True, "phase": "evidence", "total": 3,
                                                           "done": 1, "error": "x"})})
    call("duplicates status empty", "/v1/leaderboards/duplicates-backfill-status")
    call("status lb off", "/v1/leaderboards/duplicates-backfill-status", flags={features.LEADERBOARDS_FLAG: False})
    for q in ("", "?clear_first=true", "?clear_first=0", "?clear_first=maybe", "?clear_first=YES"):
        call(f"renames backfill {q}", f"/v1/leaderboards/renames-backfill{q}", method="POST")
        call(f"duplicates backfill {q}", f"/v1/leaderboards/duplicates-backfill{q}", method="POST")
    return out


def main():
    scen = scenarios()
    data = {
        "rid": RID,
        "worlds": WORLDS,
        "scenarios": scen,
        "match": match_cases(),
        "split": split_cases(),
    }
    data["routes"] = routes(scen)
    data["route_rows"] = {"renames": ROUTE_RENAMES, "duplicates": ROUTE_DUPS}
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(data, ensure_ascii=False, separators=(",", ":")) + "\n", encoding="utf-8")
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes): {len(scen)} scenarios, {len(data['routes'])} routes")


if __name__ == "__main__":
    main()
