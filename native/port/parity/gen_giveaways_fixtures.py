"""Golden fixtures for giveaways (/admin/vault/*, /admin/giveaways*, /v1/giveaways/*), from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_giveaways_fixtures.py
Writes native/kiwi-server/testdata/giveaways/golden.json.

- "bodies": request bodies sent through the real app (superuser faked, the
  service stubbed to capture what it was handed): 422 bodies, the parsed
  request models, the OverflowError 500.
- "scenario": the real routers, service and Beanie models over an in-memory
  pymongo (sequential ObjectIds from 65f0..01, a frozen clock, secrets.choice
  taking the first entrant): every reply and the final contents of every
  collection, so the Rust service can replay the same steps.
- "gated": no overrides, through the whole middleware stack: 405s, missing
  or broken credentials, a malformed body before auth.
- "oids": what `_oid` makes of odd ids.
"""
import asyncio
import datetime as dt
import json
import logging
import os
import sys
import types
import urllib.parse
import warnings
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
sys.path.insert(0, os.getcwd())
os.environ.setdefault("SECRET_KEY", "giveaways-fixture-secret")

import bson  # noqa: E402
from bson import ObjectId, json_util  # noqa: E402
from bson.codec_options import CodecOptions  # noqa: E402
from bson.json_util import CANONICAL_JSON_OPTIONS  # noqa: E402
from pymongo.asynchronous.collection import AsyncCollection  # noqa: E402
from pymongo.asynchronous.database import AsyncDatabase  # noqa: E402
from pymongo.errors import DuplicateKeyError  # noqa: E402

from app.admin import runtime_config as rc  # noqa: E402

for _key, _spec in rc.REGISTRY.items():
    rc._CACHE[_key] = (_spec.default, float("inf"))

import app.core.dependencies as deps  # noqa: E402
import app.core.utils as core_utils  # noqa: E402
from app.auth.models import User  # noqa: E402
from app.core.config import settings  # noqa: E402
from app.core.email_outbox import OutboxEmail  # noqa: E402
from app.core.errors import APIError, ErrorCode  # noqa: E402
from app.giveaways import router as gw_router  # noqa: E402
from app.giveaways import service  # noqa: E402
from app.giveaways.models import Giveaway, GiveawayEntry, PrizeCode, VaultItem  # noqa: E402
from app.main import app  # noqa: E402
from app.site_auth.dependencies import get_current_site_user  # noqa: E402
from app.site_auth.models import SiteUser  # noqa: E402

OUT = Path(__file__).resolve().parents[2] / "kiwi-server" / "testdata" / "giveaways" / "golden.json"
TZ = CodecOptions(tz_aware=True)


def ext(v):
    return json.loads(json_util.dumps(v, json_options=CANONICAL_JSON_OPTIONS))


def bson_round(doc):
    return bson.decode(bson.encode(doc), codec_options=TZ)


# --- frozen clock -------------------------------------------------------------

CLOCK = [dt.datetime(2026, 10, 22, 11, 0, 0, 123456, tzinfo=dt.timezone.utc)]


class Frozen(dt.datetime):
    @classmethod
    def now(cls, tz=None):
        return CLOCK[0]


core_utils.datetime = Frozen
service.secrets = types.SimpleNamespace(choice=lambda seq: seq[0])

# --- in-memory pymongo --------------------------------------------------------

DB: dict[str, list[dict]] = {}
NEXT_ID = [0]
UNIQUE = {"giveaway_entries": ("giveaway_id", "site_user_id"), "users": ("email",)}


def new_id():
    NEXT_ID[0] += 1
    return ObjectId(f"65f0{NEXT_ID[0]:020x}")


def _cmp(op, v, arg):
    if v is None or isinstance(v, (list, dict)) or type(v) is not type(arg) and not (
            isinstance(v, (int, float)) and isinstance(arg, (int, float))):
        return False
    return {"$lte": v <= arg, "$gte": v >= arg, "$lt": v < arg, "$gt": v > arg}[op]


def _match_value(doc, key, cond):
    present = key in doc
    v = doc.get(key)
    if isinstance(cond, dict) and cond and all(k.startswith("$") for k in cond):
        for op, arg in cond.items():
            if op == "$in":
                if not any(v == a for a in arg):
                    return False
            elif op == "$ne":
                if present and v == arg or (arg is None and not present):
                    return False
            elif op in ("$lte", "$gte", "$lt", "$gt"):
                if not present or not _cmp(op, v, arg):
                    return False
            else:
                raise NotImplementedError(op)
        return True
    if cond is None:
        return v is None
    return present and v == cond


