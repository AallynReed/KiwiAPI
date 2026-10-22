"""Golden fixtures for kiwi-server's /v1/rotations port, captured from Python.

Every deterministic body is produced by the real Python functions and route
handlers (stores replaced by fakes) and serialized the way FastAPI does it
(``TypeAdapter(response_model).dump_json``). The sweep of instants stores a
short sha256 per route; a few instants also keep the full bodies.

Run from the repo root with the production-pinned venv:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_rotations_fixtures.py
"""

from __future__ import annotations

import asyncio
import hashlib
import json
import os
import random
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path
from types import SimpleNamespace

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "rotations" / "golden.json"
os.chdir(ROOT)
sys.path.insert(0, str(ROOT))

from pydantic import TypeAdapter  # noqa: E402

from app.core.config import settings  # noqa: E402
from app.core.errors import APIError  # noqa: E402
from app.trove import calendar, captures, chaos, delves, luxion, rotations, server_time  # noqa: E402
from app.trove import router as r  # noqa: E402
from app.trove import schemas as s  # noqa: E402

UTC = timezone.utc
EPOCH = datetime(1970, 1, 1, tzinfo=UTC)


def us_of(dt: datetime) -> int:
    return (dt - EPOCH) // timedelta(microseconds=1)


def dt_of(us: int) -> datetime:
    return EPOCH + timedelta(microseconds=us)


def ms_dt(ms: int) -> datetime:
    return EPOCH + timedelta(milliseconds=ms)


def dump(model, value) -> str:
    return TypeAdapter(model).dump_json(value).decode()


def short(body: str) -> str:
    return hashlib.sha256(body.encode()).hexdigest()[:16]


def run(coro):
    return asyncio.run(coro)


