"""Golden fixtures for kiwi-server's market routes, analytics and tome values.

Pure functions (sort tokens, the dump parser, UUID timestamps, the outlier
filter, merchant bands, category helpers, the codex role rank) are called
directly. Route cases drive ``app.main.app`` at the ASGI level with asyncpg's
``acquire()`` replaced by a fake connection that records every call (method,
SQL text, arguments) and answers from canned rows, so the Rust test replays the
same rows and checks it sent the same SQL. Mongo reads (categories, interest
list) and the clock are patched; auth is overridden where a case says so.

Run from the repo root with the production-pinned venv:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_market_fixtures.py
"""

from __future__ import annotations

import asyncio
import contextlib
import datetime as dt
import decimal
import json
import logging
import os
import random
import sys
import urllib.parse
import uuid
import warnings
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "market" / "golden.json"
os.chdir(ROOT)
sys.path.insert(0, str(ROOT))

from bson import ObjectId  # noqa: E402

from app.admin import runtime_config as rc  # noqa: E402

for _key, _spec in rc.REGISTRY.items():
    rc._CACHE[_key] = (_spec.default, float("inf"))

import app.core.dependencies as deps  # noqa: E402
import app.trove.codexes.pg_store as cpg  # noqa: E402
import app.trove.market.pg_store as mpg  # noqa: E402
import app.trove.market.service as msvc  # noqa: E402
import app.trove.server_time as st  # noqa: E402
from app.core.config import settings  # noqa: E402
from app.core.ratelimit import RateLimitInfo  # noqa: E402
from app.core.utils import to_oid  # noqa: E402
from app.trove import router as trove_router  # noqa: E402
from app.trove.market import parser  # noqa: E402
from app.trove.market.models import MarketItemCategory  # noqa: E402

NOW = 1_792_000_123
NOW_DT = dt.datetime.fromtimestamp(NOW, dt.timezone.utc)
DSN = "postgresql://fixture/none"
settings.postgres_dsn = DSN

# --- fake asyncpg ----------------------------------------------------------------------

CALLS: list[dict] = []
REPLIES: list = []


def to_py(v):
    if isinstance(v, dict) and set(v) == {"$numeric"}:
        return decimal.Decimal(v["$numeric"])
    if isinstance(v, dict) and set(v) == {"$uuid"}:
        return uuid.UUID(v["$uuid"])
    return v


def record(row: dict) -> dict:
    return {k: to_py(v) for k, v in row.items()}


def arg_json(a):
    return sorted(a) if isinstance(a, list) else a


class FakeConn:
    async def _call(self, method, sql, args):
        CALLS.append({"method": method, "sql": sql, "args": [arg_json(a) for a in args]})
        if not REPLIES:
            raise RuntimeError(f"no canned reply for {sql[:40]}")
        reply = REPLIES.pop(0)
        if isinstance(reply, dict) and "$error" in reply:
            raise RuntimeError(reply["$error"])
        return reply

    async def fetch(self, sql, *args):
        return [record(r) for r in await self._call("fetch", sql, args)]

    async def fetchrow(self, sql, *args):
        r = await self._call("fetchrow", sql, args)
        return None if r is None else record(r)

    async def fetchval(self, sql, *args):
        return to_py(await self._call("fetchval", sql, args))

    async def execute(self, sql, *args):
        await self._call("execute", sql, args)
        return "OK"


@contextlib.asynccontextmanager
async def fake_acquire():
    yield FakeConn()


mpg.acquire = fake_acquire
cpg.acquire = fake_acquire
msvc._now = lambda: NOW
_REAL_SERVER_TIME = st.server_time
st.server_time = lambda now=None: _REAL_SERVER_TIME(now or NOW_DT)

STORE = {"interest": [], "categories": []}


async def fake_interest_set():
    return frozenset(STORE["interest"])


async def fake_list_categories():
    docs = [MarketItemCategory.model_construct(
        id=ObjectId(c["id"]), name=c["name"], order=c["order"], items=c["items"], created_by=None,
        created_at=NOW_DT) for c in STORE["categories"]]
    return msvc._sorted_categories(docs)


msvc._interest_items_set = fake_interest_set
msvc.list_categories = fake_list_categories

from app.main import app  # noqa: E402

# --- data ------------------------------------------------------------------------------

RNG = random.Random(20261022)
U = lambda s: {"$uuid": s}  # noqa: E731
N = lambda s: {"$numeric": str(s)}  # noqa: E731


def listing(i, name, stack, price, created_off, seen_off, kind="Item"):
    return {"id": U(str(uuid.UUID(int=(0x1f0a7c00_0000_1000_8000_000000000000 + i * 7919)))),
            "name": name, "type": kind, "stack": stack, "price": price,
            "price_each": round(price / stack, 3), "last_seen": NOW - seen_off, "created_at": NOW - created_off}