def _matches(doc, flt):
    for k, cond in (flt or {}).items():
        if k == "$and":
            if not all(_matches(doc, c) for c in cond):
                return False
        elif not _match_value(doc, k, cond):
            return False
    return True


def _sort_key(v):
    return (0,) if v is None else (1, v)


def _run(coll, flt, sort=None, limit=0):
    flt = bson_round(flt or {})
    docs = [d for d in DB.get(coll, []) if _matches(d, flt)]
    for key, direction in reversed(sort or []):
        docs.sort(key=lambda d: _sort_key(d.get(key)), reverse=direction < 0)
    if limit:
        docs = docs[:limit]
    return [bson_round(d) for d in docs]


class FakeCursor:
    def __init__(self, coll, kw):
        self.coll, self.kw = coll, kw

    async def to_list(self, length=None):
        sort = list(self.kw.get("sort") or [])
        return _run(self.coll, self.kw.get("filter"), sort, self.kw.get("limit") or 0)


def _find(self, filter=None, *a, **kw):
    kw["filter"] = filter
    return FakeCursor(self.name, kw)


async def _find_one(self, filter=None, *a, **kw):
    res = _run(self.name, filter)
    return res[0] if res else None


async def _count(self, filter, *a, **kw):
    return len(_run(self.name, filter))


def _store(coll, doc):
    stored = bson_round(doc)
    keys = UNIQUE.get(coll)
    if keys and any(all(d.get(k) == stored.get(k) for k in keys) for d in DB.get(coll, [])):
        raise DuplicateKeyError("E11000 duplicate key error", 11000)
    DB.setdefault(coll, []).append(stored)


async def _insert_one(self, doc, *a, **kw):
    oid = new_id()
    _store(self.name, {"_id": oid, **doc})
    return types.SimpleNamespace(inserted_id=oid)


async def _insert_many(self, docs, *a, **kw):
    ids = []
    for doc in docs:
        oid = new_id()
        _store(self.name, {"_id": oid, **doc})
        ids.append(oid)
    return types.SimpleNamespace(inserted_ids=ids)


async def _find_one_and_update(self, filter, update, *a, upsert=False, **kw):
    assert set(update) == {"$set"}, update
    sets = bson_round(update["$set"])
    rows = DB.setdefault(self.name, [])
    for i, d in enumerate(rows):
        if _matches(d, bson_round(filter)):
            new = dict(d)
            new.update(sets)
            rows[i] = bson_round(new)
            return bson_round(rows[i])
    if not upsert:
        return None
    new = {"_id": filter["_id"], **{k: v for k, v in sets.items() if k != "_id"}}
    _store(self.name, new)
    return bson_round(new)


async def _delete_one(self, filter, *a, **kw):
    rows = DB.get(self.name, [])
    for i, d in enumerate(rows):
        if _matches(d, bson_round(filter)):
            del rows[i]
            return types.SimpleNamespace(deleted_count=1)
    return types.SimpleNamespace(deleted_count=0)


async def _delete_many(self, filter, *a, **kw):
    rows = DB.get(self.name, [])
    keep = [d for d in rows if not _matches(d, bson_round(filter))]
    DB[self.name] = keep
    return types.SimpleNamespace(deleted_count=len(rows) - len(keep))


AsyncCollection.find = _find
AsyncCollection.find_one = _find_one
AsyncCollection.count_documents = _count
AsyncCollection.insert_one = _insert_one
AsyncCollection.insert_many = _insert_many
AsyncCollection.find_one_and_update = _find_one_and_update
AsyncCollection.delete_one = _delete_one
AsyncCollection.delete_many = _delete_many


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
    await init_beanie(database=client["kiwi"], skip_indexes=True, document_models=[
        VaultItem, PrizeCode, Giveaway, GiveawayEntry, SiteUser, User, OutboxEmail])


LOOP = asyncio.new_event_loop()
LOOP.run_until_complete(_init())

# --- ASGI ---------------------------------------------------------------------


