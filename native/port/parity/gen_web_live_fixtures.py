"""Golden fixtures for the website's live pages (app/site/ssr.py first paints), from the repo root:
    TZ=UTC S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_web_live_fixtures.py
Writes native/kiwi-server/testdata/web_live/: golden.json and upstream.json.

Drives the real website app (raw ASGI, no lifespan) with internal_get answered
from upstream.json per scenario. TZ=UTC matches the production containers,
where naive ISO timestamps read as UTC. No negative timestamps: Windows gmtime
rejects them where Linux does not.
"""
import asyncio
import base64
import datetime as dt
import hashlib
import json
import logging
import os
import shutil
import sys
import tempfile
import warnings
import zlib
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
sys.path.insert(0, os.getcwd())

assert dt.datetime(2026, 7, 1).timestamp() == 1782864000, "run with TZ=UTC"

ROOT = Path(os.getcwd())
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "web_live"
OUT.mkdir(parents=True, exist_ok=True)
GEO = Path(tempfile.mkdtemp(prefix="kiwi-web-live-geo-"))
for name in ("dbip-country-lite.mmdb", "dbip-asn-lite.mmdb"):
    shutil.copy(ROOT / "native" / "kiwi-server" / "testdata" / "web" / name, GEO / name)

ENV = {
    "SITE_ROOT": str(ROOT / "site"),
    "APP_URL": "https://trove.aallyn.net",
    "API_URL": "https://api.aallyn.net",
    "DEV_URL": "https://dev.aallyn.net",
    "INTERNAL_API_URL": "http://internal.test",
    "INTERNAL_KEY": "web-fixture-internal-key",
    "SECRET_KEY": "web-fixture-secret",
    "GEOIP_DIR": str(GEO),
    "GAMEDATA_DIR": str(ROOT / "app" / "trove" / "gamedata"),
    "DISCORD_CLIENT_ID": "1234567890",
    "DISCORD_CLIENT_SECRET": "web-fixture-discord-secret",
    "PAGEVIEW_TRACKING_ENABLED": "true",
}
os.environ.update(ENV)

import app.core.internal_api as internal_api  # noqa: E402
import app.web.feature_flags as web_flags  # noqa: E402
import app.web.meta as web_meta  # noqa: E402
import app.web.pages as web_pages  # noqa: E402
import app.geo_maintenance.gate as geo_gate  # noqa: E402
import app.embed.service as embed_service  # noqa: E402
import app.admin.runtime_config as runtime_config  # noqa: E402
import app.site.ssr as ssr  # noqa: E402
from app.web.main import app  # noqa: E402

FLAG_KEYS = list(web_flags._ALL_ATTRS)
T0 = 1760000000

LONG = ("**Fixed** the <b>gem</b> [builder](https://x.example/a) and ![shot](https://x.example/s.png) "
        "`code` # heading > quote ~strike~ | pipe _under_ " + "word " * 60)


