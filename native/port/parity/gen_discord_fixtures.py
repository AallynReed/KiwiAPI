"""Golden fixtures for kiwi-server's Discord area (interactions, slash commands,
embeds, i18n, announcement contexts, the bot dashboard), from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_discord_fixtures.py
Writes native/kiwi-server/testdata/discord/golden.json.

Slash commands run through app.discord.commands.handle with every domain read
patched to fixture data (the Rust tests feed the same fixtures to a fake
Domain). Dashboard routes drive app.main.app at the ASGI level over an
in-memory pymongo layer and a scripted discord_rest (no network), with the
site user as a dependency override. The clock is frozen.
"""
import asyncio
import copy
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
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "discord" / "golden.json"
os.chdir(ROOT)
sys.path.insert(0, str(ROOT))
os.environ.setdefault("SECRET_KEY", "discord-fixture-secret")
os.environ["DISCORD_CLIENT_ID"] = "1234567890"
os.environ["DISCORD_CLIENT_SECRET"] = "fixture-secret"
os.environ.pop("DISCORD_BOT_TOKEN", None)

import bson  # noqa: E402
from bson import ObjectId, json_util  # noqa: E402
from bson.codec_options import CodecOptions  # noqa: E402
from bson.json_util import CANONICAL_JSON_OPTIONS  # noqa: E402
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey  # noqa: E402
from pymongo.asynchronous.collection import AsyncCollection  # noqa: E402
from pymongo.asynchronous.database import AsyncDatabase  # noqa: E402

from app.admin import runtime_config as rc  # noqa: E402

for _key, _spec in rc.REGISTRY.items():
    rc._CACHE[_key] = (_spec.default, float("inf"))

import app.core.utils as cu  # noqa: E402

NOW = dt.datetime(2026, 10, 22, 9, 30, 15, 123456, tzinfo=dt.timezone.utc)
NOW_TS = NOW.timestamp()


class _Frozen(dt.datetime):
    @classmethod
    def now(cls, tz=None):
        return NOW


cu.datetime = _Frozen

from app import i18n  # noqa: E402
from app.bot import announcements as ann  # noqa: E402
from app.bot import discord_rest  # noqa: E402
from app.bot import models as bot_models  # noqa: E402
from app.bot import reconcile as bot_reconcile  # noqa: E402
from app.bot import router as bot_router  # noqa: E402
from app.core.config import settings  # noqa: E402
from app.discord import commands, embed_contexts, embeds  # noqa: E402
from app.discord import router as discord_router  # noqa: E402
from app.main import app  # noqa: E402
from app.site_auth.dependencies import get_current_site_user  # noqa: E402

OPTS = CodecOptions(tz_aware=True)


def ext(v):
    return json.loads(json_util.dumps(v, json_options=CANONICAL_JSON_OPTIONS))


def rt(doc):
    return bson.decode(bson.encode(doc), codec_options=OPTS)


# --- i18n --------------------------------------------------------------------------------

I18N_CASES = [
    ("Trove server status", "fr", None),
    ("Trove server status", "en", None),
    ("Trove   server\n status ", "de", None),
    ("Ends {when}", "fr", {"when": "<t:1:R>"}),
    ("Ends {when}", "ja", {"when": "<t:1:R>"}),
    ("Ends {when}", "ru", None),
    ("Peak · {label}", "ko", {"label": "X"}),
    ("…and {n} more", "zh-CN", {"n": "3"}),
    ("…and {n} more", "th", {"n": "3"}),
    ("…and {n} more", "pt-PT", {"n": "3"}),
    ("…and {n} more", "es", {"n": "3"}),
    ("Not a known string {x}", "fr", {"x": "1"}),
    ("Missing {kw}", "fr", {"other": "1"}),
    ("Braces {{literal}} {x}", "de", {"x": "v"}),
    ("Unbalanced {x", "de", {"x": "v"}),
    ("Positional {}", "de", {"x": "v"}),
    ("List", "fr", None),
    ("Grid view", "ja", None),
    ("Online", "xx", None),
    ("Online", None, None),
    ("🥝 Sorry, `/{name}` hit a snag. Try again in a moment.", "fr", {"name": "status"}),
    ("Cost to 100%: {precise} Precise + {rough} Rough focus", "de", {"precise": "1", "rough": "2"}),
]

FORMAT_CASES = [
    ("plain", {"a": "1"}), ("{a}-{b}", {"a": "1", "b": "2"}), ("{{a}}", {"a": "1"}), ("}}{{", {"a": "1"}),
    ("{a", {"a": "1"}), ("a}", {"a": "1"}), ("{}", {"a": "1"}), ("{0}", {"a": "1"}), ("{a!r}", {"a": "1"}),
    ("{a:>3}", {"a": "1"}), ("{ a}", {"a": "1"}), ("{b}", {"a": "1"}), ("x{a}y{a}", {"a": "é"}),
    ("{a:^6}", {"a": "ab"}), ("{a:*<4}", {"a": "é"}), ("{a:.1}", {"a": "xyz"}), ("{a:05}", {"a": "ab"}),
    ("{a:=5}", {"a": "ab"}), ("{a:+}", {"a": "ab"}), ("{a:d}", {"a": "ab"}), ("{a!s:>4s}", {"a": "ab"}),
    ("{a!x}", {"a": "ab"}), ("{a!r}", {"a": "it's"}), ("{a:,}", {"a": "ab"}), ("{a:.}", {"a": "ab"}),
]


def gen_i18n():
    out = []
    for text, lang, fmt in I18N_CASES:
        got = i18n.t(text, lang, **(fmt or {}))
        out.append({"text": text, "lang": lang, "fmt": fmt, "out": got})
    fmts = []
    for f, kw in FORMAT_CASES:
        try:
            fmts.append({"fmt": f, "kw": kw, "out": f.format(**kw)})
        except (KeyError, IndexError, ValueError):
            fmts.append({"fmt": f, "kw": kw, "out": None})
    locales = [[loc, i18n.discord_locale_to_lang(loc)] for loc in
               ["en-US", "en-GB", "pt-BR", "zh-TW", "es-419", "es-ES", "fr", "de", "ru", "ja", "ko", "th",
                "zh-CN", "pt-PT", "it", "", None, "nl"]]
    norms = [[s, i18n._norm(s)] for s in ["  a  b ", "a\u00a0\u2003b", "x\n\ty", "\u3000z\u3000", "plain"]]
    return {"t": out, "format": fmts, "locales": locales, "norm": norms,
            "langs": [[c, n] for c, n in i18n.LANGS]}


# --- slash commands ------------------------------------------------------------------------

STATUS_ONLINE = {
    "overall": "online", "auth": {"online": True},
    "environments": {"eu": {"status": "online", "online": True, "game": {"latency_ms": 41.5}},
                     "us": {"status": "online", "online": True, "game": {"latency_ms": 102.5}},
                     "pts": {"status": "online", "online": True, "game": {"latency_ms": 7}}},
    "checked_at": 1792000000.25,
}
STATUS_PARTIAL = {
    "overall": "down", "auth": None,
    "environments": {"eu": {"status": "online", "game": None}, "us": {"status": "down"}, "pts": {}},
}
STATUS_LEGACY = {
    "overall": "maintenance", "auth": {"online": False},
    "environments": {"eu": {"status": "maintenance"}, "us": {"status": "unknown"}, "pts": {"status": "online"}},
    "checked_at": 1792000000,
}

LIVE = {"estimate_24h": 18764, "estimate_7d": 52310.0, "computed_at": 1792000123}
SERIES = {"peak": {"active": 21120.5, "t": 1791900000}, "average": 17480.5, "window_end": 1792000000}
SERIES_EMPTY = {"peak": None, "average": None, "window_end": 1792000777}

CHAOS = {"item": {"name": "Radiant Mount"}, "starts_at": 1791990000, "ends_at": 1792594800}
CHAOS_EMPTY = {"item": None, "starts_at": 1791990000, "ends_at": 1792594800}

SERVER_TIME = {"now_unix": int(NOW_TS), "daily_reset_at": 1792148400, "weekly_reset_at": 1792407600}

DAILY = {"current": {"name": "Combat", "weekday": "Thursday", "emoji": "⚔️", "color": "ff5c80",
                     "normal_buffs": ["+50% Combat XP", "+25% Loot"], "premium_buffs": ["+10% Flux", "Fast"],
                     "icon": "https://x/icon.png"}}
