"""Golden fixtures for kiwi-server's outbound webhooks, DM subscriptions and the
shared embed templates, from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_webhooks_fixtures.py
Writes native/kiwi-server/testdata/webhooks/golden.json.

Pure parts (template rendering, event contexts, URL rules, DM bodies, delivery
health, the Discord retry loops over scripted httpx replies, queue fan-out
with a recording poster) are called directly. Route cases drive app.main.app at
the ASGI level over an in-memory pymongo layer, with the site user as a
dependency override; the "gated" cases (401/405/422 before any store) run
without one. The clock is frozen.
"""
import asyncio
import datetime as dt
import json
import logging
import os
import sys
import types
import warnings
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "webhooks" / "golden.json"
os.chdir(ROOT)
sys.path.insert(0, str(ROOT))
os.environ.setdefault("SECRET_KEY", "webhooks-fixture-secret")

import bson  # noqa: E402
import httpx  # noqa: E402
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

from app.bot import discord_rest  # noqa: E402
from app.core import delivery_health  # noqa: E402
from app.core.config import settings  # noqa: E402
from app.dm_subs import delivery as dm_delivery  # noqa: E402
from app.dm_subs import embeds as dm_embeds  # noqa: E402
from app.dm_subs.models import DmSubscription  # noqa: E402
from app.embed_templates import EmbedTemplate, parse_color, render_template, substitute  # noqa: E402
from app.main import app  # noqa: E402
from app.site_auth.dependencies import get_current_site_user  # noqa: E402
from app.webhooks import delivery as wh_delivery  # noqa: E402
from app.webhooks import embeds as wh_embeds  # noqa: E402
from app.webhooks import service as wh_service  # noqa: E402
from app.webhooks.models import SiteWebhook  # noqa: E402

OPTS = CodecOptions(tz_aware=True)


def ext(v):
    return json.loads(json_util.dumps(v, json_options=CANONICAL_JSON_OPTIONS))


def rt(doc):
    return bson.decode(bson.encode(doc), codec_options=OPTS)


def safe(fn, *a, **kw):
    try:
        return {"ok": fn(*a, **kw)}
    except Exception as e:  # noqa: BLE001
        return {"raises": type(e).__name__}


# --- in-memory pymongo -----------------------------------------------------------------

DB: dict[str, list[dict]] = {}
_IDS = [0]


def _match(doc, flt):
    for k, cond in (flt or {}).items():
        if k == "$and":
            if not all(_match(doc, c) for c in cond):
                return False
            continue
        v = doc.get(k)
        if isinstance(v, list):
            if cond not in v and v != cond:
                return False
        elif v != cond or (k not in doc):
            return False
    return True


class _Cursor:
    def __init__(self, coll, flt, kw):
        self.coll, self.flt, self.kw = coll, flt, kw

    async def to_list(self, length=None):
        docs = [d for d in DB.get(self.coll, []) if _match(d, self.flt)]
        for key, direction in reversed(list(self.kw.get("sort") or [])):
            docs.sort(key=lambda d: d.get(key), reverse=direction < 0)
        return [rt(d) for d in docs]


def _find(self, filter=None, *a, **kw):
    return _Cursor(self.name, filter, kw)


async def _find_one(self, filter=None, *a, **kw):
    for d in DB.get(self.name, []):
        if _match(d, filter):
            return rt(d)
    return None


async def _count(self, filter, *a, **kw):
    return sum(1 for d in DB.get(self.name, []) if _match(d, filter))


async def _insert_one(self, doc, *a, **kw):
    _IDS[0] += 1
    oid = ObjectId("66000000000000000000%04x" % _IDS[0])
    DB.setdefault(self.name, []).append(rt({"_id": oid, **doc}))
    return types.SimpleNamespace(inserted_id=oid)


async def _find_one_and_update(self, filter, update, *a, upsert=False, **kw):
    for i, d in enumerate(DB.get(self.name, [])):
        if _match(d, filter):
            new = dict(d)
            new.update(update.get("$set", {}))
            DB[self.name][i] = rt(new)
            return rt(new)
    if upsert:
        DB.setdefault(self.name, []).append(rt({**filter, **update.get("$set", {})}))
    return None


async def _delete_one(self, filter, *a, **kw):
    docs = DB.get(self.name, [])
    for i, d in enumerate(docs):
        if _match(d, filter):
            del docs[i]
            return types.SimpleNamespace(deleted_count=1)
    return types.SimpleNamespace(deleted_count=0)


AsyncCollection.find = _find
AsyncCollection.find_one = _find_one
AsyncCollection.count_documents = _count
AsyncCollection.insert_one = _insert_one
AsyncCollection.find_one_and_update = _find_one_and_update
AsyncCollection.delete_one = _delete_one


async def _cmd(self, *a, **k):
    return {"version": "7.0.0"}


async def _lcn(self, *a, **k):
    return []


AsyncDatabase.command = _cmd
AsyncDatabase.list_collection_names = _lcn


async def _init():
    from beanie import init_beanie
    from pymongo import AsyncMongoClient
    client = AsyncMongoClient("mongodb://127.0.0.1:1", tz_aware=True, connect=False, serverSelectionTimeoutMS=200)
    await init_beanie(database=client["kiwi"], skip_indexes=True, document_models=[SiteWebhook, DmSubscription])


LOOP = asyncio.new_event_loop()
LOOP.run_until_complete(_init())
run = LOOP.run_until_complete

# --- shared inputs ---------------------------------------------------------------------

