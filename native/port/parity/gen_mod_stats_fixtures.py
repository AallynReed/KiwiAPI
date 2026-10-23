"""Golden fixtures for the mod statistics snapshots (app/mod_stats), from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_mod_stats_fixtures.py
Writes native/kiwi-server/testdata/mod_stats/golden.json.

Runs the real build_rows / totals over fixed platform payloads (rows kept as
the extended JSON pymongo would store), and the real /site/mod-stats route
(raw ASGI) over fixed snapshot documents.
"""
import asyncio
import datetime as dt
import json
import logging
import os
import sys
import urllib.parse
import warnings
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
sys.path.insert(0, os.getcwd())
os.environ.update({"SECRET_KEY": "mod-stats-fixture-secret"})

from bson import ObjectId  # noqa: E402
from bson.json_util import CANONICAL_JSON_OPTIONS, dumps as bson_dumps  # noqa: E402
from pymongo.asynchronous.database import AsyncDatabase  # noqa: E402

import app.mod_stats.service as svc  # noqa: E402
from app.main import app  # noqa: E402
from app.mod_stats.models import ModStatsSnapshot  # noqa: E402

OUT = Path(__file__).resolve().parents[2] / "kiwi-server" / "testdata" / "mod_stats" / "golden.json"
KEPT = ("content-type", "content-length", "cache-control")


def ext(v):
    return json.loads(bson_dumps(v, json_options=CANONICAL_JSON_OPTIONS))


def bson_round(doc):
    import bson
    from bson.codec_options import CodecOptions
    return bson.decode(bson.encode(doc), codec_options=CodecOptions(tz_aware=True))


async def _cmd(self, *a, **k):
    return {"version": "7.0.0"}


async def _lcn(self, *a, **k):
    return []


AsyncDatabase.command = _cmd
AsyncDatabase.list_collection_names = _lcn


async def _init():
    from beanie import init_beanie
    from pymongo import AsyncMongoClient
    client = AsyncMongoClient("mongodb://127.0.0.1:1", connect=False, serverSelectionTimeoutMS=200, tz_aware=True)
    await init_beanie(database=client["kiwi"], skip_indexes=True, document_models=[ModStatsSnapshot])


LOOP = asyncio.new_event_loop()
LOOP.run_until_complete(_init())


async def asgi(method, url):
    path, _, qs = url.partition("?")
    scope = {
        "type": "http", "asgi": {"version": "3.0"}, "http_version": "1.1", "method": method,
        "scheme": "http", "path": urllib.parse.unquote(path), "raw_path": path.encode(),
        "query_string": qs.encode(), "root_path": "",
        "headers": [(b"host", b"testserver"), (b"x-request-id", b"req_modstats")],
        "client": ("127.0.0.1", 50000), "server": ("testserver", 80),
    }
    sent = []

    async def receive():
        return {"type": "http.request", "body": b"", "more_body": False}

    async def send(message):
        sent.append(message)

    await app(scope, receive, send)
    start = next(m for m in sent if m["type"] == "http.response.start")
    raw = b"".join(m.get("body", b"") for m in sent if m["type"] == "http.response.body")
    return start["status"], [(k.lower().decode(), v.decode("latin-1")) for k, v in start["headers"]], raw


def project(title, slug, top=0, d7=0, stars=0, handle="aallyn"):
    return {"title": title, "owner_handle": handle, "slug": slug, "top_release": top, "downloads_7d": d7,
            "star_count": stars}


PROJECTS = [
    project("Zakros UI - Chat", "chat", 1200, 30, 5),
    project("zakros FX - Glow", "glow", 3_000_000_000, 0, 1),
    project("Zakros UI - Clubs", "clubs", 7, 2, 0),
    project("Zakros BP - Ships", "ships"),
    project("Zakros UI - Nameplate", "nameplate", 50, 1, 2),
    project("Zakros Σ Final", "sigma", 1),
    project("Zakros İ Dot", "idot", 2),
]
TS_IDS = {"Zakros UI - Chat": 11, "Zakros UI - Clubs": "12", "Zakros UI - Nameplate": 13, "zakros FX - Glow": 14,
          "Zakros İ Dot": 99}