DAILY_BAD_COLOR = {"current": {"name": "Odd", "color": "#zz", "normal_buffs": []}}
WEEKLY = {"current": {"name": "Geode", "emoji": "🗓️", "buffs": ["+Geode topside drops"]}}

CHALLENGE_ACTIVE = {"starts_at": 1792000800, "ends_at": 1792002000, "active": True, "name": "Racing Challenge",
                    "type": "racing"}
CHALLENGE_UPCOMING = {"starts_at": 1792004400, "ends_at": 1792005600, "active": False, "name": "Coin Rush",
                      "type": "collection"}
CHALLENGE_NONE = {"starts_at": 1792004400, "ends_at": 1792005600, "active": False, "name": None, "type": None}

BIOMES = {"current": {"starts_at": 1791990000, "ends_at": 1792000800,
                      "biomes": [{"name": "Permafrost", "final_name": "Frozen Permafrost"},
                                 {"name": "Cursed Vale", "final_name": None}]},
          "upcoming": [{"starts_at": 1792000800, "ends_at": 1792011600, "biomes": [{"name": "Fae Forest"}]}]}
BIOMES_EMPTY = {"current": None, "upcoming": []}

STAMPY_ACTIVE = {"current": {"starts_at": 1791900000, "ends_at": 1792072800,
                             "biomes": [{"name": "Candoria", "final_name": "Candoria"}]}, "upcoming": []}
STAMPY_SOON = {"current": {"starts_at": 1792500000, "ends_at": 1792672800, "biomes": []}, "upcoming": []}
STAMPY_NONE = {"current": None, "upcoming": []}


def gw(title, prize, ends=None, starts=None, n=0, winner=None):
    return types.SimpleNamespace(
        title=title, prize_name=prize, entry_count=n, winner_username=winner,
        ends_at=dt.datetime.fromtimestamp(ends or 1792100000, dt.timezone.utc),
        starts_at=dt.datetime.fromtimestamp(starts or 1792000000, dt.timezone.utc))


def gw_json(g):
    return {"title": g.title, "prize_name": g.prize_name, "entry_count": g.entry_count,
            "winner_username": g.winner_username,
            "ends_at": g.ends_at.isoformat().replace("+00:00", "Z"),
            "starts_at": g.starts_at.isoformat().replace("+00:00", "Z")}


GW_OPEN = [gw(f"Drop {i}", f"Prize {i}", ends=1792100000 + i, n=i * 3) for i in range(6)]
GW_UP = [gw("Next week", "Mystery", starts=1792600000)]
GW_ENDED = [gw("Last draw", "Glim Pack", ends=1791900000, winner="kiwi"), gw("Older", "Nothing", ends=1791800000)]

NEWS = [
    types.SimpleNamespace(title="Summer Update is live!" + "!" * 260, url="https://trovegame.com/a",
                          summary="Patch notes. " * 60, image="https://x/img.png",
                          published_at=dt.datetime(2026, 10, 20, 15, 0, 0, 250000, tzinfo=dt.timezone.utc)),
    types.SimpleNamespace(title="Second", url="https://trovegame.com/b", summary="", image=None,
                          published_at=dt.datetime(2026, 10, 19, 12, 0, tzinfo=dt.timezone.utc)),
    types.SimpleNamespace(title="Third", url="https://trovegame.com/c", summary="", image=None, published_at=None),
]


def news_json(n):
    return {"url": n.url, "title": n.title, "summary": n.summary, "image": n.image,
            "published_at": n.published_at.isoformat() if n.published_at else None}


SUMMARY = {"name": "Dragon Egg", "count": 12, "total_price": 999999, "total_stack": 1234,
           "min_each": 1500.5, "max_each": 2500000.0, "avg_each": 12345.678, "median_each": 2.5}

HISTORY = {"eu": {"uptime": 0.99875, "outages": [{"status": "down"}]},
           "us": {"uptime": None, "outages": []},
           "pts": {"uptime": 1, "outages": [{}, {}]}}

REL = types.SimpleNamespace(tag_name="v2.3.4", html_url="https://github.com/AallynReed/BetterTroveTools/releases/tag/v2.3.4")
LATEST = {"windows": (REL, [{"url": "https://x/BTT.msi"}]), "linux": (REL, []), "android": None}

CHANGELOG = types.SimpleNamespace(rate_limited=True, groups=[
    {"version": "v2.3.4", "commits": [{"message": f"feat: thing {i}\nbody"} for i in range(8)]},
    {"version": None, "commits": [{"message": "fix: " + "x" * 120}]},
    {"version": "v2.3.2", "commits": []},
    {"version": "v2.3.1", "commits": [{"message": "a\r\nb"}]},
    {"version": "v2.3.0", "commits": [{"message": "c\u2028d"}]},
    {"version": "v2.2.0", "commits": [{"message": "never shown"}]},
])
CHANGELOG_EMPTY_MSG = types.SimpleNamespace(rate_limited=False, groups=[{"version": "v1", "commits": [{"message": ""}]}])


def scenario(**kw):
    base = {"status": STATUS_ONLINE, "live": LIVE, "series": SERIES, "chaos": CHAOS, "server_time": SERVER_TIME,
            "daily": DAILY, "weekly": WEEKLY, "challenge": CHALLENGE_ACTIVE, "biomes": BIOMES, "wild_mana": BIOMES,
            "stampy": STAMPY_ACTIVE, "giveaways": [GW_OPEN, GW_UP, GW_ENDED], "news": NEWS, "summary": SUMMARY,
            "history": HISTORY, "latest": LATEST, "changelog": CHANGELOG, "ping": [0.0123, 0.0045]}
    base.update(kw)
    return base


def scenario_json(s):
    return {
        "status": s["status"], "live": s["live"], "series": s["series"], "chaos": s["chaos"],
        "server_time": s["server_time"], "daily": s["daily"], "weekly": s["weekly"], "challenge": s["challenge"],
        "biomes": s["biomes"], "wild_mana": s["wild_mana"], "stampy": s["stampy"],
        "corruxion": s["corruxion"], "fluxion": s["fluxion"],
        "giveaways": [[gw_json(g) for g in part] for part in s["giveaways"]],
        "news": [news_json(n) for n in s["news"]], "summary": s["summary"], "history": s["history"],
        "latest": {p: (None if v is None else [v[0].tag_name, v[0].html_url, v[1][0]["url"] if v[1] else None])
                   for p, v in s["latest"].items()},
        "changelog": None if s["changelog"] is None else {"rate_limited": s["changelog"].rate_limited,
                                                          "groups": s["changelog"].groups},
        "ping": s["ping"],
    }


RECORDED: list = []
GUILD_LANG = {"111": "fr", "222": "ja", "333": "de", "444": "ko", "555": "ru", "666": "zh-CN", "777": "th",
              "888": "pt-PT", "999": "es"}


def install(s):
    async def _status():
        return s["status"]

    async def _live():
        return s["live"]

    async def _series(period):
        return dict(s["series"], period=period)

    async def _chaos():
        return s["chaos"]

    async def _challenge():
        return s["challenge"]

    async def _news(limit):
        return s["news"][:limit]

    async def _latest(channel):
        return s["latest"]

    async def _changelog():
        return s["changelog"]

    embeds.get_status_shared = _status
    embeds.estimate_active_players = _live
    embeds.activity_series = _series
    embeds.get_chaos_chest = _chaos
    embeds.server_time = lambda: s["server_time"]
    embeds.trove_now = lambda: NOW - dt.timedelta(hours=11)
    embeds.daily_buffs = lambda: s["daily"]
    embeds.weekly_buffs = lambda: s["weekly"]
    embeds.get_current_challenge = _challenge
    embeds.biome_rotation = lambda: s["biomes"]
    embeds.wild_mana = lambda: s["wild_mana"]
    embeds.stampy = lambda: s["stampy"]
    embeds.corruxion = lambda: s["corruxion"]
    embeds.fluxion = lambda: s["fluxion"]
    embeds.latest_news = _news
    embeds.latest_per_platform = _latest
    embeds.get_changelog = _changelog
    embeds.time = types.SimpleNamespace(time=lambda: NOW_TS, perf_counter=_perf(s["ping"]))

    from app.giveaways import service as gsvc

    async def _go():
        return s["giveaways"][0]

    async def _gu():
        return s["giveaways"][1]

    async def _ge(days=7):
        return s["giveaways"][2]

    gsvc.list_ongoing, gsvc.list_upcoming, gsvc.list_ended = _go, _gu, _ge

    from app.trove.market import service as msvc

    async def _summary(name):
        return s["summary"] if name.lower() == "dragon egg" else None

    msvc.item_summary = _summary

    from app.trove import status as smod

    async def _hist(env, days):
        return s["history"][env]

    smod.get_history = _hist


