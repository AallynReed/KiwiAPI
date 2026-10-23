"""Golden fixtures for kiwi-bot (the Rust gateway bot), from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_bot_fixtures.py
Writes native/kiwi-server/testdata/bot/golden.json.

The Python bot's own functions run against an in-memory pymongo layer, a
discord.py client whose HTTPClient.request is replaced by a recorder with
scripted replies, and patched service reads (the "sources" each scenario
lists). Every Discord request (method, path, JSON or multipart body) and the
final documents are recorded. The bot container has no site/ tree, so the
site locale catalogs are switched off like in production. The clock is frozen.
"""
import asyncio
import copy
import datetime as dt
import io
import json
import logging
import os
import sys
import time
import types
import urllib.parse
import warnings
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "bot" / "golden.json"
os.chdir(ROOT)
sys.path.insert(0, str(ROOT))
os.environ.setdefault("SECRET_KEY", "bot-fixture-secret")
os.environ["ASSET_BASE_URL"] = "https://api.aallyn.net"
os.environ.pop("POSTGRES_DSN", None)

NOW = dt.datetime(2026, 10, 22, 14, 7, 21, 654321, tzinfo=dt.timezone.utc)
NOW_TS = NOW.timestamp()


class _Frozen(dt.datetime):
    @classmethod
    def now(cls, tz=None):
        return NOW if tz is not None else NOW.replace(tzinfo=None)


time.time = lambda: NOW_TS

import bson  # noqa: E402
import discord  # noqa: E402
from bson import ObjectId, json_util  # noqa: E402
from bson.codec_options import CodecOptions  # noqa: E402
from bson.json_util import CANONICAL_JSON_OPTIONS  # noqa: E402
from pymongo.asynchronous.collection import AsyncCollection  # noqa: E402
from pymongo.asynchronous.database import AsyncDatabase  # noqa: E402

import app.core.utils as cu  # noqa: E402

cu.datetime = _Frozen

from app import i18n  # noqa: E402

i18n._SITE_LOCALES = ROOT / "no-site-in-the-bot-container"
i18n._catalog.cache_clear()

import app.core.database as core_db  # noqa: E402
import app.core.internal_api as internal_api  # noqa: E402
import app.discord.embeds as embeds  # noqa: E402
import app.giveaways.service as gw_service  # noqa: E402
import app.images.service as images_service  # noqa: E402
import app.trove.captures as captures  # noqa: E402
import app.trove.chaos as chaos  # noqa: E402
import app.trove.luxion as luxion  # noqa: E402
import app.trove.news as news  # noqa: E402
import app.trove.rotations as rotations  # noqa: E402
import app.trove.server_time as server_time  # noqa: E402
import app.trove.status as status  # noqa: E402
import app.trove.updates.read as updates_read  # noqa: E402
from app.bot import announcer, liveboard, market_watch, reconcile, stats  # noqa: E402
from app.bot.avatar_sync import sync_avatar  # noqa: E402
from app.bot.models import GuildConfig, TrackedAnnouncement  # noqa: E402
from app.bot.runner import _EVENT_TO_ANNOUNCEMENT  # noqa: E402
from app.discord import embed_contexts  # noqa: E402
from app.embed_templates import EmbedTemplate, render_template  # noqa: E402
from app.giveaways.models import Giveaway  # noqa: E402
from app.site_auth.models import SiteUser  # noqa: E402

embeds.datetime = _Frozen
OPTS = CodecOptions(tz_aware=True)


def ext(v):
    return json.loads(json_util.dumps(v, json_options=CANONICAL_JSON_OPTIONS))


def rt(doc):
    return bson.decode(bson.encode(doc), codec_options=OPTS)


# --- in-memory pymongo -----------------------------------------------------------------

DB: dict[str, list[dict]] = {}
_IDS = [0]


def _cond(v, cond):
    if isinstance(cond, dict) and cond and all(k.startswith("$") for k in cond):
        for op, arg in cond.items():
            if op == "$lte":
                if v is None or not v <= arg:
                    return False
            elif op == "$in":
                if v not in arg:
                    return False
            else:
                raise NotImplementedError(op)
        return True
    return v == cond


def _match(doc, flt):
    for k, cond in (flt or {}).items():
        if k == "$and":
            if not all(_match(doc, c) for c in cond):
                return False
            continue
        if k not in doc and not (isinstance(cond, dict) and cond):
            return False
        if not _cond(doc.get(k), cond):
            return False
    return True


class _Cursor:
    def __init__(self, coll, flt, kw):
        self.coll, self.flt, self.kw = coll, flt, kw

    async def to_list(self, length=None):
        return [rt(d) for d in DB.get(self.coll, []) if _match(d, self.flt)]


def _find(self, filter=None, *a, **kw):
    return _Cursor(self.name, filter, kw)


async def _find_one(self, filter=None, *a, **kw):
    for d in DB.get(self.name, []):
        if _match(d, filter):
            return rt(d)
    return None


async def _insert_one(self, doc, *a, **kw):
    _IDS[0] += 1
    oid = ObjectId("66000000000000000000%04x" % _IDS[0])
    body = {k: v for k, v in doc.items() if k != "_id"}
    DB.setdefault(self.name, []).append(rt({"_id": oid, **body}))
    return types.SimpleNamespace(inserted_id=oid)


async def _find_one_and_update(self, filter, update, *a, upsert=False, **kw):
    sets = update.get("$set", {})
    for i, d in enumerate(DB.get(self.name, [])):
        if _match(d, filter):
            new = dict(d)
            new.update(sets)
            DB[self.name][i] = rt(new)
            return rt(new)
    if upsert:
        new = {**filter, **sets}
        DB.setdefault(self.name, []).append(rt(new))
        return rt(new)
    return None


async def _update_one(self, filter, update, *a, upsert=False, **kw):
    await _find_one_and_update(self, filter, update, upsert=upsert)
    return types.SimpleNamespace(matched_count=1)