STEAM_IDS = {"Zakros UI - Chat": 2001, "Zakros UI - Clubs": "2002", "Zakros BP - Ships": 2003, "Zakros Σ Final": 2004}
CATALOG = [
    {"id": 11, "totaldownloads": "1,234", "likes": 7},
    {"id": "12", "totaldownloads": " 450 ", "likes": None},
    {"id": 13, "totaldownloads": 12.9, "likes": True},
    {"id": 14, "totaldownloads": "1_000", "likes": "x"},
    "not a dict",
    {"id": 11, "totaldownloads": 99, "likes": 1},
    {},
]
DETAILS = [
    {"publishedfileid": "2001", "result": 1, "subscriptions": 10, "lifetime_subscriptions": 25, "favorited": 3,
     "views": 400},
    {"publishedfileid": "2002", "result": 9},
    {"publishedfileid": "2003", "result": True, "subscriptions": "5", "lifetime_subscriptions": 5.5, "favorited": 0,
     "views": None},
    {"publishedfileid": 2004, "result": 1.0, "subscriptions": 4_000_000_000},
]


def rows_cases():
    out = []
    variants = {
        "full": (PROJECTS, TS_IDS, STEAM_IDS, CATALOG, DETAILS),
        "no_platforms": (PROJECTS[:3], {}, {}, [], []),
        "empty_catalog_entry_matches_missing_id": (PROJECTS[3:4], {}, {}, [{}], []),
        "none": ([], TS_IDS, STEAM_IDS, CATALOG, DETAILS),
    }
    for name, (projects, ts, st, cat, det) in variants.items():
        rows = svc.build_rows(projects, ts, st, cat, det)
        out.append({"name": name, "projects": projects, "trovesaurus_ids": ts, "steam_ids": st, "catalog": cat,
                    "details": det, "stored": ext(bson_round({"mods": rows}))["mods"],
                    "totals": json.dumps(svc.totals(rows), separators=(",", ":"))})
    return out


def snap(day, taken, mods):
    return {"_id": ObjectId(), "day": day, "taken_at": taken, "mods": mods}


T = dt.datetime(2026, 10, 22, 11, 5, 7, 123456, tzinfo=dt.timezone.utc)


def route_cases():
    full = svc.build_rows(PROJECTS, TS_IDS, STEAM_IDS, CATALOG, DETAILS)
    sets = {
        "empty": [],
        "one": [snap("2026-10-22", T, full)],
        "history": [snap("2026-10-22", T, full), snap("2026-10-21", T - dt.timedelta(days=1), full[:2]),
                    snap("2026-10-19", T - dt.timedelta(days=3), [])],
        "old_shapes": [snap("2026-10-20", T, [{"title": "Old", "hub": {"top_release": 3.5}, "trovesaurus": {},
                                               "steam": None, "extra": [1, 2]},
                                              {"title": "Old2", "hub": {"top_release": 2, "downloads_7d": 1},
                                               "trovesaurus": {"downloads": 0.1}, "steam": {"subscriptions": 0.2}}])],
        "floats_sum": [snap("2026-10-20", T, [{"hub": {}, "trovesaurus": {"downloads": 0.1}, "steam": None}] * 10)],
        "mixed_sum": [snap("2026-10-20", T, [
            {"hub": {"top_release": 1e16}, "trovesaurus": {"downloads": 1}, "steam": {"subscriptions": 0.1}},
            {"hub": {"top_release": 1}, "trovesaurus": {"downloads": 1e16}, "steam": {"subscriptions": 0.2}},
            {"hub": {"top_release": 1}, "trovesaurus": {"downloads": 1}, "steam": {"subscriptions": 7}},
            {"hub": {"top_release": 0.3}, "trovesaurus": {"downloads": True}, "steam": {"subscriptions": 0.3}},
        ])],
    }
    out = []
    for name, docs in sets.items():
        docs = [bson_round(d) for d in docs]
        snaps = [ModStatsSnapshot.model_validate(d) for d in sorted(docs, key=lambda d: d["day"], reverse=True)]

        class Q:
            def sort(self, *a):
                return self

            def limit(self, *a):
                return self

            async def to_list(self):
                return snaps

        ModStatsSnapshot.find_all = classmethod(lambda cls, *a, **k: Q())
        status, rh, raw = LOOP.run_until_complete(asgi("GET", "/site/mod-stats"))
        out.append({"name": name, "docs": ext(docs), "status": status,
                    "resp_headers": [[k, v] for k, v in rh if k in KEPT], "resp": raw.decode()})
    return out


def main():
    data = {"rows": rows_cases(), "route": route_cases()}
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(data, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