def _perf(ping):
    seq = [0.0, ping[0], 0.0, ping[1]]
    state = {"i": 0}

    def f():
        v = seq[state["i"] % 4]
        state["i"] += 1
        return v
    return f


class _FakeDb:
    async def command(self, *a, **k):
        return {"ok": 1}


class _FakeRedis:
    async def ping(self):
        return True


import app.core.database as cdb  # noqa: E402
import app.core.redis as credis  # noqa: E402

cdb.get_db = lambda: _FakeDb()
credis.get_redis = lambda: _FakeRedis()


async def _guild_language(guild_id, fallback=None):
    if guild_id and str(guild_id) in GUILD_LANG:
        return GUILD_LANG[str(guild_id)]
    return i18n.discord_locale_to_lang(fallback)

i18n.guild_language = _guild_language

from app.bot import stats as bot_stats  # noqa: E402


async def _record(name):
    if name:
        RECORDED.append(name)

bot_stats.record_command = _record


def cmd(name, guild=None, locale=None, guild_locale=None, options=None, **extra):
    i = {"type": 2, "data": {"name": name}}
    if options is not None:
        i["data"]["options"] = [{"name": k, "value": v} for k, v in options.items()]
    if guild is not None:
        i["guild_id"] = guild
    if locale is not None:
        i["locale"] = locale
    if guild_locale is not None:
        i["guild_locale"] = guild_locale
    i.update(extra)
    return i


def gem_opts(tier=4, type_=2, level=20, stats=((1, 512.4), (4, 25.0), (6, 30.5))):
    o = {"tier": tier, "type": type_, "level": level}
    for n, (st, v) in enumerate(stats, 1):
        o[f"stat{n}"] = st
        o[f"value{n}"] = v
    return o


MERCH_ACTIVE = {"corruxion": None, "fluxion": None}


def merchant_cases():
    from app.trove.server_time import corruxion, fluxion
    here = dt.datetime(2026, 10, 25, 12, 0, tzinfo=dt.timezone.utc)
    return [
        ("active", corruxion(here), fluxion(here)),
        ("away", corruxion(NOW), fluxion(NOW)),
    ]


def gen_commands():
    out = []
    langs = [(None, None), ("111", None), ("222", None), ("333", None), ("444", None), ("555", None),
             ("666", None), ("777", None), ("888", None), ("999", None), (None, "pt-BR")]
    merch = merchant_cases()
    base = scenario(corruxion=merch[0][1], fluxion=merch[0][2])
    names = ["status", "activity", "chaos", "servertime", "bonuses", "hourly_challenge", "longshade", "giveaways",
             "corruxion", "fluxion", "stampy", "wild_mana", "trove_news", "trove_uptime", "download", "web",
             "change_log", "ping"]
    runs = []
    for guild, locale in langs:
        for n in names:
            runs.append((base, cmd(n, guild=guild, locale=locale)))
        runs.append((base, cmd("price", guild=guild, locale=locale, options={"item": "  dragon egg "})))
        runs.append((base, cmd("price", guild=guild, locale=locale, options={"item": "Nope"})))
        runs.append((base, cmd("gem", guild=guild, locale=locale, options=gem_opts())))
    away = scenario(corruxion=merch[1][1], fluxion=merch[1][2], status=STATUS_PARTIAL, series=SERIES_EMPTY,
                    chaos=CHAOS_EMPTY, daily=DAILY_BAD_COLOR, challenge=CHALLENGE_UPCOMING, biomes=BIOMES_EMPTY,
                    stampy=STAMPY_SOON, giveaways=[[], [], []], news=[], changelog=None,
                    latest={"windows": None, "linux": None, "android": None})
    third = scenario(corruxion=merch[1][1], fluxion=merch[1][2], status=STATUS_LEGACY, challenge=CHALLENGE_NONE,
                     stampy=STAMPY_NONE, giveaways=[GW_OPEN[:2], [], GW_ENDED[1:]], news=NEWS[1:],
                     changelog=CHANGELOG_EMPTY_MSG)
    for sc in (away, third):
        for guild in (None, "111"):
            for n in names:
                runs.append((sc, cmd(n, guild=guild)))
    runs += [
        (base, cmd("activity", options={"period": "1m"})),
        (base, cmd("activity", guild="111", options={"period": "bogus"})),
        (base, cmd("price", options={"item": "   "})),
        (base, cmd("price")),
        (base, cmd("gem", options=gem_opts(tier=5))),
        (base, cmd("gem", guild="111", options=gem_opts(type_=3))),
        (base, cmd("gem", options=gem_opts(stats=((1, 1.0), (1, 2.0), (6, 3.0))))),
        (base, cmd("gem", options=gem_opts(stats=((4, 1.0), (5, 2.0), (6, 3.0))))),
        (base, cmd("gem", options=gem_opts(tier=1, type_=1, level=5, stats=((1, 99999.0), (2, 0.00001), (3, 1234567.0))))),
        (base, cmd("gem", options={"tier": 4})),
        (base, cmd("gem", options=dict(gem_opts(), value2=None))),
        (base, cmd("nope")),
        (base, cmd("nope", guild="111")),
        (base, {"type": 3, "data": {"name": "status"}}),
        (base, {"type": 2}),
        (base, {"type": 2.0, "data": {"name": "web"}, "guild_locale": "ja", "locale": "fr"}),
        (base, {"type": True, "data": {"name": "web"}}),
        (base, cmd("web", guild=111)),
        (base, cmd("web", locale="zh-TW")),
    ]
    scenarios: list = []
    for sc, interaction in runs:
        install(sc)
        RECORDED.clear()
        i18n.set_current_language("en")
        try:
            got = asyncio.run(commands.handle(copy.deepcopy(interaction)))
            res = {"ok": got}
        except Exception as e:  # noqa: BLE001
            res = {"raises": type(e).__name__}
        sj = scenario_json(sc)
        if sj not in scenarios:
            scenarios.append(sj)
        out.append({"scenario": scenarios.index(sj), "interaction": interaction, "recorded": list(RECORDED), **res})
    return {"scenarios": scenarios, "cases": out}


LUX_ACTIVE = {"active": True, "starts_at": 1791950000, "ends_at": 1792500000,
              "current_window": None, "next_window": {"starts_at": 1792050000, "ends_at": 1792060800},
              "schedule": [{"starts_at": 1791960000, "ends_at": 1791970800},
                           {"starts_at": 1792050000, "ends_at": 1792060800}]}
UPDATE = {"version_tag": "2026.10.21", "files_added": 1234, "files_modified": 8, "files_removed": 0,
          "ordinal": 142, "branch": "live-us"}


def gen_registry():
    """Every announcement type's default embed, caption and live context."""
    merch = merchant_cases()
    sc = scenario(corruxion=merch[0][1], fluxion=merch[0][2])
    install(sc)

    async def _lux():
        return LUX_ACTIVE

    async def _upd(branch):
        return types.SimpleNamespace(**UPDATE)

    embeds.get_luxion = _lux
    from app.trove.updates import read as uread
    uread.latest_version = _upd
    import app.trove.captures as caps
    import app.trove.chaos as chaos_mod
    import app.trove.leaderboards.activity as act
    import app.trove.news as news_mod
    import app.trove.rotations as rot
    import app.trove.status as smod
    caps.get_current_challenge = embeds.get_current_challenge
    chaos_mod.get_chaos_chest = embeds.get_chaos_chest
    rot.biome_rotation, rot.wild_mana, rot.stampy = embeds.biome_rotation, embeds.wild_mana, embeds.stampy
    news_mod.latest_news = embeds.latest_news
    act.activity_series, act.estimate_active_players = embeds.activity_series, embeds.estimate_active_players
    smod.get_status_shared = embeds.get_status_shared
    embed_contexts.time = types.SimpleNamespace(time=lambda: NOW_TS)
    out = []
    for lang in ("en", "fr", "ko"):
        i18n.set_current_language(lang)
        for atype in ann.ANNOUNCEMENT_TYPES:
            built = atype.build_embed()
            if asyncio.iscoroutine(built):
                built = asyncio.run(built)
            cap = asyncio.run(atype.caption()) if atype.caption else None
            try:
                ctx = {"ok": asyncio.run(embed_contexts.context(atype.key))}
            except Exception as e:  # noqa: BLE001
                ctx = {"raises": type(e).__name__}
            out.append({"key": atype.key, "lang": lang, "embed": built, "caption": cap, "context": ctx})
    i18n.set_current_language("en")
    sj = scenario_json(sc)
    sj["luxion"] = LUX_ACTIVE
    sj["update"] = UPDATE
    return {"scenario": sj, "cases": out}


