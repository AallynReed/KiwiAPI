"""Golden fixtures for the store catalog (/v1/store/*, /site/store/*), from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_store_fixtures.py
Writes native/kiwi-server/testdata/store/golden.json.

Runs the real parser, service, routers and Beanie models. Beanie is initialised
against an in-memory pymongo collection layer that records every call (filter,
projection, sort, skip, limit) and its result, so the Rust service can replay
the same calls; auth and rate limits are swapped for fixed fakes.
"""
import asyncio
import base64
import datetime as dt
import json
import logging
import os
import re
import sys
import types
import urllib.parse
import warnings
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
sys.path.insert(0, os.getcwd())
os.environ.setdefault("SECRET_KEY", "store-fixture-secret")

import bson  # noqa: E402
from bson import ObjectId, json_util  # noqa: E402
from bson.int64 import Int64  # noqa: E402
from bson.json_util import CANONICAL_JSON_OPTIONS  # noqa: E402
from pymongo.asynchronous.collection import AsyncCollection  # noqa: E402
from pymongo.asynchronous.database import AsyncDatabase  # noqa: E402
from pymongo.errors import OperationFailure  # noqa: E402

from app.admin import runtime_config as rc  # noqa: E402

for _key, _spec in rc.REGISTRY.items():
    rc._CACHE[_key] = (_spec.default, float("inf"))

import app.core.dependencies as deps  # noqa: E402
from app.admin.ingest_log import IngestLogEntry  # noqa: E402
from app.core.ratelimit import RateLimitInfo  # noqa: E402
from app.main import app  # noqa: E402
from app.trove.store import models as m  # noqa: E402
from app.trove.store import parser as sp  # noqa: E402
from app.trove.store import service as svc  # noqa: E402
from app.trove.updates import cas  # noqa: E402
from app.trove.updates.models import UpdateChange, UpdateState  # noqa: E402

OUT = Path(__file__).resolve().parents[2] / "kiwi-server" / "testdata" / "store" / "golden.json"
DAY = 86400
ANCHOR = 1760659200
NOW = ANCHOR + 7 * DAY + 3600
KEPT_HEADERS = ("content-type", "cache-control", "www-authenticate", "allow",
                "x-ratelimit-limit", "x-ratelimit-remaining", "x-ratelimit-reset", "content-length")


def ext(v):
    return json.loads(json_util.dumps(v, json_options=CANONICAL_JSON_OPTIONS))


def bson_round(doc):
    return bson.decode(bson.encode(doc))


# --- in-memory pymongo ------------------------------------------------------

DB: dict[str, list[dict]] = {}
CALLS: list[dict] = []
BLOBS: dict[str, bytes] = {}
TRACKED = {"store_products", "store_categories", "store_state", "update_state", "update_changes", "ingest_log"}


def _get(doc, path):
    cur = [doc]
    for part in path.split("."):
        nxt = []
        for c in cur:
            if isinstance(c, dict):
                if part in c:
                    v = c[part]
                    nxt.extend(v if isinstance(v, list) and part != path.split(".")[-1] else [v])
            elif isinstance(c, list):
                for e in c:
                    if isinstance(e, dict) and part in e:
                        nxt.append(e[part])
        cur = nxt
    return cur


def _match_value(values, cond):
    if isinstance(cond, dict) and any(k.startswith("$") for k in cond):
        for op, arg in cond.items():
            if op == "$ne":
                flat = [x for v in values for x in (v if isinstance(v, list) else [v])]
                if arg in flat or (arg is None and not values):
                    return False
            elif op == "$lt":
                if not any(not isinstance(v, (list, dict)) and v is not None and v < arg for v in values):
                    return False
            elif op == "$regex":
                flags = re.I if "i" in cond.get("$options", "") else 0
                try:
                    rx = re.compile(arg, flags)
                except re.error as exc:
                    raise OperationFailure(f"Regular expression is invalid: {exc}", 51091) from exc
                if not any(isinstance(v, str) and rx.search(v) for v in values):
                    return False
            elif op == "$options":
                continue
            elif op == "$elemMatch":
                if not any(isinstance(v, list) and any(_matches(e, arg) for e in v if isinstance(e, dict))
                           for v in values):
                    return False
            else:
                raise NotImplementedError(op)
        return True
    for v in values:
        if v == cond or (isinstance(v, list) and cond in v):
            return True
    return False


def _matches(doc, flt):
    for k, cond in flt.items():
        if k == "$or":
            if not any(_matches(doc, c) for c in cond):
                return False
        elif not _match_value(_get(doc, k), cond):
            return False
    return True


def _project(doc, projection):
    if not projection:
        return dict(doc)
    return {k: v for k, v in doc.items() if k == "_id" or projection.get(k)}


def _run(coll, flt, sort=None, skip=0, limit=0, projection=None):
    docs = [d for d in DB.get(coll, []) if _matches(d, flt or {})]
    for key, direction in reversed(sort or []):
        docs.sort(key=lambda d: (d.get(key) is None, d.get(key)), reverse=direction < 0)
    if skip:
        docs = docs[skip:]
    if limit:
        docs = docs[:limit]
    return [_project(d, projection) for d in docs]