LISTINGS = [
    listing(1, "Flux Dust", 10, 12000, 3600, 120),
    listing(2, "Eye of Q'bthulhu", 3, 1000, 86400 * 2, 7200, kind=None),
    listing(3, "Glim \u2713", 7, 1, 86400 * 8, 4 * 3600, kind=""),
    listing(4, "Dragon Egg", 1, 50000000, 100, 11000),
    listing(5, "Tiny", 9999, 1, 10, 10),
]
LISTINGS[2]["price_each"] = 0.143
LISTINGS[4]["price_each"] = 1e-05


def history_rows(prices, created0=NOW - 86400 * 3):
    return [{"created_at": created0 + 600 * i, "price_each": p, "last_seen": NOW - 60, "stack": 1 + i % 3,
             "price": int(p * (1 + i % 3))} for i, p in enumerate(prices)]


CLUSTER = [12000.0, 12500.0, 11800.0, 12100.0, 13000.0, 11900.0, 12050.0]
HISTORY_OUTLIERS = history_rows(CLUSTER + [50000000.0, 1.0, 12000.0, 9000000.0])
HISTORY_FEW = history_rows([1.0, 50000000.0, 3.0, 4.0])
HISTORY_FLAT = history_rows([5.5] * 8)
HISTORY_SPREAD = history_rows([1.0, 10.0, 100.0, 1000.0, 10000.0, 100000.0])

CODEX_ROWS = [
    {"lname": "flux dust", "blueprint": "item/flux_dust", "path": "prefabs/item/crafting/flux_dust"},
    {"lname": "flux dust", "blueprint": "item/flux_dust_debug", "path": "prefabs/item/crafting/flux_dust_debug"},
    {"lname": "dragon egg", "blueprint": "a", "path": "prefabs/collections/egg"},
    {"lname": "dragon egg", "blueprint": "b", "path": "prefabs/collections/egg2"},
    {"lname": "dragon egg", "blueprint": "c", "path": "prefabs/item/mount/egg"},
    {"lname": "tiny", "blueprint": "x", "path": "prefabs/collections/tiny"},
    {"lname": "tiny", "blueprint": "y", "path": "prefabs/collections/tiny2"},
    {"lname": "glim \u2713", "blueprint": "glim", "path": None},
]

CATEGORIES = [
    {"id": "65a000000000000000000c01", "name": "materials", "order": 1, "items": ["Glim \u2713", "Flux Dust", "Zeta", "glim x", "Flux Dust"]},
    {"id": "65a000000000000000000c02", "name": "Allies", "order": 0, "items": []},
    {"id": "65a000000000000000000c03", "name": "Mounts", "order": 1, "items": ["Dragon Egg", "abc", "\u00c9cu"]},
]
INTEREST = ["Flux Dust", "Dragon Egg", "Glim \u2713", "Unseen"]