async def _delete_one(self, filter, *a, **kw):
    docs = DB.get(self.name, [])
    for i, d in enumerate(docs):
        if _match(d, filter):
            del docs[i]
            return types.SimpleNamespace(deleted_count=1)
    return types.SimpleNamespace(deleted_count=0)


AsyncCollection.find = _find
AsyncCollection.find_one = _find_one
AsyncCollection.insert_one = _insert_one
AsyncCollection.find_one_and_update = _find_one_and_update
AsyncCollection.update_one = _update_one
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
    core_db._db = client["kiwi"]
    await init_beanie(database=client["kiwi"], skip_indexes=True,
                      document_models=[GuildConfig, TrackedAnnouncement, SiteUser, Giveaway])


LOOP = asyncio.new_event_loop()
LOOP.run_until_complete(_init())
run = LOOP.run_until_complete

# --- sources ---------------------------------------------------------------------------

SRC: dict = {}


def _aw(key):
    async def f(*a, **k):
        return copy.deepcopy(SRC[key])
    return f


def _sync(key, wrap=None):
    def f(*a, **k):
        v = copy.deepcopy(SRC[key])
        return {wrap: v} if wrap else v
    return f


def _when(s):
    return dt.datetime.fromisoformat(s) if s else None


def _news_items(limit):
    out = []
    for n in SRC["news"][:limit]:
        out.append(types.SimpleNamespace(
            title=n["title"], url=n["url"], summary=n.get("summary", ""), image=n.get("image"),
            published_at=_when(n.get("published_at")), categories=n.get("categories", [])))
    return out


async def _latest_news(limit):
    return _news_items(limit)


def _gw(rows):
    return [types.SimpleNamespace(
        title=g["title"], prize_name=g["prize_name"], entry_count=g.get("entry_count", 0),
        winner_username=g.get("winner_username"), starts_at=_when(g.get("starts_at")),
        ends_at=_when(g.get("ends_at"))) for g in rows]


async def _list_ongoing():
    return _gw(SRC["giveaways"][0])


async def _list_upcoming():
    return _gw(SRC["giveaways"][1])


async def _list_ended(days=7):
    return _gw(SRC["giveaways"][2])


class _GwQuery:
    def sort(self, *a):
        return self

    def limit(self, *a):
        return self

    async def to_list(self):
        created = SRC.get("newest_open_giveaway")
        return [types.SimpleNamespace(created_at=_when(created))] if created else []


Giveaway.find = classmethod(lambda cls, *a, **k: _GwQuery())


async def _latest_version(branch):
    v = SRC.get("latest_update")
    return types.SimpleNamespace(**v) if v else None


async def _internal_get(path, params=None, **kw):
    key = path + ("?" + urllib.parse.urlencode(params) if params else "")
    return copy.deepcopy(SRC.get("internal", {}).get(key))


async def _render_public(design_id, kind=None):
    return SRC.get("designs", {}).get(design_id, "").encode() or None


REAL = {
    "server_time": server_time.server_time(NOW),
    "biome_rotation": rotations.biome_rotation(NOW),
    "wild_mana": rotations.wild_mana(NOW),
    "stampy": rotations.stampy(NOW),
    "corruxion": server_time.corruxion(NOW),
    "fluxion": server_time.fluxion(NOW),
    "daily_buff": server_time.daily_buffs(NOW)["current"],
    "weekly_buff": server_time.weekly_buffs(NOW)["current"],
    "challenge_window": server_time.challenge_window(NOW),
    "chaos_window": server_time.chaos_chest_window(NOW),
}

for mod in (captures, embeds):
    mod.get_current_challenge = _aw("challenge")
for mod in (chaos, embeds):
    mod.get_chaos_chest = _aw("chaos")
for mod in (luxion, embeds):
    mod.get_luxion = _aw("luxion")
for mod in (status, embeds):
    mod.get_status_shared = _aw("status")
for mod in (server_time, embeds):
    mod.server_time = _sync("server_time")
    mod.corruxion = _sync("corruxion")
    mod.fluxion = _sync("fluxion")
    mod.daily_buffs = _sync("daily_buff", "current")
    mod.weekly_buffs = _sync("weekly_buff", "current")
for mod in (rotations, embeds):
    mod.biome_rotation = _sync("biome_rotation")
    mod.wild_mana = _sync("wild_mana")
    mod.stampy = _sync("stampy")
for mod in (news, embeds):
    mod.latest_news = _latest_news
gw_service.list_ongoing = _list_ongoing
gw_service.list_upcoming = _list_upcoming
gw_service.list_ended = _list_ended
updates_read.latest_version = _latest_version
internal_api.internal_get = _internal_get
images_service.render_public = _render_public

# --- discord ---------------------------------------------------------------------------

REQUESTS: list = []
SCRIPT: dict = {}
NEXT_MSG = [0]
CLIENT = discord.Client(intents=discord.Intents.none())
BOT_USER = {"id": "1100000000000000001", "username": "Kiwi", "discriminator": "0", "avatar": None}


def _msg(channel_id, mid):
    return {"id": str(mid), "channel_id": str(channel_id), "author": BOT_USER, "content": "",
            "timestamp": NOW.isoformat(), "edited_timestamp": None, "tts": False,
            "mention_everyone": False, "mentions": [], "mention_roles": [], "attachments": [],
            "embeds": [], "pinned": False, "type": 0}


class _Resp:
    def __init__(self, status):
        self.status = status
        self.reason = "scripted"


def _raise(outcome):
    if outcome == "transport":
        raise OSError(111, "Connection refused")
    st = int(outcome)
    data = {"code": 0, "message": "scripted"}
    if st == 404:
        raise discord.NotFound(_Resp(st), data)
    if st == 403:
        raise discord.Forbidden(_Resp(st), data)
    if st >= 500:
        raise discord.DiscordServerError(_Resp(st), data)
    raise discord.HTTPException(_Resp(st), data)