USER_ID = ObjectId("65a1b2c3d4e5f60718293a4b")
OTHER_ID = ObjectId("65a1b2c3d4e5f60718293a4c")
DISCORD_ID = 312345678901234567
HOOK_A = ObjectId("65b000000000000000000001")
HOOK_B = ObjectId("65b000000000000000000002")
HOOK_OTHER = ObjectId("65b000000000000000000003")
SUB_A = ObjectId("65c000000000000000000001")
SUB_B = ObjectId("65c000000000000000000002")
SUB_OTHER = ObjectId("65c000000000000000000003")
T0 = dt.datetime(2026, 9, 1, 12, 0, 0, 250000, tzinfo=dt.timezone.utc)
T1 = dt.datetime(2026, 9, 2, 8, 15, 0, tzinfo=dt.timezone.utc)
URL_A = "https://discord.com/api/webhooks/1234567890/AbCdEfGhIjKlMnOp_token-value"


def hook_doc(oid, owner, created, **kw):
    t = EmbedTemplate(enabled=True, title="Hi {name}", fields=[{"name": "A", "value": "{type_label}"}])
    w = SiteWebhook(owner_id=owner, url=kw.pop("url", URL_A), events=kw.pop("events", ["challenge", "game_update"]),
                    label=kw.pop("label", "Main"), templates=kw.pop("templates", {"challenge": t}),
                    created_at=created, updated_at=created, **kw)
    from beanie.odm.utils.dump import get_dict
    return rt({"_id": oid, **get_dict(w, to_db=True)})


def sub_doc(oid, owner, created, **kw):
    s = DmSubscription(owner_id=owner, owner_discord_id=kw.pop("owner_discord_id", DISCORD_ID),
                       events=kw.pop("events", ["challenge", "market_watch"]),
                       filters=kw.pop("filters", {"challenge_types": ["racing"],
                                                  "watch": [{"name": "Dragon Egg", "max_price_each": 50000.0}]}),
                       label=kw.pop("label", "Alerts"), created_at=created, updated_at=created, **kw)
    from beanie.odm.utils.dump import get_dict
    return rt({"_id": oid, **get_dict(s, to_db=True)})


def base_db():
    return {
        "site_webhooks": [
            hook_doc(HOOK_A, USER_ID, T0),
            hook_doc(HOOK_B, USER_ID, T1, url="https://ptb.discord.com/api/v10/webhooks/42/short", events=["mod_release"],
                     templates={}, active=False, consecutive_failures=12, last_status=500,
                     last_error="HTTP 500", disabled_reason="Auto-disabled after 12 failed deliveries.",
                     last_delivered_at=T0),
            hook_doc(HOOK_OTHER, OTHER_ID, T0),
        ],
        "dm_subscriptions": [
            sub_doc(SUB_A, USER_ID, T0),
            sub_doc(SUB_B, USER_ID, T1, events=["game_update"], filters={}, active=False, consecutive_failures=8,
                    disabled_reason="Auto-disabled after 8 failed DMs.", watch_state={"Dragon Egg": 1790000000}),
            sub_doc(SUB_OTHER, OTHER_ID, T0),
        ],
    }


# --- ASGI ----------------------------------------------------------------------------

KEPT = ("content-type", "cache-control", "allow", "www-authenticate", "content-length")
USER = types.SimpleNamespace(id=USER_ID, discord_id=DISCORD_ID)


async def call(method, url, headers, body: bytes):
    path, _, qs = url.partition("?")
    import urllib.parse
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


SENDS: list = []
OUTCOMES: list = []


async def _fake_post(url, body, *, image_design_id=None):
    SENDS.append({"url": url, "body": body, "design": image_design_id})
    return OUTCOMES.pop(0) if OUTCOMES else (True, 204, None)


async def _fake_dm(discord_id, body):
    SENDS.append({"discord_id": discord_id, "body": body})
    return OUTCOMES.pop(0) if OUTCOMES else (True, 200, None)


def snap(kind):
    """The one collection a case can touch: DM cases the subscriptions, the rest the webhooks."""
    keep = "dm_subscriptions" if kind.startswith("dm") else "site_webhooks"
    return {keep: ext(DB.get(keep, []))}


def http_case(name, method, url, *, body=None, raw=None, user=USER, outcomes=(), db=None, ctype="application/json",
              flags=None, headers=()):
    global DB
    DB = db if db is not None else base_db()
    before = snap(name)
    SENDS.clear()
    OUTCOMES[:] = list(outcomes)
    if user is not None:
        app.dependency_overrides[get_current_site_user] = lambda: user
    else:
        app.dependency_overrides.pop(get_current_site_user, None)
    saved = {}
    for k, v in (flags or {}).items():
        saved[k] = rc._CACHE[k]
        rc._CACHE[k] = (v, float("inf"))
    wh_delivery.post_to_discord, real_post = _fake_post, wh_delivery.post_to_discord
    discord_rest.send_dm, real_dm = _fake_dm, discord_rest.send_dm
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
        wh_delivery.post_to_discord = real_post
        discord_rest.send_dm = real_dm
        for k, v in saved.items():
            rc._CACHE[k] = v
        app.dependency_overrides.pop(get_current_site_user, None)
    case = {
        "name": name, "method": method, "url": url, "headers": hdrs,
        "user": None if user is None else {"id": str(user.id), "discord_id": user.discord_id},
        "outcomes": [list(o) for o in outcomes],
        "before": before,
        "status": status,
        "resp_headers": {k: v for k, v in rh if k in KEPT},
        "resp": resp.decode("utf-8"),
        "after": snap(name),
        "sends": json.loads(json.dumps(SENDS, default=str)),
    }
    if body is not None:
        case["body"] = body
    if raw is not None:
        case["raw"] = raw
    return case