async def call(method: str, url: str, headers: list, body: bytes):
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

    try:
        await app(scope, receive, send)
    except Exception as exc:  # noqa: BLE001 - ServerErrorMiddleware re-raises after its 500
        if not sent:
            return 500, [], type(exc).__name__.encode()
    start = next(m for m in sent if m["type"] == "http.response.start")
    raw = b"".join(m.get("body", b"") for m in sent if m["type"] == "http.response.body")
    return start["status"], [(k.decode("latin-1").lower(), v.decode("latin-1")) for k, v in start["headers"]], raw


class FakeAdmin:
    id = ObjectId("65a1b2c3d4e5f60718293a4b")
    email = "master@example.test"
    is_superuser = True


CURRENT_USER = [None]
app.dependency_overrides[deps.get_current_superuser] = lambda: FakeAdmin()
app.dependency_overrides[get_current_site_user] = lambda: CURRENT_USER[0]
app.dependency_overrides[gw_router._GW_PUBLIC.dependency] = lambda: None

# --- request bodies -------------------------------------------------------------

DATETIMES = [
    '"2026-10-22T11:00:00Z"', '"2026-10-22T11:00:00.000Z"', '"2026-10-22T11:00:00.123456789Z"',
    '"2026-10-22T11:00:00"', '"2026-10-22 11:00:00+02:00"', '"2026-10-22T11:00:00-05:30"',
    '"2026-10-22T11:00:00+0530"', '"2026-10-22T11:00Z"', '"2026-10-22t11:00:00z"', '"2026-10-22_11:00:00"',
    '"2026-10-22"', '"20261022"', '"1792665676"', '"1792665676.5"', '"1792665676123"', '"-1"',
    '1792665676', '1792665676123', '1792665676.25', '-1', '0', '1e20', '20000000000', 'true', 'false', 'null',
    '[]', '{}', '""', '"nope"', '"2026-13-01T00:00:00"', '"2026-02-30"', '"2026-02-29T00:00:00"',
    '"2026-10-22T24:00:00"', '"2026-10-22T11:60:00"', '"2026-10-22T11:00:60"', '"2026-10-22T11:00:00+24:00"',
    '"2026-10-22T11:00:00+23:59"', '"0000-01-01T00:00:00"', '"0000-01-01"', '"0001-01-01T00:00:00+01:00"',
    '"9999-12-31T23:59:59-01:00"', '"0001-01-01T00:00:00-01:00"', '"9999-12-31T23:59:59.999999Z"',
    '"2026-10-22T11:00:00."', '"2026-10-22T11:00:00,5Z"', '"2026-10-22T11:00:00 "', '" 2026-10-22T11:00:00"',
    '"2026-10-22T11"', '"1e3"', '"\\u0661\\u0662\\u0663"', '123456789012345678901234567890', '-62135596800',
    '-62135596801', '253402300800', '1.5', '-1.5', '"inf"', '"nan"', '"2026-10-22T11:00:00.5+01:00"',
    '"2026-10-22T11:00:00-00:00"', '"2026-10-22T11:00:00+00:00:30"', '"2026-1-2"', '"2026-10-22T1:00:00"',
]

T120 = "n" * 121
CREATE = [
    '{"title":"T","starts_at":"2026-10-22T11:00:00.000Z","ends_at":"2026-10-29T11:00:00.000Z","vault_item_id":"65a000000000000000000001"}',
    '{"title":"","description":null,"starts_at":"2026-10-22T11:00:00Z","ends_at":"x","vault_item_id":5}',
    '{"starts_at":"0001-01-01T00:00:00+01:00"}',
    '{"title":"T","starts_at":"2026-10-22T11:00:00Z","ends_at":"9999-12-31T23:59:59-01:00","vault_item_id":"a"}',
    '{"title":"' + "t" * 161 + '","description":"' + "d" * 4001 + '","starts_at":1,"ends_at":2,"vault_item_id":"x","extra":1}',
    '{"title":"\\u00e9t\\u00e9","description":"","starts_at":"2026-10-22","ends_at":"2026-10-23","vault_item_id":"x"}',
    '{}', '[]', '"str"', '{"title":', '', 'null', '{"title":"T","starts_at":null,"ends_at":[],"vault_item_id":null}',
]
UPDATE = ['{}', '{"title":null,"description":null,"starts_at":null,"ends_at":null,"vault_item_id":null}',
          '{"title":""}', '{"title":"' + "t" * 160 + '"}', '{"vault_item_id":7}', '{"description":"' + "d" * 4001 + '"}']