def day_series(rng: random.Random, names, days, shocks):
    """Daily rows (ORDER BY name, bucket) with optional (name, field, factor) shocks on the last day."""
    floor = (NOW // 86400) * 86400
    rows = []
    for name in names:
        base = rng.choice([50.0, 1200.0, 33333.3, 0.75])
        lst = rng.randint(3, 40)
        stack = rng.choice([1.0, 5.0, 10.0, 99.0])
        n = days if name != "short" else 5
        for d in range(n):
            bucket = floor - (n - d) * 86400
            p50 = round(base * rng.uniform(0.85, 1.15), 3)
            listings = max(1, int(lst * rng.uniform(0.7, 1.3)))
            smed = stack * rng.choice([1.0, 1.0, 1.5])
            if d == n - 1:
                for sname, field, factor in shocks:
                    if sname == name:
                        if field == "p50":
                            p50 = round(p50 * factor, 3)
                        elif field == "listings":
                            listings = int(listings * factor)
                        else:
                            smed = smed * factor
            rows.append({"name": name, "bucket": bucket, "listings": listings, "stack": listings * int(smed),
                         "p50": p50, "stack_med": smed, "stack_max": int(smed * 2)})
    return rows


def signals_sets():
    rng = random.Random(7)
    names = ["Alpha", "Beta", "Gamma", "Delta", "Epsilon", "Zeta", "Eta", "Theta", "short", "Iota"]
    sets = []
    sets.append(day_series(rng, names, 14, [
        ("Alpha", "p50", 0.2), ("Alpha", "listings", 6.0), ("Alpha", "stack", 5.0),
        ("Beta", "p50", 0.3), ("Beta", "listings", 5.0),
        ("Gamma", "p50", 3.0), ("Gamma", "listings", 0.2),
        ("Delta", "stack", 8.0), ("Epsilon", "p50", 4.0), ("Zeta", "p50", 0.1),
        ("Eta", "listings", 9.0), ("Theta", "listings", 0.0)]))
    sets.append(day_series(rng, names, 10, [(n, "p50", 1.6) for n in names]))
    sets.append(day_series(rng, names, 10, [(n, "p50", 0.5) for n in names]))
    sets.append(day_series(rng, names[:3], 4, []))
    sets.append([])
    for seed in range(6):
        r2 = random.Random(100 + seed)
        shocks = [(r2.choice(names), r2.choice(["p50", "listings", "stack"]), r2.choice([0.05, 0.4, 2.5, 12.0]))
                  for _ in range(6)]
        sets.append(day_series(r2, names, r2.randint(7, 20), shocks))
    return sets


# --- route cases -----------------------------------------------------------------------

J = {"content-type": "application/json"}
A = {"x-test-auth": "1"}
AJ = {**A, **J}


def jb(v) -> bytes:
    return json.dumps(v).encode()


def count_then(rows, total=None):
    return [len(rows) if total is None else total, rows]


SUMMARY_ROW = {"count": 4, "total_price": N(123456), "total_stack": 40, "min_each": 0.5, "max_each": 2500.0,
               "avg_each": 1234.56789, "median_each": 1000.0}
MOVER = {"name": "Flux Dust", "recent_med": 12000.0, "prior_med": 10000.0, "recent_n": 12, "change": 0.2}
MOVER_DOWN = {"name": "Glim", "recent_med": 1.5, "prior_med": 3.0, "recent_n": 7, "change": -0.5}
VOL = {"name": "Flux Dust", "listings": 40, "units": 400, "total_value": N(4800000), "median_each": 12000.0}
VOL_NULL = {"name": "Odd", "listings": 3, "units": 0, "total_value": N(0), "median_each": None}
LIQ = [
    {"name": "Flux Dust", "concluded": 10, "sold": 7, "expired": 3, "median_tts": 12345.75},
    {"name": "Glim", "concluded": 6, "sold": 0, "expired": 6, "median_tts": None},
    {"name": "Odd", "concluded": 3, "sold": 1, "expired": 2, "median_tts": 1e-05},
]
DEALS = [
    {"id": U("00000000-0000-1000-8000-00000000abcd"), "name": "Flux Dust", "stack": 10, "price": 60000,
     "price_each": 6000.0, "created_at": NOW - 500, "last_seen": NOW - 30, "median_each": 12000.0,
     "sample_size": 9, "discount": 0.5},
    {"id": U("11111111-2222-1333-8444-555555555555"), "name": "Glim", "stack": 3, "price": 2,
     "price_each": 0.667, "created_at": NOW - 900, "last_seen": NOW - 60, "median_each": 1.0,
     "sample_size": 5, "discount": 0.33299999999999996},
]
TIMELINE = [
    {"bucket": (NOW // 86400 - 2) * 86400, "listings": 5, "stack": 50, "p50": 100.0, "p25": 90.5, "p75": 110.25},
    {"bucket": (NOW // 86400 - 1) * 86400, "listings": 2, "stack": None, "p50": 1e-05, "p25": 1e-05, "p75": 3.0},
]
MEDIANS = [
    {"name": "Shapestone Ore", "count": 12, "median_each": 15.5},
    {"name": "Formicite Ore", "count": 3, "median_each": 99.0},
    {"name": "Fulgurite Samplebook", "count": 2, "median_each": 3333.33},
    {"name": "Glim", "count": 40, "median_each": 1.0},
    {"name": "Infinium Ore", "count": 1, "median_each": None},
]
TOME_CODEX = [
    {"lname": "tome: shapestone survey", "blueprint": "item/tome_shapestone", "path": "prefabs/item/tome/x"},
    {"lname": "shapestone survey", "blueprint": "item/shapestone_ore", "path": "prefabs/item/x"},
    {"lname": "glim grimoire", "blueprint": "item/TOME_glim", "path": "prefabs/item/tome/glim"},
    {"lname": "legendary tome: nekonomicon", "blueprint": "item/tome_neko", "path": "prefabs/item/tome/n"},
    {"lname": "tome: flux folio", "blueprint": "item/no_match_here", "path": "prefabs/item/f"},
]

ERR = {"$error": "boom"}

# (name, method, url, headers, body, flags). flags: pg (replies list, or False = Postgres off),
# store (categories/interest), real (Rust uses the production router), disabled.
CASES = [
    ("site_listings", "GET", "/site/market/listings", {}, None, {"pg": count_then(LISTINGS, 17)}),
    ("site_listings_filters", "GET",
     "/site/market/listings?name=Flux%20Dust&price_min=0.5&price_max=10&last_seen_after=1792000000&sort=%2Bprice_each&limit=2&offset=1",
     {}, None, {"pg": count_then(LISTINGS[:2])}),
    ("site_listings_after_old", "GET", "/site/market/listings?last_seen_after=5&sort=-name", {}, None,
     {"pg": count_then([])}),
    ("site_listings_archive", "GET", "/site/market/listings?hide_expired=false&sort=stack", {}, None,
     {"pg": count_then(LISTINGS[3:])}),
    ("site_listings_archive_after", "GET", "/site/market/listings?hide_expired=0&last_seen_after=7&price_min=3", {},
     None, {"pg": count_then(LISTINGS[:1])}),
    ("site_listings_sort_space", "GET", "/site/market/listings?sort=%20%2B-stack%20&hide_expired=no", {}, None,
     {"pg": [None, []]}),
    ("site_listings_sort_bogus", "GET", "/site/market/listings?sort=-+drop%20table&limit=500", {}, None,
     {"pg": count_then([])}),
    ("site_listings_sort_empty", "GET", "/site/market/listings?sort=&price_max=1e3", {}, None,
     {"pg": count_then([])}),
    ("site_listings_int_price", "GET", "/site/market/listings?price_min=5&price_max=5", {}, None,
     {"pg": count_then([])}),
    ("site_listings_range", "GET", "/site/market/listings?price_min=10&price_max=2", {}, None, {}),
    ("site_listings_invalid", "GET",
     "/site/market/listings?price_min=-1&price_max=x&last_seen_after=1.5&hide_expired=maybe&limit=0&offset=-1",
     {}, None, {}),
    ("site_listings_limit_501", "GET", "/site/market/listings?limit=501", {}, None, {}),
    ("site_listings_pg_error", "GET", "/site/market/listings", {}, None, {"pg": [ERR]}),
    ("v1_listings", "GET", "/v1/market/listings?limit=1000", A, None, {"pg": count_then(LISTINGS)}),
    ("v1_listings_1001", "GET", "/v1/market/listings?limit=1001&offset=x", A, None, {}),
    ("v1_listings_range", "GET", "/v1/market/listings?price_min=3&price_max=1", A, None, {}),
    ("v1_listings_archive", "GET", "/v1/market/listings?hide_expired=false&name=Tiny", A, None,
     {"pg": count_then(LISTINGS[4:])}),
    ("v1_items", "GET", "/v1/market/items", A, None, {"pg": [[{"name": "Dragon Egg"}, {"name": "Flux Dust"}]]}),
    ("v1_summary", "GET", "/v1/market/items/Flux%20Dust/summary", A, None, {"pg": [SUMMARY_ROW]}),
    ("v1_summary_missing", "GET", "/v1/market/items/Nope/summary", A, None, {"pg": [None]}),
    ("v1_summary_zero", "GET", "/v1/market/items/%E2%9C%93%20x/summary", A, None,
     {"pg": [{"count": 0, "total_price": None, "total_stack": None, "min_each": None, "max_each": None,
              "avg_each": None, "median_each": None}]}),
    ("v1_reset", "POST", "/v1/market/reset", A, None, {"pg": [3_000_000_000, None]}),
    ("site_summary", "GET", "/site/market/items/Eye%20of%20Q'bthulhu/summary", {}, None,
     {"pg": [{**SUMMARY_ROW, "avg_each": None, "median_each": None, "total_price": None}]}),
    ("site_summary_missing", "GET", "/site/market/items/%22quoted%22/summary", {}, None, {"pg": [None]}),
    ("site_items", "GET", "/site/market/items", {}, None,
     {"pg": [[{"name": n} for n in ["Dragon Egg", "Flux Dust", "Glim \u2713", "Old Thing"]]], "store": True}),
    ("site_items_empty", "GET", "/site/market/items", {}, None, {"pg": [[]], "store": True}),
    ("site_item_images", "GET", "/site/market/item-images", {}, None,
     {"pg": [[{"name": n} for n in ["Dragon Egg", "Flux Dust", "Glim \u2713", "Tiny", "Zed"]], CODEX_ROWS]}),
    ("site_item_images_none", "GET", "/site/market/item-images", {}, None, {"pg": [[]]}),
    ("site_item_images_error", "GET", "/site/market/item-images", {}, None, {"pg": [ERR]}),
    ("site_item_images_codex_error", "GET", "/site/market/item-images", {}, None,
     {"pg": [[{"name": "Tiny"}], ERR]}),
    ("site_history", "GET", "/site/market/items/Flux%20Dust/history", {}, None, {"pg": [HISTORY_OUTLIERS]}),
    ("site_history_keep", "GET", "/site/market/items/Flux%20Dust/history?keep_outliers=true&days=30", {}, None,
     {"pg": [HISTORY_OUTLIERS]}),
    ("site_history_expired", "GET", "/site/market/items/Flux%20Dust/history?include_expired=1&days=2", {}, None,
     {"pg": [HISTORY_SPREAD]}),
    ("site_history_few", "GET", "/site/market/items/x/history?days=1", {}, None, {"pg": [HISTORY_FEW]}),
    ("site_history_flat", "GET", "/site/market/items/x/history", {}, None, {"pg": [HISTORY_FLAT]}),
    ("site_history_empty", "GET", "/site/market/items/x/history", {}, None, {"pg": [[]]}),
    ("site_history_invalid", "GET", "/site/market/items/x/history?days=31&include_expired=2", {}, None, {}),
    ("timeline", "GET", "/site/market/analytics/timeline?name=Flux%20Dust", {}, None, {"pg": [TIMELINE]}),
    ("timeline_args", "GET", "/site/market/analytics/timeline?name=x&days=90&bucket_hours=168", {}, None,
     {"pg": [[]]}),
    ("timeline_missing", "GET", "/site/market/analytics/timeline", {}, None, {}),
    ("timeline_invalid", "GET", "/site/market/analytics/timeline?name=&days=91&bucket_hours=0", {}, None, {}),
    ("timeline_nopg", "GET", "/site/market/analytics/timeline?name=x&days=60", {}, None, {"pg": False}),
    ("deals", "GET", "/site/market/analytics/deals", {}, None, {"pg": [DEALS]}),
    ("deals_args", "GET", "/site/market/analytics/deals?days=30&min_discount=0.3&limit=10", {}, None,
     {"pg": [DEALS[:1]]}),
    ("deals_days1", "GET", "/site/market/analytics/deals?days=1&min_discount=0.95", {}, None, {"pg": [[]]}),
    ("deals_invalid", "GET", "/site/market/analytics/deals?min_discount=0.04&limit=201&days=0", {}, None, {}),
    ("deals_nopg", "GET", "/site/market/analytics/deals?min_discount=5e-1", {}, None, {"pg": False}),
    ("movers", "GET", "/site/market/analytics/movers", {}, None, {"pg": [[MOVER], [MOVER_DOWN]]}),
    ("movers_limit1", "GET", "/site/market/analytics/movers?limit=1&days=30", {}, None, {"pg": [[], []]}),
    ("movers_limit41", "GET", "/site/market/analytics/movers?limit=41", {}, None, {"pg": [[MOVER], []]}),
    ("movers_invalid", "GET", "/site/market/analytics/movers?limit=101", {}, None, {}),
    ("movers_nopg", "GET", "/site/market/analytics/movers", {}, None, {"pg": False}),
    ("overview", "GET", "/site/market/analytics/overview?days=3", {}, None,
     {"pg": [{"listings": 1234, "items": 56, "total_value": N(98765432100), "total_units": 7890},
             [{**MOVER, "change": -0.75}], [VOL]]}),
    ("overview_empty", "GET", "/site/market/analytics/overview", {}, None,
     {"pg": [{"listings": 0, "items": 0, "total_value": N(0), "total_units": 0}, [], []]}),
    ("overview_nopg", "GET", "/site/market/analytics/overview", {}, None, {"pg": False}),
    ("overview_invalid", "GET", "/site/market/analytics/overview?days=31", {}, None, {}),
    ("liquidity", "GET", "/site/market/analytics/liquidity?days=30&limit=100", {}, None, {"pg": [LIQ]}),
    ("liquidity_nopg", "GET", "/site/market/analytics/liquidity", {}, None, {"pg": False}),
    ("volume", "GET", "/site/market/analytics/volume?days=2", {}, None, {"pg": [[VOL, VOL_NULL]]}),
    ("volume_nopg", "GET", "/site/market/analytics/volume", {}, None, {"pg": False}),
    ("volume_invalid", "GET", "/site/market/analytics/volume?days=abc", {}, None, {}),
    ("signals_nopg", "GET", "/site/market/analytics/signals", {}, None, {"pg": False}),
    ("signals_invalid", "GET", "/site/market/analytics/signals?days=9", {}, None, {}),
    ("signals_invalid_high", "GET", "/site/market/analytics/signals?days=61", {}, None, {}),
    ("tomes", "GET", "/site/tomes", {}, None, {"pg": [MEDIANS, TOME_CODEX]}),
    ("tomes_nothing", "GET", "/site/tomes", {}, None, {"pg": [[], []]}),
    ("tomes_codex_error", "GET", "/site/tomes", {}, None, {"pg": [MEDIANS[:2], ERR]}),
    ("tomes_nopg", "GET", "/site/tomes", {}, None, {"pg": False}),
    ("site_disabled", "GET", "/site/market/items", {}, None, {"disabled": "feature_market_enabled"}),
    ("v1_disabled", "GET", "/v1/market/items", {}, None, {"disabled": "feature_market_enabled"}),
    ("tomes_disabled", "GET", "/site/tomes", {}, None, {"disabled": "feature_tomes_enabled"}),
    # Store-free answers from the production router.
    ("v1_noauth", "GET", "/v1/market/listings?limit=0", {}, None, {"real": True}),
    ("v1_items_noauth", "GET", "/v1/market/items", {"authorization": "Basic x"}, None, {"real": True}),
    ("v1_summary_noauth", "GET", "/v1/market/items/x/summary", {}, None, {"real": True}),
    ("v1_reset_noauth", "POST", "/v1/market/reset", {}, None, {"real": True}),
    ("v1_reset_get_405", "GET", "/v1/market/reset", {}, None, {"real": True}),
    ("v1_listings_post_405", "POST", "/v1/market/listings", {}, None, {"real": True}),
    ("site_items_post_405", "POST", "/site/market/items", {}, None, {"real": True}),
    ("site_tomes_put_405", "PUT", "/site/tomes", {}, None, {"real": True}),
    ("site_history_post_405", "POST", "/site/market/items/x/history", {}, None, {"real": True}),
    ("admin_list_noauth", "GET", "/admin/market/interest-items", {}, None, {"real": True}),
    ("admin_cat_noauth", "GET", "/admin/market/categories", {"authorization": "Bearer not.a.jwt"}, None,
     {"real": True}),
    ("admin_interest_delete_405", "DELETE", "/admin/market/interest-items", {}, None, {"real": True}),
    ("admin_interest_patch_405", "PATCH", "/admin/market/interest-items", {}, None, {"real": True}),
    ("admin_interest_name_405", "GET", "/admin/market/interest-items/x", {}, None, {"real": True}),
    ("admin_cat_patch_405", "PATCH", "/admin/market/categories", {}, None, {"real": True}),
    ("admin_cat_id_405", "GET", "/admin/market/categories/65a000000000000000000c01", {}, None, {"real": True}),
    ("admin_cat_order_405", "GET", "/admin/market/categories/order", {}, None, {"real": True}),
    ("admin_cat_order_post_405", "POST", "/admin/market/categories/order", {}, None, {"real": True}),
    ("admin_add_badjson_noauth", "POST", "/admin/market/interest-items", J, b'{"name": ', {"real": True}),
    ("admin_add_noauth", "POST", "/admin/market/interest-items", J, jb({"name": 5}), {"real": True}),
    ("admin_patch_noauth", "PATCH", "/admin/market/categories/order", J, jb({}), {"real": True}),
    ("admin_delete_noauth", "DELETE", "/admin/market/interest-items/Flux%20Dust", {}, None, {"real": True}),
    # Admin answers that never reach Mongo (superuser overridden).
    ("admin_add_blank", "POST", "/admin/market/interest-items", AJ, jb({"name": " \t\u3000"}), {}),
    ("admin_add_invalid", "POST", "/admin/market/interest-items", AJ, jb({"name": 5}), {}),
    ("admin_add_missing", "POST", "/admin/market/interest-items", A, None, {}),
    ("admin_add_list", "POST", "/admin/market/interest-items", AJ, jb([]), {}),
    ("admin_replace_empty", "PUT", "/admin/market/interest-items", AJ, jb({"items": ["", "  ", "\n"]}), {}),
    ("admin_replace_invalid", "PUT", "/admin/market/interest-items", AJ, jb({"items": ["a", 2, None]}), {}),
    ("admin_replace_names_key", "PUT", "/admin/market/interest-items", AJ, jb({"names": ["a"]}), {}),
    ("admin_cat_blank", "POST", "/admin/market/categories", AJ, jb({"name": "   "}), {}),
    ("admin_cat_invalid", "POST", "/admin/market/categories", AJ, jb({"name": None}), {}),
    ("admin_cat_patch_bad_id", "PATCH", "/admin/market/categories/not-an-id", AJ, jb({"name": "x"}), {}),
    ("admin_cat_patch_order", "PATCH", "/admin/market/categories/order", AJ, jb({}), {}),
    ("admin_cat_patch_invalid", "PATCH", "/admin/market/categories/zz", AJ, jb({"name": 1, "items": [1]}), {}),
    ("admin_cat_patch_short_id", "PATCH", "/admin/market/categories/65a00000000000000000000", AJ,
     jb({"items": None}), {}),
    ("admin_cat_delete_bad_id", "DELETE", "/admin/market/categories/zzz", A, None, {}),
    ("admin_cat_delete_order", "DELETE", "/admin/market/categories/order", A, None, {}),
    ("admin_cat_reorder_invalid", "PUT", "/admin/market/categories/order", AJ, jb({"ids": 5}), {}),
    ("admin_cat_reorder_missing", "PUT", "/admin/market/categories/order", AJ, jb({}), {}),
]

for i, rows in enumerate(signals_sets()):
    CASES.append((f"signals_{i}", "GET", f"/site/market/analytics/signals?days={10 + i * 4}", {}, None,
                  {"pg": [rows]}))


async def call(method: str, url: str, headers: list[tuple[bytes, bytes]], body: bytes):
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


class FakeUser:
    id = ObjectId("65a1b2c3d4e5f60718293a4b")
    email = "master@example.test"
    is_superuser = True


ARCHIVE = RateLimitInfo(limit=10, remaining=9, reset=dt.datetime(2026, 10, 22, 12, 0, 0, tzinfo=dt.timezone.utc))


async def fake_check_rate_limit(key, max_requests, window_seconds):
    return ARCHIVE


async def no_log(**_k):
    return None


KEPT = ("content-type", "cache-control", "allow", "www-authenticate", "x-ratelimit-archive-limit",
        "x-ratelimit-archive-remaining", "x-ratelimit-archive-reset", "x-ratelimit-limit")


async def http_section() -> list:
    from app.admin import ingest_log

    trove_router.check_rate_limit = fake_check_rate_limit
    ingest_log.record = no_log
    out = []
    for i, (name, method, url, headers, body, flags) in enumerate(CASES):
        app.dependency_overrides.clear()
        if "x-test-auth" in headers:
            app.dependency_overrides[trove_router._MKT.dependency] = (
                lambda: deps.TokenContext(user=FakeUser(), token=type("T", (), {"id": ObjectId(), "name": "t"})()))
            app.dependency_overrides[deps.require_master_ingest] = (
                lambda: deps.IngestAuth(user=FakeUser(), token=None))
            app.dependency_overrides[deps.get_current_superuser] = lambda: FakeUser()
        pg = flags.get("pg", [])
        settings.postgres_dsn = None if pg is False else DSN
        REPLIES[:] = [] if pg is False else json.loads(json.dumps(pg))
        CALLS.clear()
        STORE["interest"] = INTEREST if flags.get("store") else []
        STORE["categories"] = CATEGORIES if flags.get("store") else []
        flag = flags.get("disabled")
        saved = rc._CACHE.get(flag) if flag else None
        if flag:
            rc._CACHE[flag] = (False, float("inf"))
        rid = f"parity-market-{i:03d}"
        sent = [(k.encode(), v.encode()) for k, v in headers.items() if k != "x-test-auth"]
        sent.append((b"x-request-id", rid.encode()))
        status, rh, raw = await call(method, url, sent, body or b"")
        if flag:
            rc._CACHE[flag] = saved
        assert not REPLIES, (name, REPLIES)
        get = lambda k: next((v for n, v in rh if n == k), None)  # noqa: E731
        out.append({
            "name": name, "method": method, "url": url, "headers": headers,
            "body": None if body is None else body.decode("utf-8"),
            "real": bool(flags.get("real")), "disabled": flags.get("disabled"),
            "pg_enabled": pg is not False, "replies": [] if pg is False else pg,
            "store": bool(flags.get("store")), "request_id": rid,
            "calls": list(CALLS), "status": status,
            "resp_headers": {k: get(k) for k in KEPT if get(k) is not None},
            "response": raw.decode("utf-8"),
        })
    app.dependency_overrides.clear()
    settings.postgres_dsn = DSN
    return out


# --- pure sections ------------------------------------------------------------------------

SORTS = ["-last_seen", "+price_each", "price", "-stack", "--name", "+-created_at", "-+price", "bogus", "",
         "   ", " -price ", "\u3000+stack\u3000", "-PRICE", "+", "-", "name;drop", "last_seen ", "\u00a0-name"]


def uuid1_at(ticks: int, clock=0x2a3b, node=0x0242ac120002) -> uuid.UUID:
    low = ticks & 0xFFFFFFFF
    mid = (ticks >> 32) & 0xFFFF
    hi = ((ticks >> 48) & 0x0FFF) | (1 << 12)
    return uuid.UUID(fields=(low, mid, hi, (clock >> 8) | 0x80, clock & 0xFF, node))


def created_section() -> list:
    out = []
    base = 0x01B21DD213814000
    for _ in range(300):
        t = base + RNG.randint(1_600_000_000, 1_900_000_000) * 10_000_000 + RNG.randint(0, 9_999_999)
        u = uuid1_at(t)
        out.append([str(u), parser.listing_created_at(u)])
    for s in (1_792_000_000, 1_700_000_001, 1_234_567_890):
        for frac in (9_999_990, 9_999_995, 9_999_999, 0, 5, 1):
            for extra in range(0, 10):
                t = base + s * 10_000_000 + frac - (frac % 10) + extra
                u = uuid1_at(t)
                out.append([str(u), parser.listing_created_at(u)])
    for t in (0, 1, 2**53, 2**54 + 7, 2**60 - 1, base, base + 9):
        u = uuid1_at(t)
        out.append([str(u), parser.listing_created_at(u)])
    for _ in range(50):
        u = uuid.UUID(int=RNG.getrandbits(128))
        out.append([str(u), parser.listing_created_at(u)])
    return out


def dump_text() -> list[str]:
    u = [str(uuid1_at(0x01B21DD213814000 + RNG.randint(1_700_000_000, 1_800_000_000) * 10_000_000
                      + RNG.randint(0, 9_999_999))) for _ in range(12)]
    good = "".join(f"{x};Item {i};Type;{i + 1};{(i + 1) * 1000}," for i, x in enumerate(u[:5]))
    return [
        good,
        f"{u[0]};  Flux Dust \u00a0;  ;10;12000,\r\n{u[0]};Flux Dust;;1;1,{u[1].upper()};Glim;Mat;3;10,",
        f"{u[2]};Zero;;0;100,{u[3]};Free;;5;0,{u[4]};Pricey;;1;50000001,{u[5]};Cap;;1;50000000,",
        f"{u[6]};Long;;1;1234567890,{u[7]};Stack5;;12345;10,",
        f"{u[8][:8]}-{u[8][9:]}-x;Bad;;1;1,{u[9].replace('-', '')}----;Moved;;1;7,",
        f"--{u[10].replace('-', '')}--;Dashes;;2;9,",
        f"{u[11]};Arabic;;\u0663;\u0661\u0662\u0663,{u[0][:-1]}f;Mixed\u0660;;1\u0660;2\u0669\u0669,",
        f"{u[1]};Fullwidth;;\uff12;\uff15\uff10,{u[2]};Math;;\U0001d7ce\U0001d7d9;\U0001d7f6,",
        f"xx{u[3]};Pre;;1;1;{u[4]};Name;With;Semi;1;2,",
        f"{u[5]};New\nLine;;1;1,{u[6]};;;1;1,{u[7]};Tab\tName;\tT\t;1;3,",
        "",
        "garbage only",
        f"{u[8]};\u2003Em\u2003;\u2003;1;3,{u[9]};A;B;1;3",
    ]


def parse_section() -> list:
    out = []
    for text in dump_text():
        rows = parser.parse_dump(text)
        out.append({"text": text, "rows": [[str(r.id), r.name, r.type, r.stack, r.price, r.price_each,
                                              r.created_at] for r in rows]})
    return out


def outlier_section() -> list:
    out = []
    sets = [CLUSTER + [5e7], [1.0] * 5, [0.0, -1.0, 2.0, 3.0, 4.0, 5.0], [1, 2, 3, 4], [0.5, 0.5, 0.5, 0.5, 500.0],
            [12000.0, 12000.0, 12001.0, 12000.0, 12000.0, 9e6]]
    for _ in range(40):
        n = RNG.randint(4, 30)
        base = RNG.choice([1.0, 75.5, 12000.0, 3e6])
        pts = [round(base * RNG.lognormvariate(0, RNG.choice([0.05, 0.4, 1.5])), 3) for _ in range(n)]
        if RNG.random() < 0.5:
            pts.append(base * RNG.choice([1000, 1e-4]))
        sets.append(pts)
    for prices in sets:
        points = [{"price_each": p} for p in prices]
        kept, dropped = msvc._filter_price_outliers(points)
        gone = {id(d) for d in dropped}
        flags = None if kept is points else [id(p) in gone for p in points]
        out.append({"prices": prices, "dropped": flags})
    return out


def bands_section() -> list:
    out = []
    ranges = [(NOW - 14 * 86400, NOW), (NOW - 90 * 86400, NOW), (NOW - 86400, NOW), (0, 86400),
              (1_700_000_000, 1_700_000_000), (1_710_000_000, 1_712_000_000), (NOW, NOW - 10)]
    for start, end in ranges:
        out.append({"start": start, "end": end, "bands": msvc.event_bands(start, end)})
    return out


def lists_section() -> dict:
    cases = [
        ["b", "A", "c", "b", "\u00e9", "Z", "\u00c9z", "", " x "],
        [" Flux Dust ", "Flux Dust", "", "  ", "\u3000Glim\u3000", "glim2", "Alpha", "alpha beta"],
        [], ["\u0130stanbul", "zeta", "\u00df", "SS"], ["\t", "\n"],
    ]
    return {
        "sorted_lower": [{"in": c, "out": sorted(set(c), key=str.lower)} for c in cases[:1] + cases[3:4]],
        "clean_names": [{"in": c, "out": sorted({n.strip() for n in c if n and n.strip()})} for c in cases],
        "clean_items": [{"in": c, "out": sorted({i.strip() for i in c if i and i.strip()}, key=str.lower)}
                        for c in cases[1:]],
    }


class FakeCat:
    def __init__(self, id_, order, name):
        self.id, self.order, self.name = id_, order, name

    async def save(self):
        SAVES.append([str(self.id), self.order])


SAVES: list = []


async def reorder_section() -> list:
    ids = [ObjectId(f"65a00000000000000000000{i}") for i in range(6)]
    docs = [(ids[0], 2, "Gamma"), (ids[1], 0, "alpha"), (ids[2], 0, "Beta"), (ids[3], 5, "delta"),
            (ids[4], 0, "\u00c9psilon"), (ids[5], 1, "zeta")]
    cases = [[], [str(ids[3]), str(ids[0])], [str(i) for i in ids], ["bad", str(ids[2]), str(ids[2])],
             [str(ids[5]).upper(), "65a00000000000000000ffff"], [str(i) for i in reversed(ids)]]
    out = []

    class Q:
        def __init__(self, rows):
            self.rows = rows

        async def to_list(self):
            return self.rows

    orig = MarketItemCategory.find
    try:
        for case in cases:
            objs = [FakeCat(*d) for d in docs]
            MarketItemCategory.find = classmethod(lambda cls, *a, **k: Q(objs))
            SAVES.clear()
            touched = await msvc.admin_reorder_categories([to_oid(i) for i in case])
            out.append({"docs": [[str(d[0]), d[1], d[2]] for d in docs], "ids": case, "saves": list(SAVES),
                        "touched": touched})
    finally:
        MarketItemCategory.find = orig
    return out


def rank_section() -> list:
    paths = ["prefabs/item/x", "Prefabs/Item/X_DEBUG", "prefabs/item/x_notrade", "prefabs/item/mount/x",
             "prefabs/item/mount/x_debug", "prefabs/collections/x", "prefabs/other", "", None, "PREFABS/COLLECTIONS/"]
    return [[p, cpg._item_role_rank(p)] for p in paths]


async def main() -> None:
    data = {
        "python": sys.version.split()[0], "now": NOW,
        "order_by": [[s, mpg.order_by(s)] for s in SORTS],
        "created_at": created_section(),
        "parse": parse_section(),
        "outliers": outlier_section(),
        "bands": bands_section(),
        **lists_section(),
        "reorder": await reorder_section(),
        "role_rank": rank_section(),
        "store": {"interest": INTEREST, "categories": CATEGORIES},
    }
    data["http"] = await http_section()
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(data, ensure_ascii=True, indent=1) + "\n", encoding="utf-8")
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes) with Python {data['python']}")


if __name__ == "__main__":
    asyncio.run(main())