def gen_builders():
    """Announcement-only builders (no slash command), per language."""
    out = []
    lux_active = {"active": True, "starts_at": 1791950000, "ends_at": 1792500000,
                  "current_window": None, "next_window": {"starts_at": 1792050000, "ends_at": 1792060800},
                  "schedule": [{"starts_at": 1791960000, "ends_at": 1791970800},
                               {"starts_at": 1792050000, "ends_at": 1792060800},
                               {"starts_at": 1792150000, "ends_at": 1792160800}]}
    lux_open = dict(lux_active, current_window={"starts_at": 1792000000, "ends_at": 1792010800})
    lux_first = dict(lux_active, schedule=[{"starts_at": 1792050000, "ends_at": 1792060800}])
    lux_away = {"active": False, "schedule": []}
    for lang in ("en", "fr", "ja"):
        i18n.set_current_language(lang)
        for name, lux in (("active", lux_active), ("open", lux_open), ("first", lux_first), ("away", lux_away)):
            async def _lux(lx=lux):
                return lx
            embeds.get_luxion = _lux
            embeds.time = types.SimpleNamespace(time=lambda: NOW_TS)
            out.append({"kind": "luxion", "lang": lang, "input": lux, "out": asyncio.run(embeds.luxion_embed())})
            out.append({"kind": "luxion_caption", "lang": lang, "input": lux,
                        "out": asyncio.run(embeds.luxion_caption())})
        for overall in ("online", "down", "unknown", "weird"):
            async def _st(o=overall):
                return {"overall": o}
            embeds.get_status_shared = _st
            embeds.time = types.SimpleNamespace(time=lambda: NOW_TS)
            embeds.datetime = _Frozen
            out.append({"kind": "live_board", "lang": lang, "input": {"overall": overall},
                        "out": asyncio.run(embeds.live_board_embed())})
        embeds.datetime = dt.datetime
        out.append({"kind": "market_watch", "lang": lang, "input": {"summary": SUMMARY, "max": 2000.0},
                    "out": embeds.market_watch_embed("Dragon Egg", SUMMARY, 2000.0)})
        out.append({"kind": "market_watch", "lang": lang, "input": {"summary": {"count": 3}, "max": None},
                    "out": embeds.market_watch_embed("Dragon Egg", {"count": 3}, None)})
        from app.trove.updates import read as uread
        for v in (None, types.SimpleNamespace(version_tag="2026.10.21", files_added=1234, files_modified=8,
                                              files_removed=0, ordinal=142, branch="live-us")):
            async def _lv(branch, vv=v):
                return vv
            uread.latest_version = _lv
            out.append({"kind": "game_update", "lang": lang,
                        "input": None if v is None else {"version_tag": v.version_tag, "files_added": v.files_added,
                                                         "files_modified": v.files_modified,
                                                         "files_removed": v.files_removed, "ordinal": v.ordinal},
                        "out": asyncio.run(embeds.game_update_embed())})
    i18n.set_current_language("en")
    fmt_g = [[x if x == x and abs(x) != float("inf") else None, f"{x:g}"] for x in [0.0, -0.0, 1.0, 12.5, 100.0, 123456.0, 1234567.0, 0.0001, 0.00001, 512.4,
                                     99999.95, 999999.5, 1e21, 3.14159265, -2.5e-7, float("inf"), float("nan")]]
    flux = []
    for v in [1500.5, 2.5, 3.5, -0.5, 0, "12.5", "abc", None, 1e15]:
        flux.append([v, embeds._flux(v)])
    title = [[s, s.title()] for s in ["racing", "dungeon biome", "o'neil x-ray", "ÉCOLE éte", "rampage2go"]]
    return {"builders": out, "fmt_g": fmt_g, "flux": flux, "title": title}


# --- contexts --------------------------------------------------------------------------------

def gen_contexts():
    defaults = {}
    for lang, _ in i18n.LANGS:
        i18n.set_current_language(lang)
        defaults[lang] = {k: embed_contexts.default_template(k).model_dump() for k in embed_contexts.KINDS}
    i18n.set_current_language("en")
    live = []
    for lang in ("en", "fr"):
        i18n.set_current_language(lang)
        for c in (CHALLENGE_ACTIVE, CHALLENGE_UPCOMING, CHALLENGE_NONE):
            async def _c(cc=c):
                return cc
            import app.trove.captures as caps
            caps.get_current_challenge = _c
            live.append({"kind": "challenge", "lang": lang, "input": c,
                         "out": asyncio.run(embed_contexts.context("challenge"))})
        for st in (STATUS_ONLINE, STATUS_PARTIAL, STATUS_LEGACY):
            async def _s(ss=st):
                return ss
            import app.trove.status as smod
            smod.get_status_shared = _s
            live.append({"kind": "server_status", "lang": lang, "input": st,
                         "out": asyncio.run(embed_contexts.context("server_status"))})
    i18n.set_current_language("en")
    return {"defaults": defaults, "live": live}


# --- dashboard ---------------------------------------------------------------------------------

DB: dict[str, list[dict]] = {}
_IDS = [0]


def _get_path(doc, key):
    return doc.get(key)


def _match(doc, flt):
    for k, cond in (flt or {}).items():
        v = doc.get(k)
        if isinstance(cond, dict) and "$in" in cond:
            if v not in cond["$in"]:
                return False
            continue
        if v != cond or (k not in doc):
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


def _apply_set(filter, update, upsert, name):
    for i, d in enumerate(DB.get(name, [])):
        if _match(d, filter):
            new = dict(d)
            new.update(update.get("$set", {}))
            DB[name][i] = rt(new)
            return rt(new)
    if upsert:
        DB.setdefault(name, []).append(rt({**filter, **update.get("$set", {})}))
    return None


async def _find_one_and_update(self, filter, update, *a, upsert=False, **kw):
    return _apply_set(filter, update, upsert, self.name)


async def _update_one(self, filter, update, *a, upsert=False, **kw):
    _apply_set(filter, update, upsert, self.name)
    return types.SimpleNamespace(matched_count=1, modified_count=1, upserted_id=None)


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
    await init_beanie(database=client["kiwi"], skip_indexes=True,
                      document_models=[bot_models.GuildConfig, bot_models.Club, bot_models.TrackedAnnouncement])


LOOP = asyncio.new_event_loop()
LOOP.run_until_complete(_init())

WORLD: dict = {}
SENT: list = []


async def _fake_get(client, path):
    if path in WORLD.get("errors", {}):
        raise discord_rest.DiscordRestError(WORLD["errors"][path])
    if path in WORLD.get("bad", []):
        raise ValueError("bad payload")
    return copy.deepcopy(WORLD.get("paths", {}).get(path))


async def _fake_send(client, method, path, *, reason=None):
    SENT.append([method, path, urllib.parse.quote(reason[:400], safe="") if reason else None])
    if path in WORLD.get("errors", {}):
        raise discord_rest.DiscordRestError(WORLD["errors"][path])
    return None


discord_rest._get = _fake_get
discord_rest._send = _fake_send


async def _fake_bot_id(client):
    return WORLD.get("bot_id")

discord_rest._get_bot_user_id = _fake_bot_id


async def _get_token(uid):
    return WORLD.get("tokens", {}).get(str(uid))


CLEARED: list = []


async def _clear_token(uid):
    CLEARED.append(str(uid))


async def _guilds(token):
    return copy.deepcopy(WORLD.get("user_guilds"))

bot_router.get_discord_token = _get_token
bot_router.clear_discord_token = _clear_token
bot_router.fetch_discord_guilds = _guilds

for _atype in ann.ANNOUNCEMENT_TYPES:
    async def _anchor(key=_atype.key):
        v = WORLD.get("anchors", {}).get(key)
        if v == "RAISE":
            raise RuntimeError("anchor failed")
        return v
    object.__setattr__(_atype, "current_anchor", _anchor)

