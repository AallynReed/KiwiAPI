"""Golden fixtures for kiwi-server's master admin routes (``src/admin``).

Every route case drives ``app.main.app`` at the ASGI level with
``get_current_superuser`` overridden and the Beanie reads it makes replaced by
canned documents (written to the fixture as canonical extended JSON, so the
Rust test loads the very same BSON). The pipelines Python built are recorded
for the Rust test to rebuild and compare. Also writes
``src/admin/discord_commands.json`` (``COMMAND_DEFS``).

Run from the repo root with the production-pinned venv:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_admin_fixtures.py
"""

from __future__ import annotations

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
ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "admin" / "golden.json"
COMMANDS_OUT = ROOT / "native" / "kiwi-server" / "src" / "admin" / "discord_commands.json"
os.chdir(ROOT)
sys.path.insert(0, str(ROOT))

import bson  # noqa: E402
from bson import ObjectId  # noqa: E402
from bson.int64 import Int64  # noqa: E402
from bson.json_util import CANONICAL_JSON_OPTIONS  # noqa: E402
from bson.json_util import dumps as ext_dumps  # noqa: E402
from beanie import init_beanie  # noqa: E402
from beanie.odm.utils import init as beanie_init  # noqa: E402
from pymongo import AsyncMongoClient  # noqa: E402

from app.admin import pending as pending_mod  # noqa: E402
from app.admin import router as admin_router  # noqa: E402
from app.admin import runtime_config as rc  # noqa: E402
from app.auth.models import User  # noqa: E402
from app.core.config import settings  # noqa: E402
from app.core.dependencies import get_current_superuser  # noqa: E402
from app.pageviews import service as pv  # noqa: E402
from app.site_auth import usernames  # noqa: E402
from app.site_auth.models import SiteUser, UsernameChangeRequest  # noqa: E402
from app.tokens.models import ApiToken  # noqa: E402
from app.trove.modpacks.models import ModpackProject  # noqa: E402
from app.trove.mods_hub.models import ModProject  # noqa: E402
from app.usage import service as usage_service  # noqa: E402
from app.usage.models import UsageEvent  # noqa: E402

UTC = dt.timezone.utc
NOW = dt.datetime(2026, 10, 22, 14, 37, 21, 654321, tzinfo=UTC)
ADMIN_ID = "65a1b2c3d4e5f60718293a4b"


def ext(value):
    """Canonical extended JSON as a plain JSON value."""
    return json.loads(ext_dumps(value, json_options=CANONICAL_JSON_OPTIONS))


def at(ms: int) -> dt.datetime:
    return dt.datetime.fromtimestamp(ms / 1000, UTC)


