"""Golden fixtures for kiwi-server's player + class activity, from Python.

Drives the REAL activity routes (/v1 and /site, feature gates and public scope
overridden) and the real compute functions in app/trove/leaderboards/activity.py
and class_activity.py over an in-memory Postgres/Redis stand-in with a fixed
clock, and records replies, Redis writes and Postgres writes.
src/activity/tests.rs replays every case.

Run from the repo root with the production-pinned venv:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_activity_fixtures.py
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import random
import sys
import types
import warnings
from collections import deque
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "activity" / "fixtures.json"
os.chdir(ROOT)
sys.path.insert(0, str(ROOT))

from fastapi import FastAPI  # noqa: E402
from fastapi.testclient import TestClient  # noqa: E402

import app.trove.leaderboards.activity as act  # noqa: E402
import app.trove.leaderboards.cache as lbc  # noqa: E402
import app.trove.leaderboards.class_activity as cact  # noqa: E402
import app.trove.leaderboards.detection as detection  # noqa: E402
import app.trove.leaderboards.pg_store as pg  # noqa: E402
import app.trove.leaderboards.service as svc  # noqa: E402
from app.admin import runtime_config  # noqa: E402
from app.core.dependencies import AccessContext  # noqa: E402
from app.core.errors import register_error_handlers  # noqa: E402
from app.core.features import require_class_activity_enabled, require_player_activity_enabled  # noqa: E402
from app.site import router as site_router  # noqa: E402
from app.trove import stats  # noqa: E402
from app.trove import router as trove_router  # noqa: E402

NOW = 1_792_598_400  # a Wednesday, 16:00 UTC
HOUR = 3600
DAY = 86400
rng = random.Random(20261022)

# ── fakes ────────────────────────────────────────────────────────────────────

SETTINGS: dict = {}


def reset_settings() -> None:
    SETTINGS.clear()
    SETTINGS.update({
        "cheaters_cache_ttl_seconds": 1800,
        "leaderboards_hot_retention_days": 3,
        "class_activity_power_rank_threshold": 25000,
        "class_activity_effort_threshold": 50,
        "class_activity_xp_threshold": 2_000_000,
    })


async def fake_get_setting(key):
    if key.startswith("feature_"):
        return SETTINGS.get(key, True)
    return SETTINGS[key]


from app.core.config import settings  # noqa: E402

object.__setattr__(settings, "postgres_dsn", "postgresql://fake/fake")
runtime_config.get_setting = fake_get_setting
detection.trigger_warmer = lambda: None
clock = types.SimpleNamespace(time=lambda: float(NOW))
act.time = clock
cact.time = clock

REDIS: dict = {}
REDIS_WRITES: list = []


async def fake_rget(key):
    raw = REDIS.get(key)
    return json.loads(raw) if raw is not None else None


async def fake_rset(key, value, ttl=6 * 3600):
    raw = json.dumps(value)
    REDIS[key] = raw
    REDIS_WRITES.append([key, raw, ttl])


lbc._rget = fake_rget
lbc._rset = fake_rset


class Store:
    def __init__(self):
        self.estimates: list[dict] = []
        self.class_estimates: list[dict] = []
        self.stamps: list[int] = []
        self.maps: dict[int, dict[int, dict[str, float]]] = {}
        self.boards: list[dict] = []
        self.active: dict[int, set[str]] = {}
        self.class_active: dict[tuple, bool] = {}
        self.writes: list = []


S = Store()


async def f_get_estimates(window_start=None):
    rows = sorted(S.estimates, key=lambda r: r["window_end"])
    return [dict(r) for r in rows if window_start is None or r["window_end"] >= window_start]


async def f_get_class_estimates(window_start=None):
    rows = sorted(S.class_estimates, key=lambda r: (r["window_end"], r["class_index"]))
    return [dict(r) for r in rows if window_start is None or r["window_end"] >= window_start]


async def f_list_timestamps(limit=60, since=None):
    out = sorted((s for s in S.stamps if since is None or s >= since), reverse=True)
    return out[:limit]


async def f_list_boards_at(anchor):
    present = set(S.maps.get(anchor, {}))
    return [dict(b) for b in S.boards if b["uuid"] in present]


async def f_anchor_maps(anchor, uuids):
    snap = S.maps.get(anchor, {})
    return {u: dict(snap[u]) for u in uuids if u in snap and snap[u]}


async def f_record_active_window(window_end, names):
    rows = sorted({n.lower() for n in names if n})
    S.writes.append(["record_active_window", window_end, rows])
    S.active[window_end] = set(rows)


async def f_prune_active_windows(cutoff):
    S.writes.append(["prune_active_windows", cutoff])
    for k in [k for k in S.active if k < cutoff]:
        del S.active[k]
    return 0


async def f_count_active_since(early, late):
    names = set()
    for k, v in S.active.items():
        if early < k <= late:
            names |= v
    return len(names)


async def f_upsert_estimate(window_end, window_start, duration_hours, estimate, boards_analyzed,
                            computed_at, estimate_24h=None):
    S.writes.append(["upsert_estimate", window_end, window_start, duration_hours, estimate,
                     boards_analyzed, computed_at, estimate_24h])


async def f_upsert_class_estimates(rows):
    S.writes.append(["upsert_class_estimates", [
        [r["class_index"], r["window_end"], r["window_start"], r["duration_hours"], r["estimate"],
         r.get("estimate_clean"), r["computed_at"], r.get("estimate_24h"), r.get("estimate_24h_clean")]
        for r in rows]])
    for r in rows:
        S.class_estimates = [x for x in S.class_estimates
                             if (x["class_index"], x["window_end"]) != (r["class_index"], r["window_end"])]
        S.class_estimates.append(dict(r))


async def f_record_class_active_window(window_end, sets):
    rows = sorted({
        (window_end, i, n.lower(), clean is not None and n in clean)
        for i, (names, clean) in sets.items() for n in names if n
    })
    S.writes.append(["record_class_active_window", window_end, [list(r[1:]) for r in rows]])
    for k in [k for k in S.class_active if k[0] == window_end]:
        del S.class_active[k]
    for we, i, n, c in rows:
        S.class_active[(we, i, n)] = S.class_active.get((we, i, n), False) or c


async def f_prune_class_active_windows(cutoff):
    S.writes.append(["prune_class_active_windows", cutoff])
    return 0


async def f_class_rollup_since(early, late):
    out = {}
    for r in S.class_estimates:
        if early < r["window_end"] <= late:
            o = out.setdefault(r["class_index"], {"raw": 0, "clean": 0, "windows": 0, "clean_windows": 0})
            o["windows"] += 1
            o["clean_windows"] += r.get("estimate_clean") is not None
    raw: dict = {}
    clean: dict = {}
    for (we, i, n), c in S.class_active.items():
        if early < we <= late:
            raw.setdefault(i, set()).add(n)
            if c:
                clean.setdefault(i, set()).add(n)
    for i, o in out.items():
        o["raw"] = len(raw.get(i, ()))
        o["clean"] = len(clean.get(i, ()))
    S.writes.append(["class_rollup_since", early, late,
                     {str(k): v for k, v in sorted(out.items())}])
    return out


for name, fn in {
    "get_estimates": f_get_estimates, "get_class_estimates": f_get_class_estimates,
    "list_timestamps": f_list_timestamps, "list_boards_at": f_list_boards_at,
    "anchor_maps": f_anchor_maps, "record_active_window": f_record_active_window,
    "prune_active_windows": f_prune_active_windows, "count_active_since": f_count_active_since,
    "upsert_estimate": f_upsert_estimate, "upsert_class_estimates": f_upsert_class_estimates,
    "record_class_active_window": f_record_class_active_window,
    "prune_class_active_windows": f_prune_class_active_windows,
    "class_rollup_since": f_class_rollup_since,
}.items():
    setattr(pg, name, fn)


def fresh() -> None:
    global S
    S = Store()
    REDIS.clear()
    REDIS_WRITES.clear()
    reset_settings()
    act.reset_caches()
    cact.reset_caches()


def run(coro):
    return asyncio.run(coro)


# ── apps ─────────────────────────────────────────────────────────────────────

api = FastAPI()
register_error_handlers(api)
api.include_router(trove_router.activity_router, dependencies=[])
api.include_router(trove_router.class_activity_router, dependencies=[])
api.include_router(site_router.router)
api.dependency_overrides[trove_router._ACTIVITY_PUBLIC.dependency] = lambda: AccessContext(None, None, None)
api.dependency_overrides[trove_router._LB_MASTER.dependency] = lambda: None
api.dependency_overrides[require_player_activity_enabled] = lambda: None
api.dependency_overrides[require_class_activity_enabled] = lambda: None
client = TestClient(api, raise_server_exceptions=False)

BACKFILLS: list = []


async def no_backfill(**kw):
    BACKFILLS.append(kw)


act.backfill_history_chunked = no_backfill
cact.backfill_class_history_chunked = no_backfill


def reply(r) -> dict:
    return {"status": r.status_code, "cache_control": r.headers.get("cache-control"), "body": r.text}


def get(path: str) -> dict:
    return reply(client.get(path))


# ── data ─────────────────────────────────────────────────────────────────────

NAMES = ["Ash", "brook", "Cedar", "ΣΟΦΟΣ", "Ünïcode", "dash-er", "Eve", "fox", "Gale", "Hazel",
         "Ivy", "Jade", "KELVIN\u212a", "Lark", "Moss", "Nyx", "Oak", "Pip", "Quill", "Rue",
         "Sage", "Tarn", "Umber", "Vale", "Wren", "Xeno", "Yew", "Zed", "İstanbul", "ǅemal"]
PLAYERS = {i + 1: n for i, n in enumerate(NAMES)}
BY_NAME = {n: i for i, n in PLAYERS.items()}


def est_rows(n, start, step_choices, null_rate=0.15, dur_jitter=True):
    rows, t = [], start
    for _ in range(n):
        step = rng.choice(step_choices)
        prev, t = t, t + step
        dur = (t - prev) / 3600.0
        rows.append({
            "window_end": t, "window_start": prev,
            "duration_hours": round(dur, 2) if dur_jitter else dur,
            "estimate": rng.randint(0, 5000), "boards_analyzed": rng.randint(1, 60),
            "computed_at": t + 30,
            "estimate_24h": None if rng.random() < null_rate else rng.randint(100, 9000),
        })
    return rows


def capture(players, boards, lo=0, hi=1_000_000, frac=0.7, integral=True):
    out = {}
    for b in boards:
        m = {}
        for p in players:
            if rng.random() < frac:
                v = rng.randint(lo, hi)
                m[PLAYERS[p]] = float(v) if integral else v + rng.choice([0.0, 0.25, 0.5])
        out[b] = m
    return out


def bump(snapshot, prob=0.5, drop=0.1, gain=(1, 5000)):
    out = {}
    for b, m in snapshot.items():
        n = {}
        for name, v in m.items():
            if rng.random() < drop:
                continue
            n[name] = v + (rng.randint(*gain) if rng.random() < prob else 0)
        out[b] = n
    return out


# ── cases ────────────────────────────────────────────────────────────────────

def series_cases() -> list:
    out = []
    specs = [
        ("7d", 40, NOW - 8 * DAY, [6 * HOUR, 6 * HOUR, 5 * HOUR, 7 * HOUR]),
        ("1m", 40, NOW - 31 * DAY, [DAY, 20 * HOUR, 26 * HOUR]),
        ("7D", 10, NOW - 7 * DAY, [3 * HOUR, 9 * HOUR, 30 * HOUR]),
        ("bogus", 12, NOW - 5 * DAY, [6 * HOUR]),
        ("", 0, NOW - DAY, [HOUR]),
        ("1m", 1, NOW - DAY, [HOUR]),
        ("7d", 30, NOW - 6 * DAY, [2 * HOUR, 2 * HOUR, HOUR + 1800]),
    ]
    for period, n, start, steps in specs:
        fresh()
        S.estimates = est_rows(n, start, steps)
        if n > 4:
            S.estimates[3]["estimate_24h"] = S.estimates[2]["estimate_24h"] = 7777
        q = f"?period={period}" if period != "" else "?period="
        v1 = get(f"/v1/activity/series{q}")
        writes = list(REDIS_WRITES)
        REDIS.clear()
        site = get(f"/site/leaderboards/activity/series{q}")
        out.append({"period": period, "rows": S.estimates, "now": NOW, "v1": v1, "site": site,
                    "redis": writes})
    return out


def history_cases() -> list:
    out = []
    fresh()
    S.estimates = est_rows(60, NOW - 32 * DAY, [6 * HOUR, 12 * HOUR, DAY, 0])
    S.estimates[5]["duration_hours"] = 0.0
    for q in ["", "?days=1", "?days=7", "?days=30", "?days=0", "?days=31", "?days=abc", "?days=-3", "?days=2.0", "?days=99999999999999999999999", "?days=-99999999999999999999999",
              "?days=1.5"]:
        out.append({"query": q, "v1": get(f"/v1/activity/history{q}"),
                    "site": get(f"/site/leaderboards/activity/history{q}")})
    return [{"rows": S.estimates, "now": NOW, "cases": out}]


def current_cases() -> list:
    out = []
    payloads = [
        None,
        {"window_start": NOW - 6 * HOUR, "window_end": NOW, "duration_hours": 6.0, "estimate": 1234,
         "estimate_24h": 4567, "estimate_7d": 9999, "window_24h_start": NOW - DAY, "window_7d_start": NOW - 7 * DAY,
         "span_24h_hours": 24.0, "span_7d_hours": 168.0,
         "by_board": [{"uuid": 1, "name": "Ünï", "category": "Cat", "active_players": 4},
                      {"uuid": 2, "name": "B", "category": "C\"q", "active_players": 1}],
         "boards_analyzed": 2, "methodology": act._METHODOLOGY, "computed_at": NOW - 5},
        {"window_start": NOW - 6 * HOUR, "window_end": NOW, "duration_hours": 6, "estimate": 3,
         "by_board": [], "boards_analyzed": 0, "methodology": "m", "computed_at": NOW, "extra": [1, 2.5]},
        {"window_start": None, "window_end": None, "duration_hours": 1e-05, "estimate": None,
         "estimate_24h": None, "span_24h_hours": 1e16, "by_board": [], "boards_analyzed": 0,
         "methodology": "", "computed_at": 0},
    ]
    for p in payloads:
        fresh()
        if p is not None:
            REDIS["lb:cache:ready_anchor"] = json.dumps(NOW)
            REDIS[f"lb:cache:activity:{NOW}"] = json.dumps(p)
        else:
            S.stamps = [NOW]
        out.append({"payload": p, "redis": REDIS.get(f"lb:cache:activity:{NOW}"),
                    "v1": get("/v1/activity/current"), "site": get("/site/leaderboards/activity")})
    return out


def compute_cases() -> list:
    """activity._compute over fake captures (Python's dict path = kiwi_native's pair rule)."""
    out = []
    boards = [
        {"uuid": 1100, "name": "CLUB", "category": "Clubs", "player_board": False},
        {"uuid": 7, "name": "ENEMIES", "category": "Combat", "player_board": True},
        {"uuid": 3, "name": "LOOT", "category": "Combat", "player_board": True},
        {"uuid": 21012, "name": "CLUB2", "category": "Clubs", "player_board": False},
        {"uuid": 40, "name": "FLUX", "category": "Economy", "player_board": True},
        {"uuid": 41, "name": "EMPTY", "category": "Economy", "player_board": True},
        {"uuid": 9, "name": "FISH", "category": "Life", "player_board": True},
    ]
    for k in range(6):
        fresh()
        S.boards = boards
        late = NOW - k * 3 * HOUR
        early = late - rng.choice([6 * HOUR, 5 * HOUR + 1234, 30 * HOUR])
        S.stamps = sorted({late, early} | {late - d for d in rng.sample(range(HOUR, 9 * DAY, 1800), 12)})
        uuids = [b["uuid"] for b in boards]
        e_cap = capture(list(PLAYERS), uuids)
        e_cap[41] = {}
        l_cap = bump(e_cap, prob=0.4 + 0.1 * k)
        for p in rng.sample(list(PLAYERS), 4):
            l_cap[9][PLAYERS[p]] = rng.choice([0.0, 2_000_000.0])
        if k == 5:
            l_cap = {u: dict(m) for u, m in e_cap.items()}
        S.maps = {late: l_cap, early: e_cap}
        for s in S.stamps:
            if s not in (late, early) and late - 9 * DAY < s:
                S.active[s] = {PLAYERS[p].lower() for p in rng.sample(list(PLAYERS), 5)}
        payload = run(act._compute(late, early))
        out.append({"late": late, "early": early, "boards": boards, "stamps": S.stamps,
                    "late_maps": {str(u): m for u, m in l_cap.items()},
                    "early_maps": {str(u): m for u, m in e_cap.items()},
                    "payload": payload, "writes": S.writes})
    return out


CLASS_LABELS = [[stats.class_name(i), stats.class_icon(i)] for i in range(stats.class_count())]


def class_rows(windows, null_rate=0.2):
    rows = []
    for we, ws in windows:
        for i in range(stats.class_count()):
            if rng.random() < 0.1:
                continue
            e24 = None if rng.random() < null_rate else rng.randint(0, 3000)
            rows.append({"class_index": i, "window_end": we, "window_start": ws,
                         "duration_hours": round((we - ws) / 3600.0, 2), "estimate": rng.randint(0, 900),
                         "estimate_clean": None if rng.random() < 0.2 else rng.randint(0, 500),
                         "computed_at": we + 10, "estimate_24h": e24,
                         "estimate_24h_clean": None if (e24 is None or rng.random() < 0.3) else rng.randint(0, e24)})
    return rows


def class_series_cases() -> list:
    out = []
    specs = [("7d", 8 * DAY, 6 * HOUR), ("1m", 32 * DAY, DAY), ("3m", 95 * DAY, 2 * DAY), ("6m", 200 * DAY, 5 * DAY),
             ("1y", 400 * DAY, 15 * DAY), ("all", 50 * DAY, 2 * DAY), ("ALL", 3 * DAY, 5 * HOUR), ("1d", 2 * DAY, HOUR),
             ("all", 0, HOUR)]
    for period, span, step in specs:
        fresh()
        if span:
            t, windows = NOW - span, []
            while t < NOW:
                windows.append((t + step, t))
                t += step + rng.choice([0, 0, 1800, -900])
            S.class_estimates = class_rows(windows)
        if period == "1y":
            SETTINGS["class_activity_xp_threshold"] = 0
            SETTINGS["class_activity_effort_threshold"] = "75"
        v1 = get(f"/v1/class-activity/series?period={period}")
        writes = list(REDIS_WRITES)
        REDIS.clear()
        site = get(f"/site/leaderboards/class-activity/series?period={period}")
        out.append({"period": period, "rows": S.class_estimates, "now": NOW, "settings": dict(SETTINGS),
                    "v1": v1, "site": site, "redis": writes})
    return out


def class_maps(players, lo, hi, frac=0.6):
    eff = {4000 + i: {PLAYERS[p]: float(rng.randint(lo, hi)) for p in players if rng.random() < frac}
           for i in range(stats.class_count()) if rng.random() < 0.85}
    return eff


def gate_maps(players, xp=True):
    g = {1000 + i: {PLAYERS[p]: float(rng.choice([0, 10_000, 25_000, 30_000, 120_000])) for p in players
                    if rng.random() < 0.8}
         for i in range(stats.class_count()) if rng.random() < 0.8}
    if xp:
        g[21005] = {PLAYERS[p]: float(rng.choice([0, 1_999_999, 2_000_000, 9_000_000])) for p in players
                    if rng.random() < 0.7}
    return g


def donut_cases() -> list:
    out = []
    for k in range(7):
        fresh()
        anchor = NOW - k * 7 * HOUR
        earlier = anchor - rng.choice([DAY, DAY + 3 * HOUR, 40 * HOUR, 30 * HOUR])
        S.stamps = sorted({anchor, earlier, anchor - 3 * HOUR, anchor - 9 * HOUR})
        effort = class_maps(list(PLAYERS), 0, 3000)
        early = {u: {n: max(0.0, v - rng.choice([0, 0, 10, 500, -20])) for n, v in m.items() if rng.random() < 0.9}
                 for u, m in effort.items()}
        gates = gate_maps(list(PLAYERS), xp=k != 3)
        if k == 4:
            SETTINGS["class_activity_xp_threshold"] = 0
        if k == 5:
            SETTINGS["class_activity_power_rank_threshold"] = "oops"
        if k == 6:
            effort = {}
        S.maps = {anchor: {**effort, **gates}, earlier: early}
        v1 = get("/v1/class-activity/current")
        writes = list(REDIS_WRITES)
        REDIS.clear()
        cact.reset_caches()
        site = get("/site/leaderboards/class-activity/current")
        out.append({"anchor": anchor, "stamps": S.stamps, "settings": dict(SETTINGS),
                    "maps": {str(a): {str(u): m for u, m in snap.items()} for a, snap in S.maps.items()},
                    "v1": v1, "site": site, "redis": writes})
    return out


def class_compute_cases() -> list:
    out = []
    for k in range(6):
        fresh()
        late = NOW - k * 11 * HOUR
        gap = [6 * HOUR, HOUR, 6 * HOUR, 30 * HOUR, 5 * HOUR, 6 * HOUR][k]
        if k == 1:  # a short window across the Monday reset
            late = 1_792_407_600 + 1200
        if k == 2:  # a long window across it
            late = 1_792_407_600 + 3 * HOUR
        early = late - gap
        S.stamps = sorted({late, early, early - 6 * HOUR, early - 12 * HOUR, early - 20 * HOUR})
        effort_e = class_maps(list(PLAYERS), 0, 3000)
        effort_l = {u: {n: v + rng.choice([0, 0, 5, 300]) for n, v in m.items()} for u, m in effort_e.items()}
        for i in (2, 5):
            effort_l.setdefault(4000 + i, {})[PLAYERS[i + 1]] = 9999.0
        gates = gate_maps(list(PLAYERS), xp=k != 4)
        S.maps = {late: {**effort_l, **gates}, early: effort_e}
        for (s_late, s_early) in zip(S.stamps[1:-1], S.stamps[:-2]):
            for i in range(stats.class_count()):
                if rng.random() < 0.7:
                    S.class_estimates.append({"class_index": i, "window_end": s_late, "window_start": s_early,
                                              "duration_hours": 1.0, "estimate": 1,
                                              "estimate_clean": None if rng.random() < 0.3 else 1,
                                              "computed_at": 0, "estimate_24h": None, "estimate_24h_clean": None})
                    for p in rng.sample(list(PLAYERS), 3):
                        S.class_active[(s_late, i, PLAYERS[p].lower())] = rng.random() < 0.5
        payload = run(cact.estimate_class_activity(force=True))
        out.append({"late": late, "early": early, "stamps": S.stamps, "settings": dict(SETTINGS),
                    "maps": {str(a): {str(u): m for u, m in snap.items()} for a, snap in S.maps.items()},
                    "payload": payload, "writes": S.writes})
    return out


def replay_cases() -> dict:
    """The backfill's chain rollups (player and class) step by step."""
    player, klass = [], []
    stamps = [NOW - 8 * DAY]
    while stamps[-1] < NOW:
        stamps.append(stamps[-1] + rng.choice([3 * HOUR, 4 * HOUR, 6 * HOUR, 6 * HOUR, 6 * HOUR, 6 * HOUR, 8 * HOUR, 20 * HOUR, 40 * HOUR]))
    active_floor = stamps[len(stamps) // 3]

    async def table_count(early, late):
        return 10 ** 9 + early

    async def table_class(stamps_, late):
        return {"table": [stamps_[0], late]}

    pg.count_active_since = table_count
    chain: deque = deque()
    for i in range(1, len(stamps)):
        early, late = stamps[i - 1], stamps[i]
        if i % 7 == 0:
            chain.clear()
        names = {PLAYERS[p] for p in rng.sample(list(PLAYERS), rng.randint(0, 6))}
        if chain and chain[-1][1] != early:
            chain.clear()
        chain.append((early, late, names))
        got = run(act._replay_rollup_24h(chain, stamps, late, active_floor))
        player.append({"early": early, "late": late, "names": sorted(names), "reset": i % 7 == 0,
                       "result": got, "chain": [[c[0], c[1]] for c in chain]})

    real_stored = cact._stored_rollups_24h
    cact._stored_rollups_24h = table_class
    chain = deque()
    for i in range(1, len(stamps)):
        early, late = stamps[i - 1], stamps[i]
        sets = {}
        for c in range(4):
            if rng.random() < 0.8:
                a = {PLAYERS[p] for p in rng.sample(list(PLAYERS), rng.randint(0, 5))}
                cl = None if rng.random() < 0.25 else {n for n in a if rng.random() < 0.5}
                sets[c] = (a, cl)
        if i % 5 == 0:
            chain.clear()
        if chain and chain[-1][1] != early:
            chain.clear()
        chain.append((early, late, sets))
        got = run(cact._replay_rollups_24h(chain, stamps, late, active_floor))
        klass.append({"early": early, "late": late,
                      "sets": {str(c): [sorted(a), None if cl is None else sorted(cl)] for c, (a, cl) in sets.items()},
                      "reset": i % 5 == 0,
                      "result": {str(k): list(v) for k, v in got.items()} if "table" not in got else got,
                      "chain": [[c[0], c[1]] for c in chain]})
    cact._stored_rollups_24h = real_stored
    pg.count_active_since = f_count_active_since
    return {"stamps": stamps, "active_floor": active_floor, "player": player, "class": klass}


def pure_cases() -> dict:
    resets = []
    for _ in range(300):
        t0 = NOW - rng.randint(0, 30 * DAY)
        t1 = t0 + rng.choice([0, -5, 1, HOUR, 2 * HOUR, DAY - 1, DAY, DAY + 1, 6 * DAY, 7 * DAY, 9 * DAY])
        if rng.random() < 0.2:
            t0 = (t0 // DAY) * DAY + 11 * HOUR + rng.choice([-1, 0, 1])
        for kind in ("daily", "weekly", "lifetime"):
            resets.append([kind, t0, t1, bool(svc.reset_boundaries_for_kind(kind, t0, t1))])
    picks = []
    for _ in range(100):
        stamps = sorted({NOW - rng.randint(0, 10 * DAY) for _ in range(rng.randint(0, 8))} | {NOW}, reverse=True)
        late = rng.choice(stamps)
        for back in (DAY, 7 * DAY):
            picks.append([stamps, late, back, act._pick_early_anchor(stamps, late, back)])
    medians = []
    for _ in range(60):
        vals = [rng.choice([0.0, 0.5, 1.25, 2.0, 6.0, 6.0, -1.0, 24.0, 7.5]) for _ in range(rng.randint(0, 7))]
        medians.append([vals, act._median(vals)])
    return {"resets": resets, "picks": picks, "medians": medians}


def backfill_cases() -> list:
    out = []
    for path in ["/v1/activity/backfill", "/v1/activity/backfill?reset=true&total_days=0&chunk_days=60",
                 "/v1/activity/backfill?force=yes&total_days=1000", "/v1/activity/backfill?total_days=-1&chunk_days=0",
                 "/v1/activity/backfill?force=maybe&reset=2", "/v1/activity/backfill?total_days=1001&chunk_days=61",
                 "/v1/class-activity/backfill", "/v1/class-activity/backfill?reset=on&total_days=12",
                 "/v1/class-activity/backfill?force=1", "/v1/class-activity/backfill?total_days=x&force=",
                 "/v1/class-activity/backfill?chunk_days=999"]:
        BACKFILLS.clear()
        r = client.post(path)
        out.append({"path": path, "reply": reply(r), "spawned": list(BACKFILLS)})
    return out


def gate_cases() -> list:
    out = []
    for flag, path in [("feature_player_activity_enabled", "/site/leaderboards/activity"),
                       ("feature_player_activity_enabled", "/site/leaderboards/activity/history?days=abc"),
                       ("feature_class_activity_enabled", "/site/leaderboards/class-activity/series"),
                       ("feature_leaderboards_enabled", "/site/leaderboards/activity/series"),
                       ("feature_class_activity_enabled", "/site/leaderboards/activity/series?period=1m")]:
        fresh()
        SETTINGS[flag] = False
        S.estimates = est_rows(5, NOW - DAY, [4 * HOUR])
        r = client.get(path, headers={"Accept": "text/html"})
        out.append({"flag": flag, "path": path, "rows": S.estimates, "reply": reply(r)})
    return out


def main() -> None:
    data = {
        "now": NOW,
        "class_labels": CLASS_LABELS,
        "series": series_cases(),
        "history": history_cases(),
        "current": current_cases(),
        "compute": compute_cases(),
        "class_series": class_series_cases(),
        "donut": donut_cases(),
        "class_compute": class_compute_cases(),
        "replay": replay_cases(),
        "pure": pure_cases(),
        "backfill": backfill_cases(),
        "gate": gate_cases(),
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(data, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