async def _request(route, *, files=None, form=None, **kwargs):
    path = route.url[len(discord.http.Route.BASE):]
    rec = {"method": route.method, "path": path}
    if form:
        parts = list(form)
        rec["payload_json"] = json.loads(parts[0]["value"])
        rec["files"] = []
        for p in parts[1:]:
            fp = p["value"]
            fp.seek(0)
            rec["files"].append({"name": p["name"], "filename": p["filename"],
                                 "content_type": p["content_type"], "size": len(fp.read())})
    elif "json" in kwargs:
        rec["json"] = kwargs["json"]
    REQUESTS.append(rec)
    queue = SCRIPT.get(f"{route.method} {path}") or []
    outcome = queue.pop(0) if queue else 200
    if outcome != 200:
        _raise(outcome)
    if route.method == "POST":
        NEXT_MSG[0] += 1
        return _msg(route.channel_id, 1300000000000000000 + NEXT_MSG[0])
    if route.method == "PATCH":
        return _msg(route.channel_id, int(path.rsplit("/", 1)[1]))
    return None


CLIENT.http.request = _request


class FakeBot:
    def __init__(self, cached, fetch):
        self.cached = set(cached)
        self.fetch = fetch

    def get_channel(self, cid):
        return CLIENT.get_partial_messageable(cid) if cid in self.cached else None

    async def fetch_channel(self, cid):
        REQUESTS.append({"method": "GET", "path": f"/channels/{cid}"})
        outcome = self.fetch.get(str(cid), 404)
        if outcome != 200:
            _raise(outcome)
        return CLIENT.get_partial_messageable(cid)


# --- scenario plumbing -----------------------------------------------------------------

def seed_guild(oid, guild_id, **kw):
    g = GuildConfig(guild_id=guild_id, created_at=dt.datetime(2026, 9, 1, tzinfo=dt.timezone.utc),
                    updated_at=dt.datetime(2026, 9, 2, tzinfo=dt.timezone.utc), **kw)
    from beanie.odm.utils.dump import get_dict
    return {"_id": ObjectId(oid), **{k: v for k, v in get_dict(g, to_db=True).items() if k != "_id"}}


def seed_tracked(oid, **kw):
    kw.setdefault("created_at", dt.datetime(2026, 10, 22, 13, 0, tzinfo=dt.timezone.utc))
    t = TrackedAnnouncement(**kw)
    from beanie.odm.utils.dump import get_dict
    return {"_id": ObjectId(oid), **{k: v for k, v in get_dict(t, to_db=True).items() if k != "_id"}}


def seed_user(oid, **kw):
    u = SiteUser(created_at=dt.datetime(2026, 1, 1, tzinfo=dt.timezone.utc),
                 updated_at=dt.datetime(2026, 1, 2, tzinfo=dt.timezone.utc), **kw)
    from beanie.odm.utils.dump import get_dict
    return {"_id": ObjectId(oid), **{k: v for k, v in get_dict(u, to_db=True).items() if k != "_id"}}


async def py_event(bot, payload):
    """The body of KiwiBot._subscribe_events for one decoded message."""
    event_type = payload.get("type")
    mapped = _EVENT_TO_ANNOUNCEMENT.get(event_type)
    keys = (mapped,) if isinstance(mapped, str) else (mapped or ())
    for key in keys:
        try:
            await announcer.run_announcement_type(bot, key)
        except Exception:
            pass
    if event_type == "market":
        try:
            await market_watch.run_market_watch(bot)
        except Exception:
            pass
    if event_type in liveboard.BOARD_EVENTS:
        try:
            await liveboard.refresh_boards(bot)
        except Exception:
            pass


async def py_reconcile(guild_id, channels, roles):
    cfg = await GuildConfig.find_one(GuildConfig.guild_id == guild_id)
    if cfg is None:
        return
    dirty = cfg.migrate_legacy()
    dirty = reconcile.reconcile(cfg, set(channels), set(roles)) or dirty
    if dirty:
        cfg.updated_at = cu.utcnow()
        await cfg.save()


async def do(bot, action):
    kind = action["do"]
    if kind == "announce_all":
        await announcer.run_all_announcements(bot)
    elif kind == "announce":
        await announcer.run_announcement_type(bot, action["key"])
    elif kind == "cleanup":
        await announcer.cleanup_tracked(bot)
    elif kind == "refresh":
        await announcer.refresh_tracked(bot)
    elif kind == "boards":
        await liveboard.refresh_boards(bot)
    elif kind == "market":
        await market_watch.run_market_watch(bot)
    elif kind == "event":
        await py_event(bot, action["payload"])
    elif kind == "reconcile":
        await py_reconcile(action["guild_id"], action["channels"], action["roles"])
    elif kind == "avatar":
        av = action["avatar"]
        user = types.SimpleNamespace(id=action["discord_id"],
                                     avatar=types.SimpleNamespace(key=av) if av else None)
        return await sync_avatar(user)
    elif kind == "presence":
        await stats.record_presence(action["guilds"], action["members"])
    else:
        raise ValueError(kind)


DUMPED = ("guild_configs", "tracked_announcements", "site_users", "bot_presence")


def scenario(name, sources, db, actions, cached=(), fetch=None, script=None):
    SRC.clear()
    SRC.update(copy.deepcopy(sources))
    DB.clear()
    for coll, docs in db.items():
        DB[coll] = [rt(d) for d in docs]
    seeds = {c: ext(DB.get(c, [])) for c in DUMPED}
    _IDS[0] = 0
    NEXT_MSG[0] = 0
    REQUESTS.clear()
    SCRIPT.clear()
    SCRIPT.update(copy.deepcopy(script or {}))
    bot = FakeBot(cached, fetch or {})
    results = []
    for a in actions:
        try:
            r = run(do(bot, a))
            results.append({"ok": r} if r is not None else {"ok": None})
        except Exception as e:  # noqa: BLE001
            results.append({"raised": type(e).__name__})
    return {
        "name": name, "now_us": NOW_US, "sources": sources,
        "seed": seeds, "cached": sorted(cached), "fetch": fetch or {}, "script": script or {},
        "actions": actions, "results": results, "requests": copy.deepcopy(REQUESTS),
        "db": {c: ext(DB.get(c, [])) for c in DUMPED},
    }


NOW_US = int(NOW.timestamp()) * 1_000_000 + NOW.microsecond