def http_cases():
    H, D = "/v1/webhooks", "/v1/dm-subscriptions"
    a, b, o = str(HOOK_A), str(HOOK_B), str(HOOK_OTHER)
    sa, sb, so = str(SUB_A), str(SUB_B), str(SUB_OTHER)
    tmpl = {"enabled": True, "content": "<@&1> {name}", "title": "T {ends_at:R}", "color": "#ff0000",
            "fields": [{"name": "F", "value": "{status}", "inline": False}], "show_image": True,
            "image_design_id": "65d000000000000000000001"}
    c = [
        http_case("wh_events", "GET", f"{H}/events", user=None),
        http_case("wh_list", "GET", H),
        http_case("wh_list_empty", "GET", H, user=types.SimpleNamespace(id=ObjectId(), discord_id=None)),
        http_case("wh_create", "POST", H, body={"url": " https://Discord.com/api/webhooks/987/tok-en_1234567?wait=true#x ",
                                               "events": ["game_update", "challenge", "challenge", "bogus"],
                                               "label": "  New hook  ", "templates": {"challenge": tmpl,
                                                                                         "mod_release": tmpl,
                                                                                         "bogus": tmpl}}),
        http_case("wh_create_params", "POST", H, body={"url": "https://canary.discord.com/api/webhooks/1/2;p=1",
                                                      "events": ["mod_release"]}),
        http_case("wh_create_bad_scheme", "POST", H, body={"url": "http://discord.com/api/webhooks/1/2", "events": ["challenge"]}),
        http_case("wh_create_bad_host", "POST", H, body={"url": "https://evil.com/api/webhooks/1/2", "events": ["challenge"]}),
        http_case("wh_create_no_webhooks", "POST", H, body={"url": "https://discord.com/api/x/1/2", "events": ["challenge"]}),
        http_case("wh_create_short", "POST", H, body={"url": "https://discord.com/api/webhooks/1", "events": ["challenge"]}),
        http_case("wh_create_ipv6_bad", "POST", H, body={"url": "https://[discord.com/api/webhooks/1/2", "events": ["challenge"]}),
        http_case("wh_create_no_events", "POST", H, body={"url": URL_A, "events": ["nope"]}),
        http_case("wh_create_limit", "POST", H, body={"url": URL_A, "events": ["challenge"]},
                  db={"site_webhooks": [hook_doc(ObjectId(), USER_ID, T0) for _ in range(15)], "dm_subscriptions": []}),
        http_case("wh_create_422", "POST", H, body={"url": 5, "events": [], "label": "x" * 81,
                                                   "templates": {"challenge": {"enabled": "maybe", "fields": [{"inline": "x"}, 3]},
                                                                 "game_update": None, "mod_release": "str"}}),
        http_case("wh_create_422_missing", "POST", H, body={}),
        http_case("wh_create_422_list", "POST", H, body=[1, 2]),
        http_case("wh_create_422_empty", "POST", H),
        http_case("wh_create_422_text", "POST", H, raw="url=x", ctype="text/plain"),
        http_case("wh_update", "PATCH", f"{H}/{a}", body={"label": " Renamed ", "events": ["game_update"]}),
        http_case("wh_update_templates", "PATCH", f"{H}/{a}", body={"templates": {"game_update": tmpl, "challenge": tmpl}}),
        http_case("wh_update_reactivate", "PATCH", f"{H}/{b}", body={"active": True, "url": "https://discordapp.com/api/webhooks/5/6/"}),
        http_case("wh_update_deactivate", "PATCH", f"{H}/{a}", body={"active": False}),
        http_case("wh_update_bad_url", "PATCH", f"{H}/{a}", body={"url": "nope"}),
        http_case("wh_update_other", "PATCH", f"{H}/{o}", body={"label": "x"}),
        http_case("wh_update_badid", "PATCH", f"{H}/not-an-id", body={"label": "x"}),
        http_case("wh_update_events_path", "PATCH", f"{H}/events", body={"label": "x"}),
        http_case("wh_update_422", "PATCH", f"{H}/{a}", body={"active": "perhaps", "events": "x"}),
        http_case("wh_delete", "DELETE", f"{H}/{a}"),
        http_case("wh_delete_missing", "DELETE", f"{H}/65b0000000000000000000ff"),
        http_case("wh_test_generic", "POST", f"{H}/{a}/test", outcomes=[(False, 404, "Unknown Webhook")]),
        http_case("wh_test_event", "POST", f"{H}/{a}/test?event=challenge"),
        http_case("wh_test_event_default", "POST", f"{H}/{a}/test?event=game_update&event=mod_release"),
        http_case("wh_update_design", "PATCH", f"{H}/{a}", body={"templates": {"challenge": tmpl}}),
        http_case("wh_test_design", "POST", f"{H}/{a}/test?event=challenge",
                  db={"site_webhooks": [hook_doc(HOOK_A, USER_ID, T0, templates={"challenge": EmbedTemplate(**tmpl)})],
                      "dm_subscriptions": []}),
        http_case("wh_test_design_off", "POST", f"{H}/{a}/test?event=challenge",
                  db={"site_webhooks": [hook_doc(HOOK_A, USER_ID, T0, templates={"challenge": EmbedTemplate(
                      **{**tmpl, "show_image": False})})], "dm_subscriptions": []}),
        http_case("wh_test_unknown_event", "POST", f"{H}/{a}/test?event=nope"),
        http_case("wh_test_other", "POST", f"{H}/{o}/test"),
        http_case("dm_events", "GET", f"{D}/events", user=None),
        http_case("dm_list", "GET", D),
        http_case("dm_list_unlinked", "GET", D, user=types.SimpleNamespace(id=USER_ID, discord_id=None)),
        http_case("dm_create", "POST", D, body={"events": ["market_watch", "challenge", "fluxion"], "label": " Me ",
                                               "filters": {"challenge_types": ["target", "racing", "racing"],
                                                           "watch": [{"name": "  Dragon Egg  ", "max_price_each": "1_500.5"},
                                                                     {"name": "", "max_price_each": 5},
                                                                     {"name": "Zero", "max_price_each": 0},
                                                                     "loose", {"name": "x" * 130, "max_price_each": True},
                                                                     {"name": "NaN", "max_price_each": "nan"},
                                                                     {"name": "Inf", "max_price_each": "inf"}],
                                                           "extra": 1}}),
        http_case("dm_create_unlinked", "POST", D, body={"events": ["challenge"]},
                  user=types.SimpleNamespace(id=USER_ID, discord_id=0)),
        http_case("dm_create_bad_type", "POST", D, body={"events": ["challenge"], "filters": {"challenge_types": ["racing", "boss"]}}),
        http_case("dm_create_type_str", "POST", D, body={"events": ["challenge"], "filters": {"challenge_types": "racing"}}),
        http_case("dm_create_type_int", "POST", D, body={"events": ["challenge"], "filters": {"challenge_types": [1]}}),
        http_case("dm_create_type_dict", "POST", D, body={"events": ["challenge"], "filters": {"challenge_types": {"rampage": 1}}}),
        http_case("dm_create_type_num", "POST", D, body={"events": ["challenge"], "filters": {"challenge_types": 3}}),
        http_case("dm_create_no_watch", "POST", D, body={"events": ["market_watch"], "filters": {"watch": [{"name": "A"}]}}),
        http_case("dm_create_watch_name_int", "POST", D, body={"events": ["market_watch"], "filters": {"watch": [{"name": 7}]}}),
        http_case("dm_create_too_many", "POST", D, body={"events": ["market_watch"],
                                                        "filters": {"watch": [{"name": f"I{i}", "max_price_each": i + 1} for i in range(26)]}}),
        http_case("dm_create_no_events", "POST", D, body={}),
        http_case("dm_create_limit", "POST", D, body={"events": ["challenge"]},
                  db={"site_webhooks": [], "dm_subscriptions": [sub_doc(ObjectId(), USER_ID, T0) for _ in range(10)]}),
        http_case("dm_create_422", "POST", D, body={"events": "x", "filters": [1], "label": 5}),
        http_case("dm_update_events", "PATCH", f"{D}/{sa}", body={"events": ["market_watch"]}),
        http_case("dm_update_filters", "PATCH", f"{D}/{sa}", body={"filters": {"challenge_types": ["dungeon"]}}),
        http_case("dm_update_reactivate", "PATCH", f"{D}/{sb}", body={"active": True, "label": "Back"}),
        http_case("dm_update_reactivate_unlinked", "PATCH", f"{D}/{sb}", body={"active": True},
                  user=types.SimpleNamespace(id=USER_ID, discord_id=None)),
        http_case("dm_update_label_only", "PATCH", f"{D}/{sa}", body={"label": "  "}),
        http_case("dm_update_other", "PATCH", f"{D}/{so}", body={"label": "x"}),
        http_case("dm_update_422", "PATCH", f"{D}/{sa}", body={"active": [], "filters": "x"}),
        http_case("dm_delete", "DELETE", f"{D}/{sa}"),
        http_case("dm_delete_missing", "DELETE", f"{D}/zzz"),
        http_case("dm_test", "POST", f"{D}/{sa}/test", outcomes=[(False, 403, "Cannot send messages to this user")]),
        http_case("dm_test_ok", "POST", f"{D}/{sb}/test"),
    ]
    return c