GID = 4242424242424242
BOT = 999000000000000001
OWNER = 111000000000000001
ADMIN_ROLE, PING_ROLE, MOD_ROLE, MANAGED_ROLE, CLUB_PRES, CLUB_OFF, CLUB_MEM = (
    700000000000000001, 700000000000000002, 700000000000000003, 700000000000000004,
    700000000000000011, 700000000000000013, 700000000000000015)
CH_TEXT, CH_NEWS, CH_VOICE, CH_LOCKED = 800000000000000001, 800000000000000002, 800000000000000003, 800000000000000004
USER_ADMIN, USER_MOD, USER_PLAIN, USER_PINGER = 222000000000000001, 222000000000000002, 222000000000000003, 222000000000000004
MEMBER_A, MEMBER_B, MEMBER_BOT = 333000000000000001, 333000000000000002, 333000000000000003


def role(rid, name, perms="0", pos=1, managed=False, color=0):
    return {"id": str(rid), "name": name, "permissions": perms, "position": pos, "managed": managed, "color": color}


def base_world():
    roles = [role(GID, "@everyone", str(1 << 10 | 1 << 11), 0), role(ADMIN_ROLE, "Admins", str(1 << 3), 5),
             role(PING_ROLE, "Pingable", "0", 2, color=255), role(MOD_ROLE, "Mods", "0", 3),
             role(MANAGED_ROLE, "Kiwi", str(1 << 14), 4, managed=True),
             role(CLUB_PRES, "President", "0", 6), role(CLUB_OFF, "Officer", "0", 3), role(CLUB_MEM, "Member", "0", 1)]
    guild = {"id": str(GID), "name": "Parity Guild", "owner_id": str(OWNER), "roles": roles}
    channels = [
        {"id": str(CH_TEXT), "type": 0, "name": "general", "position": 2, "permission_overwrites": []},
        {"id": str(CH_NEWS), "type": 5, "name": "news", "position": 1,
         "permission_overwrites": [{"id": str(MANAGED_ROLE), "type": 0, "allow": "0", "deny": str(1 << 14)}]},
        {"id": str(CH_VOICE), "type": 2, "name": "voice", "position": 0},
        {"id": str(CH_LOCKED), "type": 0, "name": "locked", "position": 2,
         "permission_overwrites": [{"id": str(GID), "type": 0, "allow": "0", "deny": str(1 << 10)},
                                   {"id": str(BOT), "type": 1, "allow": str(1 << 10), "deny": str(1 << 11)}]},
    ]

    def member(uid, roles_, bot=False, nick=None, gname=None, avatar=None):
        return {"user": {"id": str(uid), "username": f"user{uid % 1000}", "global_name": gname, "avatar": avatar,
                         **({"bot": True} if bot else {})}, "nick": nick, "roles": [str(r) for r in roles_]}

    members = [member(MEMBER_A, [CLUB_OFF], nick="Ace"), member(MEMBER_B, [CLUB_MEM, CLUB_PRES], gname="Bee", avatar="abc"),
               member(MEMBER_BOT, [CLUB_MEM], bot=True), member(USER_PLAIN, [])]
    paths = {
        f"/guilds/{GID}": guild,
        f"/guilds/{GID}/channels": channels,
        f"/guilds/{GID}/roles": roles,
        f"/guilds/{GID}/members/{BOT}": member(BOT, [MANAGED_ROLE], bot=True),
        f"/guilds/{GID}/members/{USER_ADMIN}": member(USER_ADMIN, [ADMIN_ROLE]),
        f"/guilds/{GID}/members/{USER_MOD}": member(USER_MOD, [MOD_ROLE]),
        f"/guilds/{GID}/members/{USER_PINGER}": member(USER_PINGER, [PING_ROLE]),
        f"/guilds/{GID}/members/{USER_PLAIN}": member(USER_PLAIN, []),
        f"/guilds/{GID}/members/{OWNER}": member(OWNER, []),
        f"/guilds/{GID}/members/{MEMBER_A}": members[0],
        f"/guilds/{GID}/members/{MEMBER_B}": members[1],
        f"/guilds/{GID}/members/{MEMBER_BOT}": members[2],
        f"/guilds/{GID}/members?limit=1000&after=0": members,
        "/users/@me/guilds": [{"id": str(GID), "name": "Parity Guild", "icon": "ic"},
                              {"id": "5555", "name": "Other", "icon": None}],
        "/guilds/5555": {"id": "5555", "owner_id": "1", "roles": [role(5555, "@everyone", "0", 0)]},
        f"/guilds/5555/members/{USER_ADMIN}": member(USER_ADMIN, []),
    }
    user_guilds = [
        {"id": "77", "name": "zeta Invite", "icon": None, "owner": True, "permissions": "0"},
        {"id": str(GID), "name": "Parity Guild", "icon": "ic", "owner": False, "permissions": "8"},
        {"id": "5555", "name": "Other", "icon": None, "owner": False, "permissions": "0"},
        {"id": "88", "name": "alpha manage", "icon": "x", "owner": False, "permissions": str(1 << 5)},
        {"id": "99", "name": "Nothing", "icon": None, "owner": False, "permissions": "abc"},
    ]
    return {"paths": paths, "errors": {}, "bad": [], "bot_id": BOT, "user_guilds": user_guilds,
            "tokens": {"65a1b2c3d4e5f60718293a4b": "tok"}, "anchors": {"chaos_chest": "1791990000",
                                                                        "challenge_rampage": "RAISE"}}


USER_ID = ObjectId("65a1b2c3d4e5f60718293a4b")
T0 = dt.datetime(2026, 9, 1, 12, 0, 0, 250000, tzinfo=dt.timezone.utc)


def cfg_doc(**kw):
    c = bot_models.GuildConfig(guild_id=GID, created_at=T0, updated_at=T0, **kw)
    from beanie.odm.utils.dump import get_dict
    return rt({"_id": ObjectId("65d000000000000000000001"), **get_dict(c, to_db=True)})


def club_doc(oid, name, **kw):
    c = bot_models.Club(guild_id=GID, name=name, created_at=kw.pop("created_at", T0), updated_at=T0, **kw)
    from beanie.odm.utils.dump import get_dict
    return rt({"_id": ObjectId(oid), **get_dict(c, to_db=True)})


def legacy_doc():
    d = cfg_doc()
    d["hourly_challenge_enabled"] = True
    d["announce_channel_id"] = CH_TEXT
    d["last_announced_challenge_anchor"] = 1791000000
    d["announcements"] = {"daily_bonuses": {"enabled": True, "channel_id": CH_LOCKED, "ping_role_ids": [],
                                            "ping_role_id": PING_ROLE, "template": None, "last_anchor": "x",
                                            "channel_missing": False}}
    d["extra_field"] = "kept"
    return rt(d)


def seeded_db(cfg=None, clubs=None):
    return {
        "guild_configs": [cfg] if cfg is not None else [],
        "clubs": clubs if clubs is not None else [
            club_doc("65e000000000000000000001", "Alpha", role_links={"president": CLUB_PRES, "officer": CLUB_OFF,
                                                                      "member": CLUB_MEM, "captain": 123}),
            club_doc("65e000000000000000000002", "Beta",
                     created_at=dt.datetime(2026, 8, 1, tzinfo=dt.timezone.utc)),
        ],
    }


KEPT_HEADERS = ("content-type",)


async def call(method, url, user, body):
    path, _, qs = url.partition("?")
    raw = b"" if body is None else (body.encode() if isinstance(body, str) else json.dumps(body).encode())
    hdrs = [(b"host", b"testserver"), (b"content-type", b"application/json"),
            (b"content-length", str(len(raw)).encode())]
    scope = {"type": "http", "asgi": {"version": "3.0"}, "http_version": "1.1", "method": method,
             "scheme": "http", "path": urllib.parse.unquote(path), "raw_path": path.encode(),
             "query_string": qs.encode(), "headers": hdrs, "client": ("127.0.0.1", 1234),
             "server": ("testserver", 80), "root_path": ""}
    if user is None:
        app.dependency_overrides.pop(get_current_site_user, None)
    else:
        app.dependency_overrides[get_current_site_user] = lambda: user
    sent = {"status": None, "headers": [], "body": b""}
    got_body = [False]

    async def receive():
        if got_body[0]:
            return {"type": "http.disconnect"}
        got_body[0] = True
        return {"type": "http.request", "body": raw, "more_body": False}

    async def send(msg):
        if msg["type"] == "http.response.start":
            sent["status"] = msg["status"]
            sent["headers"] = [(k.decode().lower(), v.decode()) for k, v in msg["headers"]]
        elif msg["type"] == "http.response.body":
            sent["body"] += msg.get("body", b"")

    await app(scope, receive, send)
    return sent