UPDATE += ['{"starts_at":' + v + '}' for v in DATETIMES]
UPDATE += ['{"starts_at":"bad","ends_at":"0001-01-01T00:00:00+01:00","title":""}']
ITEM_CREATE = ['{"name":"Radiant"}', '{"name":""}', '{"name":"' + T120 + '"}', '{"name":"x","description":"' + "d" * 2001 + '"}',
               '{"description":"x"}', '{"name":null}', '{"name":"ok","description":null}', '{"name":["a"]}']
ITEM_UPDATE = ['{}', '{"name":null}', '{"name":""}', '{"name":"' + T120 + '"}', '{"description":""}', '{"description":3}']
CODES_ADD = ['{"codes":[]}', '{"codes":["a",null,3]}', '{"codes":"abc"}', '{"codes":["  a ","a","","x"]}', '{}',
             '{"codes":null}', '{"codes":[["a"]]}']
CODE_UPDATE = ['{"code":""}', '{"code":"' + "x" * 401 + '"}', '{"code":5}', '{"code":"ok"}', '{}']

BODY_ROUTES = [
    ("POST", "/admin/giveaways", "create", CREATE),
    ("PATCH", "/admin/giveaways/65a000000000000000000001", "update", UPDATE),
    ("POST", "/admin/vault/items", "create_item", ITEM_CREATE),
    ("PATCH", "/admin/vault/items/65a000000000000000000001", "update_item", ITEM_UPDATE),
    ("POST", "/admin/vault/items/65a000000000000000000001/codes", "add_codes", CODES_ADD),
    ("PATCH", "/admin/vault/codes/65a000000000000000000001", "update_code", CODE_UPDATE),
]

CAPTURED = [None]


def _jsonable(req):
    out = {}
    for k, v in req.__dict__.items():
        out[k] = v.isoformat() if isinstance(v, dt.datetime) else v
    return out


def _stub(*args):
    CAPTURED[0] = _jsonable(args[-1])
    raise APIError(400, ErrorCode.bad_request, "captured")


async def bodies_section() -> list:
    saved = {name: getattr(service, name) for _, _, name, _ in BODY_ROUTES}
    for _, _, name, _ in BODY_ROUTES:
        setattr(service, name, _stub)
    out = []
    n = 0
    for method, path, name, cases in BODY_ROUTES:
        for body in cases:
            n += 1
            rid = f"gw-body-{n:03d}"
            CAPTURED[0] = None
            headers = [(b"x-request-id", rid.encode()), (b"content-type", b"application/json")]
            status, _h, raw = await call(method, path, headers, body.encode())
            out.append({"route": name, "body": body, "request_id": rid, "status": status,
                        "response": raw.decode("utf-8"), "parsed": CAPTURED[0]})
    for ctype, body in (("text/plain", '{"name":"x"}'), ("application/vnd.api+json", '{"name":"x"}'), (None, '{"name":"x"}')):
        n += 1
        rid = f"gw-body-{n:03d}"
        CAPTURED[0] = None
        headers = [(b"x-request-id", rid.encode())] + ([(b"content-type", ctype.encode())] if ctype else [])
        status, _h, raw = await call("POST", "/admin/vault/items", headers, body.encode())
        out.append({"route": "create_item", "body": body, "content_type": ctype, "request_id": rid, "status": status,
                    "response": raw.decode("utf-8"), "parsed": CAPTURED[0]})
    for name, fn in saved.items():
        setattr(service, name, fn)
    return out


# --- scenario ---------------------------------------------------------------------

H = dt.timedelta(hours=1)
D = dt.timedelta(days=1)


def iso(t):
    return t.isoformat().replace("+00:00", "Z")


STEPS: list = []
OUTPUT: list = []
IDS: dict = {}


def step(op, **kw):
    STEPS.append({"op": op, **kw})


async def seed():
    users = [
        SiteUser(username="alice", discord_handle="alice", display_name="Alice", notify_email="Alice@Example.com",
                 is_verified=True, discord_id=1001),
        SiteUser(username="bob", discord_handle="bob", display_name=None, discord_id=1002),
        SiteUser(username="carol", discord_handle="carol", display_name="", notify_email="carol@example.com",
                 discord_id=1003),
    ]
    for u in users:
        await u.insert()
    await User(email="carol@example.com", hashed_password="x", email_bounced=True).insert()
    return users