def gated_cases():
    H, D = "/v1/webhooks", "/v1/dm-subscriptions"
    oid = str(HOOK_A)
    c = []
    for name, method, url, body in [
        ("wh_list_noauth", "GET", H, None),
        ("wh_create_noauth", "POST", H, {"url": "x"}),
        ("wh_patch_noauth", "PATCH", f"{H}/{oid}", {}),
        ("wh_delete_noauth", "DELETE", f"{H}/{oid}", None),
        ("wh_test_noauth", "POST", f"{H}/{oid}/test", None),
        ("wh_events_patch_noauth", "PATCH", f"{H}/events", {}),
        ("dm_list_noauth", "GET", D, None),
        ("dm_create_noauth", "POST", D, {}),
        ("dm_test_noauth", "POST", f"{D}/x/test", None),
        ("wh_list_put_405", "PUT", H, None),
        ("wh_list_delete_405", "DELETE", H, None),
        ("wh_id_get_405", "GET", f"{H}/{oid}", None),
        ("wh_id_post_405", "POST", f"{H}/{oid}", None),
        ("wh_test_get_405", "GET", f"{H}/{oid}/test", None),
        ("wh_events_post_405", "POST", f"{H}/events", None),
        ("wh_events_put_405", "PUT", f"{H}/events", None),
        ("dm_list_patch_405", "PATCH", D, None),
        ("dm_id_get_405", "GET", f"{D}/x", None),
        ("dm_test_delete_405", "DELETE", f"{D}/x/test", None),
        ("dm_events_post_405", "POST", f"{D}/events", None),
    ]:
        c.append(http_case(name, method, url, body=body, user=None))
    c.append(http_case("wh_create_badjson", "POST", H, raw='{"url": ', user=None))
    c.append(http_case("dm_patch_badjson", "PATCH", f"{D}/x", raw="[1,", user=None))
    c.append(http_case("wh_bad_bearer", "GET", H, user=None, headers=[("authorization", "Bearer nope")]))
    c.append(http_case("wh_events_off", "GET", f"{H}/events", user=None, flags={"feature_webhooks_enabled": False}))
    c.append(http_case("wh_list_off_noauth", "GET", H, user=None, flags={"feature_webhooks_enabled": False}))
    c.append(http_case("dm_events_off", "GET", f"{D}/events", user=None, flags={"feature_dm_subscriptions_enabled": False}))
    c.append(http_case("wh_off_badjson", "POST", H, raw="{", user=None, flags={"feature_webhooks_enabled": False}))
    c.append(http_case("wh_off_405", "PUT", H, user=None, flags={"feature_webhooks_enabled": False}))
    return c