def rotations(odd=False):
    if odd:
        return {
            "daily_buff": {"name": "   ", "emoji": "x"},
            "weekly_buff": {"name": "Hoard Week", "emoji": 7, "buffs": {"Loot +50%": 1, "XP": 2},
                            "premium_buffs": "ab"},
            "chaos": {"ends_at": True, "item": "not a dict"},
            "merchants": [
                {"name": 12, "active": 1, "state": None, "biomes": {"Neon City": 1, "Desert": 2},
                 "schedule": "abcdef", "window": [1, 2]},
                "skip me", None,
                {"name": "", "active": 0, "state": "Open", "biomes": [None, 0, "", {}, {"name": None}, "Tundra"]},
            ],
            "daily_rotation": {"Mon": 1},
            "server_time": [1, 2],
        }
    return {
        "server_time": {"now_unix": T0, "now_iso": "2025-10-09T08:53:20+00:00", "trove_day": "Thursday",
                        "daily_reset_at": T0 + 7600, "weekly_reset_at": T0 + 300000},
        "daily_buff": {"name": "Treasure Day", "emoji": "\U0001f4b0", "color": "#e6bf62",
                       "normal_buffs": ["+50% Loot", 5, "+25% XP", "Chests <3", "Fifth", "Sixth"],
                       "premium_buffs": ["Patron & friends", None, "Double", "Triple", "Quad", "Penta"]},
        "weekly_buff": {"name": "Gem Week", "emoji": "", "buffs": ["Gem drops +100%", "Geode XP"]},
        "daily_rotation": [
            {"weekday": "Monday", "name": "Treasure Day", "is_current": False},
            {"weekday": "Tuesday", "name": "Gem Day", "is_current": True},
            {"weekday": "W", "name": None},
            {"weekday": 12345, "name": 7, "is_current": "yes"},
            "junk",
        ],
        "weekly_rotation": [],
        "chaos": {"starts_at": T0 - 3600, "ends_at": T0 + 5000, "seconds_remaining": 5000,
                  "item": {"name": "Sunset Sail <Deluxe>", "identifier": "x"}},
        "merchants": [
            {"id": "corruxion", "name": "Corruxion", "active": True, "state": None, "starts_at": T0 - 100,
             "ends_at": T0 + 200, "biomes": [], "window": None,
             "schedule": [{"starts_at": T0 + i * 100, "ends_at": T0 + i * 100 + 50, "biomes": []} for i in range(6)]},
            {"id": "fluxion", "name": "Fluxion", "active": False, "state": "Voting", "starts_at": None,
             "ends_at": None, "biomes": [{"name": "Neon City", "icon": "neon.png"}, {"name": None}, "bare"],
             "schedule": [{"starts_at": 1.5, "ends_at": None, "biomes": [{"name": "Desert", "icon": 3}]}, "x"]},
            {"id": "luxion", "name": "Luxion", "active": True, "state": "Open", "starts_at": T0,
             "ends_at": T0 + 86400 * 3, "window": {"open": True, "starts_at": T0, "ends_at": T0 + 10800},
             "biomes": ["Candoria", {"name": "Fae Forest"}, {"icon": "x"}, "Fourth"], "schedule": []},
            {"id": "wild_mana", "name": "Wild Mana", "active": True, "state": "Here",
             "biomes": [{"name": "Lunar Lagoon", "icon": "lunar.png"}], "starts_at": 1.25e9, "ends_at": 1e16},
        ],
    }


def news(n=15):
    out = []
    for i in range(n):
        cats = ["Events"] if i % 3 else ["Shop Offers", "Events"]
        if i == 4:
            cats = "Patch, Shop Offers"
        if i == 5:
            cats = None
        out.append({"title": f"News <{i}> & \"more\"", "url": f"https://trovegame.com/news/{i}",
                    "category": "Events" if i % 2 else "", "categories": cats, "image": None,
                    "published_at": ["2026-10-01T10:00:00", "2026-10-01T10:00:00Z", T0 + i, 1.7e9 + 0.9,
                                     "2026-10-01T23:30:00-05:00", "2026-10-01", "nope", "", None, True,
                                     86401, "2026-10-01T10:00:00.999999+00:00", "2026-W40-4T05:06",
                                     "2026-10-01T10:00:00ZZ", "20261001T100000"][i]})
    return out


def videos(platform, n=14):
    return [{"title": f"{platform} video {i} \u2014 \u00fcber", "url": f"https://{platform}.example/{i}",
             "channel": f"Chan {i}" if i % 4 else None,
             "thumbnail_url": "https://img.example/{width}x{height}.jpg" if i % 2 else None,
             "thumbnail": f"https://img.example/t{i}.jpg" if i % 3 else "",
             "published_at": T0 - i * 86400} for i in range(n)]


def mods(n=14):
    authors = ["Alice", "alice", "ALICE", None, "Bob", "", "Carol", "Bob", "Dave", "Eve", "Frank", "Grace", "H", "I"]
    out = []
    for i in range(n):
        out.append({"handle": "" if i == 3 else f"h{i}", "slug": f"mod-{i}", "title": "" if i == 6 else f"Mod {i} <x>",
                    "author": authors[i], "owner_username": f"owner{i % 3}"})
    return out


