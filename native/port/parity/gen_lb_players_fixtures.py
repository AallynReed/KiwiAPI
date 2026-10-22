"""Golden fixtures for kiwi-server's leaderboards player reads, captured from Python.

Mounts the real leaderboards router and the real /site router (scope and site
feature-flag dependencies overridden, the real error handlers and request-id
middleware) with the clock frozen and every store call ``service.py`` makes
(``pg_store``, the Redis snapshot cache, the ``site_users`` claim lookup and
``runtime_config``) answered from a small in-memory dataset. Each case records
the calls Python made with their answers, so the Rust test replays exactly
those answers and fails on any call Python did not make. A ``core`` section
pins the pure helpers (split_series, reset cliffs, mastery, CSV/jsonb parsing).

Run from the repo root with the production-pinned venv:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_lb_players_fixtures.py
Writes native/kiwi-server/testdata/lb_players/golden.json (``--full`` keeps every
body inline instead of hashing the large ones, for debugging; don't commit that).
"""

from __future__ import annotations

import copy
import hashlib
import json
import logging
import os
import random
import sys
import warnings
from datetime import datetime
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "lb_players" / "golden.json"
os.chdir(ROOT)
sys.path.insert(0, str(ROOT))
warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)

from fastapi import FastAPI  # noqa: E402
from fastapi.testclient import TestClient  # noqa: E402

from app.admin import runtime_config  # noqa: E402
from app.core.config import settings  # noqa: E402
from app.core.errors import register_error_handlers  # noqa: E402
from app.core.observability import add_request_context_middleware  # noqa: E402
from app.site import router as site_router  # noqa: E402
from app.site_auth.models import SiteUser  # noqa: E402
from app.trove import router as trove_router  # noqa: E402
from app.trove.leaderboards import cache as lb_cache  # noqa: E402
from app.trove.leaderboards import detection, duplicates, mastery, pg_store, service  # noqa: E402

assert not settings.gamedata_dir, "run without GAMEDATA_DIR so the baseline gamedata is read"

RID = "fixture-rid"
FROZEN = 1792510200.25            # 2026-10-20 15:30:00.25 UTC, a Tuesday
TDS = 1792494000                  # that trove-day's 11:00 UTC start
DAY = 86400


class FrozenDatetime(datetime):
    @classmethod
    def now(cls, tz=None):
        return datetime.fromtimestamp(FROZEN, tz)


service.datetime = FrozenDatetime
site_router.time = SimpleNamespace(time=lambda: FROZEN)


async def _no_archive(*_a, **_k):
    return None


trove_router._enforce_lb_archive_limit = _no_archive

# --- dataset ----------------------------------------------------------------

ANCHORS = [TDS + d * DAY + h * 3600 for d in range(-8, 0) for h in (0, 12)] + [TDS, TDS + 4 * 3600]

BOARDS = {
    1: ("Trove Mastery", "Leaderboard_TroveMastery", "Mastery", None),
    7: ("Enemies Defeated", "Leaderboard_Kills", "Stats", None),
    10: ("Big Number", "Leaderboard_Big", "Stats", "daily"),
    11: ("Coins Collected", "Leaderboard_Coins", "Stats", "  "),
    20: ("Geode Mastery", "Leaderboard_GeodeMastery", "Mastery", None),
    1000: ("KNIGHT", "Leaderboard_PR_Knight", "Power Rank", None),
    1003: ("DRACOLYTE", "Leaderboard_PR_Dracolyte", "Power Rank", None),
    1005: ("CANDY BARBARIAN", "Leaderboard_PR_Candy", "Power Rank", None),
    1007: ("SHADOW HUNTER", "Leaderboard_PR_Shadow", "Power Rank", None),
    1100: ("Club Power Rank", "Leaderboard_ClubPR", "Clubs", None),
    2001: ("Delve Depth", "Leaderboard_DelveDepth", "Delves", "none"),
    2300: ("Delve Wins", "Leaderboard_DelveWins", "Delves", None),
    4003: ("DRACOLYTE", "Leaderboard_Effort_Dracolyte", "Effort", "bogus"),
    4005: ("CANDY BARBARIAN", "Leaderboard_Effort_Candy", "Effort", None),
    5003: ("DRACOLYTE", "Leaderboard_Paragon_Dracolyte", "Paragon", None),
    32000: ("Leviathans", "Leaderboard_Leviathans", "Events “Daily”", None),
}

