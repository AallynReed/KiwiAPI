"""Golden fixtures for kiwi-server's Mods Hub write auth, creator links and
issues, from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_mods_creators_issues_fixtures.py
Writes native/kiwi-server/testdata/mods_creators_issues/golden.json.

Route cases drive app.main.app at the ASGI level over an in-memory pymongo
layer (filters, sorts, update operators as MongoDB 7 applies them), with the
caller as a dependency override; "gated" cases (401/405/422 before any store)
run without one. Write-auth cases call write_auth._authorize_api with the
token already resolved. The clock and the minted creator token are fixed.
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
ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "mods_creators_issues" / "golden.json"
os.chdir(ROOT)
sys.path.insert(0, str(ROOT))
os.environ.setdefault("SECRET_KEY", "mods-fixture-secret")

import bson  # noqa: E402
from bson import ObjectId, json_util  # noqa: E402
from bson.codec_options import CodecOptions  # noqa: E402
from bson.json_util import CANONICAL_JSON_OPTIONS  # noqa: E402
from pymongo.asynchronous.collection import AsyncCollection  # noqa: E402
from pymongo.asynchronous.database import AsyncDatabase  # noqa: E402

from app.admin import runtime_config as rc  # noqa: E402

for _key, _spec in rc.REGISTRY.items():
    rc._CACHE[_key] = (_spec.default, float("inf"))

import app.core.utils as cu  # noqa: E402

NOW = dt.datetime(2026, 10, 22, 9, 30, 15, 123456, tzinfo=dt.timezone.utc)


class _Frozen(dt.datetime):
    @classmethod
    def now(cls, tz=None):
        return NOW


cu.datetime = _Frozen

from beanie.odm.utils.dump import get_dict  # noqa: E402
from beanie.odm.utils.parsing import parse_obj  # noqa: E402

from app.auth.models import User  # noqa: E402
from app.core.dependencies import get_current_user  # noqa: E402
from app.core.errors import APIError  # noqa: E402
from app.core.ratelimit import RateLimitInfo, _too_many  # noqa: E402
from app.core.security import hash_token  # noqa: E402
from app.main import app  # noqa: E402
from app.site_auth.dependencies import get_current_site_user, get_optional_site_user  # noqa: E402
from app.site_auth.models import SiteUser  # noqa: E402
from app.tokens.models import ApiToken  # noqa: E402
from app.trove.mods_hub import creators, issues, write_auth  # noqa: E402
from app.trove.mods_hub import router as hub_router  # noqa: E402
from app.trove.mods_hub.models import (  # noqa: E402
    Collaborator,
    ModCreatorLink,
    ModIssue,
    ModIssueEvent,
    ModProject,
    ModRelease,
)

OPTS = CodecOptions(tz_aware=True)
MINTED = "Zq3vN0xR8kL2mP5sT7wY9aB1cD4eF6gH"
creators.secrets = types.SimpleNamespace(token_urlsafe=lambda n: MINTED)


def ext(v):
    return json.loads(json_util.dumps(v, json_options=CANONICAL_JSON_OPTIONS))


def rt(doc):
    return bson.decode(bson.encode(doc), codec_options=OPTS)


# --- in-memory pymongo -----------------------------------------------------------------

DB: dict[str, list[dict]] = {}
_IDS = [0]
_MISSING = object()


def _same(a, b):
    if isinstance(a, bool) or isinstance(b, bool):
        return isinstance(a, bool) and isinstance(b, bool) and a == b
    return a == b


def _eq(v, want):
    if isinstance(v, list):
        return any(_same(x, want) for x in v) or v == want
    if v is _MISSING or v is None:
        return want is None
    return _same(v, want)


def _field(v, cond):
    if isinstance(cond, dict) and cond and all(str(k).startswith("$") for k in cond):
        for op, arg in cond.items():
            if op == "$in":
                if not any(_eq(v, a) for a in arg):
                    return False
            elif op == "$ne":
                if _eq(v, arg):
                    return False
            else:
                raise NotImplementedError(op)
        return True
    return _eq(v, cond)


def _match(doc, flt):
    for k, cond in (flt or {}).items():
        if k == "$and":
            if not all(_match(doc, c) for c in cond):
                return False
        elif not _field(doc.get(k, _MISSING), cond):
            return False
    return True


def _rank(v):
    if v is _MISSING or v is None:
        return (0, 0)
    if isinstance(v, bool):
        return (7, v)
    if isinstance(v, (int, float)):
        return (1, v)
    if isinstance(v, str):
        return (2, v)
    if isinstance(v, ObjectId):
        return (6, v.binary)
    if isinstance(v, dt.datetime):
        return (8, v)
    raise NotImplementedError(type(v))


def _encodable(**kw):
    bson.encode({k: v for k, v in kw.items() if v is not None})


class _Cursor:
    def __init__(self, coll, flt, kw):
        self.coll, self.flt, self.kw = coll, flt, kw

    async def to_list(self, length=None):
        _encodable(filter=self.flt, skip=self.kw.get("skip"), limit=self.kw.get("limit"))
        docs = [d for d in DB.get(self.coll, []) if _match(d, self.flt)]
        for key, direction in reversed(list(self.kw.get("sort") or [])):
            docs.sort(key=lambda d: _rank(d.get(key, _MISSING)), reverse=direction < 0)
        skip, limit = self.kw.get("skip") or 0, self.kw.get("limit") or 0
        docs = docs[skip:]
        if limit:
            docs = docs[:limit]
        return [rt(d) for d in docs]


def _find(self, filter=None, *a, **kw):
    return _Cursor(self.name, filter, kw)


async def _find_one(self, filter=None, *a, **kw):
    _encodable(filter=filter)
    for d in DB.get(self.name, []):
        if _match(d, filter):
            return rt(d)
    return None


async def _count(self, filter, *a, **kw):
    _encodable(filter=filter)
    return sum(1 for d in DB.get(self.name, []) if _match(d, filter))


async def _insert_one(self, doc, *a, **kw):
    _IDS[0] += 1
    oid = ObjectId("66000000000000000000%04x" % _IDS[0])
    DB.setdefault(self.name, []).append(rt({"_id": oid, **doc}))
    return types.SimpleNamespace(inserted_id=oid)


def _apply(doc, update):
    ops = sorted(((k, op, v) for op, fields in update.items() for k, v in fields.items()), key=lambda t: t[0])
    for k, op, v in ops:
        if op == "$set":
            doc[k] = v
        elif op == "$inc":
            doc[k] = (doc.get(k) or 0) + v
        elif op == "$addToSet":
            cur = doc.get(k)
            if not isinstance(cur, list):
                doc[k] = [v]
            elif not any(_same(x, v) for x in cur):
                cur.append(v)
        else:
            raise NotImplementedError(op)


def _update(name, flt, update, many, upsert=False):
    _encodable(filter=flt, update=update)
    out = []
    docs = DB.setdefault(name, [])
    for i, d in enumerate(docs):
        if _match(d, flt):
            new = dict(d)
            _apply(new, update)
            docs[i] = rt(new)
            out.append(docs[i])
            if not many:
                break
    if not out and upsert and "_id" in update.get("$set", {}):
        new = {}
        _apply(new, update)
        docs.append(rt(new))
    return out


async def _update_one(self, flt, update, *a, upsert=False, **kw):
    _update(self.name, flt, update, False, upsert)
    return types.SimpleNamespace(modified_count=1)


async def _update_many(self, flt, update, *a, **kw):
    _update(self.name, flt, update, True)
    return types.SimpleNamespace(modified_count=1)


async def _find_one_and_update(self, flt, update, *a, upsert=False, **kw):
    out = _update(self.name, flt, update, False, upsert)
    return rt(out[0]) if out else None


async def _delete_one(self, flt, *a, **kw):
    docs = DB.get(self.name, [])
    for i, d in enumerate(docs):
        if _match(d, flt):
            del docs[i]
            break
    return types.SimpleNamespace(deleted_count=1)


async def _delete_many(self, flt, *a, **kw):
    DB[self.name] = [d for d in DB.get(self.name, []) if not _match(d, flt)]
    return types.SimpleNamespace(deleted_count=1)


AsyncCollection.find = _find
AsyncCollection.find_one = _find_one
AsyncCollection.count_documents = _count
AsyncCollection.insert_one = _insert_one
AsyncCollection.update_one = _update_one
AsyncCollection.update_many = _update_many
AsyncCollection.find_one_and_update = _find_one_and_update
AsyncCollection.delete_one = _delete_one
AsyncCollection.delete_many = _delete_many


async def _cmd(self, *a, **k):
    return {"version": "7.0.0"}


async def _lcn(self, *a, **k):
    return []


AsyncDatabase.command = _cmd
AsyncDatabase.list_collection_names = _lcn

MODELS = [SiteUser, User, ApiToken, ModProject, ModRelease, ModIssue, ModIssueEvent, ModCreatorLink]


async def _init():
    from beanie import init_beanie
    from pymongo import AsyncMongoClient
    client = AsyncMongoClient("mongodb://127.0.0.1:1", tz_aware=True, connect=False, serverSelectionTimeoutMS=200)
    await init_beanie(database=client["kiwi"], skip_indexes=True, document_models=MODELS)


LOOP = asyncio.new_event_loop()
LOOP.run_until_complete(_init())
run = LOOP.run_until_complete

# --- seed ------------------------------------------------------------------------------

T0 = dt.datetime(2026, 9, 1, 12, 0, 0, 250000, tzinfo=dt.timezone.utc)
T1 = dt.datetime(2026, 9, 2, 8, 15, 0, tzinfo=dt.timezone.utc)
T2 = dt.datetime(2026, 9, 5, 18, 0, 0, 5000, tzinfo=dt.timezone.utc)
T3 = dt.datetime(2026, 9, 9, 7, 45, 30, tzinfo=dt.timezone.utc)

O = lambda s: ObjectId(s)  # noqa: E731
CREATOR, CREATOR2, PLAYER, COLLAB = O("65f0000000000000000000a1"), O("65f0000000000000000000a2"), O("65f0000000000000000000a3"), O("65f0000000000000000000a4")
MASTER, DEAD, DELETED, FRESH, LEGACY = O("65f0000000000000000000a5"), O("65f0000000000000000000a6"), O("65f0000000000000000000a7"), O("65f0000000000000000000a8"), O("65f0000000000000000000a9")
DEV, DEV2, DEV3, DEV4 = O("65e000000000000000000001"), O("65e000000000000000000002"), O("65e000000000000000000003"), O("65e000000000000000000004")
P = {k: O("65d0000000000000000000%02x" % i) for i, k in enumerate(
    ["alpha", "beta", "gamma", "delta", "stray", "taken", "remix_old", "remix_new", "web", "hidden"], start=1)}
R1, R2 = O("65c000000000000000000001"), O("65c000000000000000000002")
I = {k: O("65b0000000000000000000%02x" % i) for i, k in enumerate(["i1", "i2", "i3", "i4", "i5", "i6"], start=1)}
E = {k: O("65a0000000000000000000%02x" % i) for i, k in enumerate(["e1", "e2", "e3", "e4"], start=1)}
L = {k: O("6590000000000000000000%02x" % i) for i, k in enumerate(["l1", "l2", "l3", "l4", "l5", "l6", "l7"], start=1)}
TOKENS = {"one": "kiwi_creator_OneOneOneOneOneOneOneOneOneOneOne", "two": "kiwi_creator_TwoTwoTwoTwoTwoTwoTwoTwoTwoTwoTwo",
          "dead": "kiwi_creator_DeadDeadDeadDeadDeadDeadDeadDead", "del": "kiwi_creator_GoneGoneGoneGoneGoneGoneGoneGone"}


def bdoc(model, oid):
    return rt({"_id": oid, **get_dict(model, to_db=True)})


def site_user(oid, username, token=None, **kw):
    if token:
        kw.update(creator_token_hash=hash_token(TOKENS[token]), creator_token_prefix=TOKENS[token][:21],
                  creator_token_at=T0)
    return bdoc(SiteUser(username=username, discord_handle=username, is_verified=True, created_at=T0, updated_at=T0,
                         **kw), oid)


def project(key, owner, slug, title, handle, **kw):
    return bdoc(ModProject(slug=slug, title=title, owner_id=owner, owner_username=handle, owner_handle=handle,
                           created_at=T0, updated_at=T0, **kw), P[key])


def issue(key, pkey, number, author, **kw):
    pr = {"alpha": ("alpha-mod", "creator_one", "Alpha Mod"), "taken": ("taken-mod", "creator_one", "Taken Mod"),
          "beta": ("beta-mod", "creator_one", "beta"), "gamma": ("gamma-mod", "creator_one", "Gamma Mod")}[pkey]
    kw.setdefault("created_at", T0)
    kw.setdefault("updated_at", kw["created_at"])
    kw.setdefault("last_activity_at", kw["updated_at"])
    return bdoc(ModIssue(project_id=P[pkey], project_slug=pr[0], project_handle=pr[1], project_title=pr[2],
                         number=number, author_id=author, **kw), I[key])


def event(key, ikey, pkey, author, name, **kw):
    return bdoc(ModIssueEvent(issue_id=I[ikey], project_id=P[pkey], author_id=author, author_username=name, **kw), E[key])


def link(key, site_user_id, api_user_id, **kw):
    kw.setdefault("created_at", T0)
    return bdoc(ModCreatorLink(site_user_id=site_user_id, api_user_id=api_user_id, **kw), L[key])


def base_db():
    return {
        "site_users": [
            site_user(CREATOR, "creator_one", "one", display_name="Creator One"),
            site_user(CREATOR2, "creator_two", "two"),
            site_user(PLAYER, "player_x", display_name="Player X", mod_issues_seen_at=T1),
            site_user(COLLAB, "collab_y"),
            site_user(MASTER, "aallyn", display_name="Master"),
            site_user(DEAD, "gone_creator", "dead", is_active=False),
            site_user(DELETED, "deleted_creator", "del", is_deleted=True),
            site_user(FRESH, "fresh_user", display_name=""),
            rt({"_id": LEGACY, "username": "legacy_user", "is_active": True, "discord_id": 4194305,
                "token_version": 0, "created_at": T0, "updated_at": T0}),
        ],
        "users": [bdoc(User(email=f"dev{i}@example.com", hashed_password="x", created_at=T0, updated_at=T0), oid)
                  for i, oid in enumerate([DEV, DEV2, DEV3, DEV4], start=1)],
        "mod_projects": [
            project("alpha", CREATOR, "alpha-mod", "Alpha Mod", "creator_one", visibility="public", issue_seq=3,
                    open_issue_count=2, collaborators=[Collaborator(user_id=COLLAB, username="collab_y")]),
            project("beta", CREATOR, "beta-mod", "beta", "creator_one", issue_seq=1, open_issue_count=1),
            project("gamma", CREATOR, "gamma-mod", "Gamma Mod", "creator_one", visibility="public",
                    issues_enabled=False, issue_seq=1, open_issue_count=1),
            project("delta", CREATOR2, "delta-mod", "Delta", "creator_two", visibility="public"),
            project("stray", None, "stray-mod", "Stray Mod", "", visibility="public", is_stray=True),
            project("taken", CREATOR, "taken-mod", "Taken Mod", "creator_one", visibility="public", taken_down=True,
                    issue_seq=1, open_issue_count=1),
            project("remix_old", CREATOR2, "remix-old", "Remix Old", "creator_two", visibility="public", is_remix=True,
                    remix_requested_at=NOW - dt.timedelta(days=8)),
            project("remix_new", CREATOR2, "remix-new", "Remix New", "creator_two", visibility="public", is_remix=True,
                    remix_requested_at=NOW - dt.timedelta(days=1)),
            project("web", CREATOR2, "web-only", "Web Only", "creator_two", visibility="public_website_only"),
            project("hidden", CREATOR, "hidden-mod", "Ännchen", "creator_one", visibility="hidden"),
        ],
        "mod_releases": [
            bdoc(ModRelease(project_id=P["alpha"], owner_id=CREATOR, tag="v1", tmod_sha="a" * 64, created_at=T0,
                            updated_at=T0), R1),
            bdoc(ModRelease(project_id=O("65d0000000000000000000ff"), tag="v1", tmod_sha="b" * 64, created_at=T0,
                            updated_at=T0), R2),
        ],
        "mod_issues": [
            issue("i1", "alpha", 1, PLAYER, title="Crash on load", body="It **crashes**.", author_username="Player X",
                  comment_count=2, participant_ids=[COLLAB, CREATOR, PLAYER], updated_at=T2,
                  last_activity_by=PLAYER),
            issue("i2", "alpha", 2, PLAYER, kind="request", title="Add a hat", author_username="Player X",
                  status="closed", closed_at=T3, closed_by_id=PLAYER, comment_count=1,
                  participant_ids=[COLLAB, CREATOR, PLAYER], updated_at=T3, last_activity_by=CREATOR),
            issue("i3", "alpha", 3, COLLAB, title="Tidy the textures", author_username="collab_y",
                  participant_ids=[COLLAB, CREATOR], updated_at=T1, last_activity_by=COLLAB),
            issue("i4", "taken", 1, PLAYER, title="Why gone?", author_username="Player X",
                  participant_ids=[CREATOR, PLAYER], last_activity_by=PLAYER),
            issue("i5", "beta", 1, CREATOR, title="Draft todo", author_username="Creator One",
                  participant_ids=[CREATOR], last_activity_by=CREATOR),
            issue("i6", "gamma", 1, PLAYER, title="Old thread", author_username="Player X",
                  participant_ids=[CREATOR, PLAYER], last_activity_by=PLAYER),
        ],
        "mod_issue_events": [
            event("e1", "i1", "alpha", PLAYER, "Player X", body="Logs attached.", created_at=T1),
            event("e2", "i2", "alpha", CREATOR, "Creator One", body="Shipped in v2.", created_at=T2),
            event("e3", "i2", "alpha", PLAYER, "Player X", kind="closed", created_at=T3),
            event("e4", "i1", "alpha", COLLAB, "collab_y", body="Can't reproduce.", created_at=T2),
        ],
        "mod_creator_links": [
            link("l1", CREATOR, DEV, label="release CI", last_used_at=T1, request_count=4),
            link("l2", CREATOR, DEV2, label="", all_projects=False, project_ids=[P["gamma"], P["alpha"]], created_at=T1),
            link("l3", CREATOR2, DEV, label="old", revoked=True, revoked_at=T1, revoked_by_rotation=True),
            link("l4", DEAD, DEV, label="dead one", created_at=T2),
            link("l5", CREATOR, DEV3, revoked=True, revoked_at=T2),
            link("l6", CREATOR2, DEV2, all_projects=False, project_ids=[P["delta"]], created_at=T2),
            link("l7", DELETED, DEV3, created_at=T1),
        ],
    }


COLLS = ["site_users", "users", "mod_projects", "mod_releases", "mod_issues", "mod_issue_events", "mod_creator_links"]


def snap():
    out = {}
    for c in COLLS:
        docs = ext(DB.get(c, []))
        for d in docs:
            if isinstance(d.get("participant_ids"), list):
                d["participant_ids"].sort(key=lambda x: x["$oid"])
        out[c] = docs
    return out


DBS: dict[str, dict] = {}
_DB_INDEX: dict[str, str] = {}


def register(before):
    """The name of this starting database, stored once in `dbs`."""
    key = json.dumps(before, sort_keys=True)
    if key not in _DB_INDEX:
        name = "base" if not DBS else f"db{len(DBS)}"
        _DB_INDEX[key] = name
        DBS[name] = before
    return _DB_INDEX[key]


def changed(before):
    """The collections a case changed, as they ended up."""
    after = snap()
    return {c: after[c] for c in COLLS if after[c] != before[c]}


def model_of(cls, oid):
    for coll, docs in DB.items():
        for d in docs:
            if d.get("_id") == oid and coll == cls.get_settings().name:
                return parse_obj(cls, rt(d))
    raise KeyError(oid)


# --- ASGI ----------------------------------------------------------------------------

KEPT = ("content-type", "cache-control", "allow", "www-authenticate", "content-length", "retry-after",
        "x-ratelimit-limit", "x-ratelimit-remaining", "x-ratelimit-reset")


async def call(method, url, headers, body: bytes):
    path, _, qs = url.partition("?")
    hdrs = [(b"host", b"testserver")] + [(k.encode(), v.encode()) for k, v in headers]
    hdrs.append((b"content-length", str(len(body)).encode()))
    scope = {
        "type": "http", "asgi": {"version": "3.0"}, "http_version": "1.1", "method": method,
        "scheme": "http", "path": urllib.parse.unquote(path), "raw_path": path.encode(),
        "query_string": qs.encode(), "root_path": "", "headers": hdrs,
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
    start = next(x for x in sent if x["type"] == "http.response.start")
    raw = b"".join(x.get("body", b"") for x in sent if x["type"] == "http.response.body")
    return start["status"], [(k.lower().decode(), v.decode("latin-1")) for k, v in start["headers"]], raw


LIMITS: list = []
DENY: set = set()
EPOCH = dt.datetime(1970, 1, 1, tzinfo=dt.timezone.utc)


async def _fake_limit(key, max_, window):
    LIMITS.append([key, max_, window])
    if key in DENY:
        raise _too_many(RateLimitInfo(limit=1, remaining=0, reset=EPOCH), 3600, EPOCH - dt.timedelta(seconds=1))


issues.check_rate_limit = _fake_limit
REAL_TO_OID = issues.to_oid
PUB = hub_router._PUB.dependency


def http_case(name, method, url, *, body=None, raw=None, site=None, viewer=None, portal=None, write=None, db=None,
              ctype="application/json", flags=None, deny=(), headers=(), python_bug=False, gated=False):
    global DB
    DB = db if db is not None else base_db()
    _IDS[0] = 0
    before = snap()
    LIMITS.clear()
    DENY.clear()
    DENY.update(deny)
    overrides = {}
    if not gated:
        overrides[PUB] = lambda: None
        overrides[get_optional_site_user] = (lambda: model_of(SiteUser, viewer)) if viewer else (lambda: None)
        if site:
            overrides[get_current_site_user] = lambda: model_of(SiteUser, site)
        if write:
            overrides[write_auth.get_mod_write_user] = lambda: model_of(SiteUser, write)
        if portal:
            overrides[get_current_user] = lambda: model_of(User, portal)
    app.dependency_overrides.update(overrides)
    saved = {}
    for k, v in (flags or {}).items():
        saved[k] = rc._CACHE[k]
        rc._CACHE[k] = (v, float("inf"))
    issues.to_oid = REAL_TO_OID if python_bug else (lambda v, *_: REAL_TO_OID(v))
    rid = f"fx-{name}"
    hdrs = [("x-request-id", rid)] + list(headers)
    if body is not None or raw is not None:
        data = raw.encode() if raw is not None else json.dumps(body).encode()
        if ctype:
            hdrs.append(("content-type", ctype))
    else:
        data = b""
    try:
        status, rh, resp = run(call(method, url, hdrs, data))
    finally:
        for k, v in saved.items():
            rc._CACHE[k] = v
        app.dependency_overrides.clear()
        issues.to_oid = REAL_TO_OID
    case = {
        "name": name, "method": method, "url": url, "headers": hdrs,
        "site": str(site) if site else None, "viewer": str(viewer) if viewer else None,
        "portal": str(portal) if portal else None, "write": str(write) if write else None,
        "flags": flags or {}, "deny": sorted(deny), "db": None if gated else register(before),
        "status": status,
        "resp_headers": {k: v for k, v in rh if k in KEPT},
        "resp": resp.decode("utf-8"),
        "after": {} if gated else changed(before),
        "limits": [list(x) for x in LIMITS],
    }
    if body is not None:
        case["body"] = body
    if raw is not None:
        case["raw"] = raw
    return case


def one_of(name, items):
    return [x for x in items if x["name"] == name][0]


# --- cases -----------------------------------------------------------------------------

V = "/v1/mods/hub"
CL = f"{V}/creator-links"
ALPHA = f"{V}/projects/creator_one/alpha-mod/issues"
S_ALPHA = "/site/mods/projects/creator_one/alpha-mod/issues"


def many_links_db(creator=None, dev=None, n=0):
    db = base_db()
    for i in range(n):
        db["mod_creator_links"].append(rt({
            "_id": O("6580000000000000000%05x" % i), "site_user_id": creator or O("6570000000000000000%05x" % i),
            "api_user_id": dev or O("6560000000000000000%05x" % i), "label": "", "all_projects": True,
            "project_ids": [], "revoked": False, "revoked_at": None, "revoked_by_rotation": False,
            "created_at": T0, "last_used_at": None, "request_count": 0}))
    return db


def open_issues_db(n):
    db = base_db()
    for i in range(n):
        db["mod_issues"].append(bdoc(ModIssue(project_id=P["alpha"], number=100 + i, title=f"t{i}", author_id=FRESH,
                                              created_at=T0, updated_at=T0, last_activity_at=T0), O("6550000000000000000%05x" % i)))
    return db


def creator_cases():
    l1, l2, l3, l6 = (str(L[k]) for k in ("l1", "l2", "l3", "l6"))
    c = [
        http_case("cl_list_dev", "GET", CL, portal=DEV),
        http_case("cl_list_dev2", "GET", CL, portal=DEV2),
        http_case("cl_list_none", "GET", CL, portal=DEV4),
        http_case("cl_connect_new", "POST", CL, portal=DEV4, body={"token": f"  {TOKENS['two']}\n", "label": "  My Bot  "}),
        http_case("cl_connect_reactivate", "POST", CL, portal=DEV, body={"token": TOKENS["two"]}),
        http_case("cl_connect_relabel", "POST", CL, portal=DEV, body={"token": TOKENS["one"], "label": "renamed"}),
        http_case("cl_connect_same", "POST", CL, portal=DEV2, body={"token": TOKENS["one"], "label": "   "}),
        http_case("cl_connect_bad", "POST", CL, portal=DEV4, body={"token": "kiwi_creator_nope_nope"}),
        http_case("cl_connect_dead", "POST", CL, portal=DEV4, body={"token": TOKENS["dead"]}),
        http_case("cl_connect_deleted", "POST", CL, portal=DEV4, body={"token": TOKENS["del"]}),
        http_case("cl_connect_blank", "POST", CL, portal=DEV4, body={"token": "        "}),
        http_case("cl_connect_label_60", "POST", CL, portal=DEV4, body={"token": TOKENS["two"], "label": " " + "é" * 59}),
        http_case("cl_connect_creator_full", "POST", CL, portal=DEV4, body={"token": TOKENS["two"]},
                  db=many_links_db(creator=CREATOR2, n=25)),
        http_case("cl_connect_dev_full", "POST", CL, portal=DEV4, body={"token": TOKENS["two"]},
                  db=many_links_db(dev=DEV4, n=50)),
        http_case("cl_connect_dev_full_reconnect", "POST", CL, portal=DEV2, body={"token": TOKENS["two"]},
                  db=many_links_db(dev=DEV2, n=50)),
        http_case("cl_connect_422_short", "POST", CL, portal=DEV4, body={"token": "short", "label": "x" * 61}),
        http_case("cl_connect_422_types", "POST", CL, portal=DEV4, body={"token": 5, "label": None}),
        http_case("cl_connect_422_missing", "POST", CL, portal=DEV4, body={}),
        http_case("cl_connect_422_list", "POST", CL, portal=DEV4, body=[1]),
        http_case("cl_connect_422_empty", "POST", CL, portal=DEV4),
        http_case("cl_connect_422_text", "POST", CL, portal=DEV4, raw="token=x", ctype="text/plain"),
        http_case("cl_delete", "DELETE", f"{CL}/{l1}", portal=DEV),
        http_case("cl_delete_other", "DELETE", f"{CL}/{l2}", portal=DEV),
        http_case("cl_delete_revoked", "DELETE", f"{CL}/{l3}", portal=DEV),
        http_case("cl_delete_badid", "DELETE", f"{CL}/nope", portal=DEV),
        http_case("ct_get", "GET", "/site/mods/creator-token", site=CREATOR),
        http_case("ct_get_fresh", "GET", "/site/mods/creator-token", site=FRESH),
        http_case("ct_get_legacy", "GET", "/site/mods/creator-token", site=LEGACY),
        http_case("ct_get_creator2", "GET", "/site/mods/creator-token", site=CREATOR2),
        http_case("ct_issue", "POST", "/site/mods/creator-token", site=FRESH),
        http_case("ct_issue_legacy", "POST", "/site/mods/creator-token", site=LEGACY),
        http_case("ct_issue_exists", "POST", "/site/mods/creator-token", site=CREATOR),
        http_case("ct_rotate", "POST", "/site/mods/creator-token/rotate", site=CREATOR),
        http_case("ct_rotate_fresh", "POST", "/site/mods/creator-token/rotate", site=FRESH),
        http_case("cc_narrow", "PATCH", f"/site/mods/creator-connections/{l2}", site=CREATOR,
                  body={"all_projects": False, "project_ids": [str(P["gamma"]), "bad", str(P["delta"]), str(P["alpha"]),
                                                               str(P["gamma"]).upper()]}),
        http_case("cc_widen", "PATCH", f"/site/mods/creator-connections/{l2}", site=CREATOR,
                  body={"all_projects": True, "project_ids": [str(P["alpha"])]}),
        http_case("cc_defaults", "PATCH", f"/site/mods/creator-connections/{l2}", site=CREATOR, body={}),
        http_case("cc_lax_bool", "PATCH", f"/site/mods/creator-connections/{l1}", site=CREATOR,
                  body={"all_projects": "no", "project_ids": [str(P["hidden"])]}),
        http_case("cc_none_owned", "PATCH", f"/site/mods/creator-connections/{l1}", site=CREATOR,
                  body={"all_projects": False, "project_ids": ["nope", str(P["delta"])]}),
        http_case("cc_other", "PATCH", f"/site/mods/creator-connections/{l6}", site=CREATOR, body={}),
        http_case("cc_revoked", "PATCH", f"/site/mods/creator-connections/{str(L['l5'])}", site=CREATOR, body={}),
        http_case("cc_badid", "PATCH", "/site/mods/creator-connections/zzz", site=CREATOR, body={}),
        http_case("cc_422", "PATCH", f"/site/mods/creator-connections/{l1}", site=CREATOR,
                  body={"all_projects": "maybe", "project_ids": [1, None]}),
        http_case("cc_422_long", "PATCH", f"/site/mods/creator-connections/{l1}", site=CREATOR,
                  body={"project_ids": ["x"] * 501}),
        http_case("cc_422_empty", "PATCH", f"/site/mods/creator-connections/{l1}", site=CREATOR),
        http_case("cc_revoke", "DELETE", f"/site/mods/creator-connections/{l1}", site=CREATOR),
        http_case("cc_revoke_other", "DELETE", f"/site/mods/creator-connections/{l6}", site=CREATOR),
    ]
    return c


def issue_cases():
    e1, e2, e3, e4 = (str(E[k]) for k in ("e1", "e2", "e3", "e4"))
    c = [
        http_case("is_list", "GET", ALPHA),
        http_case("is_list_all_page", "GET", f"{ALPHA}?status=all&limit=1&offset=1"),
        http_case("is_list_closed", "GET", f"{ALPHA}?status=closed&limit=100"),
        http_case("is_list_all_far", "GET", f"{ALPHA}?status=all&offset=50"),
        http_case("is_list_draft", "GET", f"{V}/projects/creator_one/beta-mod/issues"),
        http_case("is_list_taken", "GET", f"{V}/projects/creator_one/taken-mod/issues"),
        http_case("is_list_hidden", "GET", f"{V}/projects/creator_one/hidden-mod/issues"),
        http_case("is_list_disabled", "GET", f"{V}/projects/creator_one/gamma-mod/issues"),
        http_case("is_list_unknown_handle", "GET", f"{V}/projects/nobody/alpha-mod/issues"),
        http_case("is_list_unknown_slug", "GET", f"{V}/projects/creator_one/nope/issues"),
        http_case("is_list_case_handle", "GET", f"{V}/projects/%20Creator_ONE%20/alpha-mod/issues"),
        http_case("is_list_stray", "GET", f"{V}/projects/STRAY/stray-mod/issues"),
        http_case("is_list_remix_old", "GET", f"{V}/projects/creator_two/remix-old/issues"),
        http_case("is_list_remix_new", "GET", f"{V}/projects/creator_two/remix-new/issues"),
        http_case("is_list_web_only", "GET", f"{V}/projects/creator_two/web-only/issues"),
        http_case("is_list_422", "GET", f"{ALPHA}?status=bogus&limit=0&offset=-1"),
        http_case("is_list_422_big", "GET", f"{ALPHA}?limit=101&offset=x"),
        http_case("is_list_huge_offset", "GET", f"{ALPHA}?offset=99999999999999999999"),
        http_case("is_get", "GET", f"{ALPHA}/1"),
        http_case("is_get_closed", "GET", f"{ALPHA}/2.0"),
        http_case("is_get_spaces", "GET", f"{ALPHA}/%203%20"),
        http_case("is_get_missing", "GET", f"{ALPHA}/9"),
        http_case("is_get_disabled", "GET", f"{V}/projects/creator_one/gamma-mod/issues/1"),
        http_case("is_get_422", "GET", f"{ALPHA}/abc"),
        http_case("is_get_huge", "GET", f"{ALPHA}/99999999999999999999"),
        http_case("is_get_huge_disabled", "GET", f"{V}/projects/creator_one/gamma-mod/issues/99999999999999999999"),
        http_case("site_list_owner_draft", "GET", "/site/mods/projects/creator_one/beta-mod/issues", viewer=CREATOR),
        http_case("site_list_collab", "GET", S_ALPHA + "?status=all", viewer=COLLAB),
        http_case("site_list_player", "GET", S_ALPHA, viewer=PLAYER),
        http_case("site_list_master_taken", "GET", "/site/mods/projects/creator_one/taken-mod/issues", viewer=MASTER),
        http_case("site_list_anon_draft", "GET", "/site/mods/projects/creator_one/beta-mod/issues"),
        http_case("site_list_422", "GET", S_ALPHA + "?limit=abc", viewer=PLAYER),
        http_case("site_get_author", "GET", f"{S_ALPHA}/1", viewer=PLAYER),
        http_case("site_get_owner", "GET", f"{S_ALPHA}/2", viewer=CREATOR),
        http_case("site_get_422", "GET", f"{S_ALPHA}/x", viewer=CREATOR),
        http_case("site_notes_anon", "GET", "/site/mods/notifications"),
        http_case("site_notes_creator", "GET", "/site/mods/notifications", viewer=CREATOR),
        http_case("site_notes_player", "GET", "/site/mods/notifications", viewer=PLAYER),
        http_case("is_create", "POST", ALPHA, write=PLAYER, body={"title": "  New bug  ", "body": " steps "}),
        http_case("is_create_request", "POST", ALPHA, write=FRESH, body={"kind": "request", "title": "Need a feature"}),
        http_case("is_create_blank", "POST", ALPHA, write=PLAYER, body={"title": "   "}),
        http_case("is_create_disabled", "POST", f"{V}/projects/creator_one/gamma-mod/issues", write=PLAYER,
                  body={"title": "x"}),
        http_case("is_create_draft_stranger", "POST", f"{V}/projects/creator_one/beta-mod/issues", write=PLAYER,
                  body={"title": "x"}),
        http_case("is_create_draft_owner", "POST", f"{V}/projects/creator_one/beta-mod/issues", write=CREATOR,
                  body={"title": "Owner note", "body": "ä"}),
        http_case("is_create_cap", "POST", ALPHA, write=FRESH, body={"title": "eleventh"}, db=open_issues_db(10)),
        http_case("is_create_under_cap", "POST", ALPHA, write=FRESH, body={"title": "tenth"}, db=open_issues_db(9)),
        http_case("is_create_limited", "POST", ALPHA, write=PLAYER, body={"title": "x"},
                  deny=[f"mod_issue_create:{PLAYER}"]),
        http_case("is_create_422", "POST", ALPHA, write=PLAYER, body={"kind": "bug", "title": "x" * 141, "body": 3}),
        http_case("is_create_422_missing", "POST", ALPHA, write=PLAYER, body={"body": "x"}),
        http_case("is_comment", "POST", f"{ALPHA}/1/comments", write=COLLAB, body={"body": " Fixed in v3. "}),
        http_case("is_comment_new_participant", "POST", f"{ALPHA}/2/comments", write=FRESH, body={"body": "+1"}),
        http_case("is_comment_missing", "POST", f"{ALPHA}/9/comments", write=PLAYER, body={"body": "x"}),
        http_case("is_comment_blank", "POST", f"{ALPHA}/1/comments", write=PLAYER, body={"body": "  \n "}),
        http_case("is_comment_disabled", "POST", f"{V}/projects/creator_one/gamma-mod/issues/1/comments", write=PLAYER,
                  body={"body": "x"}),
        http_case("is_comment_limited", "POST", f"{ALPHA}/1/comments", write=PLAYER, body={"body": "x"},
                  deny=[f"mod_issue_comment:{PLAYER}"]),
        http_case("is_comment_422", "POST", f"{ALPHA}/x/comments", write=PLAYER, body={"body": ""}),
        http_case("is_close_author", "PATCH", f"{ALPHA}/1", write=PLAYER, body={"status": "closed", "comment": " fixed now "}),
        http_case("is_close_owner", "PATCH", f"{ALPHA}/3", write=CREATOR, body={"status": "closed"}),
        http_case("is_reopen_collab", "PATCH", f"{ALPHA}/2", write=COLLAB, body={"status": "open", "comment": "   "}),
        http_case("is_status_same", "PATCH", f"{ALPHA}/1", write=PLAYER, body={"status": "open", "comment": "x"}),
        http_case("is_status_stranger", "PATCH", f"{ALPHA}/3", write=FRESH, body={"status": "closed"}),
        http_case("is_status_missing", "PATCH", f"{ALPHA}/9", write=CREATOR, body={"status": "closed"}),
        http_case("is_status_limited", "PATCH", f"{ALPHA}/3", write=CREATOR, body={"status": "closed"},
                  deny=[f"mod_issue_comment:{CREATOR}"]),
        http_case("is_status_422", "PATCH", f"{ALPHA}/1", write=PLAYER, body={"status": "done", "comment": 5}),
        http_case("is_status_422_missing", "PATCH", f"{ALPHA}/1", write=PLAYER, body={}),
        http_case("is_delete_owner", "DELETE", f"{ALPHA}/2", write=CREATOR),
        http_case("is_delete_author", "DELETE", f"{ALPHA}/1", write=PLAYER),
        http_case("is_delete_stranger", "DELETE", f"{ALPHA}/1", write=FRESH),
        http_case("is_delete_missing", "DELETE", f"{ALPHA}/9", write=CREATOR),
        http_case("is_delete_disabled_mod", "DELETE", f"{V}/projects/creator_one/gamma-mod/issues/1", write=CREATOR),
        http_case("is_delete_422", "DELETE", f"{ALPHA}/one", write=CREATOR),
        http_case("is_uncomment_author", "DELETE", f"{ALPHA}/1/comments/{e1}", write=PLAYER),
        http_case("is_uncomment_owner", "DELETE", f"{ALPHA}/1/comments/{e4}", write=CREATOR),
        http_case("is_uncomment_stranger", "DELETE", f"{ALPHA}/1/comments/{e4}", write=FRESH),
        http_case("is_uncomment_status_event", "DELETE", f"{ALPHA}/2/comments/{e3}", write=CREATOR),
        http_case("is_uncomment_wrong_issue", "DELETE", f"{ALPHA}/1/comments/{e2}", write=CREATOR),
        http_case("is_uncomment_badid", "DELETE", f"{ALPHA}/1/comments/nope", write=CREATOR),
        http_case("is_uncomment_missing_issue", "DELETE", f"{ALPHA}/9/comments/{e1}", write=CREATOR),
        http_case("is_uncomment_python_bug", "DELETE", f"{ALPHA}/1/comments/{e1}", write=PLAYER, python_bug=True),
        http_case("me_notes", "GET", f"{V}/me/issue-notifications", write=CREATOR),
        http_case("me_notes_player", "GET", f"{V}/me/issue-notifications", write=PLAYER),
        http_case("me_seen", "POST", f"{V}/me/issue-notifications/seen", write=CREATOR),
    ]
    return c


BAD_TOKEN = "kiwi_" + "a" * 43 + "_zzzzzz"


def gated_cases():
    g = []
    a = lambda *x, **k: g.append(http_case(*x, gated=True, **k))  # noqa: E731
    a("g_cl_list_noauth", "GET", CL)
    a("g_cl_add_noauth", "POST", CL, body={"token": "x" * 10})
    a("g_cl_add_badjson", "POST", CL, raw='{"token": ')
    a("g_cl_del_noauth", "DELETE", f"{CL}/abc")
    a("g_cl_put", "PUT", CL)
    a("g_cl_item_get", "GET", f"{CL}/abc")
    a("g_ct_get_noauth", "GET", "/site/mods/creator-token")
    a("g_ct_post_noauth", "POST", "/site/mods/creator-token")
    a("g_ct_rotate_noauth", "POST", "/site/mods/creator-token/rotate")
    a("g_ct_delete", "DELETE", "/site/mods/creator-token")
    a("g_ct_rotate_get", "GET", "/site/mods/creator-token/rotate")
    a("g_cc_patch_noauth", "PATCH", "/site/mods/creator-connections/abc", body={})
    a("g_cc_patch_badjson", "PATCH", "/site/mods/creator-connections/abc", raw="[1,")
    a("g_cc_delete_noauth", "DELETE", "/site/mods/creator-connections/abc")
    a("g_cc_post", "POST", "/site/mods/creator-connections/abc")
    a("g_is_create_noauth", "POST", ALPHA, body={"title": "x"})
    a("g_is_create_badjson", "POST", ALPHA, raw="{")
    a("g_is_create_badtoken", "POST", ALPHA, body={"title": "x"}, headers=[("authorization", f"Bearer {BAD_TOKEN}")])
    a("g_is_put", "PUT", ALPHA)
    a("g_is_patch_noauth", "PATCH", f"{ALPHA}/1", body={"status": "closed"})
    a("g_is_patch_noauth_badnum", "PATCH", f"{ALPHA}/x", body={"status": "nope"})
    a("g_is_delete_noauth", "DELETE", f"{ALPHA}/1")
    a("g_is_item_post", "POST", f"{ALPHA}/1")
    a("g_is_comment_noauth", "POST", f"{ALPHA}/1/comments", body={"body": "x"})
    a("g_is_comment_get", "GET", f"{ALPHA}/1/comments")
    a("g_is_uncomment_noauth", "DELETE", f"{ALPHA}/1/comments/abc")
    a("g_is_uncomment_get", "GET", f"{ALPHA}/1/comments/abc")
    a("g_me_notes_noauth", "GET", f"{V}/me/issue-notifications")
    a("g_me_notes_badtoken", "GET", f"{V}/me/issue-notifications", headers=[("authorization", f"Bearer {BAD_TOKEN}")])
    a("g_me_notes_post", "POST", f"{V}/me/issue-notifications")
    a("g_me_seen_noauth", "POST", f"{V}/me/issue-notifications/seen")
    a("g_me_seen_get", "GET", f"{V}/me/issue-notifications/seen")
    a("g_site_notes_post", "POST", "/site/mods/notifications")
    a("g_site_list_put", "PUT", S_ALPHA)
    a("g_site_get_delete", "DELETE", f"{S_ALPHA}/1")
    off_hub = {"feature_mods_hub_enabled": False}
    off_issues = {"feature_mod_issues_enabled": False}
    a("g_cl_list_hub_off", "GET", CL, flags=off_hub)
    a("g_cl_put_hub_off", "PUT", CL, flags=off_hub)
    a("g_is_create_hub_off", "POST", ALPHA, body={"title": "x"}, flags=off_hub)
    a("g_is_create_issues_off", "POST", ALPHA, body={"title": "x"}, flags=off_issues)
    a("g_is_list_issues_off", "GET", ALPHA, flags=off_issues)
    a("g_me_notes_issues_off", "GET", f"{V}/me/issue-notifications", flags=off_issues)
    a("g_ct_get_hub_off", "GET", "/site/mods/creator-token", flags=off_hub)
    a("g_cc_patch_hub_off", "PATCH", "/site/mods/creator-connections/abc", body={}, flags=off_hub)
    a("g_site_notes_hub_off", "GET", "/site/mods/notifications", flags=off_hub)
    a("g_site_notes_issues_off", "GET", "/site/mods/notifications", flags=off_issues)
    a("g_site_list_issues_off", "GET", S_ALPHA, flags=off_issues)
    return g


def auth_case(name, method, path, template, api_user, params=None, query="", headers=(), db=None):
    global DB
    DB = db if db is not None else base_db()
    before = snap()
    hdrs = [(k.lower().encode(), v.encode("latin-1")) for k, v in headers]
    req = write_auth.Request({"type": "http", "method": method, "path": path, "query_string": query.encode(),
                              "headers": hdrs, "route": types.SimpleNamespace(path=template),
                              "path_params": params or {}})
    user = model_of(User, api_user)
    token = ApiToken(user_id=api_user, name="t", prefix="kiwi_x", hashed_token="h", scopes=0)

    async def resolve(creds):
        return user, token

    async def limits(*a, **k):
        return None

    real = (write_auth._resolve_token, write_auth._enforce_token_limits)
    write_auth._resolve_token, write_auth._enforce_token_limits = resolve, limits
    try:
        res = run(write_auth._authorize_api(req, None, types.SimpleNamespace(credentials=BAD_TOKEN)))
        out = {"user": str(res.user.id), "link": str(res.link.id)}
    except APIError as e:
        out = {"status": e.status_code, "code": getattr(e.code, "value", e.code),
               "message": e.message}
    finally:
        write_auth._resolve_token, write_auth._enforce_token_limits = real
    return {"name": name, "method": method, "template": template, "api_user": str(api_user),
            "params": params or {}, "query": query, "headers": [list(h) for h in headers],
            "db": register(before), "result": out, "after": changed(before)}


def auth_cases():
    me = f"{V}/me/projects"
    proj = f"{V}/projects/{{handle}}/{{slug}}"
    rel = f"{V}/releases/{{release_id}}"
    hs = lambda h, s: {"handle": h, "slug": s}  # noqa: E731
    return [
        auth_case("wa_issue_route", "POST", f"{V}/projects/creator_one/alpha-mod/issues", f"{proj}/issues", DEV,
                  hs("creator_one", "alpha-mod")),
        auth_case("wa_delete_project", "DELETE", f"{V}/projects/creator_one/alpha-mod", proj, DEV,
                  hs("creator_one", "alpha-mod")),
        auth_case("wa_head", "HEAD", me, me, DEV),
        auth_case("wa_ambiguous", "GET", me, me, DEV),
        auth_case("wa_query_creator", "GET", me, me, DEV, query="creator=creator_one"),
        auth_case("wa_query_last_wins", "GET", me, me, DEV, query="creator=gone_creator&creator=Creator_One"),
        auth_case("wa_header_creator", "GET", me, me, DEV, headers=[("X-Kiwi-Creator", "  CREATOR_ONE ")]),
        auth_case("wa_empty_query_header", "GET", me, me, DEV, query="creator=",
                  headers=[("X-Kiwi-Creator", "creator_one")]),
        auth_case("wa_inactive_creator", "GET", me, me, DEV, query="creator=gone_creator"),
        auth_case("wa_unknown_creator", "GET", me, me, DEV, query="creator=Nobody%20Here"),
        auth_case("wa_not_connected_creator", "GET", me, me, DEV, query="creator=creator_two"),
        auth_case("wa_no_links", "GET", me, me, DEV4),
        auth_case("wa_single_deleted", "GET", me, me, DEV3),
        auth_case("wa_patch_owned", "PATCH", f"{V}/projects/creator_one/alpha-mod", proj, DEV,
                  hs("creator_one", "alpha-mod")),
        auth_case("wa_patch_case_handle", "PATCH", f"{V}/projects/CREATOR_ONE/alpha-mod", proj, DEV,
                  hs(" CREATOR_ONE", "alpha-mod")),
        auth_case("wa_patch_other_creator", "PATCH", f"{V}/projects/creator_two/delta-mod", proj, DEV,
                  hs("creator_two", "delta-mod")),
        auth_case("wa_patch_unknown_project", "PATCH", f"{V}/projects/creator_one/nope", proj, DEV,
                  hs("creator_one", "nope")),
        auth_case("wa_patch_unknown_project_named", "PATCH", f"{V}/projects/creator_one/nope", proj, DEV,
                  hs("creator_one", "nope"), query="creator=creator_one"),
        auth_case("wa_narrow_uncovered", "PATCH", f"{V}/projects/creator_one/beta-mod", proj, DEV2,
                  hs("creator_one", "beta-mod")),
        auth_case("wa_narrow_covered", "PATCH", f"{V}/projects/creator_one/alpha-mod", proj, DEV2,
                  hs("creator_one", "alpha-mod")),
        auth_case("wa_narrow_create", "POST", f"{V}/projects", f"{V}/projects", DEV2, query="creator=creator_one"),
        auth_case("wa_narrow_fork", "POST", f"{V}/projects/creator_two/delta-mod/fork", f"{proj}/fork", DEV2,
                  hs("creator_two", "delta-mod")),
        auth_case("wa_all_create", "POST", f"{V}/projects", f"{V}/projects", DEV, query="creator=creator_one"),
        auth_case("wa_release", "PATCH", f"{V}/releases/{R1}", rel, DEV, {"release_id": str(R1)}),
        auth_case("wa_release_badid", "DELETE", f"{V}/releases/nope", rel, DEV, {"release_id": "nope"},
                  query="creator=creator_one"),
        auth_case("wa_release_orphan", "PATCH", f"{V}/releases/{R2}", rel, DEV, {"release_id": str(R2)}),
        auth_case("wa_release_config", "POST", f"{V}/releases/{R1}/config", f"{rel}/config", DEV2,
                  {"release_id": str(R1)}),
        auth_case("wa_stray", "PATCH", f"{V}/projects/stray/stray-mod", proj, DEV, hs("stray", "stray-mod")),
        auth_case("wa_banner_delete", "DELETE", f"{V}/projects/creator_one/alpha-mod/banner", f"{proj}/banner", DEV,
                  hs("creator_one", "alpha-mod")),
        auth_case("wa_previews_sha", "DELETE", f"{V}/projects/creator_one/gamma-mod/previews/ab", f"{proj}/previews/{{sha}}",
                  DEV2, {"handle": "creator_one", "slug": "gamma-mod", "sha": "ab"}),
    ]


def main():
    mask_tok = ApiToken(user_id=DEV, name="t", prefix="p", hashed_token="h", scopes=1)

    async def resolve(creds):
        return model_of(User, DEV), mask_tok

    global DB
    DB = base_db()
    real = write_auth._resolve_token
    write_auth._resolve_token = resolve
    try:
        run(write_auth._authorize_api(None, None, types.SimpleNamespace(credentials="x")))
        scope_denied = None
    except APIError as e:
        scope_denied = {"status": e.status_code, "code": getattr(e.code, "value", e.code), "message": e.message}
    finally:
        write_auth._resolve_token = real

    out = {
        "now": NOW.isoformat(),
        "minted": MINTED,
        "api_routes": {k: sorted(v) for k, v in write_auth._API_ROUTES.items()},
        "create_routes": sorted(write_auth._CREATE_ROUTES),
        "creator_router_routes": sorted(
            [r.path, sorted(r.methods)[0]] for r in hub_router.mods_creator_write_router.routes),
        "rate_limits": {k: list(run(rc.get_rate_limit(k))) for k in ("mod_issue_create", "mod_issue_comment")},
        "scope_denied": scope_denied,
        "creators": creator_cases(),
        "issues": issue_cases(),
        "gated": gated_cases(),
        "auth": auth_cases(),
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    out["dbs"] = DBS
    OUT.write_text(json.dumps(out, ensure_ascii=False, separators=(",", ":")) + "\n", encoding="utf-8")
    n = sum(len(out[k]) for k in ("creators", "issues", "gated", "auth"))
    print(f"wrote {OUT} ({n} cases)")


if __name__ == "__main__":
    main()