def full():
    return {
        "/site/rotations": rotations(),
        "/site/feeds/news?limit=16": {"items": news()},
        "/site/feeds/videos?platform=youtube": {"platform": "youtube", "items": videos("youtube"), "fetched_at": T0},
        "/site/feeds/videos?platform=twitch": {"platform": "twitch", "items": videos("twitch", 3), "fetched_at": T0},
        "/site/leaderboards/records": {
            "trove_mastery": {"level": 1234, "points": 9876543, "player_name": "Zed <b>"},
            "geode_mastery": {"level": 345.0, "points": None, "player_name": ""},
            "power_rank": {"value": 41234.5678, "player_name": "O'Neil"},
        },
        "/site/mods/projects?sort=recent&limit=40": {"items": mods() + ["junk"], "total": 15},
        "/site/calendar/events": {
            "ongoing": [{"event_id": i, "name": f"Event {i} & co", "url": "" if i % 2 else f"https://ev.example/{i}",
                         "category": "Bonus", "starts_at": T0 - 86400 * i, "ends_at": T0 + 3600 * i}
                        for i in range(22)],
            "upcoming": [{"name": "Later", "url": "https://ev.example/later", "starts_at": "2026-11-01T00:00:00Z",
                          "ends_at": None}, 5],
            "now": T0,
        },
        "/site/btt/releases?limit=24": {"channel": None, "count": 4, "total": 4, "items": [
            {"tag_name": "v2.1.0", "name": "", "body": LONG, "html_url": "https://github.com/x/releases/v2.1.0",
             "channel": "release", "published_at": "2026-09-30T12:00:00Z"},
            {"tag_name": "v2.1.0-beta.1", "name": "Beta <1>", "body": None, "html_url": None, "channel": "beta",
             "published_at": None},
            {"tag_name": "", "name": "untagged"},
            {"tag_name": 210, "name": None, "body": "short\n\nnotes \u00a0 here\x1c!", "channel": "",
             "published_at": T0},
        ]},
        "/site/trove-status": {"overall": "online", "checked_at": T0, "environments": {
            "eu": {"status": "online"}, "us": {"status": "maintenance mode"}, "pts": "down", "extra": {}}},
        "/site/leaderboards/activity": {"estimate_24h": 12345, "estimate_7d": 45678.9, "window_end": T0},
        "/site/leaderboards/class-activity/current": {"total_active": 98765, "classes": [
            {"name": "Knight", "active_players": 100, "share": 0.25},
            {"name": "Bard", "active_players": 300.5, "share": 0.12345},
            {"name": "Neon Ninja", "active_players": 100, "share": True},
            {"name": "Ice Sage", "active_players": 100, "share": 1},
            {"name": "Revenant", "active_players": 2500, "share": 0.0005},
            {"name": "", "active_players": 5},
            {"name": "Vanguardian", "active_players": "many"},
            {"name": "Chloromancer", "active_players": 0, "share": None},
            {"name": "Lunar Lancer", "active_players": True, "share": 0.125},
        ]},
        "/site/giveaways": {"items": [
            {"prize_name": "Sunset Sail", "title": "Weekly <draw>", "description": LONG, "status": "open",
             "entry_count": 0, "ends_at": "2026-10-30T11:00:00", "winner_username": None},
            {"prize_name": "", "title": None, "description": None, "status": None, "entry_count": None,
             "ends_at": None, "winner_username": "Win & Ner"},
        ] + [{"prize_name": f"Prize {i}", "entry_count": 1234567, "status": "closed"} for i in range(12)]},
        "/site/mod-stats": {"mods": [
            {"title": "Zakros UI - Chat", "handle": "zakros", "slug": "chat"},
            {"title": "Zakros UI - alpha bars", "handle": "zakros", "slug": "bars"},
            {"title": "Zakros FX - Glow", "handle": "zakros", "slug": "glow"},
            {"title": "Zakros BP - Ship", "handle": "zakros", "slug": "ship"},
            {"title": "Other - <script>alert(1)</script>", "handle": "zakros", "slug": "x"},
            {"title": "Aardvark - Thing", "handle": "zakros", "slug": "aard"},
            {"title": "Loose Mod", "handle": "zakros", "slug": "loose"},
            {"title": "Zakros UI - \u00c9clair", "handle": "zakros", "slug": "eclair"},
            {"title": "No Handle - X", "handle": None, "slug": "nh"},
            {"title": "", "handle": "zakros", "slug": "empty"},
            {"title": "Zakros UI - Chat", "handle": "other", "slug": 5},
        ]},
        "/site/leaderboards/timestamps?limit=1": {"items": ["x", 1.5, T0, T0 - 3600], "count": 4},
        f"/site/leaderboards/boards?created_at={T0}": {"created_at": T0, "items": [
            {"uuid": 1001, "name": "Trove Mastery", "entries": 123456},
            {"uuid": 1002, "name": "Power Rank", "entries": None},
            {"uuid": 1003, "name": "", "entries": 5},
            {"uuid": 1004, "name": "Effort <Knight>", "entries": 0},
        ]},
        f"/site/leaderboards/1001/entries?created_at={T0}&limit=25": {"items": [
            {"rank": 1, "player_name": "Some One", "score": 987654.0},
            {"rank": 2, "player_name": "caf\u00e9 <b>", "score": 12.5},
            {"rank": 3, "player_name": "100%/x?y", "score": None},
            {"rank": None, "player_name": "", "score": 1},
            {"rank": 4.0, "player_name": "O'Neil", "score": True},
        ]},
        "/site/leaderboards/players/Some%20One/profile": {
            "player_name": "Some One", "verified": True,
            "summary": {"best_rank": 3, "boards_appeared": 12, "top10_count": 0, "latest_anchor": T0},
            "boards": [
                {"board_name": "Trove Mastery", "category": "Mastery", "latest_rank": 5, "best_rank": 3,
                 "latest_score": 1234567},
                {"board_name": "", "latest_rank": 1},
                {"board_name": "Effort <Bard>", "category": None, "latest_rank": 0, "best_rank": None,
                 "latest_score": None},
                {"board_name": "Delves", "latest_rank": 1500.0, "best_rank": 2.5, "latest_score": 0},
            ]},
        "/site/leaderboards/players/caf%C3%A9%20%3Cb%3E/profile": {
            "player_name": None, "summary": "x", "boards": "y"},
        "/site/updates/branches": {"items": [
            {"branch": "live-us", "current_version": "v1.2.3"}, {"branch": "pts", "current_version": None},
            {"branch": "", "current_version": "x"}, "junk"]},
        "/site/updates/live-us/versions?limit=24": {"items": [
            {"version_tag": "v1.2.3", "captured_at": "2026-10-20T11:00:00+00:00", "files_added": 1234,
             "files_modified": None, "files_removed": 2.5},
            {"version_tag": "", "captured_at": T0},
            {"version_tag": "v1.2.2", "captured_at": None, "files_added": 0, "files_modified": "x",
             "files_removed": True},
        ]},
    }