# --- inputs ------------------------------------------------------------------------------

N = int(NOW_TS)
CW = REAL["challenge_window"]
CHW = REAL["chaos_window"]
BIOME_START = REAL["biome_rotation"]["current"]["starts_at"]


def trimmed(rot):
    return {**rot, "upcoming": rot["upcoming"][:2]}


def win(start, hours=3):
    return {"starts_at": start, "ends_at": start + hours * 3600}


SCHED = [win(1792573200), win(1792670400), win(1792767600), win(1792864800), win(1792962000)]
LUX_OPEN = {"active": True, "starts_at": 1792540800, "ends_at": 1793145600, "schedule": SCHED,
            "current_window": SCHED[1], "next_window": SCHED[2], "run_id": "r-1"}
SCHED_GAP = [win(1792573200), win(1792657800), win(1792755000), win(1792852200)]
LUX_BETWEEN = {**LUX_OPEN, "schedule": SCHED_GAP, "current_window": None, "next_window": SCHED_GAP[2]}
SCHED_LATE = [win(N + 7200), win(N + 7200 + 97200)]
LUX_FIRST = {"active": True, "starts_at": N - 3600, "ends_at": N + 6 * 86400, "schedule": SCHED_LATE,
             "current_window": None, "next_window": SCHED_LATE[0]}
LUX_OFF = {"active": False, "starts_at": None, "ends_at": None, "schedule": [], "current_window": None,
           "next_window": None}

ENV_UNKNOWN = {"status": "unknown", "online": False, "game": None}
STATUS_ONLINE = {"overall": "online", "checked_at": 1792678000.5, "auth": {"online": True},
                 "environments": {"eu": {"status": "online", "online": True, "game": {"latency_ms": 41.5}},
                                  "us": {"status": "online", "online": True, "game": {"latency_ms": 88}},
                                  "pts": {"status": "down", "online": False, "game": None}}}
STATUS_PARTIAL = {"overall": "down", "checked_at": 1792678001, "auth": {"online": False},
                  "environments": {"eu": {"status": "online", "game": {"latency_ms": 12.5}},
                                   "us": {"status": "down"}, "pts": ENV_UNKNOWN}}
STATUS_UNKNOWN = {"overall": "unknown", "auth": None,
                  "environments": {"eu": ENV_UNKNOWN, "us": ENV_UNKNOWN, "pts": ENV_UNKNOWN}}
STATUS_MAINT = {"overall": "maintenance"}

NEWS = [
    {"title": "Delve Into The Shadows Update", "url": "https://trovegame.com/news/delve-shadows",
     "summary": "New depths, new loot. " * 40, "image": "https://cdn.example/news1.jpg",
     "published_at": "2026-10-21T16:00:00.123000+00:00"},
    {"title": "Patch Notes 2026.10.21", "url": "https://trovegame.com/news/patch-notes",
     "summary": "Fixes", "image": None, "published_at": "2026-10-21T15:00:00+00:00"},
    {"title": "T" * 260, "url": "https://trovegame.com/news/long", "summary": "", "image": None,
     "published_at": None},
    {"title": "Shop Offers", "url": "https://trovegame.com/news/shop", "summary": "Deals",
     "published_at": "2026-10-19T10:30:00+00:00"},
    {"title": "Community Spotlight", "url": "https://trovegame.com/news/spotlight", "summary": "Art",
     "published_at": "2026-10-18T09:00:00+00:00"},
]


def gw(title, prize, start, end, n=0, winner=None):
    return {"title": title, "prize_name": prize, "starts_at": start, "ends_at": end, "entry_count": n,
            "winner_username": winner}


GIVEAWAYS = [
    [gw("Holiday Drop", "Mount Code", "2026-10-20T11:00:00Z", "2026-10-25T11:00:00Z", 42),
     gw("Flash", "Glim Pack", "2026-10-22T11:00:00Z", "2026-10-23T11:00:00.500000Z", 3)],
    [gw("Next Week", "Mystery Prize", "2026-10-29T11:00:00Z", "2026-11-01T11:00:00Z")],
    [gw(f"Past {i}", f"Prize {i}", "2026-10-10T11:00:00Z", f"2026-10-2{i % 2}T11:00:00Z", i,
        "kiwifan" if i % 2 else None) for i in range(6)],
]

UPDATE = {"branch": "live-us", "ordinal": 142, "version_tag": "2026.10.21-1", "files_added": 1234,
          "files_modified": 56, "files_removed": 0}

INTERNAL = {
    "/v1/activity/current": {"estimate_24h": 18764, "estimate_7d": 52310, "computed_at": 1792677000.25,
                             "methodology": "x"},
    "/v1/activity/series?period=7d": {"peak": {"active": 21120.5, "t": 1792500000}, "average": 17480.4,
                                      "window_end": 1792677000},
    "/site/market/items/Dragon%20Egg/summary": {"name": "Dragon Egg", "count": 3, "min_each": 42000.5,
                                                "median_each": 45000, "avg_each": 46000.2, "max_each": 52000,
                                                "total_stack": 7},
    "/site/market/items/Sunfire%20Rose/summary": {"count": 1, "min_each": 100.4},
    "/site/market/items/Glim%20Pack/summary": {"count": 2, "min_each": 15},
    "/site/market/items/Banker%20Even/summary": {"count": 5, "min_each": 2.5},
    "/site/market/items/Caf%C3%A9%20%26%20Co%2F1/summary": {"count": 1, "min_each": 3.5},
}

BASE = {
    "server_time": REAL["server_time"],
    "biome_rotation": trimmed(REAL["biome_rotation"]),
    "wild_mana": trimmed(REAL["wild_mana"]),
    "stampy": trimmed(REAL["stampy"]),
    "corruxion": REAL["corruxion"],
    "fluxion": REAL["fluxion"],
    "daily_buff": REAL["daily_buff"],
    "weekly_buff": REAL["weekly_buff"],
    "challenge": {**CW, "name": "Rampage Alert: Shadow Tower", "type": "rampage",
                  "captured_at": "2026-10-22T14:00:05Z"},
    "chaos": {"item": {"name": "Radiant Mount", "rarity": "legendary"}, **CHW, "source": "capture"},
    "luxion": LUX_OPEN,
    "status": STATUS_ONLINE,
    "news": NEWS,
    "newest_open_giveaway": "2026-10-22T11:00:00.750000+00:00",
    "giveaways": GIVEAWAYS,
    "latest_update": UPDATE,
    "internal": INTERNAL,
    "designs": {"64f000000000000000000001": "PNG-DESIGN-1"},
}