async def call(app, method: str, url: str, body: bytes = b"", ctype: str | None = None, rid: str = "parity-adm"):
    path, _, qs = url.partition("?")
    headers = [(b"host", b"testserver"), (b"x-request-id", rid.encode()),
               (b"content-length", str(len(body)).encode())]
    if ctype:
        headers.append((b"content-type", ctype.encode()))
    scope = {
        "type": "http", "asgi": {"version": "3.0"}, "http_version": "1.1", "method": method,
        "scheme": "http", "path": urllib.parse.unquote(path), "raw_path": path.encode(),
        "query_string": qs.encode(), "root_path": "", "headers": headers,
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
    hdrs = {k.decode("latin-1").lower(): v.decode("latin-1") for k, v in start["headers"]}
    out = {"status": start["status"], "body": raw.decode("utf-8")}
    for name in ("allow", "www-authenticate"):
        if name in hdrs:
            out[name] = hdrs[name]
    return out


class FakeQuery:
    def __init__(self, docs, total=None, log=None, filt=None):
        self.docs = list(docs)
        self.total = len(self.docs) if total is None else total
        if log is not None:
            log.append(filt)

    def sort(self, *a, **k):
        return self

    def skip(self, *a, **k):
        return self

    def limit(self, *a, **k):
        return self

    async def to_list(self, *a, **k):
        return self.docs

    async def count(self):
        return self.total

    async def first_or_none(self):
        return self.docs[0] if self.docs else None


class FakeAgg:
    def __init__(self, rows):
        self.rows = rows

    async def to_list(self, *a, **k):
        return self.rows


def queue_agg(cls, outputs: list, log: list):
    """``cls.aggregate`` answers each call with the next canned row list."""
    def aggregate(pipeline, *a, **k):
        log.append(ext(pipeline))
        return FakeAgg(outputs.pop(0))
    cls.aggregate = aggregate


# --- canned documents -----------------------------------------------------------------

U1, U2, U3 = ObjectId("65a000000000000000000a01"), ObjectId("65a000000000000000000a02"), ObjectId("65a000000000000000000a03")
T1, T2, T3 = ObjectId("65a000000000000000000b01"), ObjectId("65a000000000000000000b02"), ObjectId("65a000000000000000000b03")

USERS = [
    {"_id": U1, "email": "ada@Example.COM", "hashed_password": "x", "display_name": "Ada", "is_active": True,
     "is_verified": True, "is_superuser": False, "roles": [], "token_version": 2, "github_id": None,
     "email_bounced": False, "created_at": at(1_760_000_000_123), "updated_at": at(1_760_000_000_123),
     "last_login_at": at(1_761_000_000_000)},
    {"_id": U2, "email": "bob@kiwi-mail.net", "hashed_password": "x", "display_name": None, "is_active": False,
     "is_verified": False, "is_superuser": False, "roles": ["beta"], "token_version": Int64(5),
     "github_id": Int64(12345678901), "email_bounced": True, "created_at": at(1_750_000_000_000),
     "updated_at": at(1_750_000_000_000), "last_login_at": None},
    {"_id": U3, "email": "root@kiwi-mail.net", "hashed_password": "x", "display_name": "Root", "is_active": True,
     "is_verified": True, "is_superuser": True, "roles": [], "token_version": 0, "github_id": None,
     "email_bounced": False, "created_at": at(1_740_000_000_001), "updated_at": at(1_740_000_000_001),
     "last_login_at": None},
]

TOKENS = [
    {"_id": T1, "user_id": U1, "name": "laptop", "prefix": "kiwi_ab12cd34", "hashed_token": "h1", "scopes": 0,
     "ip_salt": "s", "allowed_ip_hashes": ["a", "b"], "revoked": False, "revoked_at": None, "revoke_reason": None,
     "expires_at": at(1_790_000_000_000), "expiry_warned": False, "created_at": at(1_760_000_100_500),
     "last_used_at": at(1_761_500_000_250), "rotated_at": None, "request_count": 12},
    {"_id": T2, "user_id": U1, "name": "bot", "prefix": "kiwi_zz99yy88", "hashed_token": "h2", "scopes": 1536,
     "ip_salt": None, "allowed_ip_hashes": [], "revoked": True, "revoked_at": at(1_761_000_000_000),
     "revoke_reason": "Revoked by administrator", "expires_at": None, "expiry_warned": False,
     "created_at": at(1_760_000_200_000), "last_used_at": at(1_761_600_000_999), "rotated_at": at(1_760_500_000_000),
     "request_count": Int64(3000000000)},
]

TOKEN_ROWS = [
    {"_id": U1, "token_count": 2, "total_requests": Int64(3000000012), "last_used_at": at(1_761_600_000_999)},
    {"_id": U2, "token_count": 1, "total_requests": 0, "last_used_at": None},
]
RL_ROWS = [{"_id": U1, "rate_limited": 4}, {"_id": U3, "rate_limited": Int64(1)}]

EVENTS = [
    {"_id": ObjectId("65a000000000000000000e03"), "user_id": U1, "token_id": T1, "method": "GET",
     "route": "/v1/stats/classes", "path": "/v1/stats/classes", "status_code": 200, "duration_ms": 12,
     "created_at": at(1_761_700_000_003)},
    {"_id": ObjectId("65a000000000000000000e02"), "user_id": U1, "token_id": T2, "method": "POST",
     "route": "/v1/x/{id}", "path": "/v1/x/1", "status_code": 429, "duration_ms": 1.25,
     "created_at": at(1_761_700_000_000)},
    {"_id": ObjectId("65a000000000000000000e01"), "user_id": U2, "token_id": T2, "method": "GET",
     "route": "/v1/y", "path": "/v1/y?é=1", "status_code": 500, "duration_ms": 1e16,
     "created_at": at(1_761_600_000_000)},
]

S1, S2, S3 = ObjectId("65a000000000000000000c01"), ObjectId("65a000000000000000000c02"), ObjectId("65a000000000000000000c03")


def site_user(oid, **kw):
    base = {"_id": oid, "username": "user", "discord_handle": "", "notify_email": None, "display_name": None,
            "is_active": True, "is_verified": True, "is_deleted": False, "discord_id": None,
            "discord_avatar": None, "token_version": 0, "claimed_trove_name": None,
            "claimed_trove_display": None, "claimed_at": None, "claim_verified": False, "claim_baseline": {},
            "claim_verified_at": None, "creator_token_hash": None, "creator_token_prefix": None,
            "creator_token_at": None, "mod_issues_seen_at": None, "created_at": at(1_755_000_000_000),
            "updated_at": at(1_755_000_000_000), "last_login_at": None}
    base.update(kw)
    return base


SITE_USERS = [
    site_user(S1, username="kiwi_fan", discord_handle="KiwiFan", display_name="Kiwi Fän",
              discord_id=Int64(812345678901234567), claimed_trove_name="sir kiwi", claimed_trove_display="Sir Kiwi",
              claimed_at=at(1_756_000_000_100), claim_baseline={"12": 4.0, "1000": 17}, last_login_at=at(1_761_000_000_456)),
    site_user(S2, username="lurker", discord_id=0, claimed_trove_name="lurk", claimed_trove_display="",
              claimed_at=at(1_757_000_000_000), claim_verified=True, claim_verified_at=at(1_757_100_000_000)),
    site_user(S3, username="quiet", discord_handle="", created_at=at(1_754_000_000_000)),
]

REQUESTS = [
    {"_id": ObjectId("65a000000000000000000d01"), "site_user_id": S1, "current_username": "kiwi_fan",
     "requested_username": "sir_kiwi", "status": "pending", "reason": "", "resolved_by": None,
     "resolved_at": None, "created_at": at(1_761_000_000_000)},
    {"_id": ObjectId("65a000000000000000000d02"), "site_user_id": S2, "current_username": "lurker",
     "requested_username": "lurk.er", "status": "rejected", "reason": "Not your IGN",
     "resolved_by": ObjectId(ADMIN_ID), "resolved_at": at(1_761_000_100_010), "created_at": at(1_760_900_000_000)},
]


def admin_user() -> User:
    return User.model_validate({**USERS[2], "_id": ObjectId(ADMIN_ID), "email": "master@aallyn.net"})


# --- sections ----------------------------------------------------------------------------

async def users_section(app) -> dict:
    out: dict = {"users": ext(USERS), "tokens": ext(TOKENS), "token_rows": ext(TOKEN_ROWS), "rl_rows": ext(RL_ROWS),
                 "events": ext(EVENTS)}
    users = [User.model_validate(u) for u in USERS]
    tokens = [ApiToken.model_validate(t) for t in TOKENS]
    log: list = []
    queue_agg(ApiToken, [TOKEN_ROWS], log)
    queue_agg(UsageEvent, [RL_ROWS], log)
    User.find_all = lambda *a, **k: FakeQuery(users)
    out["list"] = await call(app, "GET", "/admin/users?days=30")

    async def get_user(oid, *a, **k):
        return next((u for u in users if u.id == oid), None)

    User.get = get_user
    ApiToken.find = lambda *a, **k: FakeQuery(tokens)
    out["get"] = await call(app, "GET", f"/admin/users/{U1}")
    out["tokens_list"] = await call(app, "GET", f"/admin/users/{U1}/tokens")
    out["get_missing"] = await call(app, "GET", f"/admin/users/{ObjectId('65a0000000000000000000ff')}")
    ApiToken.find = lambda *a, **k: FakeQuery([])
    out["get_no_tokens"] = await call(app, "GET", f"/admin/users/{U2}")

    totals = [{"_id": None, "count": 7, "error_count": 3, "rate_limited": 1, "avg_duration_ms": 12.345}]
    by_day = [{"_id": "2026-10-20", "count": 4, "error_count": 1, "avg_duration_ms": 2.675},
              {"_id": "2026-10-21", "count": 3, "error_count": 2, "avg_duration_ms": None}]
    by_endpoint = [{"_id": {"route": "/v1/a", "method": "GET"}, "count": 5, "error_count": 0, "avg_duration_ms": 1.005},
                   {"_id": {"route": "/v1/b", "method": "POST"}, "count": 2, "error_count": 3, "avg_duration_ms": 1e16}]
    alog: list = []
    queue_agg(UsageEvent, [totals, by_day, by_endpoint], alog)
    out["activity_rows"] = ext([totals, by_day, by_endpoint])
    out["activity"] = await call(app, "GET", f"/admin/users/{U1}/activity?days=3")
    out["activity_pipelines"] = alog
    queue_agg(UsageEvent, [[], [], []], [])
    out["activity_empty"] = await call(app, "GET", f"/admin/users/{U1}/activity")

    events = [UsageEvent.model_validate(e) for e in EVENTS]
    flog: list = []

    def find_events(query, *a, **k):
        flog.append(ext(query))
        return FakeQuery(events)

    UsageEvent.find = find_events
    out["events_more"] = await call(app, "GET", f"/admin/events?limit=2&user_id={U1}&status_code=429&cursor=65a000000000000000000e09")
    out["events_all"] = await call(app, "GET", "/admin/events?limit=3")
    out["events_filters"] = flog
    out["events_bad_cursor"] = await call(app, "GET", "/admin/events?cursor=nope")

    top = [{"_id": U1, "count": 9, "error_count": 2, "rate_limited": 1}, {"_id": ObjectId("65a0000000000000000000fe"), "count": 1, "error_count": 0, "rate_limited": 0}]
    olog: list = []
    queue_agg(UsageEvent, [totals, top], olog)
    User.find = lambda *a, **k: FakeQuery([users[0]])
    out["overview_rows"] = ext([totals, top])
    out["overview"] = await call(app, "GET", "/admin/activity?days=2&top=5")
    out["overview_pipelines"] = olog
    queue_agg(UsageEvent, [[], []], [])
    out["overview_empty"] = await call(app, "GET", "/admin/activity")
    return out


async def pending_section(app) -> dict:
    cases = [
        [0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        [1, 2, None, 0, 1, 3, 0, 1, 2, 1],
        [0, 0, 1, 1, None, None, 5, 0, 0, 2],
        [-1, 0, 0, 0, 0, 1, 0, 0, 1, None],
    ]
    out = []
    original = pending_mod._SOURCES
    for counts in cases:
        def make(n):
            async def count():
                if n is None:
                    raise RuntimeError("boom")
                return n
            return count
        pending_mod._SOURCES = tuple(s._replace(count=make(n)) for s, n in zip(original, counts))
        out.append({"counts": counts, **await call(app, "GET", "/admin/pending")})
    pending_mod._SOURCES = original
    return {"cases": out, "tabs": [[s.tab, s.one, s.many] for s in original]}


def pick_keys() -> dict:
    by = {}
    for key, spec in rc.REGISTRY.items():
        if spec.type == "int" and spec.min_value is not None and spec.max_value is not None:
            by.setdefault("int", key)
        if spec.type == "float" and spec.min_value is not None and spec.max_value is not None:
            by.setdefault("float", key)
        if spec.type == "bool":
            by.setdefault("bool", key)
        if spec.type == "str" and spec.pattern is None:
            by.setdefault("str", key)
        if spec.pattern is not None:
            by.setdefault("pattern", key)
        if spec.type == "float":
            by["float_last"] = key
    return by


async def config_section(app) -> dict:
    keys = pick_keys()
    admin_oid = ObjectId(ADMIN_ID)
    raw = [
        {"_id": ObjectId("65a000000000000000000f01"), "key": keys["int"], "value": "7",
         "updated_at": at(1_761_000_000_123), "updated_by_user_id": admin_oid},
        {"_id": ObjectId("65a000000000000000000f02"), "key": keys["float"], "value": "3",
         "updated_at": at(1_761_000_000_000), "updated_by_user_id": None},
        {"_id": ObjectId("65a000000000000000000f03"), "key": keys["bool"], "value": "not json",
         "updated_at": at(1_761_000_001_000), "updated_by_user_id": admin_oid},
        {"_id": ObjectId("65a000000000000000000f04"), "key": keys["str"], "value": "\"caf\\u00e9 \\ud83e\\udd5d\"",
         "updated_at": at(1_761_000_002_500), "updated_by_user_id": admin_oid},
        {"_id": ObjectId("65a000000000000000000f05"), "key": keys["float_last"], "value": "2.5e-05",
         "updated_at": at(1_761_000_003_000), "updated_by_user_id": None},
        {"_id": ObjectId("65a000000000000000000f06"), "key": "not_registered", "value": "1",
         "updated_at": at(1_761_000_004_000), "updated_by_user_id": None},
    ]
    docs = [rc.RuntimeConfig.model_validate(d) for d in raw]
    rc.RuntimeConfig.find_all = lambda *a, **k: FakeQuery(docs)
    out = {"keys": keys, "overrides": ext(raw), "list": await call(app, "GET", "/admin/config")}
    rc.RuntimeConfig.find_all = lambda *a, **k: FakeQuery([])
    out["list_plain"] = await call(app, "GET", "/admin/config")

    bodies = [
        ("missing-key", "PUT", "/admin/config/no_such_key", b'{"value": 1}'),
        ("missing-key-del", "DELETE", "/admin/config/no_such_key", b""),
        ("encoded-key", "PUT", "/admin/config/caf%C3%A9%20key", b'{"value": 1}'),
        ("no-body", "PUT", f"/admin/config/{keys['int']}", b""),
        ("no-value", "PUT", f"/admin/config/{keys['int']}", b'{"other": 1}'),
        ("bad-json", "PUT", f"/admin/config/{keys['int']}", b'{"value": '),
        ("list-body", "PUT", f"/admin/config/{keys['int']}", b"[1]"),
    ]
    values = [
        ("int", True), ("int", 1.0), ("int", "5"), ("int", None), ("int", -100000), ("int", 10 ** 9),
        ("float", True), ("float", "x"), ("float", -1), ("float", 1e9), ("float", 0.0),
        ("bool", 1), ("bool", "true"), ("str", 5), ("pattern", "ZZZ"), ("pattern", ["ZZ"]),
    ]
    for kind, v in values:
        bodies.append((f"{kind}-{json.dumps(v)}", "PUT", f"/admin/config/{keys[kind]}", json.dumps({"value": v}).encode()))
    errors = []
    for name, method, url, body in bodies:
        res = await call(app, method, url, body, "application/json" if body else None, rid=f"parity-cfg-{len(errors)}")
        errors.append({"name": name, "method": method, "url": url, "sent": body.decode(), **res})
    out["errors"] = errors

    ok = []
    for kind, v in [("int", 5), ("int", Int64(7)), ("float", 3), ("float", 2.5), ("bool", False), ("str", "héllo \"x\""),
                    ("pattern", " zz , QQ "), ("pattern", "")]:
        spec = rc.REGISTRY[keys[kind]]
        validated = rc._validate(spec, v)
        ok.append({"key": spec.key, "value": ext(v) if isinstance(v, Int64) else v, "stored": json.dumps(validated)})
    out["valid"] = ok
    return out


async def site_section(app) -> dict:
    users = [SiteUser.model_validate(u) for u in SITE_USERS]
    out: dict = {"site_users": ext(SITE_USERS), "requests": ext(REQUESTS)}

    async def get_site(oid, *a, **k):
        return next((u for u in users if u.id == oid), None)

    SiteUser.get = get_site
    out["get"] = [await call(app, "GET", f"/admin/site-users/{u['_id']}") for u in SITE_USERS]
    out["get_bad_id"] = await call(app, "GET", "/admin/site-users/xyz")
    flog: list = []
    SiteUser.find = lambda query=None, *a, **k: FakeQuery(users, total=41, log=flog, filt=ext(query or {}))
    agg_log: list = []
    queue_agg(ModProject, [[{"_id": S1, "n": 3}, {"_id": S3, "n": Int64(1)}]], agg_log)
    queue_agg(ModpackProject, [[{"_id": S1, "n": 1}]], agg_log)
    out["list"] = await call(app, "GET", "/admin/site-users?q=%20Kiwi.Fan%2B(1)%20&limit=3&offset=2")
    SiteUser.find = lambda query=None, *a, **k: FakeQuery([], total=0, log=flog, filt=ext(query or {}))
    out["list_empty"] = await call(app, "GET", "/admin/site-users?q=%20%20")
    out["list_filters"] = flog
    out["list_pipelines"] = agg_log

    SiteUser.find = lambda query=None, *a, **k: FakeQuery(users[:2], log=flog, filt=ext(query or {}))
    out["claims"] = await call(app, "GET", "/admin/site-claims")
    out["claims_all"] = await call(app, "GET", "/admin/site-claims?pending_only=false")

    reqs = [UsernameChangeRequest.model_validate(r) for r in REQUESTS]
    rlog: list = []
    UsernameChangeRequest.find = lambda query=None, *a, **k: FakeQuery(reqs, log=rlog, filt=ext(query or {}))
    out["requests_list"] = await call(app, "GET", "/admin/username-requests")
    out["requests_all"] = await call(app, "GET", "/admin/username-requests?status=")
    out["requests_filters"] = rlog

    names = ["  Kiwi_Fan ", "ab", "a" * 25, ".lead", "trail.", "dou..ble", "Admin", "ok.name", "KELVIN\u212a", "naïve",
             "ab\n", "x_y.z9", "null", "abc def"]
    norm = []
    for n in names:
        try:
            norm.append({"raw": n, "name": usernames.normalize_username(n)})
        except Exception as exc:  # noqa: BLE001
            norm.append({"raw": n, "status": exc.status_code, "code": str(getattr(exc.code, "value", exc.code)), "message": exc.message})
    out["normalize"] = norm
    return out


async def misc_section(app) -> dict:
    out: dict = {}
    from app.discord.commands import COMMAND_DEFS
    out["commands"] = await call(app, "GET", "/admin/discord/commands")
    settings.discord_client_id = None
    settings.discord_bot_token = None
    out["register_no_creds"] = await call(app, "POST", "/admin/discord/register-commands?guild_id=1")
    out["clear_no_guild"] = await call(app, "POST", "/admin/discord/clear-guild-commands")

    presence = {"_id": "current", "guild_count": 12, "member_count": Int64(3456), "updated_at": at(1_761_800_000_120)}
    commands = [{"_id": "status", "count": 40, "last_used_at": at(1_761_800_000_000)},
                {"_id": "gem", "count": 7.9}, {"_id": "ping", "count": Int64(2), "last_used_at": None}]

    class Coll:
        def __init__(self, docs):
            self.docs = docs

        async def find_one(self, *a, **k):
            return self.docs[0] if self.docs else None

        def find(self, *a, **k):
            docs = self.docs

            class Cur:
                def sort(self, *a, **k):
                    return self

                def __aiter__(self):
                    async def gen():
                        for d in docs:
                            yield d
                    return gen()
            return Cur()

    from app.bot import stats as bot_stats
    for name, pres in (("bot_stats", [presence]), ("bot_stats_empty", [])):
        db = {"bot_presence": Coll(pres), "bot_command_usage": Coll(commands if pres else [])}
        bot_stats.get_db = lambda db=db: db
        out[name] = {"presence": ext(pres), "commands": ext(commands if pres else []),
                     **await call(app, "GET", "/admin/bot/stats")}
    COMMANDS_OUT.write_text(json.dumps(COMMAND_DEFS, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    return out


PV_FACET = {
    "totals": [{"_id": "all", "visitors": 5, "visits": 7, "pages": 12, "bounces": 2, "engaged": 340}],
    "median": [{"_id": None, "m": 42.5}],
    "series": [{"_id": "2026-10-21", "visitors": 2, "visits": 3, "pages": 5, "bounces": 1, "engaged": 100},
               {"_id": "2026-10-22", "visitors": 3, "visits": 4, "pages": 7, "bounces": 1, "engaged": 240},
               {"_id": "2026-10-22T13:00", "visitors": 1, "visits": 1, "pages": 1, "bounces": 1, "engaged": 3}],
    "eng": [{"_id": 0, "n": 2}, {"_id": 3, "n": 4}, {"_id": 11, "n": 1}],
    "eng_med": [{"_id": None, "m": 12.5}],
    "eng_channel": [{"_id": {"k": "search", "b": 2}, "n": 3}, {"_id": {"k": "direct", "b": 0}, "n": 2},
                    {"_id": {"b": 1}, "n": 1}, {"_id": {"k": "direct", "b": 4}, "n": 1},
                    {"_id": {"k": "None", "b": 4}, "n": 1}],
    "eng_med_channel": [{"_id": "search", "m": 7.5}, {"_id": None, "m": 2.5}, {"_id": "direct", "m": None}],
    "eng_device": [{"_id": {"k": "mobile", "b": 5}, "n": 2}, {"_id": {"k": "desktop", "b": 5}, "n": 2}],
    "eng_med_device": [{"_id": "mobile", "m": 30.5}, {"_id": "desktop", "m": 31.5}],
    "speed_all": [{"_id": None, "n": 4, "seen": 3, "slow": 1, "vis": [1234.5, 2500.5], "ttfb": [300.2],
                   "dcl": [None], "load": [4000.0], "cls": [12.5], "cls_n": 2, "cls_poor": 0}],
    "speed_bands": [{"_id": 0, "n": 1}, {"_id": 6, "n": 2}, {"_id": 0, "n": 1}],
    "speed_device": [{"_id": "desktop", "n": 3, "seen": 3, "slow": 0, "vis": [900.0, 1100.0], "ttfb": [100.0],
                      "dcl": [500.0], "load": [800.0], "cls": [0.0], "cls_n": 3, "cls_poor": 0}],
    "speed_browser": [],
    "speed_entry": [{"_id": None, "n": 1, "seen": 0, "slow": 0, "vis": [None, None], "ttfb": [None], "dcl": [None],
                     "load": [None], "cls": [None], "cls_n": 0, "cls_poor": 0}],
    "channels": [{"_id": "direct", "visitors": 3, "visits": 4, "pages": 6, "bounces": 1, "engaged": 100},
                 {"_id": None, "visitors": 1, "visits": 1, "pages": 1, "bounces": 1, "engaged": 0}],
    "sources": [{"_id": "Google", "visitors": 1, "visits": 2, "pages": 2, "bounces": 0, "engaged": 12.5}],
    "entries": [{"_id": "/", "visitors": 4, "visits": 5, "pages": 9, "bounces": 1, "engaged": 300}],
    "exits": [],
    "devices": [{"_id": "mobile", "visitors": 2, "visits": 2, "pages": 2, "bounces": 2, "engaged": Int64(5)}],
    "browsers": [],
    "os": [],
    "langs": [{"_id": "fr", "visitors": 1, "visits": 1, "pages": 1, "bounces": 0, "engaged": 20}],
    "pages": [{"_id": "/mods/a/b", "visitors": 2, "views": 5, "engaged": 50, "route": "/mods/{owner}/{slug}"}],
    "sections": [{"_id": "/mods/{owner}/{slug}", "visitors": 2, "views": 5, "engaged": 50, "route": None}],
}
PV_PREV = {
    "totals": [{"_id": "all", "visitors": 1, "visits": 1, "pages": 1, "bounces": 1, "engaged": 0}],
    "median": [{"_id": None, "m": None}],
    "series": [{"_id": "2026-09-22", "visitors": 1, "visits": 1, "pages": 1, "bounces": 1, "engaged": 0}],
    "eng": [{"_id": 0, "n": 1}],
    "eng_med": [],
}


async def pageviews_section(app) -> dict:
    pv.utcnow = lambda: NOW
    out = []
    cases = [
        ("month", "/admin/pageviews", at(1_700_000_000_000), [PV_FACET, PV_PREV]),
        ("day-filtered", "/admin/pageviews?days=1&channel=search&lang=fr&device=", None, [PV_FACET]),
        ("week-recent", "/admin/pageviews?days=7&entry=%2Fmods%2Fx&source=Google", at(1_792_000_000_000), [PV_FACET]),
        ("empty", "/admin/pageviews?days=90", at(1_600_000_000_007), [{}, {}]),
        ("nothing", "/admin/pageviews?days=2", at(1_700_000_000_000), [None, None]),
    ]
    for name, url, started, facets in cases:
        pv._memo.clear()
        plog: list = []
        queue = list(facets)

        async def fake_aggregate(pipeline, queue=queue, plog=plog):
            plog.append(ext(pipeline))
            f = queue.pop(0)
            return [] if f is None else [f]

        async def fake_since(started=started):
            return started

        pv._aggregate = fake_aggregate
        pv.tracking_since = fake_since
        res = await call(app, "GET", url)
        out.append({"name": name, "url": url, "started": ext(started) if started else None,
                    "facets": [ext(f) if f is not None else None for f in facets], "pipelines": plog, **res})
    pv._memo.clear()
    live_facet = {"visitors": [{"n": 3}], "pages": [{"_id": "/", "visitors": 2}, {"_id": "/mods", "visitors": 1}]}
    llog: list = []

    async def live_aggregate(pipeline):
        llog.append(ext(pipeline))
        return [live_facet]

    pv._aggregate = live_aggregate
    live = await call(app, "GET", "/admin/pageviews/live")
    return {"now": NOW.isoformat(), "cases": out,
            "live": {"facet": ext(live_facet), "pipeline": llog[0], **live}}


async def params_section(app) -> list:
    """422s and 405s answered before any handler runs."""
    reqs = [
        ("GET", "/admin/users?days=0"),
        ("GET", "/admin/users?days=abc&days=366"),
        ("GET", "/admin/users/not-an-id"),
        ("GET", "/admin/users/not-an-id/activity?days=0"),
        ("DELETE", "/admin/users/65a000000000000000000a0g/tokens"),
        ("POST", "/admin/users/x/terminate"),
        ("POST", "/admin/tokens/123/revoke"),
        ("GET", "/admin/events?user_id=&status_code=99&limit=201"),
        ("GET", "/admin/events?status_code=1.5&limit=x"),
        ("GET", "/admin/activity?days=366&top=0"),
        ("GET", "/admin/pageviews?days=91&channel=" + "c" * 41 + "&lang=" + "l" * 11 + "&entry=" + "e" * 501),
        ("GET", "/admin/site-users?limit=0&offset=-1"),
        ("GET", "/admin/site-claims?pending_only=maybe"),
        ("POST", "/admin/discord/clear-guild-commands"),
        ("PUT", "/admin/users/65a000000000000000000a01/tokens"),
        ("PATCH", "/admin/config/x"),
        ("GET", "/admin/config/x"),
        ("POST", "/admin/pending"),
        ("GET", "/admin/site-users/x/username"),
    ]
    out = []
    for i, (method, url) in enumerate(reqs):
        res = await call(app, method, url, rid=f"parity-adm-p{i}")
        out.append({"method": method, "url": url, **res})
    bodies = [
        ("/admin/site-users/x/username", b'{"username": "ab"}'),
        ("/admin/site-users/x/username", b"{}"),
        ("/admin/site-users/x/username", b'{"username": 5}'),
        ("/admin/site-users/x/claimed-name", b'{"name": "' + b"n" * 65 + b'"}'),
        ("/admin/site-users/x/claimed-name", b""),
        ("/admin/username-requests/x/reject", b'{"reason": ' + json.dumps("r" * 2001).encode() + b"}"),
        ("/admin/username-requests/x/reject", b"[]"),
    ]
    for i, (url, body) in enumerate(bodies):
        res = await call(app, "POST", url, body, "application/json", rid=f"parity-adm-b{i}")
        out.append({"method": "POST", "url": url, "sent": body.decode(), **res})
    return out


def saves_section() -> dict:
    """What Beanie's ``save()`` ``$set``s for each loaded document."""
    from beanie.odm.utils.dump import get_dict
    out = {}
    for name, cls, raws in (("users", User, USERS), ("tokens", ApiToken, TOKENS), ("site_users", SiteUser, SITE_USERS),
                            ("requests", UsernameChangeRequest, REQUESTS)):
        out[name] = [{"raw": ext(r), "saved": ext(get_dict(cls.model_validate(r), to_db=True))} for r in raws]
    return out


ROUTES = [
    ("GET", "/admin/users"), ("GET", f"/admin/users/{U1}/tokens"), ("DELETE", f"/admin/users/{U1}/tokens"),
    ("POST", f"/admin/users/{U1}/terminate"), ("POST", f"/admin/users/{U1}/reactivate"),
    ("GET", f"/admin/users/{U1}/activity"), ("GET", f"/admin/users/{U1}"), ("POST", f"/admin/tokens/{T1}/revoke"),
    ("GET", "/admin/events"), ("GET", "/admin/pending"), ("GET", "/admin/activity"), ("GET", "/admin/pageviews"),
    ("GET", "/admin/pageviews/live"), ("GET", "/admin/config"), ("PUT", "/admin/config/x"),
    ("DELETE", "/admin/config/x"), ("GET", "/admin/discord/commands"), ("POST", "/admin/discord/register-commands"),
    ("GET", "/admin/bot/stats"), ("POST", "/admin/discord/clear-guild-commands"), ("GET", "/admin/site-claims"),
    ("POST", "/admin/site-claims/x/approve"), ("POST", "/admin/site-claims/x/reject"),
    ("GET", "/admin/username-requests"), ("POST", "/admin/username-requests/x/approve"),
    ("POST", "/admin/username-requests/x/reject"), ("GET", "/admin/site-users"), ("GET", "/admin/site-users/x"),
    ("POST", "/admin/site-users/x/deactivate"), ("POST", "/admin/site-users/x/activate"),
    ("POST", "/admin/site-users/x/logout"), ("POST", "/admin/site-users/x/username"),
    ("POST", "/admin/site-users/x/claimed-name"), ("POST", "/admin/site-users/x/refresh-discord"),
    ("GET", "/admin/embed/uploads"),
    ("HEAD", "/admin/users"), ("PUT", f"/admin/users/{U1}/tokens"), ("POST", "/admin/config/x"),
    ("HEAD", "/admin/config/x"), ("DELETE", "/admin/site-users/x"), ("GET", "/admin/users/not-an-id"),
]


async def unauth_section(app) -> list:
    """Without credentials: routing, 405s, and the body read before the auth dependency."""
    out = []
    for i, (method, url) in enumerate(ROUTES):
        res = await call(app, method, url, rid=f"parity-adm-u{i}")
        out.append({"method": method, "url": url, **res})
    for i, url in enumerate(["/admin/config/x", "/admin/site-users/x/username", "/admin/username-requests/x/reject"]):
        method = "PUT" if "config" in url else "POST"
        res = await call(app, method, url, b'{"value": ', "application/json", rid=f"parity-adm-j{i}")
        out.append({"method": method, "url": url, "body_sent": '{"value": ', **res})
    return out


async def main() -> None:
    async def no_server(self):
        self._database_major_version = 7
        self._existing_collections = []

    beanie_init.Initializer._load_cached_info = no_server
    client = AsyncMongoClient("mongodb://127.0.0.1:1/", tz_aware=True, connect=False)
    await init_beanie(database=client["fixtures"], document_models=[
        User, ApiToken, UsageEvent, SiteUser, UsernameChangeRequest, rc.RuntimeConfig, ModProject, ModpackProject,
    ], skip_indexes=True)
    for key, spec in rc.REGISTRY.items():
        rc._CACHE[key] = (spec.default, float("inf"))
    admin_router.utcnow = lambda: NOW
    usage_service.utcnow = lambda: NOW
    from app.main import app
    app.dependency_overrides[get_current_superuser] = admin_user
    data = {"python": sys.version.split()[0], "now": NOW.isoformat(), "admin_id": ADMIN_ID}
    data["params"] = await params_section(app)
    data["users"] = await users_section(app)
    data["pending"] = await pending_section(app)
    data["config"] = await config_section(app)
    data["site"] = await site_section(app)
    data["misc"] = await misc_section(app)
    data["pageviews"] = await pageviews_section(app)
    app.dependency_overrides.clear()
    data["unauth"] = await unauth_section(app)
    data["saves"] = saves_section()
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(data, ensure_ascii=True, indent=1) + "\n", encoding="utf-8")
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes) with Python {data['python']}")


if __name__ == "__main__":
    asyncio.run(main())