def odd():
    return {
        "/site/rotations": rotations(odd=True),
        "/site/feeds/news?limit=16": {"items": "nope"},
        "/site/feeds/videos?platform=youtube": [1, 2],
        "/site/feeds/videos?platform=twitch": {"items": [{"title": None, "thumbnail_url": ""}]},
        "/site/leaderboards/records": {"trove_mastery": "x", "geode_mastery": {}, "power_rank": {"value": "big"}},
        "/site/mods/projects?sort=recent&limit=40": {"items": [{"handle": 5, "slug": 6, "title": None,
                                                                "author": "Seven"}]},
        "/site/calendar/events": {"ongoing": {"a": 1}, "upcoming": None},
        "/site/btt/releases?limit=24": {"items": None},
        "/site/trove-status": {"overall": "down", "checked_at": "2026-10-23T01:02:03Z", "environments": {
            "eu": {"status": None}, "us": {"status": "they're up"}, "pts": {"status": 503}}},
        "/site/leaderboards/activity": {"estimate_24h": True, "estimate_7d": "x", "window_end": "bad"},
        "/site/leaderboards/class-activity/current": {"total_active": 0, "classes": "nope"},
        "/site/giveaways": {"items": [{}]},
        "/site/mod-stats": {"mods": [{"title": "Nope", "handle": "", "slug": "x"}]},
        "/site/leaderboards/timestamps?limit=1": {"items": [True]},
        "/site/leaderboards/boards?created_at=True": {"items": [{"uuid": None, "name": "Solo", "entries": 7}]},
        "/site/leaderboards/boards?created_at=true": {"items": [{"uuid": None, "name": "Solo", "entries": 7}]},
        "/site/leaderboards/players/Some%20One/profile": [1],
        "/site/updates/branches": None,
        "/site/updates/live-us/versions?limit=24": {"items": [{"version_tag": 3}]},
    }