# --- pure parts ------------------------------------------------------------------------

CTX = {"name": "Racing Challenge", "ends_at": 1718201200, "f": 1718201200.9, "s": " 12 ", "bad": "x1", "b": True,
       "none": None, "lst": [1, "a", None, 2.5, {"k": False}], "d": {"a": 1}, "fl": 2.5, "big": 12345678901234567890,
       "neg": -3, "u": "ünï ✓"}


def template_cases():
    out = []
    texts = ["{name}", "{name:R}", "{ends_at:R}", "{ends_at:t}{ends_at:x}", "{f:F}", "{s:d}", "{bad:R}", "{b:R}",
             "{none}", "{none:R}", "{lst}", "{lst:R}", "{d}", "{fl}", "{big}", "{neg:D}", "{missing}", "{u}",
             "{name:RR}", "{ na}", "{name}{", "{{name}}", "", None, "plain", "{NAME}", "{f}"]
    for t in texts:
        out.append({"kind": "substitute", "text": t, "out": substitute(t, CTX)})
    for col in ["#F2A33C", "f2a33c", "##123456", " #00ff00 ", "123", "-5", "1_000", "#12345", "1234567", "zz",
                "0x10", "", None, " 42 ", "\u0661\u0662"]:
        out.append({"kind": "color", "text": col, "out": parse_color(col)})
    tmpls = [
        {},
        {"enabled": True, "title": "  {name}  ", "url": " https://x/{name} ", "description": "D " * 3000,
         "color": "#46D39A", "fields": [{"name": "", "value": ""}, {"name": "{none}", "value": "v"},
                                         {"name": "n", "value": "", "inline": False}] + [{"name": f"f{i}", "value": "x"} for i in range(30)],
         "footer": "  ", "content": "  <@1> {name}  ", "show_image": True, "image_url": "  "},
        {"title": "x" * 300, "url": "ftp://nope", "color": "999", "footer": "y" * 3000, "content": "c" * 2500,
         "image_url": " https://img/{name}.png ", "show_image": True},
        {"title": "t", "show_image": True, "image_design_id": "abc", "image_url": "https://ignored"},
        {"title": "t", "show_image": False, "image_url": "https://ignored"},
        {"title": "{lst} {d}", "description": "{b} {fl}", "color": "nope"},
    ]
    for i, t in enumerate(tmpls):
        for default in [None, "https://default/img.png", ""]:
            embed, content = render_template(EmbedTemplate(**t), CTX, default_image_url=default)
            out.append({"kind": "render", "template": t, "default_image_url": default, "embed": embed, "content": content})
    return out


def webhook_payloads():
    return {
        "challenge": [
            {"name": "Racing Challenge", "type": "racing", "starts_at": 1718200000, "ends_at": 1718201200,
             "active": True, "is_friday_window": False, "captured_at": "2026-10-22 09:00:00+00:00"},
            {"name": None, "type": None, "active": False},
            {"type": "boss_rush_x", "ends_at": "1718201200", "is_friday_window": 1},
            {"type": "ǆungle_ßtorm", "ends_at": 1.5e9},
            {"type": 5},
            {},
            None,
            [1],
        ],
        "mod_release": [
            {"project": {"title": "Neon HUD", "slug": "neon-hud", "handle": "aallyn", "owner": "Aallyn",
                         "summary": "  A   " + "s" * 250, "tags": ["ui", "", "hud"], "download_count": 1234567,
                         "star_count": "42"},
             "release": {"tag": "v2", "title": None, "branch": "main", "format": "tmod", "size": 5_000_000,
                         "changelog": "line\n" * 300, "sha256": "abcdef0123456789abcdef", "filename": "x.tmod"},
             "previous": {"tag": "v1"}, "change": {"size_delta": -2048, "release_count": 3, "is_first_release": False},
             "download_url": "https://dl", "page_url": "", "image_url": "https://img/x.png"},
            {"project": {"slug": "s", "tags": "ab"}, "release": {"size": 512.9, "format": 3, "sha256": [1, 2]},
             "change": {"size_delta": 0, "is_first_release": True}},
            {"project": {}, "release": {"size": "nan"}, "change": {"size_delta": "7"}},
            {"project": {"tags": [1]}},
            {"project": {"tags": 5}},
            {"project": "nope"},
            {"release": {"sha256": 5}},
            {"release": {"size": "-inf"}},
            {"release": {"size": 2048.5, "tag": 7}, "previous": {"tag": 6.5}},
            {},
        ],
        "game_update": [
            {"version": {"branch": "live-us", "ordinal": 142, "version_tag": "2024.06.01", "files_added": 12,
                         "files_modified": 8, "files_removed": 1, "bytes_added": 5, "completed_at": "2026-10-22T09:00:00+00:00"}},
            {"version": {"ordinal": 0, "files_added": 1e3}},
            {"version": {"ordinal": "x7", "files_added": "1,000"}},
            {"version": None},
            {"version": "v"},
            {},
        ],
        "bogus": [{}],
    }


def context_cases():
    asset = settings.asset_url
    out = {"asset": asset, "items": []}
    for event, payloads in webhook_payloads().items():
        for data in payloads:
            item = {"event": event, "data": data}
            spec = wh_embeds._TYPES.get(event)
            if spec is not None:
                item["context"] = safe(spec[0], data or {})
            for tname, tmpl in [("none", None), ("custom", EmbedTemplate(enabled=True, title="{name}{project_title}{version_tag}",
                                                                        description="{type_label}|{tags}|{size_label}|{size_delta}|{downloads}|{files_added}|{sha_short}|{version_change}",
                                                                        content="{status} {is_friday} {ordinal}",
                                                                        fields=[{"name": "{update_kind}", "value": "{summary}"}],
                                                                        show_image=True)),
                                ("disabled", EmbedTemplate(enabled=False, title="never"))]:
                item[f"render_{tname}"] = safe(wh_embeds.render, event, data, tmpl)
            out["items"].append(item)
    for event in ("challenge", "mod_release", "game_update", "bogus"):
        out["items"].append({"event": event, "sample_default": wh_embeds.render_sample(event),
                             "variables": wh_embeds.variables(event)})
    out["test_body"] = wh_embeds.test_body()
    return out