STAMPY_LIVE = {"current": {**win(N - 3600, 48), "biomes": [{"name": "Candoria", "final_name": "Candoria"}]},
               "upcoming": []}
QUIET = {
    **BASE,
    "challenge": {**CW, "active": False, "name": None, "type": None, "captured_at": None},
    "chaos": {"item": None, **CHW},
    "luxion": LUX_OFF,
    "status": STATUS_UNKNOWN,
    "news": [],
    "newest_open_giveaway": None,
    "giveaways": [[], [], []],
    "latest_update": None,
    "internal": {},
    "stampy": {"current": None, "upcoming": []},
    "corruxion": {**REAL["corruxion"], "active": False},
    "fluxion": {**REAL["fluxion"], "active": False},
}


def ann(**kw):
    return kw


CHAOS_TMPL = {"enabled": True, "title": "Chaos: {item_name}", "content": "Heads up {item_name}!",
              "description": "{desc_line}", "color": "#B794F6",
              "fields": [{"name": "Rotates", "value": "{ends_at:R}"}, {"name": "", "value": ""}],
              "footer": "kiwi", "show_image": True, "image_design_id": "64f000000000000000000001"}


def main_guilds():
    g1 = seed_guild("65f000000000000000000001", 900000000000000001, language="en", announcements={
        "challenge_rampage": ann(enabled=True, channel_id=1001, ping_role_ids=[5001, 5002]),
        "challenge_collection": ann(enabled=True, channel_id=1001),
        "chaos_chest": ann(enabled=True, channel_id=1001, template=CHAOS_TMPL),
        "daily_bonuses": ann(enabled=True, channel_id=1002, template={"enabled": True, "title": "Daily {daily_name}"}),
        "longshade": ann(enabled=True, channel_id=1002, last_anchor=str(BIOME_START)),
        "wild_mana": ann(enabled=True, channel_id=1002),
        "stampy": ann(enabled=True, channel_id=1002),
        "corruxion": ann(enabled=True, channel_id=1002),
        "fluxion": ann(enabled=True, channel_id=1002, ping_role_ids=[5003]),
        "luxion": ann(enabled=True, channel_id=1003),
        "trove_news": ann(enabled=True, channel_id=1003),
        "giveaways": ann(enabled=True, channel_id=1003),
        "activity": ann(enabled=True, channel_id=1003),
        "server_status": ann(enabled=True, channel_id=1003),
        "game_update": ann(enabled=True, channel_id=1003),
    }, live_board={"enabled": True, "channel_id": 1004},
        config_perms={"announcements": [5001, 5002], "board": [5009]})
    g2 = seed_guild("65f000000000000000000002", 900000000000000002, language="fr", announcements={
        "challenge_rampage": ann(enabled=True, channel_id=2001),
        "luxion": ann(enabled=True, channel_id=2001),
        "server_status": ann(enabled=True, channel_id=2001),
        "trove_news": ann(enabled=True, channel_id=2002, template={
            "enabled": True, "title": "📰 {title}", "url": "{url}", "description": "{summary}",
            "fields": [{"name": "Plus", "value": "{more}", "inline": False}], "show_image": True}),
        "activity": ann(enabled=True, channel_id=2002, template={
            "enabled": True, "title": "Activité {label}", "fields": [{"name": "24h", "value": "{last_24h}"}],
            "show_image": True}),
        "giveaways": ann(enabled=True, channel_id=2002, template={
            "enabled": True, "description": "{open_lines}\n{upcoming_lines}", "show_image": False}),
        "game_update": ann(enabled=True, channel_id=2002, template={
            "enabled": True, "title": "MAJ {version_tag}", "description": "+{files_added}", "show_image": True}),
        "longshade": ann(enabled=True, channel_id=2002, template={
            "enabled": True, "description": "{current_biomes}\n{rotates_at:R}", "show_image": False}),
        "challenge_collection": ann(enabled=True, channel_id=2002, template={"enabled": True, "title": "{name}"}),
    }, live_board={"enabled": True, "channel_id": 2003, "message_id": 1290000000000000002})
    g3 = seed_guild("65f000000000000000000003", 900000000000000003, language="ja", announcements={
        "chaos_chest": ann(enabled=True, channel_id=3001, channel_missing=True),
        "challenge_rampage": ann(enabled=False, channel_id=3001),
        "luxion": ann(enabled=True, channel_id=3001),
        "server_status": ann(enabled=True, channel_id=3001, ping_role_ids=[5005], template={
            "enabled": True, "content": "@everyone status {overall} <@123>", "title": "{overall_text}"}),
        "wild_mana": ann(enabled=True, channel_id=3001, template={"enabled": True, "title": "{next_biomes}"}),
    })
    g4 = seed_guild("65f000000000000000000004", 900000000000000004, hourly_challenge_enabled=True,
                    announce_channel_id=4001, last_announced_challenge_anchor=1792674000, announcements={
                        "server_status": ann(enabled=True, channel_id=4002, ping_role_id=5004)})
    g5 = seed_guild("65f000000000000000000005", 900000000000000005, language="de", announcements={
        "challenge_rampage": ann(enabled=True, channel_id=5001),
        "server_status": ann(enabled=True, channel_id=5002),
        "trove_news": ann(enabled=True, channel_id=5003),
    })
    g6 = seed_guild("65f000000000000000000006", 900000000000000006, language="xx", announcements={
        "game_update": ann(enabled=True, channel_id=6001),
        "giveaways": ann(enabled=True, channel_id=6001, template={"enabled": True, "title": "{open_count}"}),
        "activity": ann(enabled=True, channel_id=6001, last_anchor="0"),
        "stampy": ann(enabled=True, channel_id=6001, template={"enabled": True, "title": "{state_line}"}),
    })
    return [g1, g2, g3, g4, g5, g6]