COMMON = {
    "/site/embed/allowed-origins": {"origins": ["https://partner.example"]},
    "/internal/geo-maintenance": {"countries": []},
}
OFF = {"leaderboards_enabled", "mods_hub_enabled", "server_status_enabled", "calendar_enabled", "updates_enabled",
       "player_activity_enabled", "giveaways_enabled"}

SCENARIOS = {
    "full": {"/site/feature-flags": {k: True for k in FLAG_KEYS}, **COMMON, **full()},
    "odd": {"/site/feature-flags": {**{k: True for k in FLAG_KEYS}, "cheater_detection_enabled": False,
                                    "renames_enabled": 0, "duplicates_enabled": "yes"}, **COMMON, **odd()},
    "off": {"/site/feature-flags": {k: k not in OFF for k in FLAG_KEYS}, **COMMON, **{
        k: v for k, v in full().items() if not k.startswith("/site/mod-stats")}},
    "down": {},
}
(OUT / "upstream.json").write_text(json.dumps(SCENARIOS, indent=1, ensure_ascii=False), encoding="utf-8")

CURRENT = {"name": "full"}
FETCHED = []


async def fake_internal_get(path, params=None, *, timeout=8.0, headers=None):
    key = path
    if params:
        key += "?" + "&".join(f"{k}={v}" for k, v in params.items())
    FETCHED.append(key)
    data = SCENARIOS[CURRENT["name"]].get(key)
    return json.loads(json.dumps(data)) if data is not None else None


async def no_db(*a, **k):
    raise RuntimeError("no database in the website container")


for mod in (internal_api, web_flags, web_meta, web_pages, geo_gate):
    mod.internal_get = fake_internal_get
runtime_config.get_setting = no_db


def reset():
    web_flags._cache.update({"at": -1e9, "flags": None})
    web_meta._SITEMAP_CACHE.update({"body": None, "at": -1e9})
    geo_gate._cache.update({"at": -1e9, "countries": frozenset()})
    embed_service._origins_cache.update({"at": -1e9, "value": None})
    ssr._cache.clear()
    ssr._locks.clear()


async def call(method, raw_path, headers):
    path_part, _, query = raw_path.partition("?")
    from urllib.parse import unquote
    scope = {
        "type": "http", "asgi": {"version": "3.0", "spec_version": "2.4"}, "http_version": "1.1",
        "method": method, "scheme": "http", "path": unquote(path_part), "raw_path": path_part.encode(),
        "query_string": query.encode(), "root_path": "",
        "headers": [(k.lower().encode("latin-1"), v.encode("latin-1")) for k, v in headers],
        "client": ("203.0.113.7", 50000), "server": ("testserver", 80),
    }
    sent = False

    async def receive():
        nonlocal sent
        if not sent:
            sent = True
            return {"type": "http.request", "body": b"", "more_body": False}
        await asyncio.sleep(3600)

    out = {"status": None, "headers": [], "body": b""}

    async def send(msg):
        if msg["type"] == "http.response.start":
            out["status"] = msg["status"]
            out["headers"] = [(k.decode("latin-1"), v.decode("latin-1")) for k, v in msg["headers"]]
        elif msg["type"] == "http.response.body":
            out["body"] += msg.get("body", b"")

    await app(scope, receive, send)
    return out


HTML = "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"
PAGES = ["/status", "/calendar", "/streams", "/releases", "/zakros-ui-stats", "/giveaways", "/activity",
         "/class-activity", "/leaderboards", "/updates", "/player/Some%20One"]