def norm_body(b: bytes):
    try:
        doc = json.loads(b)
    except ValueError:
        return {"raw": b.decode("utf-8", "replace")}
    if isinstance(doc, dict) and isinstance(doc.get("error"), dict):
        doc["error"]["request_id"] = None
        return {"error_json": doc}
    return {"bytes": b.decode()}


def user_ns(discord_id):
    return types.SimpleNamespace(id=USER_ID, discord_id=discord_id)


def T(path):
    return f"/v1/site-auth/discord{path}"


def dashboard_cases():
    W = base_world
    cfg_perms = cfg_doc(config_perms={"manage_announcements": [MOD_ROLE], "manage_ping_roles": [PING_ROLE],
                                      "manage_clubs": [MOD_ROLE], "unknown_cap": [123]},
                        announcements={"chaos_chest": {"enabled": True, "channel_id": CH_TEXT,
                                                       "ping_role_ids": [PING_ROLE, 55]},
                                       "trove_news": {"enabled": True, "channel_id": 123, "channel_missing": False},
                                       "stampy": {"enabled": False, "channel_id": CH_NEWS, "channel_missing": True,
                                                  "template": {"enabled": True, "title": "Hi {biome}",
                                                               "fields": [{"name": "A", "value": "B"}]}}},
                        live_board={"enabled": True, "channel_id": CH_LOCKED, "message_id": 5},
                        market_watch={"enabled": True, "channel_id": CH_TEXT, "ping_role_ids": [PING_ROLE],
                                      "items": [{"name": "Dragon Egg", "max_price_each": 1000.0,
                                                 "last_alert_sig": "900"}]},
                        language="fr")
    c = []

    def case(name, method, tmpl, path, user=USER_ADMIN, body=None, world=None, db=None):
        c.append({"name": name, "method": method, "template": tmpl, "path": path,
                  "user": user, "body": body, "world": world or W(), "db": db or seeded_db(cfg_perms)})

    G = f"/guilds/{GID}"
    case("list_ok", "GET", "/v1/site-auth/discord/guilds", T("/guilds"))
    w = W()
    w["user_guilds"] = None
    case("list_token_rejected", "GET", "/v1/site-auth/discord/guilds", T("/guilds"), world=w)
    w = W()
    w["tokens"] = {}
    case("list_no_token", "GET", "/v1/site-auth/discord/guilds", T("/guilds"), world=w)
    case("list_unlinked", "GET", "/v1/site-auth/discord/guilds", T("/guilds"), user=0)
    w = W()
    w["errors"]["/users/@me/guilds"] = "GET /users/@me/guilds -> 500: boom"
    case("list_bot_guilds_fail", "GET", "/v1/site-auth/discord/guilds", T("/guilds"), world=w)
    case("list_mod", "GET", "/v1/site-auth/discord/guilds", T("/guilds"), user=USER_MOD)
    case("list_plain", "GET", "/v1/site-auth/discord/guilds", T("/guilds"), user=USER_PLAIN)
    case("list_legacy_migrates", "GET", "/v1/site-auth/discord/guilds", T("/guilds"), db=seeded_db(legacy_doc()))

    D = "/v1/site-auth/discord/guilds/{guild_id}"
    case("detail_admin", "GET", D, T(G))
    case("detail_owner", "GET", D, T(G), user=OWNER)
    case("detail_mod", "GET", D, T(G), user=USER_MOD)
    case("detail_pinger", "GET", D, T(G), user=USER_PINGER)
    case("detail_plain", "GET", D, T(G), user=USER_PLAIN)
    case("detail_no_cfg", "GET", D, T(G), db=seeded_db())
    case("detail_legacy", "GET", D, T(G), db=seeded_db(legacy_doc()))
    case("detail_not_member", "GET", D, T("/guilds/5555"), user=USER_PLAIN)
    case("detail_unlinked", "GET", D, T(G), user=0)
    case("detail_bad_id", "GET", D, T("/guilds/abc"))
    case("detail_float_id", "GET", D, T(f"/guilds/{GID}.0"))
    w = W()
    w["errors"][f"/guilds/{GID}"] = f"GET /guilds/{GID} -> 502: upstream"
    case("detail_discord_down", "GET", D, T(G), world=w)
    w = W()
    del w["paths"][f"/guilds/{GID}/members/{BOT}"]
    case("detail_bot_missing", "GET", D, T(G), world=w)
    w = W()
    w["bad"].append(f"/guilds/{GID}/channels")
    case("detail_bad_payload", "GET", D, T(G), world=w)

    A = "/v1/site-auth/discord/guilds/{guild_id}/announcements"
    case("ann_enable", "PUT", A, T(G + "/announcements"), body={"announcements": {
        "chaos_chest": {"enabled": True, "channel_id": str(CH_NEWS), "ping_role_ids": [str(PING_ROLE), str(PING_ROLE), "x"]},
        "challenge_rampage": {"enabled": True, "channel_id": str(CH_TEXT)},
        "retired": {"enabled": True},
        "longshade": {"enabled": False, "channel_id": None, "ping_role_ids": [str(MOD_ROLE), str(ADMIN_ROLE)]}}})
    case("ann_new_cfg", "PUT", A, T(G + "/announcements"), db=seeded_db(), body={"announcements": {
        "game_update": {"enabled": True, "channel_id": str(CH_TEXT), "ping_role_ids": []}}})
    case("ann_no_channel", "PUT", A, T(G + "/announcements"), body={"announcements": {
        "stampy": {"enabled": True, "channel_id": str(CH_VOICE)}}})
    case("ann_pinger_toggle", "PUT", A, T(G + "/announcements"), user=USER_PINGER, body={"announcements": {
        "stampy": {"enabled": True, "channel_id": str(CH_TEXT)}}})
    case("ann_pinger_pings", "PUT", A, T(G + "/announcements"), user=USER_PINGER, body={"announcements": {
        "chaos_chest": {"enabled": True, "channel_id": str(CH_TEXT), "ping_role_ids": [str(MOD_ROLE)]}}})
    case("ann_mod_pings", "PUT", A, T(G + "/announcements"), user=USER_MOD, body={"announcements": {
        "chaos_chest": {"enabled": True, "channel_id": str(CH_TEXT), "ping_role_ids": [str(MOD_ROLE)]}}})
    case("ann_plain", "PUT", A, T(G + "/announcements"), user=USER_PLAIN, body={"announcements": {}})
    case("ann_422_numbers", "PUT", A, T(G + "/announcements"), body={"announcements": {
        "x": {"enabled": "yes!", "channel_id": 5, "ping_role_ids": [1, "2"]}, "y": 3}})
    case("ann_422_missing", "PUT", A, T(G + "/announcements"), body={})
    case("ann_422_path_and_body", "PUT", A, T("/guilds/x/announcements"), body={"announcements": []})
    case("ann_422_json", "PUT", A, T(G + "/announcements"), body='{"announcements": ')
    case("ann_bool_coerce", "PUT", A, T(G + "/announcements"), body={"announcements": {
        "wild_mana": {"enabled": "true", "channel_id": str(CH_TEXT)}, "fluxion": {"enabled": 0}}})

    TP = "/v1/site-auth/discord/guilds/{guild_id}/announcements/{key}/template"
    case("tmpl_set", "PUT", TP, T(G + "/announcements/chaos_chest/template"),
         body={"template": {"enabled": True, "title": "T", "color": "#fff000",
                            "fields": [{"name": "n", "value": "v", "inline": False}], "image_design_id": "d"}})
    case("tmpl_clear", "PUT", TP, T(G + "/announcements/stampy/template"), body={"template": None})
    case("tmpl_empty_body", "PUT", TP, T(G + "/announcements/stampy/template"), body={})
    case("tmpl_luxion", "PUT", TP, T(G + "/announcements/luxion/template"), body={})
    case("tmpl_unknown", "PUT", TP, T(G + "/announcements/nope/template"), body={})
    case("tmpl_pinger", "PUT", TP, T(G + "/announcements/stampy/template"), user=USER_PINGER, body={})
    case("tmpl_422", "PUT", TP, T(G + "/announcements/stampy/template"),
         body={"template": {"fields": [{"name": 1}], "show_image": "maybe"}})
    w = W()
    del w["paths"][f"/guilds/{GID}/channels"]
    case("tmpl_saves_before_snapshot", "PUT", TP, T(G + "/announcements/stampy/template"),
         body={"template": {"enabled": True}}, world=w)

    LG = "/v1/site-auth/discord/guilds/{guild_id}/language"
    case("lang_set", "PUT", LG, T(G + "/language"), body={"language": "ja"})
    case("lang_bad", "PUT", LG, T(G + "/language"), body={"language": "xx"})
    case("lang_new", "PUT", LG, T(G + "/language"), body={"language": "pt-PT"}, db=seeded_db())
    case("lang_422", "PUT", LG, T(G + "/language"), body={"language": 5})

    LB = "/v1/site-auth/discord/guilds/{guild_id}/live-board"
    case("board_switch", "PUT", LB, T(G + "/live-board"), body={"enabled": True, "channel_id": str(CH_TEXT)})
    case("board_same", "PUT", LB, T(G + "/live-board"), body={"enabled": True, "channel_id": str(CH_LOCKED)})
    case("board_no_channel", "PUT", LB, T(G + "/live-board"), body={"enabled": True, "channel_id": "123"})
    case("board_off", "PUT", LB, T(G + "/live-board"), body={})

    MW = "/v1/site-auth/discord/guilds/{guild_id}/market-watch"
    items = [{"name": " dragon egg ", "max_price_each": "1500.5"}, {"name": "DRAGON EGG"}, {"name": "  "},
             {"name": "Box", "max_price_each": 0}, {"name": "Gem", "max_price_each": -3},
             {"name": "Flux", "max_price_each": 7}]
    case("mw_set", "PUT", MW, T(G + "/market-watch"),
         body={"enabled": True, "channel_id": str(CH_NEWS), "ping_role_ids": [str(MOD_ROLE), str(MOD_ROLE), "1"],
               "items": items})
    case("mw_mod_no_ping_cap", "PUT", MW, T(G + "/market-watch"), user=USER_MOD,
         body={"enabled": False, "ping_role_ids": [str(MOD_ROLE)], "items": [{"name": "x" * 3}] * 60})
    case("mw_no_channel", "PUT", MW, T(G + "/market-watch"), body={"enabled": True})
    case("mw_422", "PUT", MW, T(G + "/market-watch"), body={"items": [{"max_price_each": "abc"}, "x"]})
    case("mw_pinger", "PUT", MW, T(G + "/market-watch"), user=USER_PINGER, body={})

    CL = "/v1/site-auth/discord/guilds/{guild_id}/clubs"
    case("clubs_list", "GET", CL, T(G + "/clubs"))
    case("clubs_list_mod", "GET", CL, T(G + "/clubs"), user=USER_MOD)
    case("clubs_list_pinger", "GET", CL, T(G + "/clubs"), user=USER_PINGER)
    case("clubs_create", "POST", CL, T(G + "/clubs"))
    full = seeded_db(cfg_perms, clubs=[club_doc(f"65e00000000000000000000{i}", f"C{i}") for i in range(1, 4)])
    case("clubs_create_full", "POST", CL, T(G + "/clubs"), db=full)

    CU = "/v1/site-auth/discord/guilds/{guild_id}/clubs/{club_id}"
    case("club_update", "PUT", CU, T(G + "/clubs/65e000000000000000000001"),
         body={"name": "  " + "N" * 120 + " ", "public": True, "description": "  " + "d" * 1200,
               "banner_url": " https://x/b.png ", "avatar_url": "", "discord_url": None,
               "website_url": "http://site", "role_links": {"president": str(CLUB_PRES), "officer": str(MANAGED_ROLE),
                                                            "member": "abc", "boss": str(CLUB_OFF),
                                                            "captain": str(CLUB_OFF)}})
    case("club_update_bad_url", "PUT", CU, T(G + "/clubs/65e000000000000000000001"),
         body={"name": "ok", "avatar_url": "ftp://x"})
    case("club_update_long_url", "PUT", CU, T(G + "/clubs/65e000000000000000000001"),
         body={"name": "ok", "website_url": "https://" + "a" * 495})
    case("club_update_no_name", "PUT", CU, T(G + "/clubs/65e000000000000000000001"), body={"name": "   "})
    case("club_update_missing", "PUT", CU, T(G + "/clubs/65e00000000000000000ffff"), body={"name": "x"})
    case("club_update_bad_oid", "PUT", CU, T(G + "/clubs/nope"), body={"name": "x"})
    case("club_update_422", "PUT", CU, T(G + "/clubs/nope"), body={"public": "x", "role_links": {"a": 1}})
    case("club_delete", "DELETE", CU, T(G + "/clubs/65e000000000000000000002"))
    case("club_delete_other_guild", "DELETE", CU, T("/guilds/5555/clubs/65e000000000000000000002"))

    RO = "/v1/site-auth/discord/guilds/{guild_id}/clubs/{club_id}/roster"
    case("roster", "GET", RO, T(G + "/clubs/65e000000000000000000001/roster"))
    case("roster_unlinked", "GET", RO, T(G + "/clubs/65e000000000000000000002/roster"))
    w = W()
    w["errors"][f"/guilds/{GID}/members?limit=1000&after=0"] = "GET members -> 403: Missing Access"
    case("roster_no_intent", "GET", RO, T(G + "/clubs/65e000000000000000000001/roster"), world=w)
    w = W()
    w["errors"][f"/guilds/{GID}/members?limit=1000&after=0"] = "Could not reach Discord: timeout"
    case("roster_down", "GET", RO, T(G + "/clubs/65e000000000000000000001/roster"), world=w)

    RA = "/v1/site-auth/discord/guilds/{guild_id}/clubs/{club_id}/roster/{member_id}/{action}"
    case("promote", "POST", RA, T(G + f"/clubs/65e000000000000000000001/roster/{MEMBER_A}/promote"))
    case("demote", "POST", RA, T(G + f"/clubs/65e000000000000000000001/roster/{MEMBER_A}/demote"))
    case("demote_pres", "POST", RA, T(G + f"/clubs/65e000000000000000000001/roster/{MEMBER_B}/demote"))
    case("promote_pres", "POST", RA, T(G + f"/clubs/65e000000000000000000001/roster/{MEMBER_B}/promote"))
    case("promote_bot", "POST", RA, T(G + f"/clubs/65e000000000000000000001/roster/{MEMBER_BOT}/promote"))
    case("promote_rankless", "POST", RA, T(G + f"/clubs/65e000000000000000000001/roster/{USER_PLAIN}/promote"))
    case("promote_gone", "POST", RA, T(G + "/clubs/65e000000000000000000001/roster/12345/promote"))
    case("roster_bad_action", "POST", RA, T(G + f"/clubs/65e000000000000000000001/roster/{MEMBER_A}/kick"))
    case("roster_bad_member", "POST", RA, T(G + "/clubs/65e000000000000000000001/roster/abc/kick"))
    case("roster_unlinked_club", "POST", RA, T(G + f"/clubs/65e000000000000000000002/roster/{MEMBER_A}/promote"))
    w = W()
    w["errors"][f"/guilds/{GID}/members/{MEMBER_B}/roles/{CLUB_OFF}"] = "PUT x -> 403: Missing Permissions"
    case("demote_pres_role_fail", "POST", RA, T(G + f"/clubs/65e000000000000000000001/roster/{MEMBER_B}/demote"),
         world=w)
    w = W()
    w["errors"][f"/guilds/{GID}/members/{MEMBER_A}/roles/{CLUB_MEM}"] = "PUT x -> 500: oops"
    case("demote_role_fail", "POST", RA, T(G + f"/clubs/65e000000000000000000001/roster/{MEMBER_A}/demote"), world=w)
    w = W()
    w["errors"][f"/guilds/{GID}/members/{MEMBER_A}/roles/{CLUB_MEM}"] = "PUT x -> 403: Missing Permissions"
    case("demote_role_403", "POST", RA, T(G + f"/clubs/65e000000000000000000001/roster/{MEMBER_A}/demote"), world=w)
    w = W()
    w["errors"][f"/guilds/{GID}/members/{MEMBER_A}"] = "GET -> 500: down"
    case("roster_member_fetch_fail", "POST", RA,
         T(G + f"/clubs/65e000000000000000000001/roster/{MEMBER_A}/promote"), world=w)

    PM = "/v1/site-auth/discord/guilds/{guild_id}/permissions"
    case("perms_set", "PUT", PM, T(G + "/permissions"), body={"permissions": {
        "manage_clubs": [str(MOD_ROLE), str(MOD_ROLE), str(PING_ROLE), str(GID), str(MANAGED_ROLE)],
        "manage_ping_roles": [], "bogus": [str(MOD_ROLE)], "manage_announcements": [str(ADMIN_ROLE)]}})
    case("perms_new", "PUT", PM, T(G + "/permissions"), db=seeded_db(legacy_doc()),
         body={"permissions": {"manage_clubs": [str(MOD_ROLE)]}})
    case("perms_mod", "PUT", PM, T(G + "/permissions"), user=USER_MOD, body={"permissions": {}})
    case("perms_422", "PUT", PM, T(G + "/permissions"), body={"permissions": {"a": "b"}})

    case("meta", "GET", "/v1/site-auth/discord/announcement-meta", T("/announcement-meta"))
    return c