def url_cases():
    raws = [URL_A, " https://DISCORD.com:443/api/webhooks/1/2 ", "https://user:pw@discord.com/api/webhooks/1/2",
            "https://discord.com/api/webhooks/1/2/extra?x=1#f", "https://discord.com/api/webhooks/1/2;params",
            "https://discord.com/a;b/webhooks/1/2", "https://discord.com/api/web\thooks/1/2",
            "\x00 https://discord.com/api/webhooks/1/2", "HTTPS://discord.com/api/webhooks/1/2",
            "https://discord.com//webhooks//1//2", "https://discord.com/webhooks/1/2", "https://discord.com/webhooks/1",
            "https://discord.com/x/y", "https:discord.com/api/webhooks/1/2", "https://ptb.discord.com/api/webhooks/a/b",
            "https://canary.discordapp.com/api/webhooks/1/2", "https://discord.com./api/webhooks/1/2",
            "https://[::1]/api/webhooks/1/2", "https://[::1/api/webhooks/1/2", "https://a[b]@discord.com/api/webhooks/1/2",
            "https://[1.2.3.4]/api/webhooks/1/2", "https://[v1.x]/api/webhooks/1/2", "https://[vz.x]/x",
            "https://x[::1]/a", "https://[::1]x/a", "https://discord\uff0fcom/api/webhooks/1/2",
            "https://disc\u00f6rd.com/api/webhooks/1/2", "https://discord.com:abc/api/webhooks/1/2",
            "https://DISCORD.COM/API/WEBHOOKS/1/2", "https://discord.com/api/webhooks/1/2/", "", "   ",
            "javascript:alert(1)", "https://discord.com", "1https://discord.com/api/webhooks/1/2",
            "https://discord.com/api/webhooks/%31/%32", "//discord.com/api/webhooks/1/2"]
    out = []
    for r in raws:
        try:
            out.append({"raw": r, "ok": wh_service.normalise_url(r)})
        except Exception as e:  # noqa: BLE001
            msg = getattr(e, "message", None)
            out.append({"raw": r, "status": getattr(e, "status_code", 500), "message": msg if msg else None,
                        "raises": type(e).__name__})
    masks = []
    for u in [URL_A, "https://discord.com/api/webhooks/1/abcdefg", "https://discord.com/api/webhooks/1/abcdef",
              "https://discord.com/api/webhooks/1/", "https://discord.com", "https://discord.com/x;y/z12345678",
              "https://discord.com/api/webhooks/1/\u00fc\u00f1\u00ef\u00e7\u00f8d\u00e9\u00df"]:
        masks.append({"url": u, "out": wh_service._mask_url(u)})
    return {"normalise": out, "mask": masks}


def dm_cases():
    payloads = [
        ("challenge", {"name": "Racing Challenge", "window_ends_at": 1718201200, "ends_at": 5}),
        ("challenge", {"name": "  Collection Challenge ", "ends_at": "1718201200"}),
        ("challenge", {"name": "RAMPAGE ALERT!", "window_ends_at": 0, "ends_at": 1718201200.9}),
        ("challenge", {"name": "Target Challenge"}),
        ("challenge", {"name": "Deep Dungeon", "ends_at": 300000000000}),
        ("challenge", {"name": "", "ends_at": -5}),
        ("challenge", {"name": 5}),
        ("challenge", {"ends_at": "abc"}),
        ("challenge", {"ends_at": [1]}),
        ("challenge", None),
        ("corruxion", {"active": True, "starts_at": 1, "ends_at": 1718201200}),
        ("fluxion", {"ends_at": None}),
        ("fluxion", {"ends_at": 99999999999999999999}),
        ("game_update", {"version": {"branch": "live-us", "ordinal": 142, "version_tag": "2024.06.01",
                                     "files_added": 1, "completed_at": "2026-10-22T09:00:00+00:00", "x": None, "f": 1.5,
                                     "t": True, "l": ["a", 1]}}),
        ("game_update", {"version_tag": "v9", "branch": "pts"}),
        ("game_update", {"tag": "t1", "version": "x"}),
        ("game_update", {}),
        ("market_watch", {"name": "Dragon Egg", "price_each": 1234567.0, "max_price_each": 1500000, "median_each": 999.5}),
        ("market_watch", {"name": None, "price_each": 999.5, "max_price_each": 1000.0}),
        ("market_watch", {"price_each": "x", "median_each": 0.5}),
        ("market_watch", {"price_each": 1000000.0}),
        ("market_watch", {"price_each": 2500.0}),
        ("market_watch", {"price_each": 1500.0, "max_price_each": 1e7}),
        ("market_watch", {"price_each": 2.5}),
        ("market_watch", {"price_each": 3.5}),
        ("market_watch", {"price_each": -1234.5}),
        ("market_watch", {"price_each": "inf"}),
        ("market_watch", {"price_each": "nan"}),
        ("unknown", {}),
        ("luxion", {"name": "x"}),
    ]
    out = []
    for event, data in payloads:
        out.append({"event": event, "data": data, "body": safe(dm_embeds.build, event, data)})
    return {"build": out, "test_body": dm_embeds.test_body()}