CACHED = [1001, 1002, 1003, 1004, 1005, 2001, 2002, 2003, 3001, 3002, 4001, 4002, 5003, 6001, 7001, 8001]
FETCH = {"5001": 404, "5002": 200, "9001": 404, "9002": 403, "9003": 500}


def tracked_seeds():
    return [
        seed_tracked("66100000000000000000000a", guild_id=900000000000000001, channel_id=1001,
                     message_id=1200000000000000001, kind="challenge_rampage", anchor="1792674000",
                     expires_at=1792675200, refresh=True, refresh_v="m5"),
        seed_tracked("66100000000000000000000b", guild_id=900000000000000002, channel_id=2001,
                     message_id=1200000000000000002, kind="luxion", anchor="1792540800:1792573200",
                     expires_at=1793145600, refresh=True, refresh_v="d6"),
    ]


SCENARIOS = []


def add(*a, **kw):
    SCENARIOS.append(scenario(*a, **kw))


MAIN_SCRIPT = {"POST /channels/5003/messages": [403], "POST /channels/6001/messages": [500],
               "DELETE /channels/2001/messages/1200000000000000002": [404]}

add("backstop", BASE, {"guild_configs": main_guilds(), "tracked_announcements": tracked_seeds()},
    [{"do": "announce_all"}, {"do": "announce_all"}], cached=CACHED, fetch=FETCH, script=MAIN_SCRIPT)
add("events", BASE, {"guild_configs": main_guilds(), "tracked_announcements": tracked_seeds()},
    [{"do": "event", "payload": {"type": "challenge", "data": {}, "ts": 1}},
     {"do": "event", "payload": {"type": "luxion"}},
     {"do": "event", "payload": {"type": "server_status"}},
     {"do": "event", "payload": {"type": "chaos"}},
     {"do": "event", "payload": {"type": "nope"}},
     {"do": "event", "payload": {"data": 1}}], cached=CACHED, fetch=FETCH, script=MAIN_SCRIPT)
add("quiet", QUIET, {"guild_configs": main_guilds()}, [{"do": "announce_all"}], cached=CACHED, fetch=FETCH)
add("transport", BASE, {"guild_configs": main_guilds()[:1]},
    [{"do": "announce", "key": "server_status"}, {"do": "announce", "key": "game_update"}],
    cached=CACHED, script={"POST /channels/1003/messages": ["transport"]})
add("off_language_fallbacks", {**BASE, "status": STATUS_PARTIAL, "luxion": LUX_BETWEEN},
    {"guild_configs": [main_guilds()[1], main_guilds()[2]]},
    [{"do": "announce", "key": "server_status"}, {"do": "announce", "key": "luxion"},
     {"do": "announce", "key": "wild_mana"}], cached=CACHED)

MW_ITEMS = [
    {"name": "Dragon Egg", "max_price_each": 50000.0},
    {"name": "Sunfire Rose", "max_price_each": None, "last_alert_sig": "100"},
    {"name": "Glim Pack", "max_price_each": 10.0, "last_alert_sig": "12"},
    {"name": "Missing Thing"},
    {"name": "Banker Even"},
    {"name": "Café & Co/1", "max_price_each": 3},
]


def market_guilds():
    return [
        seed_guild("65f000000000000000000011", 900000000000000011, language="ja", market_watch={
            "enabled": True, "channel_id": 3002, "ping_role_ids": [5006, 5007], "items": MW_ITEMS}),
        seed_guild("65f000000000000000000012", 900000000000000012, market_watch={
            "enabled": True, "channel_id": 1005, "items": [{"name": "Dragon Egg"}]}),
        seed_guild("65f000000000000000000013", 900000000000000013, market_watch={
            "enabled": True, "channel_id": 3002, "channel_missing": True, "items": [{"name": "Dragon Egg"}]}),
        seed_guild("65f000000000000000000014", 900000000000000014, market_watch={
            "enabled": True, "channel_id": 9001, "items": [{"name": "Banker Even"}]}),
    ]


add("market", BASE, {"guild_configs": market_guilds()},
    [{"do": "event", "payload": {"type": "market"}}, {"do": "market"}], cached=CACHED, fetch=FETCH,
    script={"POST /channels/1005/messages": [403]})


def tr(oid, gid, ch, mid, kind, exp, **kw):
    return seed_tracked(oid, guild_id=gid, channel_id=ch, message_id=mid, kind=kind, anchor="a",
                        expires_at=exp, **kw)


CLEANUP = [
    tr("661000000000000000000001", 1, 1001, 11, "chaos_chest", N - 10),
    tr("661000000000000000000002", 1, 1001, 12, "longshade", N - 10),
    tr("661000000000000000000003", 1, 1002, 13, "stampy", N - 10),
    tr("661000000000000000000004", 1, 1002, 14, "fluxion", N - 10),
    tr("661000000000000000000005", 1, 1002, 15, "luxion", N + 10),
    tr("661000000000000000000006", 1, 1002, 16, "server_status", None),
    tr("661000000000000000000007", 1, 9001, 17, "wild_mana", N - 10),
    tr("661000000000000000000008", 1, 9002, 18, "daily_bonuses", N - 10),
    tr("661000000000000000000009", 1, 1003, 19, "corruxion", N),
    tr("66100000000000000000000a", 1, 9003, 20, "challenge_dungeon", 0),
]
add("cleanup", BASE, {"tracked_announcements": CLEANUP}, [{"do": "cleanup"}], cached=CACHED, fetch=FETCH,
    script={"DELETE /channels/1001/messages/12": [404], "DELETE /channels/1002/messages/13": [403],
            "DELETE /channels/1002/messages/14": [500]})
add("cleanup_transport", BASE, {"tracked_announcements": CLEANUP[:2]}, [{"do": "cleanup"}], cached=CACHED,
    script={"DELETE /channels/1001/messages/11": ["transport"]})