def _record(coll, entry):
    if coll in TRACKED:
        CALLS.append(entry)


class FakeCursor:
    def __init__(self, coll, kw):
        self.coll, self.kw = coll, kw

    async def to_list(self, length=None):
        sort = [[k, d] for k, d in (self.kw.get("sort") or [])]
        res = _run(self.coll, self.kw.get("filter"), sort, self.kw.get("skip") or 0,
                   self.kw.get("limit") or 0, self.kw.get("projection"))
        _record(self.coll, {"op": "find", "coll": self.coll, "filter": ext(self.kw.get("filter") or {}),
                            "projection": ext(self.kw.get("projection")), "sort": sort,
                            "skip": self.kw.get("skip") or 0, "limit": self.kw.get("limit") or 0,
                            "result": ext(res)})
        return res


def _find(self, filter=None, *a, **kw):
    kw["filter"] = filter
    return FakeCursor(self.name, kw)


async def _find_one(self, filter=None, *a, projection=None, **kw):
    res = _run(self.name, filter, projection=projection)
    hit = res[0] if res else None
    _record(self.name, {"op": "find_one", "coll": self.name, "filter": ext(filter or {}),
                        "projection": ext(projection), "result": ext(hit)})
    return hit


async def _count(self, filter, *a, **kw):
    n = len(_run(self.name, filter))
    _record(self.name, {"op": "count", "coll": self.name, "filter": ext(filter), "result": n})
    return n


async def _delete_many(self, filter, *a, **kw):
    keep = [d for d in DB.get(self.name, []) if not _matches(d, filter)]
    n = len(DB.get(self.name, [])) - len(keep)
    DB[self.name] = keep
    _record(self.name, {"op": "delete_many", "coll": self.name, "filter": ext(filter), "result": n})
    return types.SimpleNamespace(deleted_count=n)


async def _insert_one(self, doc, *a, **kw):
    oid = ObjectId("65f000000000000000000001")
    stored = bson_round({"_id": oid, **doc})
    DB.setdefault(self.name, []).append(stored)
    _record(self.name, {"op": "insert_one", "coll": self.name, "doc": ext(stored)})
    return types.SimpleNamespace(inserted_id=oid)


AsyncCollection.find = _find
AsyncCollection.find_one = _find_one
AsyncCollection.count_documents = _count
AsyncCollection.delete_many = _delete_many
AsyncCollection.insert_one = _insert_one


async def _cmd(self, *a, **k):
    return {"version": "7.0.0"}


async def _lcn(self, *a, **k):
    return []


AsyncDatabase.command = _cmd
AsyncDatabase.list_collection_names = _lcn


async def _init():
    from beanie import init_beanie
    from pymongo import AsyncMongoClient
    client = AsyncMongoClient("mongodb://127.0.0.1:1", connect=False, serverSelectionTimeoutMS=200)
    await init_beanie(database=client["kiwi"], skip_indexes=True, document_models=[
        m.StoreProductDoc, m.StoreCategoryDoc, m.StoreStateDoc, UpdateState, UpdateChange, IngestLogEntry])


LOOP = asyncio.new_event_loop()
LOOP.run_until_complete(_init())


class FakeBlob:
    def __init__(self, sha):
        self.sha = sha

    def is_file(self):
        ok = self.sha in BLOBS
        if not ok:
            CALLS.append({"op": "blob", "sha": self.sha, "result": None})
        return ok

    def read_bytes(self):
        data = BLOBS[self.sha]
        CALLS.append({"op": "blob", "sha": self.sha, "result": base64.b64encode(data).decode()})
        return data


cas.ContentStore.path_for = lambda self, sha: FakeBlob(sha)

# --- auth + limits ------------------------------------------------------------

LIMIT = RateLimitInfo(limit=600, remaining=599, reset=dt.datetime(2026, 1, 2, 3, 4, 5, tzinfo=dt.timezone.utc))


async def _limit(*_a, **_k):
    return LIMIT


async def _no_use(*_a, **_k):
    return None


USER = types.SimpleNamespace(id=ObjectId("65a1b2c3d4e5f60718293a4b"), email="master@aallyn.net",
                             is_superuser=True, is_active=True)
TOKENS = {
    "tok_all": types.SimpleNamespace(id=ObjectId("65a1b2c3d4e5f60718293a51"), name="Store bot",
                                     scopes=0, allowed_ip_hashes=[], ip_salt=None),
    "tok_feeds": types.SimpleNamespace(id=ObjectId("65a1b2c3d4e5f60718293a52"), name="feeds only",
                                       scopes=2, allowed_ip_hashes=[], ip_salt=None),
}


async def _resolve(creds):
    tok = TOKENS.get(creds.credentials)
    if tok is None:
        raise deps._not_authenticated("Invalid or revoked API token")
    return USER, tok


deps._resolve_token = _resolve
deps.check_rate_limit = _limit
deps.record_token_use = _no_use
svc.utcnow = lambda: dt.datetime.fromtimestamp(NOW, dt.timezone.utc)