def health_cases():
    out = []
    starts = [dict(active=True, consecutive_failures=0), dict(active=True, consecutive_failures=11),
              dict(active=True, consecutive_failures=7, last_error="old", last_status=500)]
    outcomes = [(True, 204, None), (False, 404, "gone"), (False, 410, ""), (False, 500, "boom"), (False, None, "net"),
                (False, 403, "nope"), (False, 429, "rate limited by Discord")]
    for feature, kw in [
        ("webhooks", dict(permanent_statuses=(404, 410), permanent_reason="Discord webhook was deleted (HTTP {status}).",
                          auto_disable_reason="Auto-disabled after {failures} failed deliveries.", max_failures=12)),
        ("dm_subs", dict(permanent_statuses=(403,), permanent_reason="The bot can't DM you (share a server with it and allow DMs).",
                         auto_disable_reason="Auto-disabled after {failures} failed DMs.", max_failures=8)),
    ]:
        for start in starts:
            for ok, status, error in outcomes:
                doc = types.SimpleNamespace(active=True, consecutive_failures=0, last_status=None, last_error=None,
                                            last_delivered_at=None, disabled_reason=None, updated_at=None)
                for k, v in start.items():
                    setattr(doc, k, v)
                before = dict(vars(doc))
                delivery_health.record_delivery(doc, ok, status, error, **kw)
                after = {k: (v.isoformat() if isinstance(v, dt.datetime) else v) for k, v in vars(doc).items()}
                out.append({"feature": feature, "start": before, "outcome": [ok, status, error], "after": after})
    return out


class Scripted(httpx.AsyncBaseTransport):
    def __init__(self, replies):
        self.replies = list(replies)
        self.requests = []

    async def handle_async_request(self, request):
        body = await request.aread()
        ct = request.headers.get("content-type", "")
        self.requests.append({"url": str(request.url), "auth": request.headers.get("authorization"),
                              "json": json.loads(body) if ct == "application/json" else None,
                              "raw": body.decode() if ct == "application/json" else None,
                              "multipart": ct.startswith("multipart/form-data"),
                              "payload_json": _payload_json(body) if ct.startswith("multipart") else None})
        r = self.replies.pop(0)
        if "error" in r:
            raise httpx.ConnectError(r["error"])
        headers = {"Retry-After": r["retry_after"]} if "retry_after" in r else {}
        return httpx.Response(r["status"], headers=headers, content=r.get("text", "").encode())


def _payload_json(body: bytes):
    marker = b'name="payload_json"\r\n\r\n'
    i = body.index(marker) + len(marker)
    return body[i:body.index(b"\r\n--", i)].decode()


def post_cases():
    scripts = [
        [{"status": 204}],
        [{"status": 429, "retry_after": "2.5"}, {"status": 200}],
        [{"status": 429, "retry_after": "30"}, {"status": 429}, {"status": 429, "retry_after": "x"}],
        [{"status": 429, "retry_after": "abc"}, {"status": 204}],
        [{"status": 404, "text": '{"message": "Unknown Webhook", "code": 10015}'}],
        [{"status": 400, "text": ""}],
        [{"status": 500, "text": "x" * 300}, {"status": 502, "text": ""}, {"status": 503, "text": "busy"}],
        [{"error": "[Errno -2] Name or service not known"}, {"status": 204}],
        [{"error": "e1"}, {"error": "e2"}, {"error": "e3" * 150}],
        [{"status": 302, "text": "moved"}, {"status": 301}, {"status": 204}],
        [{"status": 500}, {"error": "late"}, {"status": 410, "text": "gone"}],
    ]
    body = {"embeds": [{"title": "Hé ✓", "color": 1}], "content": "x"}
    out = []
    for i, script in enumerate(scripts):
        for png in (None, b"\x89PNGdata") if i < 2 else (None,):
            transport = Scripted(script)
            sleeps = []

            async def fake_sleep(s, _sl=sleeps):
                _sl.append(s)
            real_client, real_sleep, real_render = httpx.AsyncClient, asyncio.sleep, wh_delivery._render_attachment
            wh_delivery.httpx = types.SimpleNamespace(AsyncClient=lambda **kw: real_client(transport=transport, **kw),
                                                      Response=httpx.Response)

            async def fake_render(design_id, _png=png):
                return {"files[0]": ("embed-image.png", _png, "image/png")} if (design_id and _png) else None
            wh_delivery._render_attachment = fake_render
            wh_delivery.asyncio = types.SimpleNamespace(sleep=fake_sleep, gather=asyncio.gather)
            try:
                res = run(wh_delivery.post_to_discord("https://discord.com/api/webhooks/1/2", body,
                                                      image_design_id="d" if png else None))
            finally:
                wh_delivery.httpx = httpx
                wh_delivery.asyncio = asyncio
                wh_delivery._render_attachment = real_render
                del real_sleep
            out.append({"script": script, "png": png is not None, "outcome": list(res), "sleeps": sleeps,
                        "requests": transport.requests})
    return out


def dm_send_cases():
    scripts = [
        ("tok", [{"status": 200, "text": '{"id": "555"}'}, {"status": 200}]),
        ("tok", [{"status": 200, "text": '{"id": 555}'}, {"status": 429, "retry_after": "0.5"}, {"status": 429},
                 {"status": 429, "retry_after": "nope"}]),
        ("tok", [{"status": 200, "text": '{"id": "1"}'}, {"status": 429, "retry_after": "9"}, {"status": 201}]),
        ("tok", [{"status": 403, "text": '{"message": "Cannot send messages to this user", "code": 50007}'}]),
        ("tok", [{"status": 403, "text": ""}]),
        ("tok", [{"status": 200, "text": '{"id": ""}'}]),
        ("tok", [{"status": 200, "text": "{}"}]),
        ("tok", [{"status": 200, "text": "not json"}]),
        ("tok", [{"status": 200, "text": "[1]"}]),
        ("tok", [{"status": 200, "text": '{"id": "9"}'}, {"status": 500, "text": "oops"}]),
        ("tok", [{"error": "[Errno 111] Connection refused"}]),
        ("tok", [{"status": 200, "text": '{"id": "9"}'}, {"error": "boom"}]),
        ("", []),
        (None, []),
    ]
    out = []
    body = {"embeds": [{"title": "🥝 x"}]}
    for token, script in scripts:
        transport = Scripted(script)
        sleeps = []

        async def fake_sleep(s, _sl=sleeps):
            _sl.append(s)
        real_client = httpx.AsyncClient
        discord_rest.httpx = types.SimpleNamespace(AsyncClient=lambda **kw: real_client(transport=transport, **kw),
                                                   HTTPError=httpx.HTTPError)
        real_sleep = discord_rest._sleep
        discord_rest._sleep = fake_sleep
        settings.discord_bot_token, real_token = token, settings.discord_bot_token
        try:
            res = safe(lambda: run(discord_rest.send_dm(DISCORD_ID, body)))
        finally:
            discord_rest.httpx = httpx
            discord_rest._sleep = real_sleep
            settings.discord_bot_token = real_token
        out.append({"token": token, "script": script, "result": {"ok": list(res["ok"])} if "ok" in res else res,
                    "sleeps": sleeps, "requests": transport.requests})
    return out