# --- captured Luxion runs the calendar is drawn with (newest first, ms precision) ---
RUNS_MS = [
    (us_of(datetime(2027, 2, 1, 11, tzinfo=UTC)) // 10**6, 1801519199999, None),
    (us_of(datetime(2026, 8, 3, 11, tzinfo=UTC)) // 10**6, 1785770525123, None),
    (us_of(datetime(2026, 7, 21, 11, tzinfo=UTC)) // 10**6, 1784646000500, 1785150000000),
    (us_of(datetime(2025, 10, 10, 11, tzinfo=UTC)) // 10**6, 1760094000000, None),
]


def runs_for_calendar() -> list[dict]:
    return [{"day_anchor": a, "first_seen": ms_dt(f), "ended": None if e is None else ms_dt(e)} for a, f, e in RUNS_MS]


def bodies_at(now: datetime) -> dict[str, str]:
    st = server_time
    return {
        "server_time": dump(s.ServerTime, s.ServerTime(**st.server_time(now))),
        "daily_buffs": dump(s.DailyBuffs, s.DailyBuffs(**st.daily_buffs(now))),
        "weekly_buffs": dump(s.WeeklyBuffs, s.WeeklyBuffs(**st.weekly_buffs(now))),
        "corruxion": dump(s.Corruxion, s.Corruxion(**st.corruxion(now))),
        "fluxion": dump(s.Fluxion, s.Fluxion(**st.fluxion(now))),
        "gardening": dump(s.Gardening, s.Gardening(**st.gardening(now))),
        "biomes": dump(s.BiomeRotationFeed, s.BiomeRotationFeed(**rotations.biome_rotation(now))),
        "wild_mana": dump(s.BiomeRotationFeed, s.BiomeRotationFeed(**rotations.wild_mana(now))),
        "stampy": dump(s.BiomeRotationFeed, s.BiomeRotationFeed(**rotations.stampy(now))),
        "challenge": dump(s.ChallengeCurrentOut, s.ChallengeCurrentOut(
            **st.challenge_window(now), name=None, type=None, captured_at=None)),
        "calendar": dump(s.YearlyCalendar, s.YearlyCalendar(
            **calendar.yearly_calendar(now, luxion_runs=runs_for_calendar()))),
    }


def raw_at(now: datetime) -> dict:
    w = server_time.chaos_chest_window(now)
    return {"chaos": [w["starts_at"], w["ends_at"]], "week": delves.current_week_id(now),
            "pull": delves._seconds_until_next_pull(now)}


def instants() -> list[datetime]:
    out: set[datetime] = set()
    days = [datetime(2026, 10, 19, tzinfo=UTC) + timedelta(days=i) for i in range(7)]
    days += [datetime(2025, 12, 29, tzinfo=UTC), datetime(2027, 1, 1, tzinfo=UTC),
             datetime(2028, 2, 29, tzinfo=UTC), datetime(2026, 3, 29, tzinfo=UTC),
             datetime(2026, 10, 25, tzinfo=UTC), datetime(2025, 11, 3, tzinfo=UTC)]
    times = [(10, 59, 59, 999999), (11, 0, 0, 0), (11, 0, 0, 1), (11, 19, 59, 999999), (11, 20, 0, 0),
             (11, 29, 59, 999999), (11, 30, 0, 0), (11, 49, 59, 999999), (11, 50, 0, 0), (12, 0, 0, 0),
             (13, 59, 59, 999999), (14, 0, 0, 0), (23, 59, 59, 999999), (0, 0, 0, 0), (0, 30, 0, 0)]
    for d in days:
        for h, m, sec, us in times:
            out.add(d.replace(hour=h, minute=m, second=sec, microsecond=us))
    for edge in [datetime(2025, 12, 31, 23, 59, 59, 999999, tzinfo=UTC), datetime(2026, 1, 1, tzinfo=UTC),
                 datetime(2026, 12, 31, 23, 59, 59, 999999, tzinfo=UTC), datetime(2028, 12, 31, 23, 59, 59, tzinfo=UTC),
                 datetime(2024, 6, 18, 11, tzinfo=UTC), datetime(2025, 10, 10, 11, tzinfo=UTC)]:
        out.add(edge)
    seed = datetime(2026, 10, 22, 12, 34, 56, 123456, tzinfo=UTC)
    marks: list[int] = []
    for fn in (server_time.corruxion, server_time.fluxion):
        res = fn(seed)
        marks += [res["starts_at"], res["ends_at"]] + [w["starts_at"] for w in res["schedule"][:3]]
    g = server_time.gardening(seed)
    marks += [w["starts_at"] for w in g["upcoming"][:4]] + [w["ends_at"] for w in g["upcoming"][:4]]
    for feed in (rotations.biome_rotation(seed), rotations.stampy(seed), rotations.wild_mana(seed)):
        marks += [w["starts_at"] for w in feed["upcoming"][:3]] + [w["ends_at"] for w in feed["upcoming"][:2]]
    marks += list(server_time.chaos_chest_window(seed).values())
    marks += [luxion.next_grid_slot(1785770525) + k * 27 * 3600 for k in range(3)]
    for m in marks:
        for delta in (-1, 0, 1):
            out.add(EPOCH + timedelta(seconds=m, microseconds=delta))
    rng = random.Random(20261022)
    lo, hi = us_of(datetime(2024, 7, 1, tzinfo=UTC)), us_of(datetime(2029, 1, 1, tzinfo=UTC))
    for i in range(160):
        us = rng.randrange(lo, hi)
        if i % 4 == 0:
            us -= us % 10**6
        out.add(dt_of(us))
    return sorted(out)


def sweep() -> tuple[list, dict]:
    rows, full = [], {}
    keep = {us_of(datetime(2026, 10, 22, 12, 34, 56, 123456, tzinfo=UTC))}
    for now in instants():
        b = bodies_at(now)
        us = us_of(now)
        rows.append({"us": us, "h": {k: short(v) for k, v in b.items()}, **raw_at(now)})
        if us in keep:
            full[str(us)] = {k: v for k, v in b.items() if k != "calendar"}
    for k in keep:
        if str(k) not in full:
            full[str(k)] = {k2: v for k2, v in bodies_at(dt_of(k)).items() if k2 != "calendar"}
    return rows, full


# --- Luxion -------------------------------------------------------------------

def luxion_cases() -> list[dict]:
    cases = []
    runs = [
        (1785754800, 1785770525123, 1785780000999, None),
        (1784631600, 1784646000500, 1785140000000, 1785150000000),
        (1760094000, 1760094000000, 1760100000000, None),
        (1801479600, 1801519199999, 1801519199999, None),
    ]
    for anchor, first_ms, last_ms, ended_ms in runs:
        doc = SimpleNamespace(started_at=anchor, first_seen_at=ms_dt(first_ms), last_seen_at=ms_dt(last_ms),
                              ended_at=None if ended_ms is None else ms_dt(ended_ms))
        sched = luxion.schedule_for(anchor, doc.first_seen_at, doc.ended_at)
        probes = {anchor - 1, first_ms // 1000 - 1, first_ms // 1000, first_ms // 1000 + 1,
                  anchor + 7 * 86400, anchor + 7 * 86400 - 1, anchor + 20 * 86400}
        for w in sched[:3]:
            probes |= {w["starts_at"] - 1, w["starts_at"], w["ends_at"] - 1, w["ends_at"], w["ends_at"] + 3600}
        if ended_ms is not None:
            probes |= {ended_ms // 1000 - 1, ended_ms // 1000}

        async def latest(doc=doc):
            return doc

        captures.get_latest_luxion = latest
        for p in sorted(probes):
            for frac in (0, 999999):
                now = EPOCH + timedelta(seconds=p, microseconds=frac)
                body = dump(s.Luxion, s.Luxion(**run(luxion.get_luxion(now))))
                case = {"anchor": anchor, "first_ms": first_ms, "last_ms": last_ms, "ended_ms": ended_ms,
                        "now_us": us_of(now)}
                case.update({"body": body} if len(cases) % 25 == 0 else {"h": short(body)})
                cases.append(case)
    return cases


def luxion_away() -> str:
    async def none():
        return None

    captures.get_latest_luxion = none
    return dump(s.Luxion, s.Luxion(**run(luxion.get_luxion())))


# --- Chaos chest ----------------------------------------------------------------

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
        {"items": [{"name": "No Times", "identifier": None, "blueprint": "", "start": None, "end": None}],
         "fetched_ms": 1792000000500},
        {"items": [{"name": "Shifted", "identifier": "x", "blueprint": "BP/Ünï+code&.blueprint",
                    "start": week["starts_at"] + 3600, "end": week["ends_at"] + 7200}], "fetched_ms": 1792000000001},
        {"items": [{"name": "", "identifier": None, "blueprint": None, "start": None, "end": None}],
         "fetched_ms": 1792000000001},
    ]
    captures_ = [None, {"name": "Captured Egg", "captured_ms": 1792050000777}]
    resolved = [None, "item/collections/mount_dragon~1.blueprint"]
    out = []
    for cap in captures_:
        for feed in feeds:
            for res in resolved:
                for now in (now0, dt_of(us_of(now0) + 6 * 86400 * 10**6), datetime(2026, 10, 20, 11, tzinfo=UTC)):
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
                    body = dump(s.ChaosChest, s.ChaosChest(**run(chaos.get_chaos_chest(now))))
                    out.append({"capture": cap, "feed": feed, "resolved": res, "now_us": us_of(now), "body": body})
    return out


def image_urls() -> list[list]:
    out = []
    orig = settings.api_url
    for api in ("https://api.aallyn.net", "http://x.test/", "https://h//"):
        settings.api_url = api
        for bp in (None, "", "item/a.blueprint", "a b/c?d=e&f#g", "ÅÆ~_.-!*'()", "%2F"):
            out.append([api, bp, chaos.item_image_url(bp)])
    settings.api_url = orig
    return out


CHAOS_PAYLOADS = [
    None, [], {}, "x", 5, [1], [{"name": "A"}], [{}],
    {"name": "Plain", "identifier": "Item\\Path\\x", "blueprint": "ITEM/Ä.Blueprint", "start": "1790000000",
     "end": 1790604800.9},
    {"data": {"name": "Wrapped", "start": " 12 ", "end": "1_000"}},
    {"data": [{"name": "inner"}], "name": "Outer"},
    {"data": {"data": {"name": "deep"}}, "name": "ignored"},
    {"name": 0}, {"name": False}, {"name": 7, "identifier": 5, "blueprint": ["B"], "start": True, "end": "x"},
    {"name": "S", "start": "3.5", "end": None, "extra": 1},
    {"name": "N", "start": "-7", "end": "+8"},
    {"name": {"k": "v"}, "start": [], "end": {}},
]

DELVE_PAYLOADS = [
    None, [], "x", {}, {"depths": []}, {"depths": [{"a": 1}], "total": 9},
    {"depths": [{"a": 1}, {"b": 2.5}], "total": "9"}, {"depths": "nope", "total": 3},
    {"depths": [{"a": 1}], "total": True}, {"depths": [{"a": 1}], "total": 2.0},
    {"total": 4},
]


def classify() -> list[list]:
    names = [None, "", "  ", "Collection Challenge", " Collection Challenge\t", "collection challenge",
             "RAMPAGE ALERT!", "Rampage", "Racing Challenge", "RACING", "Target Challenge", "Cursed Vale",
             "DUNGEON", "\x1cTarget Challenge\x1f", "　Racing Challenge　"]
    return [[n, captures.classify_challenge(n)] for n in names]


# --- history pages and delves (route handlers with fake stores) ----------------

def histories() -> dict:
    out = {}
    lux_docs = [
        SimpleNamespace(started_at=1785754800, first_seen_at=ms_dt(1785770525123),
                        last_seen_at=ms_dt(1785780000999), ended_at=None),
        SimpleNamespace(started_at=1784631600, first_seen_at=ms_dt(1784646000500),
                        last_seen_at=ms_dt(1785140000000), ended_at=ms_dt(1785150000000)),
        SimpleNamespace(started_at=1760094000, first_seen_at=ms_dt(1760094000000),
                        last_seen_at=ms_dt(1760094000000), ended_at=None),
    ]

    async def lux(limit=50, offset=0):
        return lux_docs, 17

    captures.list_luxion_history = lux
    out["luxion_history"] = {
        "docs": [[d.started_at, us_of(d.first_seen_at) // 1000, us_of(d.last_seen_at) // 1000,
                  None if d.ended_at is None else us_of(d.ended_at) // 1000] for d in lux_docs],
        "total": 17,
        "body": dump(s.LuxionHistoryPage, run(r.list_luxion_history(ctx=None, limit=50, offset=0))),
    }

    chaos_docs = [SimpleNamespace(week_anchor=1792062000, name="Captured Egg", captured_at=ms_dt(1792050000777)),
                  SimpleNamespace(week_anchor=1791457200, name="Ünïcode \"q\" \\ item", captured_at=ms_dt(1791457200000))]

    async def ch(limit=50, offset=0):
        return chaos_docs, 2

    captures.list_chaos_chest_history = ch
    out["chaos_history"] = {
        "docs": [[d.week_anchor, d.name, us_of(d.captured_at) // 1000] for d in chaos_docs],
        "total": 2,
        "body": dump(s.ChaosChestHistoryPage, run(r.list_chaos_chest_history(ctx=None, limit=50, offset=0))),
    }

    names = ["Collection Challenge", "RAMPAGE ALERT!", "Racing Challenge", "Target Challenge", "Cursed Vale"]
    chal_docs = [SimpleNamespace(window_anchor=1792000000 + i * 3600, window_ends_at=1792000000 + i * 3600 + 1200,
                                 name=n, is_friday_window=bool(i % 2), captured_at=ms_dt(1792000000000 + i * 3600123))
                 for i, n in enumerate(names)]

    async def chal(limit=50, offset=0):
        return chal_docs, 1234

    captures.list_challenge_history = chal
    out["challenge_history"] = {
        "docs": [[d.window_anchor, d.window_ends_at, d.name, d.is_friday_window, us_of(d.captured_at) // 1000]
                 for d in chal_docs],
        "total": 1234,
        "body": dump(s.ChallengeHistoryPage, run(r.list_challenge_history(ctx=None, limit=50, offset=0))),
    }

    current = []
    orig_window = server_time.challenge_window
    for now in (datetime(2026, 10, 23, 11, 35, 1, 5, tzinfo=UTC), datetime(2026, 10, 22, 12, 34, 56, tzinfo=UTC)):
        for doc in (None, chal_docs[1]):
            class FakeChallengeCapture:
                window_anchor = 0

                @staticmethod
                async def find_one(*_a, doc=doc, **_k):
                    return doc

            captures.ChallengeCapture = FakeChallengeCapture
            server_time.challenge_window = lambda now_=None, now=now: orig_window(now)
            body = dump(s.ChallengeCurrentOut, run(r.get_current_challenge(ctx=None)))
            current.append({"now_us": us_of(now), "doc": None if doc is None else [doc.name, us_of(doc.captured_at) // 1000],
                            "body": body})
    server_time.challenge_window = orig_window
    out["challenge_current"] = current

    depths = [
        {"depth": 1, "boss": {"name": "Ünï", "hp": 1.0, "ratio": 0.1, "tiny": 1e-7, "huge": 1e16, "neg": -0.0},
         "z": None, "a": [True, False, 2**40, -3, 12345.678, "x y"], "b": {}},
        {"zz": [], "depth": 2, "nested": [[1, 2.5], {"q": "\"quoted\""}], "big": 1.5e300, "small": 5e-324},
    ]
    weeks = {50: SimpleNamespace(week=50, total=300, depth_count=2, fetched_at=ms_dt(1792000000999), depths=depths),
             49: SimpleNamespace(week=49, total=0, depth_count=0, fetched_at=ms_dt(1791000000000), depths=[])}
    delves.current_week_id = lambda now=None: 50

    async def get_week(wk):
        return weeks.get(wk)

    delves.get_week = get_week
    dcases = []
    for week, current_id in ((None, 50), (49, 50), (50, 50), (None, 51), (7, 50)):
        delves.current_week_id = lambda now=None, c=current_id: c
        try:
            body = dump(s.DelveRotationOut, run(r.get_delves(ctx=None, week=week)))
            dcases.append({"week": week, "current": current_id, "status": 200, "body": body})
        except APIError as e:
            dcases.append({"week": week, "current": current_id, "status": e.status_code, "message": e.message})
    out["delves"] = {"depths": depths, "cases": dcases,
                     "docs": [[w.week, w.total, w.depth_count, us_of(w.fetched_at) // 1000] for w in weeks.values()]}

    rows = [{"week": 50, "total": 300, "count": 2, "fetched_at": ms_dt(1792000000999)},
            {"week": 49, "total": 0, "count": 0, "fetched_at": ms_dt(1791000000000)}]

    async def list_weeks():
        return rows

    delves.list_weeks = list_weeks
    delves.current_week_id = lambda now=None: 51
    out["delve_weeks"] = {
        "rows": [[w["week"], w["total"], w["count"], us_of(w["fetched_at"]) // 1000] for w in rows],
        "current": 51,
        "body": dump(s.DelveWeekList, run(r.list_delve_weeks(ctx=None))),
    }
    return out


def main() -> None:
    rows, full = sweep()
    data = {
        "sweep": rows,
        "full": full,
        "luxion_runs": [list(x) for x in RUNS_MS],
        "luxion": luxion_cases(),
        "luxion_away": luxion_away(),
        "chaos": chaos_cases(),
        "image_urls": image_urls(),
        "chaos_normalize": [[p, chaos.normalize(p)] for p in CHAOS_PAYLOADS],
        "delve_normalize": [[p, delves.normalize_payload(p)] for p in DELVE_PAYLOADS],
        "classify": classify(),
        "histories": histories(),
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(data, ensure_ascii=False, separators=(",", ":")) + "\n", encoding="utf-8")
    print(f"wrote {OUT.relative_to(ROOT)}: {len(rows)} instants, {len(data['luxion'])} luxion, "
          f"{len(data['chaos'])} chaos cases")


if __name__ == "__main__":
    main()