PLAYERS = [(1, "KiwiKnight"), (2, "DupeName"), (3, "Ünïcödé"), (4, "ColdOnly")]
ENTRIES: list[tuple[int, int, int, int, float]] = []   # player_id, board, anchor, rank, score


def since_reset(anchor: int, weekly: bool) -> float:
    start = ((anchor - 39600) // DAY) * DAY + 39600
    if weekly:
        while datetime.utcfromtimestamp(start).weekday() != 0:
            start -= DAY
    return (anchor - start) / 3600


for i, a in enumerate(ANCHORS):
    rows = [
        (1, 1, a, 2 + i % 3, 2_500_000.0 + i * 1234.5),
        (1, 20, a, 9, 150_000.0 + i * 10.0),
        (1, 1003, a, 1, 40123.0),
        (1, 1000, a, 4, 35000.6),
        (1, 4003, a, 1 + i % 4, since_reset(a, True) * 37.0),
        (1, 5003, a, 6, since_reset(a, True) * 3.5),
        (1, 32000, a, 2, since_reset(a, False) * 100.0 + 5.0),
        (1, 2001, a, 3, 777.0),
        (1, 10, a, 1, 1e16 if i % 2 else 2.5e-07),
        (1, 1100, a, 1, 5_000_000.0),
        (2, 1, a, 50, 1_000_000.0 + i),
        (2, 1005, a, 2, 39999.5),
    ]
    # Board 11: a live line climbing past a frozen one, listed in either order.
    live, frozen = 7_000_000.0 + i * 100_000.0, 8_100_000.0
    pair = [(2, 11, a, 3 if live > frozen else 4, live), (2, 11, a, 4 if live > frozen else 3, frozen)]
    rows += pair if i % 2 else pair[::-1]
    # Board 2300 (weekly): a third identity for a stretch, then gone again.
    rows.append((2, 2300, a, 10, since_reset(a, True) * 11.0))
    rows.append((2, 2300, a, 12, 400.0 + i))
    if 8 <= i <= 11:
        rows.append((2, 2300, a, 11, since_reset(a, True) * 9.0 + 1.0))
    # Board 7: six rows at two captures (the greedy matcher), one elsewhere.
    if i in (13, 14):
        rows += [(2, 7, a, 20 + k, 1000.0 * (k + 1) + i * k) for k in range(6)]
    else:
        rows.append((2, 7, a, 20, 1000.0 + i))
    # Board 4005: two lines plus a capture that emitted one row twice.
    rows.append((2, 4005, a, 5, since_reset(a, True) * 20.0))
    rows.append((2, 4005, a, 9, 42.0))
    if i == 11:
        rows.append((2, 4005, a, 9, 42.0))
    if i % 4 == 0:
        rows.append((2, 999, a, 1, 3.0 * i))
    if i % 3 == 1:
        rows += [(3, 1, a, 7, 900_000.0 + i), (3, 20, a, 30, 12_345.0), (3, 1007, a, 2, 30_250.5)]
    if i < 4:
        rows.append((4, 1, a, 99, 12.0 + i))
    ENTRIES.extend(rows)


def build_agg() -> list[dict]:
    out: dict[tuple[int, int], dict] = {}
    for pid, board, a, rank, score in ENTRIES:
        cur = out.get((pid, board))
        if cur is None:
            out[(pid, board)] = {"pid": pid, "leaderboard": board, "best_rank": rank, "appearances": 1,
                                 "first_seen": a, "last_seen": a, "latest_rank": rank,
                                 "latest_score": score, "last_rise": None}
            continue
        cur["best_rank"] = min(cur["best_rank"], rank)
        cur["appearances"] += 1
        if a > cur["last_seen"]:
            if score > cur["latest_score"]:
                cur["last_rise"] = a
            cur["last_seen"], cur["latest_rank"], cur["latest_score"] = a, rank, score
    return list(out.values())


AGG = build_agg()

RENAMES = [
    {"id": 12, "from_name": "AncientKiwi", "to_name": "OldKiwi", "from_anchor": ANCHORS[2],
     "to_anchor": ANCHORS[3], "confidence": 0.75, "matched_boards": 2,
     "evidence": {"boards": [{"uuid": 1, "score_from": 1.0, "score_to": 1.0}], "summary": "two boards"},
     "method_version": 1, "created_at": ANCHORS[3] + 60},
    {"id": 11, "from_name": "OldKiwi", "to_name": "KiwiKnight", "from_anchor": ANCHORS[5],
     "to_anchor": ANCHORS[6], "confidence": 1.0, "matched_boards": 3, "evidence": {},
     "method_version": 1, "created_at": ANCHORS[6] + 60},
    {"id": 15, "from_name": "Ünïcödé", "to_name": "ÜNÏ", "from_anchor": ANCHORS[7],
     "to_anchor": ANCHORS[8], "confidence": 0.5, "matched_boards": 2,
     "evidence": {"drift": 1e-05, "note": "“quoted” ✓"}, "method_version": 2, "created_at": ANCHORS[8]},
]

DUPLICATES = {
    "dupename": {"name": "DupeName", "kind": "same_name", "verdict": "one_live", "boards": 3,
                 "max_occurrences": 3, "spellings": ["DupeName"], "first_anchor": ANCHORS[0],
                 "last_anchor": ANCHORS[-1],
                 "evidence": {"summary": "Trove's own capture lists “DupeName” 2 times on the same board.",
                              "boards": []},
                 "method_version": 1, "updated_at": ANCHORS[-1] + 5},
    "ünïcödé": {"name": "Ünïcödé", "kind": "case", "verdict": "case_only", "boards": 1,
                "max_occurrences": 2, "spellings": ["Ünïcödé", "ünïcödé"], "first_anchor": ANCHORS[1],
                "last_anchor": ANCHORS[4], "evidence": {}, "method_version": 1, "updated_at": 7},
}

READY = ANCHORS[-2]
CHEATERS = {
    READY: json.loads(json.dumps({
        "players": [{"player_name": "x", "leaderboards": [], "confidence": 0.5}],
        "clusters": [
            {"boards": [{"uuid": 1}], "stem": "kiwi", "label": "kiwi*", "method": "name_stem",
             "confidence": 0.8125, "member_count": 3, "members": ["kiwiknight2", "KiwiKnight", "Kiwi3"],
             "members_truncated": False, "board_count": 2, "corroborated_by": ["comovement"],
             "summary": "Three “kiwi” accounts.", "extra": 1},
            {"stem": "other", "members": ["someone", None], "confidence": 1e-05},
            {"label": "case", "members": [None, "KIWIKNIGHT"], "member_count": 2, "summary": None},
            {"stem": "uni", "members": ["ÜNÏCÖDÉ"], "confidence": 12345678.9},
        ],
        "computed_at": 1792500000, "anchor": READY,
    })),
}

VERIFIED = {"kiwiknight", "ünïcödé"}

# --- recorded fakes ---------------------------------------------------------

CALLS: list[dict] = []
SETTINGS: dict = {}
OVERRIDES: dict = {}
FAIL: set[str] = set()
STATE: dict = {}


def answer(fn: str, args: list, compute):
    if fn in FAIL:
        CALLS.append({"fn": fn, "args": args, "raise": True})
        raise RuntimeError(f"injected {fn} failure")
    result = compute()
    CALLS.append({"fn": fn, "args": args, "result": copy.deepcopy(result)})
    return copy.deepcopy(result)


def who(name: str):
    for pid, pname in PLAYERS:
        if pname.lower() == name.strip().lower():
            return pid, pname
    return None


def _entry_dicts(pname: str, rows) -> list[dict]:
    return [{"player_name": pname, "rank": r, "score": s, "leaderboard": b, "created_at": a}
            for _pid, b, a, r, s in rows]


async def f_player_rows(name, *, limit, uuid=None, window_start=None):
    def compute():
        w = who(name)
        if w is None:
            return []
        rows = [e for e in ENTRIES if e[0] == w[0] and e[1] not in (1100, 21012)
                and (window_start is None or e[2] >= window_start) and (uuid is None or e[1] == uuid)]
        rows.sort(key=lambda e: -e[2])
        return _entry_dicts(w[1], rows[:limit])
    if uuid is not None and not -2**31 <= uuid < 2**31 and who(name) is not None:
        # asyncpg refuses the int4 parameter once the player resolved.
        CALLS.append({"fn": "player_rows", "args": [name, limit, uuid, window_start], "raise": True})
        raise OverflowError("value out of int32 range")
    return answer("player_rows", [name, limit, uuid, window_start], compute)


async def f_player_rows_window(name, window_start, window_end=None):
    def compute():
        w = who(name)
        if w is None:
            return []
        return _entry_dicts(w[1], [e for e in ENTRIES if e[0] == w[0] and e[2] >= window_start
                                   and e[1] not in (1100, 21012)])
    return answer("player_rows_window", [name, window_start], compute)


async def f_canonical(name):
    return answer("player_canonical_name", [name], lambda: (who(name) or (None, None))[1])


async def f_summary(name):
    def compute():
        w = who(name)
        if w is None:
            return []
        rows = [a for a in AGG if a["pid"] == w[0] and a["leaderboard"] not in (1100, 21012)]
        rows.sort(key=lambda a: (a["best_rank"], -a["last_seen"]))
        keys = ("leaderboard", "best_rank", "appearances", "first_seen", "last_seen", "latest_rank", "latest_score")
        return [{k: a[k] for k in keys} for a in rows]
    return answer("player_board_summary", [name], compute)


async def f_last_played(name, *, excluded):
    def compute():
        w = who(name)
        skip = set(excluded) | {1100, 21012}
        vals = [a["last_rise"] for a in AGG if w and a["pid"] == w[0] and a["leaderboard"] not in skip
                and a["last_rise"] is not None]
        return max(vals) if vals else None
    return answer("player_last_played", [name, sorted(excluded)], compute)


def _meta(uuids):
    return {u: {"name": BOARDS[u][0], "name_id": BOARDS[u][1], "category": BOARDS[u][2],
                "reset_kind": pg_store._effective_reset_kind(BOARDS[u][3], u)} for u in uuids if u in BOARDS}


async def f_board_meta(uuids):
    if not uuids:
        return {}
    out = answer("board_meta", [list(uuids)], lambda: [[u, m] for u, m in _meta(uuids).items()])
    return dict((u, m) for u, m in out)


async def f_board_kinds(uuids):
    if not uuids:
        return {}
    out = answer("board_kinds", [list(uuids)], lambda: [[u, m["reset_kind"]] for u, m in _meta(uuids).items()])
    return dict((u, k) for u, k in out)


async def f_renames(name):
    def compute():
        lowered = name.strip().lower()
        rows = [r for r in RENAMES if r["from_name"].strip().lower() == lowered
                or r["to_name"].strip().lower() == lowered]
        rows.sort(key=lambda r: (-r["to_anchor"], -r["id"]))
        return rows
    return answer("renames_for_name", [name], compute)


async def f_duplicate(name):
    return answer("duplicate_for_name", [name], lambda: DUPLICATES.get(name.strip().lower()))


async def f_timestamps(limit=60, since=None):
    return answer("list_timestamps", [limit, since], lambda: sorted(set(STATE.get("anchors", ANCHORS)),
                                                                     reverse=True)[:limit])


async def f_ready():
    return answer("get_ready_anchor", [], lambda: STATE.get("ready", READY))


async def f_cheaters(anchor):
    return answer("get_cheaters", [anchor], lambda: CHEATERS.get(anchor))


async def f_find_one(query):
    claimed = query["claimed_trove_name"]
    assert query == {"claimed_trove_name": claimed, "claim_verified": True}
    found = answer("site_user_claim", [claimed], lambda: claimed in VERIFIED)
    return object() if found else None


async def f_setting(key):
    value = OVERRIDES[key] if key in OVERRIDES else runtime_config._spec(key).default
    SETTINGS[key] = value
    return value


pg_store.player_rows = f_player_rows
pg_store.player_rows_window = f_player_rows_window
pg_store.player_canonical_name = f_canonical
pg_store.player_board_summary = f_summary
pg_store.player_last_played = f_last_played
pg_store.board_meta = f_board_meta
pg_store.board_kinds = f_board_kinds
pg_store.renames_for_name = f_renames
pg_store.duplicate_for_name = f_duplicate
pg_store.list_timestamps = f_timestamps
lb_cache.get_ready_anchor = f_ready
lb_cache.get_cheaters = f_cheaters
SiteUser.find_one = f_find_one
runtime_config.get_setting = f_setting

# --- app --------------------------------------------------------------------

app = FastAPI()
register_error_handlers(app)
add_request_context_middleware(app)
app.include_router(trove_router.leaderboards_router)
app.include_router(site_router.router)
CTX = SimpleNamespace(token=SimpleNamespace(id="parity"))
app.dependency_overrides[trove_router._LB.dependency] = lambda: CTX
app.dependency_overrides[trove_router._LB_PUBLIC.dependency] = lambda: CTX
app.dependency_overrides[site_router._resolve_feature_flags] = lambda: None
client = TestClient(app, raise_server_exceptions=False)


BODY_INLINE = 12000
ANSWERS: list[dict] = []
_ANSWER_AT: dict[str, int] = {}


def answer_index(c: dict) -> int:
    key = json.dumps(c, sort_keys=True)
    if key not in _ANSWER_AT:
        _ANSWER_AT[key] = len(ANSWERS)
        ANSWERS.append(c)
    return _ANSWER_AT[key]


def call(name: str, path: str, *, overrides=None, fail=(), state=None) -> dict:
    CALLS.clear()
    SETTINGS.clear()
    OVERRIDES.clear()
    OVERRIDES.update(overrides or {})
    FAIL.clear()
    FAIL.update(fail)
    STATE.clear()
    STATE.update(state or {})
    detection._CACHE.clear()
    detection._LAST_GOOD = None
    r = client.get(path, headers={"X-Request-ID": RID})
    out = {"name": name, "path": path, "status": r.status_code,
           "content_type": r.headers.get("content-type"), "cache_control": r.headers.get("cache-control")}
    if len(r.content) > BODY_INLINE and "--full" not in sys.argv:
        out["body_sha256"] = hashlib.sha256(r.content).hexdigest()
    else:
        out["body"] = r.content.decode("utf-8")
    # Detection's own knobs only steer a recompute; the Rust profile never reads them.
    out["settings"] = {k: v for k, v in SETTINGS.items() if not k.startswith("cheaters_")}
    out["calls"] = [answer_index(c) for c in CALLS]
    return out


def cases() -> list[dict]:
    out = []
    v1, site = "/v1/leaderboards/players", "/site/leaderboards/players"
    uni = "%C3%9Cn%C3%AFc%C3%B6d%C3%A9"
    names = {"kiwi": "KiwiKnight", "spaced": "%20kiwiknight%20", "dupe": "DupeName", "uni": uni,
             "cold": "ColdOnly", "nobody": "Nobody"}
    for key, n in names.items():
        out.append(call(f"v1_history_{key}", f"{v1}/{n}/history"))
        out.append(call(f"v1_profile_{key}", f"{v1}/{n}/profile"))
        out.append(call(f"v1_boards_{key}", f"{v1}/{n}/boards"))
        out.append(call(f"v1_series_{key}", f"{v1}/{n}/series"))
        out.append(call(f"site_history_{key}", f"{site}/{n}/history"))
        out.append(call(f"site_profile_{key}", f"{site}/{n}/profile"))
        out.append(call(f"site_series_{key}", f"{site}/{n}/series"))
    out += [
        call("v1_history_uuid_limit", f"{v1}/KiwiKnight/history?uuid=1003&limit=5"),
        call("v1_history_dupe_all", f"{v1}/DupeName/history?limit=500"),
        call("v1_history_uuid_float", f"{v1}/KiwiKnight/history?uuid=11.0&limit=3"),
        call("v1_history_uuid_int32", f"{v1}/KiwiKnight/history?uuid=9999999999"),
        call("v1_history_uuid_int32_nobody", f"{v1}/Nobody/history?uuid=9999999999"),
        call("v1_history_bad_both", f"{v1}/KiwiKnight/history?limit=501&uuid=x"),
        call("v1_history_bad_limit", f"{v1}/KiwiKnight/history?limit=0"),
        call("v1_history_empty_uuid", f"{v1}/KiwiKnight/history?uuid="),
        call("v1_history_tier3", f"{v1}/KiwiKnight/history", overrides={"leaderboards_pg_tier_after_days": 3}),
        call("v1_series_days3", f"{v1}/KiwiKnight/series?days=3"),
        call("v1_series_days30", f"{v1}/DupeName/series?days=30"),
        call("v1_series_tier0", f"{v1}/KiwiKnight/series?days=2",
             overrides={"leaderboards_pg_tier_after_days": 0}),
        call("v1_series_bad", f"{v1}/KiwiKnight/series?days=31"),
        call("v1_series_bad_text", f"{v1}/KiwiKnight/series?days=ab"),
        call("v1_profile_dupe_no_flag", f"{v1}/DupeName/profile",
             overrides={"feature_leaderboard_duplicates_enabled": False}),
        call("v1_profile_summary_fails", f"{v1}/KiwiKnight/profile", fail={"player_board_summary"}),
        call("v1_profile_history_fails", f"{v1}/KiwiKnight/profile", fail={"player_rows_window"}),
        call("site_history_uuid", f"{site}/DupeName/history?uuid=11&limit=6"),
        call("site_history_limit3", f"{site}/KiwiKnight/history?limit=3"),
        call("site_history_weekly", f"{site}/KiwiKnight/history?uuid=4003&limit=12"),
        call("site_history_2300", f"{site}/DupeName/history?uuid=2300&limit=40"),
        call("site_history_7", f"{site}/DupeName/history?uuid=7&limit=20"),
        call("site_history_4005", f"{site}/DupeName/history?uuid=4005&limit=40"),
        call("site_history_bad", f"{site}/KiwiKnight/history?limit=abc&uuid=1.5"),
        call("site_history_tier2", f"{site}/DupeName/history", overrides={"leaderboards_pg_tier_after_days": 2}),
        call("site_series_days1", f"{site}/KiwiKnight/series?days=1"),
        call("site_series_days30", f"{site}/DupeName/series?days=30"),
        call("site_series_bad", f"{site}/KiwiKnight/series?days=0"),
        call("site_profile_flags_off", f"{site}/KiwiKnight/profile",
             overrides={"feature_cheater_detection_enabled": False, "feature_alt_clusters_enabled": False}),
        call("site_profile_one_flag", f"{site}/KiwiKnight/profile",
             overrides={"feature_cheater_detection_enabled": False}),
        call("site_profile_no_ready", f"{site}/KiwiKnight/profile", state={"ready": None}),
        call("site_profile_no_anchor", f"{site}/KiwiKnight/profile", state={"ready": None, "anchors": []}),
        call("site_profile_stale_ready", f"{site}/KiwiKnight/profile", state={"ready": ANCHORS[0]}),
        call("site_profile_csv", f"{site}/KiwiKnight/profile",
             overrides={"last_played_excluded_board_uuids": " 1, abc,1_003 ,\u0661\u0660,,4003,5003,32000"}),
        call("site_profile_csv_all", f"{site}/KiwiKnight/profile",
             overrides={"last_played_excluded_board_uuids": "1,20,1000,1003,4003,5003,32000,2001,10"}),
        call("site_profile_failures", f"{site}/KiwiKnight/profile",
             fail={"player_last_played", "renames_for_name", "get_cheaters"}),
        call("site_profile_dupe_fails", f"{site}/DupeName/profile", fail={"duplicate_for_name"}),
        call("site_profile_dupe_no_flag", f"{site}/DupeName/profile",
             overrides={"feature_leaderboard_duplicates_enabled": False}),
        call("site_profile_timestamps_fail", f"{site}/KiwiKnight/profile", state={"ready": None},
             fail={"list_timestamps"}),
        call("site_profile_canonical_fails", f"{site}/KiwiKnight/profile", fail={"player_canonical_name"}),
        call("site_profile_oldkiwi", f"{site}/oldkiwi/profile"),
    ]
    return out


# --- pure helpers -------------------------------------------------------------

def pts(rows):
    return [{"created_at": a, "rank": r, "score": s} for a, r, s in rows]


SPLIT_INPUTS = [
    [],
    [(3, 1, 10.0), (1, 2, 5.0), (2, 1, 7.0)],
    [(1, 1, 100.0), (1, 2, 50.0), (2, 1, 110.0), (2, 2, 50.0), (3, 2, 50.0), (3, 1, 120.0)],
    [(1, 3, 8.1e6), (1, 4, 7.0e6), (2, 3, 8.1e6), (2, 4, 7.5e6), (3, 3, 8.1e6), (3, 4, 8.0e6),
     (4, 3, 8.5e6), (4, 4, 8.1e6), (5, 4, 8.1e6), (5, 3, 9.0e6)],
    [(1, 1, 5.0), (1, 1, 5.0), (2, 1, 6.0), (2, 2, 4.0), (2, 2, 4.0)],
    [(1, 1, 30.0), (1, 2, 20.0), (2, 1, 31.0), (2, 2, 21.0), (2, 3, 1.0), (3, 1, 32.0), (3, 3, 2.0),
     (4, 2, 22.0), (4, 1, 33.0), (4, 3, 3.0)],
    [(1, k, 100.0 * (6 - k)) for k in range(6)] + [(2, k, 100.0 * (6 - k) + k) for k in range(6)]
    + [(3, k, 100.0 * (6 - k) + 2 * k) for k in range(5)],
    [(1, 1, 0.0), (1, 2, -0.0), (2, 1, 0.1), (2, 2, 0.2)],
    [(1, 1, 10.0), (2, 1, 9.0), (2, 2, 30.0), (3, 1, 0.0), (3, 2, 31.0)],
    [(5, 1, 1.0), (4, 2, 2.0), (4, 1, 3.0), (9, 1, 4.0), (9, 2, 5.0), (9, 3, 6.0), (7, 2, 2.5)],
    # Only the live line's own rate tells the two continuations apart.
    [(1, 1, 125.0), (1, 2, 100.0), (2, 1, 125.0), (2, 2, 110.0), (3, 1, 125.0), (3, 2, 120.0),
     (4, 1, 131.0), (4, 2, 126.0)],
    [(1, 1, 1000.0), (1, 2, 700.0), (2, 1, 1000.0), (2, 2, 700.0), (3, 1, 1000.0), (3, 2, 700.0),
     (4, 1, 1000.0), (4, 2, 800.0), (5, 1, 1000.0), (5, 2, 900.0), (6, 1, 1003.0), (6, 2, 999.0),
     (7, 2, 1004.0), (7, 1, 1101.0)],
]


def random_splits(n: int = 60) -> list:
    """Random walks of 2-6 identities sharing a name, some frozen, some
    missing from a capture, an occasional reset and a re-emitted row."""
    rng = random.Random(20261020)
    out = []
    for _ in range(n):
        k = rng.choice([2, 2, 3, 3, 4, 5, 6, 7])
        ids = [(round(rng.uniform(0, 1e5), 1), rng.choice([0.0, round(rng.uniform(1, 5e3), 1)])) for _ in range(k)]
        captures = rng.randint(3, 12)
        reset_at = rng.choice([None, None, rng.randint(1, captures - 1)])
        rows = []
        for a in range(captures):
            snap = []
            for start, rate in ids:
                if rng.random() < 0.15:
                    continue
                since = a if reset_at is None or a < reset_at else a - reset_at
                base = start if reset_at is None or a < reset_at else 0.0
                snap.append(round(base + rate * since + rng.choice([0.0, 0.0, rng.uniform(-50, 50)]), 1))
            snap.sort(reverse=True)
            rows += [(1000 + 3600 * a, pos + 1, sc) for pos, sc in enumerate(snap)]
            if rows and rng.random() < 0.1:
                rows.append(rows[-1])
        if rng.random() < 0.5:
            rng.shuffle(rows)
        out.append(rows)
    return out


SPLIT_INPUTS += random_splits()


def core_section() -> dict:
    out: dict = {}
    anchors = [0, 39599, 39600, 39601, TDS - 1, TDS, int(FROZEN), -1, -86400 * 3 + 5]
    out["trove_day_start"] = [[a, pg_store.trove_day_start(a)] for a in anchors]
    out["cold_keep_from"] = [[n, d, pg_store.cold_keep_from(n, d)] for n in (int(FROZEN), TDS, TDS - 1)
                             for d in (-2, 0, 1, 7, 30)]
    spans = [(TDS - 5 * DAY + 1, TDS + 3600), (TDS - 1, TDS), (TDS, TDS), (TDS, TDS - 1),
             (TDS - DAY, TDS - DAY + 1), (TDS - 9 * DAY, TDS + DAY), (TDS - 6 * DAY - 39600, TDS - DAY),
             (0, 20 * DAY), (3 * DAY + 7, 9 * DAY)]
    out["reset_boundaries"] = [[k, a, b, service.reset_boundaries_for_kind(k, a, b)]
                               for k in ("daily", "weekly", "none", "default") for a, b in spans]
    inj = [
        ("daily", [(TDS - 3600, 4, 120.0), (TDS + 3600, 2, 10.0)]),
        ("daily", [(TDS - 3600, 4, 0.0), (TDS + DAY * 2, 2, 10.0)]),
        ("weekly", [(TDS - 3 * DAY, 4, 5.5), (TDS - DAY, 3, 1.0), (TDS - DAY + 60, 3, 2.0), (TDS + 8 * DAY, 1, 9.0)]),
        ("daily", [(TDS - 1, 1, 7.0), (TDS, 1, 0.5)]),
        ("default", [(TDS - 3600, 4, 120.0), (TDS + 3600, 2, 10.0)]),
        ("none", [(1, 1, 1.0)]),
        ("weekly", [(TDS, 1, 3.0)]),
        ("daily", [(TDS - 2 * DAY - 7, 9, 1e16), (TDS - 5, 3, -4.0), (TDS + 5, 2, 6.0)]),
    ]
    out["inject"] = [{"kind": k, "points": pts(p), "out": service._inject_reset_zeros(pts(p), k)} for k, p in inj]
    splits = []
    for rows in SPLIT_INPUTS:
        inp = pts(rows)
        res = duplicates.split_series(inp)
        ident = {id(p): i for i, p in enumerate(inp)}
        splits.append({"points": inp, "out": [{"slot": s["slot"], "idx": [ident[id(p)] for p in s["points"]]}
                                              for s in res]})
    out["split"] = splits
    out["level"] = [[p, list(mastery.level_from_points(p))]
                    for p in (-5, 0, 1, 24, 25, 26, 100, 10_000, 28_000, 29_000, 150_010, 2_500_000, 10**12)]
    out["level_score"] = [[s, mastery.level_from_points(round(s))[0]]
                          for s in (0.5, 1.5, 2.5, 24.5, 25.5, 2_533_334.5, 150_210.0, 1e300)]
    csvs = ["", " 4003, abc,1_000 ,\u0661\u0660,,4003", "1100,21012", "+5, -3, 0x10, 1__0, 07", "\u00a012\u3000"]
    out["csv"] = [[c, sorted(service._parse_board_csv(c))] for c in csvs]
    jsonb = ['{"b": 1, "a": 2.0}', '"{\\"z\\": 1, \\"a\\": [1, 2.5]}"', '"not json"', '[]', '0', '{}',
             '"\\"inner\\""', '"null"', '[1, 2]', '{"n": 10000000000000000, "f": 1e-05}']
    rename_rows, dup_rows = [], []
    for text in jsonb:
        ev = json.loads(text)
        row = pg_store._rename_row({"id": 1, "from_name": "a", "to_name": "b", "from_anchor": 1, "to_anchor": 2,
                                    "confidence": 0.5, "matched_boards": 2, "evidence": ev, "method_version": 1,
                                    "created_at": 3})
        rename_rows.append([text, row["evidence"]])
        d = pg_store._duplicate_row({"name": "n", "kind": "case", "verdict": "v", "boards": 1, "max_occurrences": 2,
                                     "spellings": json.loads(text), "first_anchor": 1, "last_anchor": 2,
                                     "evidence": json.loads(text), "method_version": 1, "updated_at": 4})
        dup_rows.append([text, d["spellings"], d["evidence"]])
    out["rename_evidence"] = rename_rows
    out["duplicate_json"] = dup_rows
    lowers = ["KiwiKnight", " Ünïcödé ", "İSTANBUL", "ΣΊΣΥΦΟΣ", "ẞtraße", "\u00a0x\u2003"]
    out["lower"] = [[s, s.strip().lower()] for s in lowers]
    out["effective_reset_kind"] = [[ov, u, pg_store._effective_reset_kind(ov, u)]
                                   for ov in (None, "daily", "weekly", "none", "bogus", "", "Daily")
                                   for u in (1, 32000, 4003, 21012, 99)]
    return out


def main() -> None:
    golden = {"rid": RID, "now": FROZEN, "tier_default": settings.leaderboards_pg_tier_after_days,
              "core": core_section(), "cases": cases()}
    golden["answers"] = ANSWERS
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(golden, ensure_ascii=False, separators=(",", ":")) + "\n", encoding="utf-8")
    statuses: dict[int, int] = {}
    for c in golden["cases"]:
        statuses[c["status"]] = statuses.get(c["status"], 0) + 1
    print(f"wrote {OUT} ({len(golden['cases'])} cases, statuses {statuses})")


if __name__ == "__main__":
    main()