def _sub(v):
    if isinstance(v, str):
        for k, val in IDS.items():
            v = v.replace("{" + k + "}", val)
        return v
    if isinstance(v, dict):
        return {k: _sub(x) for k, x in v.items()}
    if isinstance(v, list):
        return [_sub(x) for x in v]
    return v


ROUTES = {
    "list_items": ("GET", "/admin/vault/items"),
    "create_item": ("POST", "/admin/vault/items"),
    "update_item": ("PATCH", "/admin/vault/items/{id}"),
    "delete_item": ("DELETE", "/admin/vault/items/{id}"),
    "list_codes": ("GET", "/admin/vault/items/{id}/codes"),
    "add_codes": ("POST", "/admin/vault/items/{id}/codes"),
    "update_code": ("PATCH", "/admin/vault/codes/{id}"),
    "delete_code": ("DELETE", "/admin/vault/codes/{id}"),
    "list_admin": ("GET", "/admin/giveaways"),
    "create": ("POST", "/admin/giveaways"),
    "update": ("PATCH", "/admin/giveaways/{id}"),
    "draw_now": ("POST", "/admin/giveaways/{id}/draw"),
    "cancel": ("POST", "/admin/giveaways/{id}/cancel"),
    "mine": ("GET", "/v1/giveaways/mine"),
    "me": ("GET", "/v1/giveaways/me"),
    "enter": ("POST", "/v1/giveaways/{id}/enter"),
    "ongoing": ("GET", "/v1/giveaways/ongoing"),
    "upcoming": ("GET", "/v1/giveaways/upcoming"),
    "ended": ("GET", "/v1/giveaways/ended"),
}


async def run_steps(users):
    by_name = {u.username: u for u in users}
    for i, s in enumerate(STEPS):
        s = dict(s)
        if s["op"] == "clock":
            CLOCK[0] = dt.datetime.fromisoformat(s["now"])
            OUTPUT.append(s)
            continue
        if s["op"] == "run_due":
            await service.run_due()
            OUTPUT.append(s)
            continue
        if s["op"] == "list_public":
            items = [g.model_dump(mode="json") for g in await service.list_public()]
            OUTPUT.append({**s, "status": 200, "response": json.dumps(
                {"items": items}, ensure_ascii=False, separators=(",", ":"))})
            continue
        method, path = ROUTES[s["op"]]
        target = _sub(s.get("id", ""))
        path = path.replace("{id}", urllib.parse.quote(target, safe=""))
        if s.get("query"):
            path += "?" + s["query"]
        CURRENT_USER[0] = by_name.get(s.get("user"))
        if CURRENT_USER[0] is not None:
            CURRENT_USER[0] = await SiteUser.get(CURRENT_USER[0].id)
        body = s.get("body")
        raw = b"" if body is None else json.dumps(_sub(body)).encode()
        rid = f"gw-step-{i:03d}"
        headers = [(b"x-request-id", rid.encode())] + ([(b"content-type", b"application/json")] if body is not None else [])
        status, rh, out = await call(method, path, headers, raw)
        rec = {**s, "id": target, "body": None if body is None else _sub(body), "request_id": rid,
               "status": status, "response": out.decode("utf-8"),
               "cache_control": next((v for k, v in rh if k == "cache-control"), None)}
        OUTPUT.append(rec)
        if s.get("save") and status < 300:
            IDS[s["save"]] = json.loads(out)["id"]
        if s.get("save_codes"):
            for name, idx in s["save_codes"].items():
                IDS[name] = json.loads(out)[idx]["id"]