def requests_for(scenario):
    reqs = []

    def add(name, path, method="GET", headers=None):
        h = [("host", "trove.aallyn.net"), ("x-request-id", f"live-{scenario}-{len(reqs)}"),
             ("cookie", "btt_theme=dark")]
        if headers is not None:
            h = h[:2] + list(headers)
        reqs.append({"name": name, "method": method, "path": path, "headers": h})

    add("home voxel dark", "/", headers=[("accept", HTML), ("cookie", "btt_theme=dark")])
    add("home voxel light", "/", headers=[("cookie", "btt_design=voxel; btt_theme=light")])
    add("home classic", "/", headers=[("accept", HTML), ("cookie", "btt_design=classic; btt_theme=light")])
    add("home auto", "/", headers=[])
    for p in PAGES:
        add("page " + p, p, headers=[("accept", HTML), ("cookie", "btt_theme=dark")])
    add("page json accept", "/status", headers=[("accept", "application/json")])
    if scenario == "down":
        return reqs
    add("activity 1M", "/activity?period=1M")
    add("activity 7d then bogus", "/activity?period=7d&period=bogus")
    add("activity bogus then 7d", "/activity?period=bogus&period=7d")
    add("activity blank", "/activity?period=")
    add("player unicode", "/player/caf%C3%A9%20%3Cb%3E", headers=[("accept", HTML)])
    add("player quote", "/player/O'Neil", headers=[("accept", HTML), ("cookie", "btt_design=classic")])
    add("player unknown", "/player/nobody")
    add("head home", "/", method="HEAD", headers=[("cookie", "btt_theme=light")])
    add("head leaderboards", "/leaderboards", method="HEAD")
    add("home again", "/", headers=[("cookie", "btt_theme=dark")])
    return reqs


async def run():
    golden = []
    for scenario in SCENARIOS:
        CURRENT["name"] = scenario
        reset()
        for r in requests_for(scenario):
            FETCHED.clear()
            res = await call(r["method"], r["path"], r["headers"])
            golden.append({
                "scenario": scenario, **r,
                "status": res["status"], "resp_headers": res["headers"],
                "resp_body_z": base64.b64encode(zlib.compress(res["body"], 9)).decode(),
                "fetched": [k for k in FETCHED if k.startswith("/site/") and k != "/site/feature-flags"
                            and k != "/site/embed/allowed-origins"],
            })
    return golden


def fingerprint():
    h = hashlib.sha256()
    for sub in ("templates", "static"):
        base = ROOT / "site" / sub
        for p in sorted(base.rglob("*")):
            if p.is_file():
                h.update(p.relative_to(ROOT / "site").as_posix().encode() + b"\0")
                h.update(p.read_bytes())
    for p in (ROOT / "app" / "geo_maintenance" / "maintenance.html",):
        h.update(p.read_bytes())
    return h.hexdigest()


def formatter_cases():
    """ssr.num / date / datetime_attr / text over awkward inputs."""
    nums = [0, 7, -7, 1234567, -1234567, 2 ** 63 + 5, 2500.0, -0.0, 1234.5678, 0.005, 2.675, 0.125, -1234.5,
            -0.001, 1e20, 1.5e300, 999.995, 0.1, True, False, None, "12", [1]]
    dates = [0, T0, 86401, 1.7e9 + 0.9, 1.5, "2026-10-01T10:00:00", "2026-10-01T10:00:00Z", "2026-10-01",
             "2026-10-01T23:30:00-05:00", "2026-10-01T10:00:00.999999+00:00", "2026-W40-4", "2026-10-01T10",
             "nope", "", None, True, 32503593600, "1970-01-01T00:00:00.5-00:00", "1970-01-02T00:00:00.5"]
    texts = [None, "", "plain", LONG, "<a href='x'>link</a>text<>", "<<b>>x", "a\u00a0\u2003b\x1c\x1dc\u200bd",
             "x" * 300, "[a](b) ![c](d) [e] (f) [g](h", "  lead  " + "y" * 250 + " tail", "\u00e9" * 230,
             "word " * 44 + "end"]
    return {
        "num": [[v, ssr.num(v)] for v in nums],
        "date": [[v, ssr.date(v), ssr.datetime_attr(v)] for v in dates],
        "text": [[v, ssr.text(v), ssr.text(v, 20)] for v in texts],
    }


async def main():
    golden = await run()
    data = {"fingerprint": fingerprint(), "env": ENV, "requests": golden, "formatters": formatter_cases()}
    for k in ("SITE_ROOT", "GEOIP_DIR", "GAMEDATA_DIR"):
        data["env"] = {**data["env"], k: None}
    (OUT / "golden.json").write_text(json.dumps(data, indent=0, ensure_ascii=False), encoding="utf-8")
    print(f"{len(golden)} requests")


asyncio.run(main())