async def call(method, url, headers):
    path, _, qs = url.partition("?")
    scope = {
        "type": "http", "asgi": {"version": "3.0"}, "http_version": "1.1", "method": method,
        "scheme": "http", "path": urllib.parse.unquote(path), "raw_path": path.encode(),
        "query_string": qs.encode(), "root_path": "",
        "headers": [(b"host", b"testserver")] + headers + [(b"content-length", b"0")],
        "client": ("127.0.0.1", 50000), "server": ("testserver", 80),
    }
    done = asyncio.Event()
    pending = [{"type": "http.request", "body": b"", "more_body": False}]
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
    start = next(x for x in sent if x["type"] == "http.response.start")
    raw = b"".join(x.get("body", b"") for x in sent if x["type"] == "http.response.body")
    return start["status"], [(k.lower().decode(), v.decode("latin-1")) for k, v in start["headers"]], raw


# --- datasets -------------------------------------------------------------------


def price(cur, cost, can=True, monthly=0, sale=""):
    return {"currency": cur, "cost": cost, "can_purchase": can, "monthly": monthly, "sale": sale}


def tex(t, x=0, y=0, text="", overlay=False):
    return {"texture": t, "x": x, "y": y, "text": text, "overlay": overlay}


def product(code, kind, **kw):
    doc = m.StoreProductDoc(code=code, kind=kind, **kw)
    from beanie.odm.utils.dump import get_dict
    return get_dict(doc, to_db=True)


_OID = [0]


def oid():
    _OID[0] += 1
    return ObjectId(f"66{_OID[0]:022x}")


def stored(d):
    return bson_round({"_id": oid(), **d})