def queue_cases():
    """Fan-out from one decoded queue job: who gets what, and the stored health after."""
    out = []
    jobs = [
        {"type": "challenge", "data": {"name": "Racing Challenge", "type": "racing", "starts_at": 1718200000,
                                       "ends_at": 1718201200, "active": True, "is_friday_window": False}, "ts": 1},
        {"type": "game_update", "data": {"version": {"branch": "live-us", "ordinal": 7, "version_tag": "v7"}}, "ts": 1},
        {"type": "mod_release", "data": {"project": {"title": "M"}}, "ts": 1},
        {"type": "corruxion", "data": {"active": True, "ends_at": 1718201200}, "ts": 1},
        {"type": "challenge", "data": {"name": "Target Challenge", "ends_at": 1718201200}},
        {"type": "challenge", "data": {"name": 5}},
        {"type": "market_watch", "data": {}},
        {"type": None},
        {"data": {}},
        [1, 2],
    ]
    for job in jobs:
        for feature in ("webhooks", "dm_subs"):
            global DB
            DB = base_db()
            DB["site_webhooks"].append(hook_doc(ObjectId("65b0000000000000000000a1"), OTHER_ID, T1,
                                                events=["challenge", "mod_release"], templates={}))
            DB["dm_subscriptions"].append(sub_doc(ObjectId("65c0000000000000000000a1"), OTHER_ID, T1,
                                                  events=["challenge", "game_update", "corruxion"], filters={},
                                                  consecutive_failures=7))
            before = snap(feature)
            SENDS.clear()
            OUTCOMES[:] = [(False, 410, "Unknown Webhook"), (False, 500, "HTTP 500"), (True, 200, None)]
            if feature == "webhooks":
                wh_delivery.post_to_discord, real = _fake_post, wh_delivery.post_to_discord
                try:
                    res = safe(lambda: run(wh_delivery._deliver(job)))
                finally:
                    wh_delivery.post_to_discord = real
            else:
                discord_rest.send_dm, real = _fake_dm, discord_rest.send_dm
                try:
                    res = safe(lambda: run(dm_delivery._deliver(job)))
                finally:
                    discord_rest.send_dm = real
            out.append({"feature": feature, "job": job, "raises": res.get("raises"), "before": before,
                        "outcomes": [[False, 410, "Unknown Webhook"], [False, 500, "HTTP 500"], [True, 200, None]],
                        "sends": json.loads(json.dumps(SENDS, default=str)),
                        "after": snap(feature)})
    return out


def market_cases():
    out = []
    now = 1792000000
    for prices, outcomes in [
        ({"Dragon Egg": 45000.0, "Other": 1.0}, [(True, 200, None)]),
        ({"Dragon Egg": 50000.0}, [(False, 403, "blocked")]),
        ({"Dragon Egg": 50000.5}, []),
        ({"Dragon Egg": 1.0}, [(False, 500, "x")]),
        ({}, []),
    ]:
        global DB
        DB = base_db()
        DB["dm_subscriptions"].append(sub_doc(ObjectId("65c0000000000000000000b1"), OTHER_ID, T1, events=["market_watch"],
                                              filters={"watch": [{"name": "Dragon Egg", "max_price_each": 60000.0},
                                                                 {"name": "Other", "max_price_each": 2}]},
                                              watch_state={"Dragon Egg": now - 3600, "Other": now - 13 * 3600}))
        before = snap("dm")
        SENDS.clear()
        outcomes = list(outcomes) + [(True, 200, None)] * 4
        OUTCOMES[:] = outcomes
        discord_rest.send_dm, real = _fake_dm, discord_rest.send_dm
        real_time = dm_delivery.time
        dm_delivery.time = types.SimpleNamespace(time=lambda: now + 0.75)
        try:
            res = safe(lambda: run(dm_delivery.check_market(prices)))
        finally:
            discord_rest.send_dm = real
            dm_delivery.time = real_time
        out.append({"prices": prices, "now": now, "outcomes": [list(o) for o in outcomes],
                    "raises": res.get("raises"), "before": before, "sends": json.loads(json.dumps(SENDS, default=str)),
                    "after": snap("dm")})
    return out


def main():
    golden = {
        "now": NOW.isoformat(),
        "asset": settings.asset_url,
        "templates": template_cases(),
        "contexts": context_cases(),
        "urls": url_cases(),
        "dm": dm_cases(),
        "health": health_cases(),
        "post": post_cases(),
        "dm_send": dm_send_cases(),
        "queue": queue_cases(),
        "market": market_cases(),
        "http": http_cases(),
        "gated": gated_cases(),
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(golden, ensure_ascii=False, indent=1, default=str) + "\n", encoding="utf-8")
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