REFRESH = [
    tr("662000000000000000000001", 900000000000000001, 1003, 21, "luxion", N + 9000, refresh=True, refresh_v="h3"),
    tr("662000000000000000000002", 900000000000000002, 2001, 22, "challenge_rampage", N + 2430, refresh=True,
       refresh_v="m41"),
    tr("662000000000000000000003", 900000000000000001, 1003, 23, "chaos_chest", N - 5, refresh=True,
       refresh_v="m1"),
    tr("662000000000000000000004", 900000000000000001, 1003, 24, "fluxion", N + 5 * 86400 + 30, refresh=True,
       refresh_v="d5"),
    tr("662000000000000000000005", 900000000000000003, 3001, 25, "stampy", N + 100, refresh=True, refresh_v="m2"),
    tr("662000000000000000000006", 900000000000000099, 1002, 26, "corruxion", N + 4000, refresh=True,
       refresh_v="m59"),
    tr("662000000000000000000007", 900000000000000006, 6001, 27, "wild_mana", N + 86400 * 3, refresh=True,
       refresh_v="d1"),
    tr("662000000000000000000008", 900000000000000001, 9001, 28, "longshade", N + 60, refresh=True),
    tr("662000000000000000000009", 900000000000000001, 9003, 29, "daily_bonuses", N + 60, refresh=True),
    tr("66200000000000000000000a", 900000000000000002, 2002, 30, "server_status", None, refresh=True),
    tr("66200000000000000000000b", 900000000000000002, 2002, 31, "server_status", N + 60, refresh=False),
    tr("66200000000000000000000c", 900000000000000002, 2002, 32, "challenge_collection", 0, refresh=True,
       refresh_v="none0"),
]
add("refresh", BASE, {"guild_configs": main_guilds(), "tracked_announcements": REFRESH}, [{"do": "refresh"}],
    cached=CACHED, fetch=FETCH,
    script={"PATCH /channels/3001/messages/25": [404], "PATCH /channels/1002/messages/26": [403],
            "PATCH /channels/6001/messages/27": [500]})
add("refresh_transport", BASE, {"tracked_announcements": REFRESH[:2]}, [{"do": "refresh"}], cached=CACHED,
    script={"PATCH /channels/1003/messages/21": ["transport"]})


def board_guilds():
    gs = main_guilds()[:2]
    gs += [
        seed_guild("65f000000000000000000021", 900000000000000021, language="pt-PT",
                   live_board={"enabled": True, "channel_id": 7001, "message_id": 1290000000000000021}),
        seed_guild("65f000000000000000000022", 900000000000000022,
                   live_board={"enabled": True, "channel_id": 8001, "message_id": 1290000000000000022}),
        seed_guild("65f000000000000000000023", 900000000000000023,
                   live_board={"enabled": True, "channel_id": 9001}),
        seed_guild("65f000000000000000000024", 900000000000000024,
                   live_board={"enabled": True, "channel_id": 7001, "channel_missing": True}),
        seed_guild("65f000000000000000000025", 900000000000000025, language="fr", hourly_challenge_enabled=True,
                   live_board={"enabled": True, "channel_id": 8001}),
    ]
    return gs


add("boards", {**BASE, "status": STATUS_PARTIAL}, {"guild_configs": board_guilds()}, [{"do": "boards"}],
    cached=CACHED, fetch=FETCH,
    script={"PATCH /channels/7001/messages/1290000000000000021": [404],
            "PATCH /channels/8001/messages/1290000000000000022": [403]})
add("boards_send_fail", BASE, {"guild_configs": board_guilds()[2:3]}, [{"do": "boards"}], cached=CACHED,
    script={"PATCH /channels/7001/messages/1290000000000000021": [404],
            "POST /channels/7001/messages": [500]})

add("reconcile", BASE, {"guild_configs": main_guilds()},
    [{"do": "reconcile", "guild_id": 900000000000000001, "channels": [1001, 1003, 1004],
      "roles": [5001, 900000000000000001]},
     {"do": "reconcile", "guild_id": 900000000000000004, "channels": [4001, 4002], "roles": [5004]},
     {"do": "reconcile", "guild_id": 900000000000000002, "channels": [2001, 2002, 2003], "roles": []},
     {"do": "reconcile", "guild_id": 42, "channels": [], "roles": []}])


def board_missing():
    g = seed_guild("65f000000000000000000031", 900000000000000031, announcements={
        "chaos_chest": ann(enabled=False, channel_id=3101, channel_missing=True)},
        live_board={"enabled": False, "channel_id": 3102, "channel_missing": True})
    return [g]


add("reconcile_restore", BASE, {"guild_configs": board_missing()},
    [{"do": "reconcile", "guild_id": 900000000000000031, "channels": [3101, 3102], "roles": []}])

USERS = [
    seed_user("651000000000000000000001", username="kiwi", discord_id=312345678901234567, discord_avatar="abc"),
    seed_user("651000000000000000000002", username="same", discord_id=2, discord_avatar="same"),
    seed_user("651000000000000000000004", username="gone", discord_id=4, discord_avatar="x",
              claim_baseline={"1": 2.5}),
]
add("avatar", BASE, {"site_users": USERS},
    [{"do": "avatar", "discord_id": 312345678901234567, "avatar": "a_def"},
     {"do": "avatar", "discord_id": 2, "avatar": "same"},
     {"do": "avatar", "discord_id": 3, "avatar": "new"},
     {"do": "avatar", "discord_id": 4, "avatar": None}])
add("presence", BASE, {}, [{"do": "presence", "guilds": 12, "members": 3456},
                           {"do": "presence", "guilds": 13, "members": 3500}])

# --- pure parts ----------------------------------------------------------------------------

LANGS = [c for c, _ in i18n.LANGS]


def build(fn, lang, *args):
    try:
        i18n.set_current_language(lang)
        out = fn(*args)
        if asyncio.iscoroutine(out):
            out = run(out)
        return {"ok": out}
    except Exception as e:  # noqa: BLE001
        return {"raised": type(e).__name__}