def gen_dashboard():
    out = []
    worlds: list = []
    dbs: list = []

    def intern(pool, v):
        if v not in pool:
            pool.append(v)
        return pool.index(v)

    for cse in dashboard_cases():
        WORLD.clear()
        WORLD.update(copy.deepcopy(cse["world"]))
        SENT.clear()
        CLEARED.clear()
        DB.clear()
        DB.update({k: [rt(d) for d in v] for k, v in cse["db"].items()})
        _IDS[0] = 0
        before = {k: [ext(d) for d in v] for k, v in DB.items()}
        uid = cse["user"]
        user = user_ns(uid if uid else None)
        r = LOOP.run_until_complete(call(cse["method"], cse["path"], user, cse["body"]))
        out.append({
            "name": cse["name"], "method": cse["method"], "template": cse["template"], "path": cse["path"],
            "user_discord_id": uid, "body": cse["body"], "world": intern(worlds, cse["world"]),
            "db_before": intern(dbs, before),
            "status": r["status"], "response": norm_body(r["body"]),
            "db_after": {k: [ext(d) for d in v] for k, v in DB.items()},
            "sent": list(SENT), "cleared": list(CLEARED),
        })
    return {"worlds": worlds, "dbs": dbs, "cases": out}


# --- models, permissions, interactions ---------------------------------------------------------