def build_steps():
    now = CLOCK[0]
    step("clock", now=now.isoformat())
    step("create_item", body={"name": "Radiant Mount", "description": "A shiny mount"}, save="A")
    step("create_item", body={"name": "Dragon Egg"}, save="B")
    step("create_item", body={"name": "Empty Drawer", "description": None}, save="C")
    step("list_items")
    step("add_codes", id="{A}", body={"codes": ["  AAA-111 ", "AAA-111", "", "BBB-222", "\u3000CCC-333\u3000",
                                                "x" * 401, "DDD-444", "EEE-555", "\x1cFFF-000\x1f"]})
    step("add_codes", id="{A}", body={"codes": ["BBB-222", "GGG-666"]})
    step("add_codes", id="{B}", body={"codes": ["EGG-1", "EGG-2"]})
    step("add_codes", id="{C}", body={"codes": ["   ", ""]})
    step("add_codes", id="65f0ffffffffffffffffffff", body={"codes": ["x"]})
    step("add_codes", id="nope", body={"codes": ["x"]})
    step("list_codes", id="{A}", save_codes={"DDD": 3, "EEE": 2})
    step("list_codes", id="65f0ffffffffffffffffffff")
    step("update_code", id="{EEE}", body={"code": "EEE-555-fixed"})
    step("delete_code", id="{DDD}")
    step("delete_code", id="{DDD}")
    step("update_code", id="65F0FFFFFFFFFFFFFFFFFFFF", body={"code": "x"})
    step("update_item", id="{B}", body={"description": "Hatches soon"})
    step("update_item", id="{A}", body={"name": "Radiant Mount II", "description": None})
    step("update_item", id="65f0ffffffffffffffffffff", body={})
    step("update_item", id="bad id", body={})
    step("list_items")
    win = lambda a, b: {"starts_at": iso(now + a), "ends_at": iso(now + b)}  # noqa: E731
    step("create", body={"title": "Bad window", "vault_item_id": "{A}", **win(D, D)})
    step("create", body={"title": "Past", "vault_item_id": "{A}", **win(-3 * D, -H)})
    step("create", body={"title": "No drawer", "vault_item_id": "65f0ffffffffffffffffffff", **win(-H, D)})
    step("create", body={"title": "Empty", "vault_item_id": "{C}", **win(-H, D)})
    step("create", body={"title": "Bad id", "vault_item_id": "zz", **win(-H, D)})
    step("create", body={"title": "Weekly Radiant", "vault_item_id": "{A}", **win(-H, 2 * D)}, save="G1")
    step("create", body={"title": "Egg Hunt", "description": "Custom desc", "vault_item_id": "{B}",
                         "starts_at": "2026-10-23T11:00:00.250+00:00", "ends_at": "2026-10-25T06:00:00-05:00"}, save="G2")
    step("create", body={"title": "Flash", "description": "", "vault_item_id": "{A}", **win(-2 * H, H)}, save="G3")
    step("create", body={"title": "To cancel", "vault_item_id": "{A}", **win(-H, D)}, save="G4")
    step("create", body={"title": "Soon", "vault_item_id": "{A}", "starts_at": iso(now + H / 2),
                         "ends_at": "2026-10-30"}, save="G5")
    step("create", body={"title": "Lonely", "vault_item_id": "{A}", **win(-H, H + H / 2)}, save="G6")
    step("list_admin")
    step("update", id="{G2}", body={"title": "Egg Hunt Deluxe", "vault_item_id": "{A}"})
    step("update", id="{G2}", body={"starts_at": iso(now - H / 2)})
    step("update", id="{G2}", body={"ends_at": iso(now - H)})
    step("update", id="{G2}", body={"vault_item_id": "{C}"})
    step("update", id="{G2}", body={"vault_item_id": "65f0ffffffffffffffffffff"})
    step("update", id="65f0ffffffffffffffffffff", body={})
    step("enter", id="{G1}", user="alice")
    step("enter", id="{G1}", user="alice")
    step("enter", id="{G1}", user="bob")
    step("enter", id="{G3}", user="carol")
    step("enter", id="{G2}", user="alice")
    step("enter", id="{G5}", user="alice")
    step("enter", id="65f0ffffffffffffffffffff", user="alice")
    step("enter", id="0000", user="alice")
    step("cancel", id="{G4}")
    step("cancel", id="{G4}")
    step("draw_now", id="{G4}")
    step("update", id="{G4}", body={"title": "x"})
    step("enter", id="{G4}", user="bob")
    step("draw_now", id="{G1}")
    step("cancel", id="{G1}")
    step("mine", user="alice")
    step("me", user="alice")
    step("me", user="bob")
    step("mine", user="carol")
    step("ongoing")
    step("upcoming")
    step("ended")
    step("list_public")
    step("clock", now=(now + 2 * H).isoformat())
    step("run_due")
    step("ongoing")
    step("upcoming")
    step("ended", query="days=1")
    step("list_public")
    step("me", user="carol")
    step("me", user="alice")
    step("list_codes", id="{A}")
    step("delete_item", id="{A}")
    step("delete_item", id="{C}")
    step("delete_item", id="{B}")
    step("delete_item", id="{B}")
    step("list_items")
    step("list_admin")
    step("clock", now=(now + 10 * D).isoformat())
    step("ended")
    step("ended", query="days=30")
    step("list_public")