VARIANTS = {
    "base": BASE,
    "quiet": QUIET,
    "partial": {**BASE, "status": STATUS_PARTIAL, "luxion": LUX_BETWEEN, "stampy": STAMPY_LIVE,
                "daily_buff": {"name": "Gems", "normal_buffs": ["+1 Gem"], "color": "zz", "icon": None},
                "internal": {}, "news": NEWS[1:2]},
    "first": {**BASE, "status": STATUS_MAINT, "luxion": LUX_FIRST,
              "daily_buff": {"premium_buffs": ["a", "b"], "color": "0x10ff20", "icon": "https://x/i.png"},
              "weekly_buff": {}, "corruxion": {**REAL["corruxion"], "active": True}},
}

BUILDERS = {
    "status": embeds.status_embed, "activity": embeds.activity_embed, "chaos": embeds.chaos_embed,
    "bonuses": embeds.bonuses_embed, "challenge": embeds.challenge_embed,
    "longshade": embeds.longshade_embed, "wild_mana": embeds.wild_mana_embed, "stampy": embeds.stampy_embed,
    "corruxion": embeds.corruxion_embed, "fluxion": embeds.fluxion_embed, "luxion": embeds.luxion_embed,
    "luxion_caption": embeds.luxion_caption, "giveaways": embeds.giveaways_embed,
    "trove_news": embeds.trove_news_embed, "game_update": embeds.game_update_embed,
    "live_board": embeds.live_board_embed,
}

EMBEDS = []
for vname, src in VARIANTS.items():
    SRC.clear()
    SRC.update(copy.deepcopy(src))
    for bname, fn in BUILDERS.items():
        for lang in LANGS:
            EMBEDS.append({"builder": bname, "variant": vname, "lang": lang, **build(fn, lang)})
MW_CASES = [("Dragon Egg", INTERNAL["/site/market/items/Dragon%20Egg/summary"], 50000.0),
            ("Glim", {"count": 2, "min_each": "abc"}, None), ("X", {}, 12.5)]
MARKET_EMBEDS = [{"name": n, "summary": s, "max": m, "lang": lang,
                  **build(embeds.market_watch_embed, lang, n, s, m)} for n, s, m in MW_CASES for lang in LANGS]

CTX_KEYS = ["challenge_rampage", "chaos_chest", "daily_bonuses", "longshade", "wild_mana", "stampy",
            "corruxion", "fluxion", "trove_news", "giveaways", "activity", "server_status", "game_update",
            "unknown_key"]
CONTEXTS = []
for vname, src in VARIANTS.items():
    SRC.clear()
    SRC.update(copy.deepcopy(src))
    for key in CTX_KEYS:
        for lang in ("en", "fr", "ja", "th"):
            CONTEXTS.append({"key": key, "variant": vname, "lang": lang,
                             **build(embed_contexts.context, lang, key)})

TOKENS = [{"expires": e, "now": N, "token": announcer._refresh_token(e, N)}
          for e in (None, 0, N - 1, N, N + 1, N + 59, N + 60, N + 3599, N + 3600, N + 86399, N + 86400,
                    N + 86400 * 9 + 5)]
MATCH = [{"summary": s, "max": m, "sig": market_watch.match_item(s, m)} for s, m in [
    (None, None), ({}, None), ({"count": 0, "min_each": 5}, None), ({"count": 2}, None),
    ({"count": 2, "min_each": 0.5}, None), ({"count": 2, "min_each": 1.5}, None), ({"count": 2, "min_each": 2.5}, 3),
    ({"count": 2, "min_each": 3.5}, 3.0), ({"count": 2, "min_each": "7"}, None), ({"count": True, "min_each": 9}, 9)]]


def t_cases(site: bool):
    import re
    i18n._SITE_LOCALES = (ROOT / "site" / "static" / "locales") if site else ROOT / "no-site-in-the-bot-container"
    i18n._catalog.cache_clear()
    overlay = json.loads((ROOT / "app" / "locales" / "fr.json").read_text(encoding="utf-8"))
    keys = list(overlay)
    keys += ["  Read   more  ", "Not a known string", "Ends {when}", "{emoji} Daily — {name} ({weekday})",
             "Peak · {label}", "Account auth", "Live"]
    if site:
        keys = keys[:40] + keys[-7:]
    out = []
    for k in keys:
        names = re.findall(r"\{([A-Za-z_][A-Za-z0-9_]*)\}", k)
        fmt = {n: f"<{n}>" for n in names}
        for lang in LANGS:
            out.append({"text": k, "lang": lang, "fmt": fmt, "out": i18n.t(k, lang, **fmt)})
    return out


GOLDEN = {
    "now_us": NOW_US,
    "asset": "https://api.aallyn.net",
    "variants": VARIANTS,
    "scenarios": SCENARIOS,
    "embeds": EMBEDS,
    "market_embeds": MARKET_EMBEDS,
    "contexts": CONTEXTS,
    "tokens": TOKENS,
    "match_item": MATCH,
    "t_bot": t_cases(False),
    "t_site": t_cases(True),
    "embed_to_dict": [{"in": d, "out": discord.Embed.from_dict(d).to_dict()} for d in [
        {"title": "", "description": "", "url": None, "color": 0, "fields": []},
        {"image": {"url": "u"}, "description": "d", "color": 5, "timestamp": "2026-10-22T09:30:15.123456+00:00",
         "title": "T", "url": "U", "footer": {"text": "f"}, "fields": [{"name": "a", "value": "b", "inline": True}],
         "thumbnail": {"url": "t"}},
        {"timestamp": "2026-10-22T09:30:15+00:00", "title": 5},
    ]],
}

OUT.parent.mkdir(parents=True, exist_ok=True)
OUT.write_text(json.dumps(GOLDEN, ensure_ascii=False, default=str), encoding="utf-8")
print(f"wrote {OUT} ({OUT.stat().st_size} bytes): {len(SCENARIOS)} scenarios, {len(EMBEDS)} embeds, "
      f"{len(CONTEXTS)} contexts")