def gen_models():
    from beanie.odm.utils.dump import get_dict
    cases = []
    docs = [cfg_doc(), legacy_doc(), cfg_doc(announcements={"hourly_challenge": {"enabled": True, "channel_id": 5,
                                                                                  "ping_role_ids": [7],
                                                                                  "last_anchor": "9"},
                                                            "challenge_racing": {"enabled": False}},
                                             config_perms={"a": [1, 2]}, market_watch={"items": [{"name": "x", "max_price_each": 5}]})]
    for d in docs:
        cfg = bot_models.GuildConfig.model_validate(d)
        changed = cfg.migrate_legacy()
        cases.append({"in": ext(d), "migrated": changed, "out": ext(rt(get_dict(cfg, to_db=True)))})
    new_cfg = bot_models.GuildConfig(guild_id=5)
    new_club = bot_models.Club(guild_id=5, name="New Club", updated_by=7)
    return {"guild_configs": cases, "new_config": ext(rt(get_dict(new_cfg, to_db=True))),
            "new_club": ext(rt(get_dict(new_club, to_db=True)))}


def gen_reconcile():
    out = []
    base = cfg_doc(announcements={"a": {"enabled": True, "channel_id": 1, "ping_role_ids": [10, 11]},
                                  "b": {"enabled": False, "channel_id": 2, "channel_missing": True},
                                  "c": {"enabled": True, "channel_id": None, "channel_missing": True},
                                  "d": {"enabled": False, "channel_id": 3, "channel_missing": True}},
                   live_board={"enabled": False, "channel_id": 9, "message_id": 4, "channel_missing": False},
                   config_perms={"x": [10], "y": [10, 12]})
    for channels, roles in (([1, 2], [10, 11, 12]), ([2], [12]), ([], []), ([1, 2, 3, 9], [11])):
        cfg = bot_models.GuildConfig.model_validate(base)
        changed = bot_reconcile.reconcile(cfg, channels, roles)
        from beanie.odm.utils.dump import get_dict
        out.append({"channels": channels, "roles": roles, "changed": changed,
                    "out": ext(rt(get_dict(cfg, to_db=True)))})
    ranks = []
    for linked in ([], ["president"], ["president", "officer", "member"], ["vice_president", "captain"],
                   list(bot_models.CLUB_RANKS)):
        for r in bot_models.CLUB_RANKS + ("bogus",):
            ranks.append([r, linked, bot_models.promote_rank(r, set(linked)), bot_models.demote_rank(r, set(linked))])
    return {"reconcile": out, "ranks": ranks}


def gen_permissions():
    w = base_world()
    guild = w["paths"][f"/guilds/{GID}"]
    chans = w["paths"][f"/guilds/{GID}/channels"]
    members = [w["paths"][f"/guilds/{GID}/members/{u}"] for u in (BOT, USER_ADMIN, USER_MOD, OWNER)]
    out = []
    for m in members:
        for ch in chans:
            if ch["type"] in (0, 5):
                out.append({"member": m, "channel": ch,
                            "base": discord_rest.guild_permissions(guild, m),
                            "effective": discord_rest.effective_channel_permissions(guild, m, ch)})
    pre = [discord_rest.preflight_for(guild, members[0], chans, cid) for cid in (CH_TEXT, CH_NEWS, CH_LOCKED, 1)]
    return {"guild": guild, "cases": out, "preflight": pre,
            "text_channels": discord_rest.text_channels(chans),
            "assignable": discord_rest.assignable_roles(guild["roles"], GID)}


def gen_verify():
    sk = Ed25519PrivateKey.from_private_bytes(bytes(range(32)))
    pk = sk.public_key().public_bytes_raw().hex()
    body = b'{"type":1}'
    ts = "1792000000"
    sig = sk.sign(ts.encode() + body).hex()
    cases = []
    settings.discord_public_key = pk
    for key, s, t, b in [
        (pk, sig, ts, body), (pk, sig.upper(), ts, body), (pk, " ".join(sig[i:i + 2] for i in range(0, 128, 2)), ts, body),
        (pk, sig, ts + "1", body), (pk, sig, ts, body + b" "), (pk, sig[:-2], ts, body), (pk, sig + "00", ts, body),
        (pk, sig[:-1], ts, body), (pk, "zz" + sig[2:], ts, body), (pk[:-2], sig, ts, body), ("", sig, ts, body),
        (" " + pk + " ", sig, ts, body), (pk, sig, "", body),
    ]:
        settings.discord_public_key = key
        cases.append({"key": key, "sig": s, "ts": t, "body": b.decode(), "ok": discord_router._verify(s, t, b)})
    fromhex = []
    for s in ["", "00ff", "0 0", "00 ff", " 0aff ", "0", "0g", "\t0a\n0b", "ÿÿ"]:
        try:
            fromhex.append([s, bytes.fromhex(s).hex()])
        except ValueError:
            fromhex.append([s, None])
    return {"public_key": pk, "cases": cases, "fromhex": fromhex}


def gen_meta():
    keys = [t.key for t in ann.ANNOUNCEMENT_TYPES]
    return {"keys": keys, "catalog": ann.catalog(),
            "customizable": {k: embed_contexts.has_customization(k) for k in keys},
            "commands": commands.COMMAND_DEFS}


def main():
    golden = {
        "i18n": gen_i18n(),
        "commands": gen_commands(),
        "builders": gen_builders(),
        "registry": gen_registry(),
        "contexts": gen_contexts(),
        "dashboard": gen_dashboard(),
        "models": gen_models(),
        "reconcile": gen_reconcile(),
        "permissions": gen_permissions(),
        "verify": gen_verify(),
        "meta": gen_meta(),
        "settings": {"client_id": settings.discord_client_id, "api_url": settings.api_url,
                     "oauth": bool(settings.discord_oauth_enabled), "asset_url": settings.asset_url,
                     "now": NOW.isoformat()},
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(golden, ensure_ascii=False, indent=1, default=str) + "\n", encoding="utf-8")
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