async def scenario_section() -> dict:
    settings.smtp_host = "smtp.example.test"
    users = await seed()
    seeded = {k: ext(v) for k, v in DB.items()}
    build_steps()
    await run_steps(users)
    settings.smtp_host = None
    return {"seed": seeded, "next_id": 0, "steps": OUTPUT, "final": {k: ext(v) for k, v in DB.items()}}


GATED = [
    ("PUT", "/admin/vault/items", None, None), ("GET", "/admin/vault/items/x", None, None),
    ("DELETE", "/admin/vault/items/x/codes", None, None), ("GET", "/admin/vault/codes/x", None, None),
    ("DELETE", "/admin/giveaways", None, None), ("GET", "/admin/giveaways/x", None, None),
    ("GET", "/admin/giveaways/x/draw", None, None), ("PUT", "/admin/giveaways/x/cancel", None, None),
    ("POST", "/v1/giveaways/mine", None, None), ("DELETE", "/v1/giveaways/me", None, None),
    ("GET", "/v1/giveaways/x/enter", None, None), ("POST", "/v1/giveaways/ongoing", None, None),
    ("PATCH", "/v1/giveaways/upcoming", None, None), ("DELETE", "/v1/giveaways/ended", None, None),
    ("GET", "/admin/giveaways", None, None), ("GET", "/admin/vault/items", "Bearer nope", None),
    ("POST", "/admin/giveaways", None, '{"title":'), ("POST", "/admin/giveaways", None, '{"title":""}'),
    ("POST", "/admin/vault/items/x/codes", "Basic abc", '{"codes":[]}'), ("DELETE", "/admin/vault/codes/x", None, None),
    ("PATCH", "/admin/giveaways/x", "Bearer a.b.c", "[1,"), ("POST", "/admin/giveaways/x/draw", None, None),
]


async def gated_section() -> list:
    app.dependency_overrides.clear()
    out = []
    for i, (method, path, auth, body) in enumerate(GATED):
        rid = f"gw-gate-{i:03d}"
        headers = [(b"x-request-id", rid.encode())]
        if auth:
            headers.append((b"authorization", auth.encode()))
        if body is not None:
            headers.append((b"content-type", b"application/json"))
        status, rh, raw = await call(method, path, headers, (body or "").encode())
        get = lambda k: next((v for n, v in rh if n == k), None)  # noqa: E731
        out.append({"method": method, "path": path, "auth": auth, "body": body, "request_id": rid, "status": status,
                    "headers": {k: get(k) for k in ("content-type", "allow", "www-authenticate") if get(k) is not None},
                    "response": raw.decode("utf-8")})
    return out


def oids_section() -> list:
    out = []
    for v in ["65f000000000000000000001", "65F0000000000000000000AB", "65f00000000000000000000", "65f0000000000000000000011",
              "00000000000000000000 00 ", "0000000000000000000000 0", " 000000000000000000000000"[:24], "zzzzzzzzzzzzzzzzzzzzzzzz",
              "\u0660" * 24, "", "0" * 12, "0 0" + "0" * 21, "\t" + "0" * 22 + "\n"]:
        try:
            o = service._oid(v)
            out.append({"value": v, "ok": o.binary.hex()})
        except APIError as e:
            out.append({"value": v, "status": e.status_code, "message": e.message})
    return out


def main():
    bodies = LOOP.run_until_complete(bodies_section())
    scenario = LOOP.run_until_complete(scenario_section())
    gated = LOOP.run_until_complete(gated_section())
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps({"bodies": bodies, "scenario": scenario, "gated": gated, "oids": oids_section()},
                              indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {OUT} ({len(bodies)} bodies, {len(scenario['steps'])} steps)")


if __name__ == "__main__":
    main()