A = ANCHOR
MAIN_PRODUCTS = [
    product("pack_alpha", "product", name="$Pack_Alpha", image="ui/store/alpha.dds", info="Alpha pack info",
            tradable=True, prices=[price("TWC", 100.0, sale="Sale_25"), price("TWP", 2500.0)],
            promo="$Promo_New", deal_expires_at=A + 2 * DAY, textures=[tex("ui/store/bg.dds"),
                                                                       tex("ui/store/fg.dds", 4, -2, "NEW", True)],
            loot_title="$Loot", loot_body="Common 50%\nRare 5%", categories=[0, 3], first_seen=A - 10 * DAY,
            last_seen=A, price_history=[{"ts": A - 10 * DAY, "prices": [price("TWC", 120.0)], "price_string": ""},
                                        {"ts": A - 2 * DAY, "prices": [price("TWC", 100.0, sale="Sale_25"),
                                                                       price("TWP", 2500.0)], "price_string": ""}],
            availability=[[A - 10 * DAY, A - 5 * DAY], [A - 2 * DAY, A]]),
    product("starter_beta", "starter", name="Ünïcode ✓ 🐉 \"quoted\"", image="", price_string="€4.99",
            price_string_currency="EUR", price_string_sale="", categories=[0], first_seen=A, last_seen=A,
            price_history=[{"ts": A, "prices": [], "price_string": "€4.99"}], availability=[[A, A]]),
    product("patron_gamma", "patron", name="Patron", prices=[price("TWC", 300.0, monthly=300, sale="Patron_Sale")],
            categories=[3], first_seen=A - 20 * DAY, last_seen=A - 3 * DAY,
            price_history=[{"ts": A - 20 * DAY, "prices": [price("TWC", 300.0, monthly=300)], "price_string": ""}],
            availability=[[A - 20 * DAY, A - 3 * DAY]]),
    product("class_delta", "class", name="$Class_Delta", class_level=30, class_power_rank=1, class_shield_frame=2,
            class_sub_name="$Sub_Delta", class_icon="ui/class/delta.dds", prices=[price("TWC", 6250.0, can=False)],
            categories=[5], first_seen=A - 1 * DAY, last_seen=A, availability=[[A - DAY, A]]),
    product("trial_eta", "trial", name="Trial", interact_label="$Try", interact_enabled=True,
            trial_limits="1h", price_string="$9.99", price_string_currency="USD", price_string_sale="Sale_10",
            categories=[], first_seen=A - 40 * DAY, last_seen=A - 30 * DAY,
            availability=[[A - 40 * DAY, A - 35 * DAY], [A - 32 * DAY, A - 30 * DAY]]),
]
MAIN_RAW = [
    {"code": "legacy_eps", "kind": "product", "name": "Legacy", "last_seen": A - 100 * DAY},
    {"code": "weird_zeta", "kind": "interactable", "name": "Wëird [zeta]", "image": "ui/z.png",
     "prices": [{"currency": "TWC", "cost": 7, "can_purchase": True},
                {"currency": "TWP", "cost": 1e16, "can_purchase": False, "monthly": Int64(5), "sale": ""},
                {"currency": "TWC", "cost": 0.1 + 0.2, "can_purchase": True, "monthly": 0, "sale": "x"},
                {"currency": "EUR", "cost": -0.0, "can_purchase": True}],
     "deal_expires_at": Int64(9_000_000_000_000), "categories": [Int64(7), 0], "first_seen": Int64(A - 50 * DAY),
     "last_seen": A, "textures": [{"texture": "a"}],
     "price_history": [{"ts": A - 50 * DAY, "prices": [{"currency": "TWC", "cost": 3.5, "can_purchase": True}]},
                       {"ts": A - 49 * DAY, "prices": [{"currency": "GEM", "cost": 1.0, "can_purchase": True},
                                                        {"currency": "TWC", "cost": 9.25, "can_purchase": True}],
                        "price_string": "x"},
                       {"ts": A - 48 * DAY}],
     "availability": [[A - 50 * DAY, A - 50 * DAY + DAY // 2], [A - 40 * DAY, A - 40 * DAY + 3 * DAY // 2],
                      [A - 30 * DAY, A - 30 * DAY + 5 * DAY // 2], [1, 2, 3], [], [A - DAY, A]]},
]
MAIN_CATEGORIES = [
    {"index": 0, "label": "$Store_Tab_Featured", "icon": "", "codes": ["pack_alpha", "starter_beta", "weird_zeta"],
     "last_seen": A},
    {"index": 3, "label": "$Store_Tab_Packs", "icon": "ui/icon/packs.dds", "codes": ["pack_alpha", "patron_gamma"],
     "last_seen": A - 3 * DAY},
    {"index": -2, "label": "Ünï", "codes": [], "last_seen": A},
    {"index": 5, "last_seen": 0},
]

DATASETS = {
    "main": {
        "store_state": [{"key": "state", "last_anchor": A, "title": "$Store", "product_count": 5, "category_count": 4}],
        "store_products": MAIN_PRODUCTS + MAIN_RAW,
        "store_categories": MAIN_CATEGORIES,
    },
    "empty": {},
    "nostate": {
        "store_products": MAIN_PRODUCTS[:2] + MAIN_RAW[:1],
        "store_categories": MAIN_CATEGORIES[:2],
    },
    "zero_anchor": {
        "store_state": [{"key": "state", "last_anchor": 0}],
        "store_products": [product("zero_a", "product", first_seen=0, last_seen=0),
                           product("zero_b", "product", first_seen=A, last_seen=A, availability=[[A, A]])],
        "store_categories": [{"index": 1, "label": "z", "icon": "", "codes": ["zero_a"], "last_seen": 0}],
    },
    "null_anchor": {
        "store_state": [{"key": "state", "last_anchor": None, "title": ""}],
        "store_products": [product("na", "product", first_seen=A, last_seen=A)],
    },
    "nonfinite": {
        "store_state": [{"key": "state", "last_anchor": A}],
        "store_products": [product("inf_theta", "product", last_seen=A, first_seen=A,
                                   prices=[price("TWC", float("inf")), price("TWP", float("-inf"))])],
    },
    "texture": {
        "update_state": [
            {"branch": "live-us", "path": "ui/store/alpha.dds", "content_sha256": "aa" * 32, "size": 4},
            {"branch": "live-us", "path": "ui/store/Mixed.PNG", "content_sha256": "bb" * 32, "size": 3},
            {"branch": "live-us", "path": "ui/store/lower.png", "content_sha256": "cc" * 32, "size": 3},
            {"branch": "live-us", "path": "ui/store/nosha.dds", "content_sha256": "", "size": 0},
            {"branch": "live-us", "path": "ui/store/missingblob.dds", "content_sha256": "dd" * 32, "size": 1},
            {"branch": "live-us", "path": "ui/(x)+[y].jpg", "content_sha256": "ee" * 32, "size": 2},
            {"branch": "pts", "path": "ui/store/pts.dds", "content_sha256": "ff" * 32, "size": 1},
        ],
        "update_changes": [
            {"branch": "live-us", "ordinal": 3, "path": "ui/store/gone.dds", "type": "added",
             "content_sha256": "11" * 32, "size": 5},
            {"branch": "live-us", "ordinal": 5, "path": "ui/store/gone.dds", "type": "modified",
             "content_sha256": "12" * 32, "size": 6},
            {"branch": "live-us", "ordinal": 7, "path": "ui/store/gone.dds", "type": "removed",
             "content_sha256": None, "size": 0},
            {"branch": "live-us", "ordinal": 2, "path": "ui/store/old.dds", "type": "modified",
             "content_sha256": "13" * 32, "size": 1},
        ],
    },
}
BLOB_DATA = {"aa" * 32: b"DDS \x00\xff", "bb" * 32: b"\x89PNG", "cc" * 32: b"png",
             "ee" * 32: b"jj", "12" * 32: b"gone-v5"}


def load(name):
    DB.clear()
    for coll, docs in DATASETS[name].items():
        DB[coll] = [stored(d) for d in docs]
    BLOBS.clear()
    if name == "texture":
        BLOBS.update(BLOB_DATA)


# --- http cases -------------------------------------------------------------------

TOK = {"authorization": "Bearer tok_all"}
P = "/v1/store/products"
SP = "/site/store/products"

HTTP = [
    ("main", "v1_products", P, TOK),
    ("main", "v1_products_inactive", P + "?active=false", TOK),
    ("main", "v1_products_cat", P + "?category=3&active=false", TOK),
    ("main", "v1_products_kind", P + "?kind=patron&active=0", TOK),
    ("main", "v1_products_kind_empty", P + "?kind=&currency=", TOK),
    ("main", "v1_products_currency", P + "?currency=TWC&active=no", TOK),
    ("main", "v1_products_sale", P + "?on_sale=true&active=off", TOK),
    ("main", "v1_products_sale_yes", P + "?on_sale=YES", TOK),
    ("main", "v1_products_q", P + "?q=ALPHA", TOK),
    ("main", "v1_products_q_unicode", P + "?q=%C3%9Cn%C3%AF&active=false", TOK),
    ("main", "v1_products_q_empty", P + "?q=", TOK),
    ("main", "v1_products_page", P + "?limit=2&offset=1&active=false", TOK),
    ("main", "v1_products_page_far", P + "?limit=1000&offset=50", TOK),
    ("main", "v1_products_all_filters", P + "?category=0&kind=product&currency=TWC&q=a&active=1&on_sale=on"
                                             "&limit=5&offset=0", TOK),
    ("main", "v1_products_repeat", P + "?kind=x&kind=starter&limit=1.0", TOK),
    ("main", "v1_products_bad", P + "?category=x&active=maybe&on_sale=2&limit=0&offset=-1", TOK),
    ("main", "v1_products_bad2", P + "?category=&limit=1001&offset=1.5", TOK),
    ("main", "v1_products_bad3", P + "?category=1e3&limit=abc", TOK),
    ("main", "v1_products_bad_regex", P + "?q=%5B", TOK),
    ("main", "v1_products_plus", P + "?q=a+b&kind=%2B", TOK),
    ("nostate", "v1_products_nostate", P, TOK),
    ("zero_anchor", "v1_products_zero", P, TOK),
    ("null_anchor", "v1_products_null", P, TOK),
    ("empty", "v1_products_empty", P, TOK),
    ("nonfinite", "v1_products_nonfinite", P, TOK),
    ("main", "site_products", SP, {}),
    ("main", "site_products_inactive", SP + "?active=false&limit=3", {}),
    ("main", "site_products_bad", SP + "?limit=1001&offset=x", {}),
    ("main", "site_products_q", SP + "?q=%C3%BC&active=false", {}),
    ("nonfinite", "site_products_nonfinite", SP, {}),
    ("main", "v1_categories", "/v1/store/categories", TOK),
    ("nostate", "v1_categories_nostate", "/v1/store/categories", TOK),
    ("zero_anchor", "v1_categories_zero", "/v1/store/categories", TOK),
    ("empty", "v1_categories_empty", "/v1/store/categories", TOK),
    ("main", "site_categories", "/site/store/categories", {}),
    ("main", "v1_timeline", "/v1/store/timeline", TOK),
    ("main", "v1_timeline_kind", "/v1/store/timeline?kind=product&limit=2", TOK),
    ("main", "v1_timeline_bad", "/v1/store/timeline?limit=0&kind=a", TOK),
    ("main", "v1_timeline_bad_high", "/v1/store/timeline?limit=4001", TOK),
    ("nostate", "v1_timeline_nostate", "/v1/store/timeline", TOK),
    ("zero_anchor", "v1_timeline_zero", "/v1/store/timeline", TOK),
    ("empty", "v1_timeline_empty", "/v1/store/timeline", TOK),
    ("main", "v1_timeline_none", "/v1/store/timeline?kind=nothing", TOK),
    ("main", "site_timeline", "/site/store/timeline?limit=4000", {}),
    ("main", "site_timeline_bad", "/site/store/timeline?limit=-1", {}),
    ("main", "v1_product", P + "/pack_alpha", TOK),
    ("main", "v1_product_beta", P + "/starter_beta", TOK),
    ("main", "v1_product_gamma", P + "/patron_gamma", TOK),
    ("main", "v1_product_delta", P + "/class_delta", TOK),
    ("main", "v1_product_eta", P + "/trial_eta", TOK),
    ("main", "v1_product_legacy", P + "/legacy_eps", TOK),
    ("main", "v1_product_weird", P + "/weird_zeta", TOK),
    ("main", "v1_product_missing", P + "/nope%20x%C3%A9", TOK),
    ("main", "v1_product_badutf8", P + "/%FF%C3", TOK),
    ("nostate", "v1_product_nostate", P + "/pack_alpha", TOK),
    ("zero_anchor", "v1_product_zero", P + "/zero_a", TOK),
    ("nonfinite", "v1_product_nonfinite", P + "/inf_theta", TOK),
    ("main", "site_product", SP + "/pack_alpha", {}),
    ("main", "site_product_weird", SP + "/weird_zeta", {}),
    ("main", "site_product_gamma", SP + "/patron_gamma", {}),
    ("main", "site_product_missing", SP + "/nope", {}),
    ("nonfinite", "site_product_nonfinite", SP + "/inf_theta", {}),
    ("texture", "texture_exact", "/site/store/texture?path=ui/store/alpha.dds", {}),
    ("texture", "texture_backslash", "/site/store/texture?path=%20%5Cui%5Cstore%5Calpha.dds%0A", {}),
    ("texture", "texture_lower", "/site/store/texture?path=/UI/Store/Lower.PNG", {}),
    ("texture", "texture_regex", "/site/store/texture?path=ui/store/mixed.png", {}),
    ("texture", "texture_special", "/site/store/texture?path=UI/(X)%2B[Y].JPG", {}),
    ("texture", "texture_removed", "/site/store/texture?path=ui/store/gone.dds", {}),
    ("texture", "texture_not_removed", "/site/store/texture?path=ui/store/old.dds", {}),
    ("texture", "texture_nosha", "/site/store/texture?path=ui/store/nosha.dds", {}),
    ("texture", "texture_missing_blob", "/site/store/texture?path=ui/store/missingblob.dds", {}),
    ("texture", "texture_other_branch", "/site/store/texture?path=ui/store/pts.dds", {}),
    ("texture", "texture_missing", "/site/store/texture?path=nothing/here", {}),
    ("texture", "texture_no_ext", "/site/store/texture?path=", {}),
    ("texture", "texture_no_param", "/site/store/texture", {}),
    ("main", "reset", "/v1/store/reset", TOK),
]


def run_http():
    out = []
    for i, (dataset, name, url, headers) in enumerate(HTTP):
        load(dataset)
        CALLS.clear()
        method = "POST" if name == "reset" else "GET"
        hdrs = [(k.encode(), v.encode("latin-1")) for k, v in headers.items()]
        hdrs.append((b"x-request-id", f"req_s_{i:03d}".encode()))
        status, rh, raw = LOOP.run_until_complete(call(method, url, hdrs))
        out.append({
            "name": name, "dataset": dataset, "method": method, "url": url,
            "headers": [[k.decode(), v.decode("latin-1")] for k, v in hdrs],
            "status": status,
            "resp_headers": {k: v for k, v in rh if k in KEPT_HEADERS},
            "resp": base64.b64encode(raw).decode() if name.startswith("texture") else raw.decode("utf-8"),
            "calls": list(CALLS),
        })
    return out


GATED = [
    ("v1_disabled", "GET", P + "?limit=0", {}),
    ("v1_disabled_product", "GET", P + "/x", TOK),
    ("site_disabled", "GET", SP + "?limit=0", {}),
    ("site_disabled_texture", "GET", "/site/store/texture", {}),
    ("reset_disabled", "POST", "/v1/store/reset", {}),
    ("v1_noauth", "GET", P + "?limit=0", {}),
    ("v1_badtoken", "GET", "/v1/store/categories", {"authorization": "Bearer nope"}),
    ("v1_scope", "GET", "/v1/store/timeline?limit=0", {"authorization": "Bearer tok_feeds"}),
    ("reset_noauth", "POST", "/v1/store/reset", {}),
    ("products_405", "POST", P, TOK),
    ("product_405", "DELETE", P + "/x", TOK),
    ("reset_405", "GET", "/v1/store/reset", TOK),
    ("site_405", "POST", "/site/store/texture", {}),
]


def run_gated():
    out = []
    for i, (name, method, url, headers) in enumerate(GATED):
        load("main")
        flag = rc._CACHE["feature_store_enabled"]
        if "disabled" in name:
            rc._CACHE["feature_store_enabled"] = (False, float("inf"))
        hdrs = [(k.encode(), v.encode("latin-1")) for k, v in headers.items()]
        hdrs.append((b"x-request-id", f"req_g_{i:03d}".encode()))
        status, rh, raw = LOOP.run_until_complete(call(method, url, hdrs))
        rc._CACHE["feature_store_enabled"] = flag
        out.append({"name": name, "method": method, "url": url,
                    "headers": [[k.decode(), v.decode("latin-1")] for k, v in hdrs], "status": status,
                    "resp_headers": {k: v for k, v in rh if k in KEPT_HEADERS}, "resp": raw.decode("utf-8")})
    return out


# --- parser -----------------------------------------------------------------------


def frepr(x):
    return None if x is None else repr(x)


def parsed_json(d: sp.ParsedStoreDump):
    def prod(p):
        o = p._asdict()
        o["prices"] = [{**pr._asdict(), "cost": frepr(pr.cost)} for pr in p.prices]
        o["textures"] = [t._asdict() for t in p.textures]
        o["deal_seconds"] = frepr(p.deal_seconds)
        return o
    return {
        "categories": [c._asdict() for c in d.categories],
        "products": [prod(p) for p in d.products],
        "balance": None if d.balance is None else [frepr(d.balance[0]), frepr(d.balance[1])],
        "title": d.title, "instructions": d.instructions, "done": d.done,
    }


def prod_line(code="c", kind="product", name="", image="", owned="0", tradable="0", informational="0", info="",
              prices="", ps="", promo="", deal="", interact="", cls="", textures="", lt="", lb="", cats=""):
    return ";".join([code, kind, name, image, owned, tradable, informational, info, prices, ps, promo, deal,
                     interact, cls, textures, lt, lb, cats])


PARSE = {
    "empty": "",
    "basic": "\n".join([
        "[kiwistore.swf]",
        "boot = 1",
        "cat$0 = %24Store_Tab_Featured;ui%2Ficon.dds;pack_a,pack_b,,pack_c",
        "cat$3=Packs;;",
        "prod$pack_a = " + prod_line("pack_a", "product", "%24Pack%20A", "ui%2Fa.dds", "1", "1", "0", "Info%3B%3A%7C",
                                     "TWC:100:1:0:Sale_25|TWP:2500.5:0:0:|TWC:100:1:0:Sale_25", "", "%24Promo",
                                     "3600", "", "", "ui%2Fbg.dds:0:0::0|ui%2Ffg.dds:4:-2:NEW:1|ui%2Fbg.dds:0:0::0",
                                     "%24Loot", "Common%2050%25%0ARare", "0,3"),
        "prod$pack_b = " + prod_line("pack_b", "starter", "B", ps="%E2%82%AC4.99:EUR:", cats="0"),
        "prod$cls = " + prod_line("cls", "class", cls="30:1:2:%24Sub:ui%2Fc.dds", interact="%24Try:1:1h"),
        "meta$balance = 1234;56.5",
        "meta$title = %24Store%20Title",
        "meta$instructions = Press%20X",
        "done = true",
    ]),
    "line_breaks": "cat$1=a;b;c\r\ncat$2=d;e;f\rcat$4=g;h;i\x0bcat$5=j;k;l\x0ccat$6=m;n;o\x1ccat$7=p;q;r\x1d"
                   "cat$8=s;t;u\x1ecat$9=v;w;x\x85cat$10=y;z;0\u2028cat$11=1;2;3\u2029cat$12=4;5;6\n\n",
    "whitespace": "\u3000 cat$1 \u00a0=\u2003 a;b;c \x1f\n\tdone\t=\tTRUE \n# cat$9=x;y;z\n; cat$9=x;y;z\n"
                  "cat$9\n=nokey\n  [section]\ncat$ 13 =a;b;c\ncat$-1=a;b;c\ncat$+2=a;b;c\ncat$1_0=a;b;c\n"
                  "cat$\u0663=arabic;b;c\ncat$x=bad;b;c\ncat$14=too;many;fields;here\ncat$15=two;fields",
    "done_variants": "done=true\ndone=false\n",
    "done_last_true": "done=False\ndone = tRuE",
    "first_wins": "cat$1=first;a;b\ncat$1=second;a;b\n" + "prod$x=" + prod_line("dup", name="first") + "\n"
                  + "prod$y=" + prod_line("dup", name="second") + "\nmeta$title=one\nmeta$title=two",
    "bad_products": "\n".join([
        "prod$a=" + prod_line("a", "bogus"),
        "prod$b=" + prod_line("", "product"),
        "prod$c=" + prod_line("c") + ";extra",
        "prod$d=only;three;fields",
        "prod$e=" + prod_line("%2541", "trial"),
        "prod$f=" + prod_line("f", "PRODUCT"),
        "prod$g=" + prod_line("%20", "patron"),
    ]),
    "numbers": "\n".join([
        "prod$n1=" + prod_line("n1", prices="TWC: 1_000 :1:0:|TWC:1e3:1: 7 :a|TWC:inf:1:0:|TWC:-Infinity:0:0:|"
                                            "TWC:nan:1:0:|TWC:nan:1:0:|TWC:0.0:1:0:|TWC:-0.0:1:0:|TWC:.5:1:0:|"
                                            "TWC:5.:1:0:|TWC:1__0:1:0:|TWC:_1:1:0:|TWC:1_:1:0:|TWC:0x10:1:0:|"
                                            "TWC:\u0661\u0662:1:0:|TWC:1.5:1:1.0:|TWC:1:1:0x1:|TWC::1:0:|"
                                            "TWC:1:1:0|TWC:1:1:0::|:1:2:3:4|TWC:1e400:1:0:|TWC:1e-400:1:0:|"
                                            "TWC:0.1:1:+5:|TWC:1_0.2_5:1:0:|TWC:1._5:1:0:|TWC:1e1_0:1:0:",
                                     deal="12.75"),
        "prod$n2=" + prod_line("n2", deal="-3.9", cls="x:1:2:a:b", textures="t:1:x:a:1|t:1:2:a:1|t:+1:-0:a:0|"
                                                                             "t:1:2:a:1|t:1:2:a:2|t:1:2|t:1:2:3:4:5"),
        "prod$n3=" + prod_line("n3", deal="soon", cls="1:2:3:%ZZ:%e2%82", cats="1, 2,x,,-3,\u0664,3_0"),
        "prod$n4=" + prod_line("n4", deal=" 1e3 ", cls="1:2:3", interact="a:1", ps="a:b"),
        "prod$n5=" + prod_line("n5", deal="nan", ps="x:y:z:w", interact="l:0:t:x"),
        "meta$balance=1;2;3",
        "meta$balance=a;2",
    ]),
    "balance_ok": "meta$balance= 1e2 ; -0 ",
    "percent": "\n".join([
        "meta$title=%e2%9c%93%20%FF%C3%28%ED%A0%80%F0%9F%90%89%zz%4%%41+plus%2b",
        "meta$instructions=é%C3%A9ü%C3%BC%E2é%82%AC",
        "cat$1=%C3%9C%;%;a%2Cb",
    ]),
    "no_equals": "just text\nprod$x\n=\n=value",
    "equals_in_value": "meta$title=a=b=c\ncat$1=a=b;c=d;e=f",
}


def run_parse():
    return [{"name": k, "text": v, "result": parsed_json(sp.parse_dump(v))} for k, v in PARSE.items()]


# --- ingest -----------------------------------------------------------------------


async def _save(self, *a, **k):
    from beanie.odm.utils.dump import get_dict
    if self.id is None:
        self.id = oid()
    coll = self.get_settings().name
    body = bson_round({"_id": self.id, **get_dict(self, to_db=True)})
    docs = DB.setdefault(coll, [])
    for i, d in enumerate(docs):
        if d["_id"] == self.id:
            docs[i] = body
            break
    else:
        docs.append(body)
    return self


for _model in (m.StoreProductDoc, m.StoreCategoryDoc, m.StoreStateDoc):
    _model.save = _save


def dump_line(code, **kw):
    return "prod$" + code + "=" + prod_line(code, **kw)


INGESTS = {
    "lifecycle": [
        (A, "\n".join([
            "cat$0=Featured;;a,b",
            dump_line("a", prices="TWC:100:1:0:", lb="loot%20a", lt="LT", deal="100.9"),
            dump_line("b", kind="starter", ps="%E2%82%AC1:EUR:"),
            "meta$title=First",
        ])),
        (A + DAY, "\n".join([
            "cat$0=Featured;icon;b,a",
            dump_line("a", prices="TWC:100:1:0:", lb="", lt="ignored"),
            dump_line("b", kind="starter", ps="%E2%82%AC2:EUR:"),
            "done=true",
        ])),
        (A + DAY, "\n".join([
            dump_line("a", prices="TWC:100:1:0:|TWC:100:1:0:"),
        ])),
        (A + 3 * DAY, "\n".join([
            "cat$1=New;;c",
            dump_line("c", kind="patron", prices="TWC:300:1:300:|TWP:5:0:0:"),
        ])),
        (A + 4 * DAY, "\n".join([
            dump_line("a", prices="TWP:5:0:0:|TWC:100:0:0:", lb="new%20loot"),
            dump_line("c", kind="patron", prices="TWP:5:0:0:|TWC:300:0:300:|TWC:300:1:300:"),
            "meta$title=",
        ])),
        (A + 4 * DAY, "boot = 1\n# nothing\n"),
        (A + 5 * DAY, "\n".join([
            dump_line("a", kind="trial", prices="TWC:-0.0:1:0:", deal="-1.5"),
            dump_line("c", kind="patron", prices="TWC:0:1:300:|TWP:5:1:0:"),
        ])),
    ],
    "legacy_backfill": [
        ("seed", None),
        (A + 10 * DAY, dump_line("old", prices="TWC:1:1:0:")),
        (A + 11 * DAY, dump_line("old2", prices="TWC:1:1:0:")),
        (A + 12 * DAY, dump_line("old3")),
    ],
}

LEGACY_SEED = [
    {"code": "old", "kind": "product", "first_seen": A, "last_seen": A + 2 * DAY},
    {"code": "old2", "kind": "product", "first_seen": A, "last_seen": A + 11 * DAY},
    {"code": "old3", "kind": "product", "first_seen": 0, "last_seen": A},
]


def snapshot():
    def sort(coll, key):
        return ext(sorted(DB.get(coll, []), key=lambda d: d[key]))
    return {"products": sort("store_products", "code"), "categories": sort("store_categories", "index"),
            "state": ext(DB.get("store_state", []))}


def run_ingest():
    out = []
    for name, steps in INGESTS.items():
        DB.clear()
        rows = []
        seed = []
        for anchor, text in steps:
            if anchor == "seed":
                seed = [stored(d) for d in LEGACY_SEED]
                DB["store_products"] = list(seed)
                continue
            try:
                summary = LOOP.run_until_complete(svc.insert_dump(text, timestamp=anchor))
                rows.append({"anchor": anchor, "text": text, "summary": summary, **snapshot()})
            except ValueError as exc:
                rows.append({"anchor": anchor, "text": text, "error": str(exc), **snapshot()})
        out.append({"name": name, "seed": ext(seed), "steps": rows})
    return out


def datasets():
    out = {}
    for name in DATASETS:
        load(name)
        out[name] = {coll: ext(docs) for coll, docs in DB.items()}
    return out


def main():
    data = {
        "python": sys.version.split()[0],
        "now": NOW,
        "datasets": datasets(),
        "blobs": {sha: base64.b64encode(b).decode() for sha, b in BLOB_DATA.items()},
        "http": run_http(),
        "gated": run_gated(),
        "parse": run_parse(),
        "ingest": run_ingest(),
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(data, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {OUT} ({len(data['http'])} http, {len(data['gated'])} gated, {len(data['parse'])} parse, "
          f"{len(data['ingest'])} ingest)")


main()
