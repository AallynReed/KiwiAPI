"""Golden fixtures for kiwi-server's site_live area (the remaining /site live +
reference reads, crawler files, the site flag map and site search).

The site router is mounted on a bare FastAPI app with the real error handlers;
every store read is replaced by the fixture inputs recorded next to each
output, so the Rust tests (src/site_live/tests.rs, src/gems/site_tests.rs,
src/routes/btt/site_tests.rs) rebuild the same bodies from the same inputs.

Run from the repo root with the production-pinned venv:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_site_live_fixtures.py
"""

from __future__ import annotations

import asyncio
import datetime as dt
import hashlib
import json
import logging
import os
import sys
import tempfile
import types
import warnings
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "site_live"
os.chdir(ROOT)
sys.path.insert(0, str(ROOT))
os.environ.setdefault("SECRET_KEY", "x")

assert sys.version_info[:2] == (3, 13), "run with the production-pinned 3.13 venv"

import httpx  # noqa: E402
from fastapi import FastAPI  # noqa: E402
from fastapi.encoders import jsonable_encoder  # noqa: E402
from fastapi.testclient import TestClient  # noqa: E402

import app.site.router as S  # noqa: E402
from app.admin import router as admin_router  # noqa: E402
from app.admin import runtime_config  # noqa: E402
from app.core import features  # noqa: E402
from app.core.config import settings  # noqa: E402
from app.core.errors import register_error_handlers  # noqa: E402
from app.site import clubs_page, site_changelog  # noqa: E402
from app.site import feature_map as FM  # noqa: E402
from app.site import search as SS  # noqa: E402
from app.site import search_index as SI  # noqa: E402
from app.trove import btt_releases, calendar, luxion, rotations, server_time  # noqa: E402
from app.trove.gems import builds as gem_builds  # noqa: E402

UTC = dt.timezone.utc
FLAGS: dict[str, bool] = {}


async def fake_is_enabled(flag: str) -> bool:
    return bool(FLAGS.get(flag, runtime_config.REGISTRY[flag].default))


features.is_enabled = fake_is_enabled
api = FastAPI()
register_error_handlers(api)
api.include_router(S.router)
client = TestClient(api, raise_server_exceptions=False)


def dumps(v) -> str:
    return json.dumps(v, ensure_ascii=False, allow_nan=False, separators=(",", ":"))


def run(coro):
    return asyncio.new_event_loop().run_until_complete(coro)


def iso(d: dt.datetime | None) -> str | None:
    return d.isoformat() if d else None


def parse_iso(s: str | None) -> dt.datetime | None:
    return dt.datetime.fromisoformat(s) if s else None


# --- flag map, path gate, robots ----------------------------------------------------

GATE_PATHS = [
    "/mods", "/mods/x/y", "/site/mods/projects", "/modpacks", "/modpacks/a/b", "/site/modpacks/projects",
    "/market", "/site/market/items", "/store", "/site/store/products", "/leaderboards", "/player/Bob",
    "/site/leaderboards/boards", "/site/leaderboards/activity", "/site/leaderboards/activity/history",
    "/site/leaderboards/class-activity/current", "/activity", "/activity/og.png", "/class-activity",
    "/clubs", "/site/clubs", "/updates", "/site/updates/live/tree", "/codexes", "/codexes/crafting",
    "/site/codexes/search", "/status", "/status/og.png", "/site/trove-status", "/site/trove-status/history",
    "/giveaways", "/site/giveaways", "/commands", "/server-time", "/site/server-time", "/calendar",
    "/site/calendar/events", "/site/calendar/yearly", "/site/calendarx", "/streams", "/releases",
    "/site/btt/latest", "/site/btt/changelog", "/site/bttx", "/classes", "/site/stats/classes",
    "/site/stats/sigil", "/star-chart", "/sound-studio", "/site/sound-studio/build", "/mod-workshop",
    "/site/mod-workshop/build", "/blueprint-editor", "/site/blueprint-editor/save", "/gem-simulator",
    "/gem-evaluator", "/site/gems/evaluate", "/site/gems/evaluate-simple", "/site/gems/stat-range",
    "/site/gems/lookups", "/gem-builds", "/site/gems/builds/options", "/site/gems/builds/calculate",
    "/site/gems/builds/health-optimizer", "/site/gems/parse-star-chart", "/calculators", "/dressing-room",
    "/site/dressing/classes", "/abilities", "/gem-abilities", "/guides", "/allies", "/gem-tools",
    "/gems-guide", "/fishing-guide", "/gear-guide", "/loot-collector-guide", "/community-sites", "/tomes",
    "/site/tomes", "/unlock-debug", "/unlock_debug", "/site/unlock-debug", "/drop/abc", "/site/drops/x",
    "/robots.txt", "/sitemap.xml", "/BingSiteAuth.xml", "/site/search", "/site/feature-flags",
    "/site/changelog", "/site/screenshots.json", "/site/rotations", "/site/feeds/news", "/",
]


def flag_cases() -> dict:
    cases = []
    variants: list[tuple[str, list[str]]] = [("all on", [])]
    variants += [(attr, [attr]) for attr in FM.SITE_FEATURE_FLAGS]
    variants += [
        ("calculators+builds", ["calculators_enabled", "gem_builds_enabled"]),
        ("abilities+leaderboards", ["abilities_enabled", "leaderboards_enabled"]),
        ("guides+mods", ["guides_enabled", "mods_hub_enabled", "gem_tools_enabled", "allies_enabled"]),
        ("everything", list(FM.SITE_FEATURE_FLAGS)),
    ]
    for name, off in variants:
        f = {k: k not in off for k in FM.SITE_FEATURE_FLAGS}
        resolved = FM.apply_derived(dict(f))
        cases.append({
            "name": name,
            "off": off,
            "resolved": resolved,
            "blocked": [p for p in GATE_PATHS if FM.feature_blocks(p, resolved)],
            "sitemap": [p for p, a in FM.SITEMAP_PAGES if a is None or resolved.get(a, True)],
        })
    hosts = ["api.aallyn.net", "API.AALLYN.NET", "aallyn.net", "www.aallyn.net", "dev.aallyn.net",
             "trove.aallyn.net", "docs.aallyn.net", "", "localhost", "evil.example"]
    robots = [{"host": h, "body": FM.robots_body(h)} for h in hosts]
    return {"cases": cases, "robots": robots,
            "keys": list(FM.SITE_FEATURE_FLAGS), "flags": list(FM.SITE_FEATURE_FLAGS.values())}


def http_cases() -> list[dict]:
    out = []

    def rec(name, path, headers=None, flags=None):
        FLAGS.clear()
        FLAGS.update(flags or {})
        r = client.get(path, headers=headers or {})
        out.append({
            "name": name, "path": path, "headers": headers or {}, "flags": flags or {},
            "status": r.status_code, "body": r.text,
            "content_type": r.headers.get("content-type"), "cache_control": r.headers.get("cache-control"),
        })

    for host in ("api.aallyn.net", "trove.aallyn.net", "dev.aallyn.net", "aallyn.net"):
        rec(f"robots {host}", "/robots.txt", {"host": host})
    rec("bing", "/BingSiteAuth.xml")
    rec("flags default", "/site/feature-flags")
    off = {features.MARKET_FLAG: False, features.DRESSING_ROOM_FLAG: False, features.RENAMES_FLAG: False,
           features.DUPLICATES_FLAG: False, features.CHEATER_DETECTION_FLAG: False}
    rec("flags some off", "/site/feature-flags", flags=off)
    rec("server time gated", "/site/server-time", flags={features.SERVER_TIME_FLAG: False})
    rec("clubs gated", "/site/clubs", {"accept": "text/html"}, flags={features.CLUBS_FLAG: False})
    rec("sigil 422", "/site/stats/sigil?power_rank=-1&mastery=x")
    rec("sigil missing", "/site/stats/sigil")
    rec("search bad branch", "/site/search?q=market&branch=nope")
    rec("search 422", "/site/search?limit=0&offset=-1")
    rec("search gated 404 before 422", "/site/btt/releases?limit=0", flags={features.BTT_RELEASES_FLAG: False})
    rec("releases 422", "/site/btt/releases?limit=101&offset=-1")
    rec("news 422", "/site/feeds/news?limit=0")
    rec("star chart too long", "/site/gems/parse-star-chart?code=" + "A" * 8193)
    FLAGS.clear()
    return out


# --- search ---------------------------------------------------------------------------

SEARCH_QUERIES = [
    "market", "Market", "MARKET ", "gem", "gems", "abil", "anal", "ganda", "gánda", "straße", "STRASSE",
    "server time", "time server", "a", "", "  up ", "trove.exe", "debug console", "x", "store", "class",
    "renames", "tab", "updates: files", "mod", "Mods", "ﬁsh", "Ｍarket", "évents", "calendar events",
    "loot", "zakros", "beta", "about", "cheat", "alts", "my", "stray", "guide", "rate my gem", "ui", "çlub",
]


def all_flags(**off) -> dict:
    f = {k: True for k in FM.SITE_FEATURE_FLAGS}
    f.update({"cheater_detection_enabled": True, "alt_clusters_enabled": True, "leaderboard_renames_enabled": True})
    for k, v in off.items():
        f[k] = v
    return f


def search_index_cases() -> dict:
    variants = {
        "all": all_flags(),
        "some off": all_flags(market_enabled=False, cheater_detection_enabled=False, gem_tools_enabled=False,
                              mods_hub_enabled=False, updates_enabled=False),
        "no extras": {k: True for k in FM.SITE_FEATURE_FLAGS},
        "all off": {k: False for k in FM.SITE_FEATURE_FLAGS},
    }
    cases = []
    for vname, flags in variants.items():
        for q in SEARCH_QUERIES:
            hits = SI.search_destinations(q, flags, limit=200)
            cases.append({"variant": vname, "query": q,
                          "hits": [[d.name, SI.score(d, q)] for d in hits]})
    nav = {}
    for vname, flags in variants.items():
        nav[vname] = {
            "menu": [[s["group"], [d.name for d in s["items"]]] for s in SI.nav_menu(flags)],
            "paths": SI.nav_paths(flags),
            "active": {p: SI.nav_is_active(p, flags) for p in
                       ["/", "", "/market", "/market/", "/mods/a/b", "/modpacks", "/marketplace", "/gem-tools",
                        "/codexes/crafting", "/dashboard", "/abilities", "/abilities?tab=gems", "/clubs/x"]},
        }
    norm = {s: SI.normalize(s) for s in ["Gánda", "ŞTRAßE", "ﬁsh", "Ｍarket ", " Ǆ ", "ﬀ", "İstanbul", "ΣΑΣ"]}
    return {"variants": variants, "cases": cases, "nav": nav, "normalize": norm}


def search_cases() -> dict:
    """search() with the codex / player / mod backends replaced by recorded rows."""
    calls: list = []

    def fake(subject, query, limit, offset):
        n = (len(query) * 7 + len(subject) * 3) % 9
        if subject == "collections" and query == "zz":
            raise RuntimeError("backend down")
        rows = [{"name": f"{subject}-{query}-{i}", "path": f"/x/{subject}/{i}", "detail": "", "kind": subject}
                for i in range(n)]
        page = rows[offset:offset + limit]
        return page, n + (5 if subject == "players" else 0)

    async def run_fake(subject, query, flags, branch, limit, offset):
        if subject == SI.SUBJECT_PAGES:
            return await SS._pages(query, flags, limit, offset)
        try:
            out = fake(subject, query, limit, offset)
        except RuntimeError:
            calls.append({"subject": subject, "query": query, "limit": limit, "offset": offset,
                          "rows": [], "total": 0, "raised": True})
            raise
        calls.append({"subject": subject, "query": query, "limit": limit, "offset": offset,
                      "rows": out[0], "total": out[1]})
        return out

    SS._run = run_fake
    cases = []
    flag_sets = {
        "all": all_flags(),
        "no codex": all_flags(codexes_enabled=False),
        "pages only": all_flags(codexes_enabled=False, leaderboards_enabled=False, mods_hub_enabled=False),
    }
    for fname, flags in flag_sets.items():
        for q in ["market", "gem", "x", " mo ", "zz", "server time"]:
            for subject, limit, offset in [(None, 30, 0), ("pages", 30, 0), ("pages", 2, 1), ("players", 3, 2),
                                           ("mods", 30, 0), ("items", 1, 0), ("bogus", 30, 0)]:
                calls.clear()
                out = run(SS.search(q, flags, branch="live-us", subject=subject, limit=limit, offset=offset))
                cases.append({"flags": fname, "query": q, "subject": subject, "limit": limit, "offset": offset,
                              "calls": list(calls), "out": out})
    return {"flags": flag_sets, "cases": cases}


def row_cases() -> dict:
    from app.trove.codexes import read as codexes_read
    from app.trove.leaderboards import pg_store
    from app.trove.modpacks import service as packs
    from app.trove.mods_hub import service as mods

    codex_rows = [
        {"name": "Gánda Ünicorn", "codex_type": "mount", "category": "Mounts", "blueprint": "mount_ganda_?x&y"},
        {"name": "", "codex_type": "ally", "category": None, "blueprint": None},
        {"name": "Item / x", "codex_type": "item", "category": "", "blueprint": ""},
        {"name": "Ring", "codex_type": "recipe", "category": "Crafting", "blueprint": "a b"},
    ]

    async def qe(branch, **kw):
        return codex_rows, 77

    codexes_read.query_entries = qe
    codex = {s: run(SS._codex("q", s, "live-us", 30, 0))[0]
             for s in ("collections", "items", "recipes", "styles")}
    names = ["Bob", "a b", "x/y?z", "Ünï", "100%"]

    async def sp(query, limit=25):
        return names, len(names)

    pg_store.search_players = sp
    players = run(SS._players("bo", 2, 1))
    cards = [
        {"handle": "h", "slug": "s", "title": "T", "owner_username": "O", "banner_sha": "abc", "preview_sha": "def"},
        {"handle": "h", "slug": "s2", "title": "", "owner_username": None, "banner_sha": None, "preview_sha": "p q"},
        {"handle": "", "slug": "s3", "title": "X"},
        {"handle": "h", "slug": None},
        {"handle": "stray", "slug": "z", "title": "Z", "owner_username": "", "banner_sha": "", "preview_sha": None},
    ]

    async def lp(**kw):
        return cards, 42

    mods.list_public = lp
    packs.list_public = lp
    return {"codex_rows": codex_rows, "codex": codex, "names": names, "players": players, "cards": cards,
            "mods": run(SS._mods("q", 30, 0, modpacks=False)), "modpacks": run(SS._mods("q", 30, 0, modpacks=True))}


# --- rotations + calendar -----------------------------------------------------------

NOWS = [
    dt.datetime(2026, 10, 23, 12, 34, 56, 789000, tzinfo=UTC),
    dt.datetime(2026, 10, 24, 11, 0, 0, tzinfo=UTC),
    dt.datetime(2026, 10, 25, 10, 59, 59, tzinfo=UTC),
    dt.datetime(2026, 3, 8, 3, 0, 0, tzinfo=UTC),
    dt.datetime(2025, 10, 12, 18, 30, 0, tzinfo=UTC),
]


def lux_scenarios(now: dt.datetime) -> list[dict | None]:
    reset = server_time.current_daily_reset(now)
    return [
        None,
        {"started_at": reset - 2 * 86400, "first_seen_at": reset - 2 * 86400 + 600,
         "last_seen_at": reset + 3600, "ended_at": None},
        {"started_at": reset - 20 * 86400, "first_seen_at": reset - 20 * 86400 + 60,
         "last_seen_at": reset - 15 * 86400, "ended_at": reset - 14 * 86400},
        {"started_at": reset, "first_seen_at": reset + 7200, "last_seen_at": reset + 7200, "ended_at": None},
    ]


def ts_dt(v):
    return None if v is None else dt.datetime.fromtimestamp(v, UTC)


def rotation_cases() -> list[dict]:
    out = []
    chaos_items = [
        None,
        {"name": "Chaos Thing", "identifier": "id1", "blueprint": "bp_x",
         "image_url": "https://api.aallyn.net/v1/codexes/render?blueprint=bp_x&dim=256"},
    ]
    for now in NOWS:
        server_time.real_utc_now = lambda now=now: now
        rotations.real_utc_now = lambda now=now: now
        S.time = types.SimpleNamespace(time=lambda now=now: now.timestamp(), monotonic=lambda: 0.0)
        for li, lux in enumerate(lux_scenarios(now)):
            for item in chaos_items:
                async def fake_lux(lux=lux, now=now):
                    if lux is None:
                        return luxion._away()
                    return luxion.build_response(lux["started_at"], ts_dt(lux["first_seen_at"]),
                                                 ts_dt(lux["last_seen_at"]), now, ended_at=ts_dt(lux["ended_at"]))
                chaos = {"starts_at": 1761649200, "ends_at": 1762254000, "seconds_remaining": 1234,
                         "item": item, "fetched_at": now, "active": True}

                async def fake_chaos(chaos=chaos):
                    return chaos
                S.trove_luxion.get_luxion = fake_lux
                S.trove_chaos.get_chaos_chest = fake_chaos
                r = client.get("/site/rotations")
                assert r.status_code == 200, r.text
                out.append({"now": iso(now), "now_us": int(now.timestamp()) * 1_000_000 + now.microsecond,
                            "lux": lux, "chaos": {k: chaos[k] for k in ("starts_at", "ends_at", "seconds_remaining", "item")},
                            "body": r.text, "cache_control": r.headers["cache-control"]})
    return out


def calendar_cases() -> list[dict]:
    out = []
    for now in NOWS:
        for runs in ([], [{"day_anchor": 1761649200, "first_seen": 1761650000, "ended": None},
                          {"day_anchor": 1759000000, "first_seen": None, "ended": 1759300000}]):
            body = calendar.yearly_calendar(now, luxion_runs=[
                {"day_anchor": r["day_anchor"], "first_seen": ts_dt(r["first_seen"]), "ended": ts_dt(r["ended"])}
                for r in runs])
            out.append({"now_us": int(now.timestamp()) * 1_000_000 + now.microsecond, "runs": runs,
                        "body": dumps(body)})
    return out


# --- classes, gems --------------------------------------------------------------------

def classes_case() -> dict:
    r = client.get("/site/stats/classes")
    return {"status": r.status_code, "body": r.text, "cache_control": r.headers.get("cache-control")}


def gem_cases() -> list[dict]:
    src = json.loads((ROOT / "native/kiwi-server/testdata/gems/routes.json").read_text(encoding="utf-8"))
    keep = ("lookups", "stat-range", "builds/options", "evaluate", "evaluate-simple", "builds/calculate",
            "builds/health-optimizer")
    out = []

    def rec(name, method, path, body=None, raw=None, ct="application/json"):
        headers, content = {}, None
        if raw is not None:
            content, headers = raw.encode(), {"content-type": ct}
        elif body is not None:
            content, headers = json.dumps(body).encode(), {"content-type": ct}
        r = client.request(method, path, content=content, headers=headers)
        row = {"name": name, "method": method, "path": path, "status": r.status_code, "body": r.text,
               "cache_control": r.headers.get("cache-control")}
        if content is not None:
            row["request"] = content.decode()
            row["content_type"] = ct
        out.append(row)

    for k in src:
        route = k["path"].split("?")[0].removeprefix("/v1/gems/")
        if route not in keep:
            continue
        path = k["path"].replace("/v1/gems/", "/site/gems/")
        if "request" in k:
            rec(k["name"], k["method"], path, raw=k["request"], ct=k.get("content_type", "application/json"))
        else:
            rec(k["name"], k["method"], path)
    foods = json.loads((ROOT / "app/trove/gamedata/builds/food.json").read_text(encoding="utf-8"))
    allies = json.loads((ROOT / "app/trove/gamedata/builds/ally.json").read_text(encoding="utf-8"))
    extra = [
        {"build_type": "Health", "character": "Knight", "subclass": "Bard"},
        {"build_type": "Health", "character": "Revenant", "subclass": "Gunslinger", "food": "fishing_phys_3_base"},
        {"subclass": "Gunslinger", "subclass_active": True, "character": "Gunslinger"},
        {"ally_buff": False, "ally": "spidermonkey_heckbug"},
        {"ally_buff": True, "ally": "spidermonkey_heckbug", "food": "fishing_phys_3_base"},
        {"character": "Chloromancer", "subclass": "Ice Sage", "star_chart": "SC:c|ca|ca0|ca0a|ca0a0|pz|q1"},
        {"character": "Ice Sage", "subclass": "Ice Sage", "star_chart": "SC:ca0a0|ga1b", "bounty_hunt": True},
    ]
    extra += [{"food": f} for f in list(foods)[:6]] + [{"ally": a, "ally_buff": False} for a in list(allies)[:6]]
    for i, cfg in enumerate(extra):
        rec(f"builds_extra_{i}", "POST", "/site/gems/builds/calculate", cfg)
    parser = gem_builds._engine().star_parser
    chart_rows = json.loads((ROOT / "native/kiwi-server/testdata/gems/star_chart.json").read_text(encoding="utf-8"))
    codes = [r["code"] for r in chart_rows] + ["SC:" + "z" * 50, "SC:" + parser.selectable_paths[0][:3]]
    for i, code in enumerate(codes):
        rec(f"chart_{i}", "GET", "/site/gems/parse-star-chart?" + httpx.QueryParams({"code": code}).__str__())
    rec("chart_empty", "GET", "/site/gems/parse-star-chart")
    rec("raw_bad_json", "POST", "/site/gems/evaluate", raw='{"tier": ')
    rec("raw_empty", "POST", "/site/gems/evaluate", raw="")
    rec("raw_bad_json_gated", "POST", "/site/gems/evaluate", raw='{"x"')
    return out


# --- btt, changelog, admin refresh ----------------------------------------------------

RELEASES = [
    {"release_id": 9, "tag_name": "v2.0.0-beta.1", "name": "Beta", "body": "b", "html_url": "https://g/9",
     "prerelease": True, "published_at": "2026-10-20T10:00:00.123000+00:00", "fetched_at": "2026-10-21T00:00:00+00:00",
     "assets": [{"name": "BTT-2.0.0.msi", "url": "u1", "size": 10, "content_type": "application/x-msi", "download_count": 3}]},
    {"release_id": 8, "tag_name": "v1.9.0", "name": "", "body": "", "html_url": "https://g/8", "prerelease": False,
     "published_at": "2026-10-10T10:00:00+00:00", "fetched_at": "2026-10-21T00:00:00+00:00",
     "assets": [{"name": "btt.AppImage", "url": "u2", "size": 20, "content_type": None, "download_count": 0},
                {"name": "btt_1.9.0_amd64.deb", "url": "u3", "size": 21, "content_type": "x", "download_count": 7},
                {"name": "notes.txt", "url": "u4", "size": 1, "content_type": "text/plain", "download_count": 1, "extra": 1.5}]},
    {"release_id": 7, "tag_name": "v1.8.0", "name": "Ünï", "body": "x", "html_url": "https://g/7", "prerelease": False,
     "published_at": "2026-09-01T00:00:00+00:00", "fetched_at": "2026-10-21T00:00:00+00:00",
     "assets": [{"name": "BTT-Setup-1.8.0.exe", "url": "u5", "size": 30, "content_type": "a", "download_count": 9},
                {"name": "btt-1.8.0.apk", "url": "u6", "size": 31, "content_type": "b", "download_count": 2}]},
]


def ns_release(d: dict):
    return types.SimpleNamespace(**{**d, "published_at": parse_iso(d["published_at"]),
                                    "fetched_at": parse_iso(d["fetched_at"])})


def btt_cases() -> dict:
    docs = [ns_release(d) for d in RELEASES]
    out = {"releases": RELEASES, "latest": [], "lists": [], "changelog": []}

    async def lpp(channel):
        chan = [d for d in docs if d.prerelease == btt_releases.CHANNELS[channel]]
        return {p: btt_releases.walk_latest(chan, p) for p in btt_releases.PLATFORMS}

    S.trove_btt.latest_per_platform = lpp
    for ch in ("release", "beta", "nope", ""):
        r = client.get("/site/btt/latest", params={"channel": ch})
        out["latest"].append({"channel": ch, "status": r.status_code, "body": r.text,
                              "cache_control": r.headers.get("cache-control")})

    for ch, limit, offset in ((None, 30, 0), ("beta", 30, 0), ("release", 1, 1), ("bogus", 2, 0)):
        async def lr(channel, limit, offset, ch=ch):
            chan = [d for d in docs if channel is None or d.prerelease == btt_releases.CHANNELS[channel]]
            return chan[offset:offset + limit], len(chan)
        S.trove_btt.list_releases = lr
        params = {"limit": limit, "offset": offset}
        if ch is not None:
            params["channel"] = ch
        r = client.get("/site/btt/releases", params=params)
        out["lists"].append({"channel": ch, "limit": limit, "offset": offset, "status": r.status_code, "body": r.text})

    stored = [None,
              {"repo": "AallynReed/BetterTroveTools", "rate_limited": True, "fetched_at": "2026-10-22T01:02:03.456000+00:00",
               "groups": [{"version": "Unreleased", "commits": [{"sha": "abc", "short_sha": "abc", "message": "m",
                                                                  "type": None, "url": ""}]}]}]
    for doc in stored:
        async def gc(doc=doc):
            if doc is None:
                return None
            return types.SimpleNamespace(**{**doc, "fetched_at": parse_iso(doc["fetched_at"])})
        S.trove_btt.get_changelog = gc
        r = client.get("/site/btt/changelog")
        out["changelog"].append({"doc": doc, "status": r.status_code, "body": r.text})
    return out


def site_changelog_cases() -> list[dict]:
    tags = [{"name": "v1.1", "commit": {"sha": "s2"}}, {"name": "v1.0", "commit": {"sha": "s4"}}]
    commits = [{"sha": f"s{i}", "commit": {"message": f"feat(x): change {i}\n\nbody"}, "html_url": f"https://c/{i}"}
               for i in range(1, 6)]
    scenarios = [
        ("ok", tags, commits),
        ("rate limited", tags, {"message": "API rate limit exceeded for 1.2.3.4."}),
        ("tags failed", RuntimeError("boom"), commits),
        ("both failed", RuntimeError("boom"), RuntimeError("down")),
        ("tags dict", {"message": "x"}, commits),
    ]
    out = []
    for name, t, c in scenarios:
        async def gh(path, t=t, c=c):
            v = t if path.startswith("tags") else c
            if isinstance(v, Exception):
                raise v
            return v
        site_changelog._gh_get = gh
        data = run(site_changelog._build())
        out.append({"name": name, "tags": None if isinstance(t, Exception) else t,
                    "commits": None if isinstance(c, Exception) else c,
                    "body": dumps(data), "cached": json.dumps(data)})
    return out


def status_messages() -> list[dict]:
    out = []
    for code, location in ((403, None), (404, None), (500, None), (429, None), (599, None), (302, "https://x/y")):
        def handler(request, code=code, location=location):
            headers = {"location": location} if location else {}
            return httpx.Response(code, headers=headers, json={"message": "nope"})
        c = httpx.Client(transport=httpx.MockTransport(handler))
        resp = c.get("https://api.github.com/repos/o/r/releases?per_page=100")
        try:
            resp.raise_for_status()
            msg = None
        except httpx.HTTPStatusError as e:
            msg = str(e)
        out.append({"code": code, "location": location,
                    "url": "https://api.github.com/repos/o/r/releases?per_page=100", "message": msg})
    for body in (b"", b"  x", b"{\n  \"a\": 1,\n  oops}", b"[1, 2"):
        try:
            json.loads(body)
            msg = None
        except json.JSONDecodeError as e:
            msg = str(e)
        out.append({"body": body.decode(), "message": msg})
    return out


def refresh_cases() -> list[dict]:
    out = []
    docs = [ns_release(d) for d in RELEASES]
    scenarios = [
        ("ok", 3, {"groups": 4, "rate_limited": False}, True),
        ("releases fail", RuntimeError("Client error '403 Forbidden' for url 'x'"), {"groups": 0, "rate_limited": True}, True),
        ("both fail", RuntimeError("a"), RuntimeError("b"), False),
    ]
    for name, rel, chl, downloads_ok in scenarios:
        async def rr(rel=rel):
            if isinstance(rel, Exception):
                raise rel
            return rel

        async def rc(chl=chl):
            if isinstance(chl, Exception):
                raise chl
            return chl

        async def lpp(channel):
            chan = [d for d in docs if d.prerelease == btt_releases.CHANNELS[channel]]
            return {p: btt_releases.walk_latest(chan, p) for p in btt_releases.PLATFORMS}

        class FakeFind:
            async def to_list(self, ok=downloads_ok):
                if not ok:
                    raise RuntimeError("db")
                return docs

        btt_releases.refresh_releases = rr
        btt_releases.refresh_changelog = rc
        btt_releases.latest_per_platform = lpp
        from app.trove.models import BttRelease
        BttRelease.find_all = classmethod(lambda cls: FakeFind())
        body = run(admin_router.refresh_btt_releases())
        out.append({"name": name,
                    "releases": None if isinstance(rel, Exception) else rel,
                    "releases_error": str(rel) if isinstance(rel, Exception) else None,
                    "changelog": None if isinstance(chl, Exception) else chl,
                    "changelog_error": str(chl) if isinstance(chl, Exception) else None,
                    "downloads_ok": downloads_ok, "body": dumps(body)})
    return out


# --- feeds, calendar events, clubs, sitemap, screenshots -------------------------------

NEWS = [
    {"url": "https://n/1", "title": "Patch", "author": "Team Trove", "summary": "s", "category": "News",
     "categories": ["News", "Patch"], "image": None, "published_at": "2026-10-22T11:00:00+00:00"},
    {"url": "https://n/2", "title": "Sälé", "author": "A", "summary": "", "category": "Shop",
     "categories": [], "image": "https://i/2.png", "published_at": "2026-10-21T11:00:00.250000+00:00"},
    {"url": "https://n/3", "title": "x", "author": "A", "summary": "", "category": "News",
     "categories": ["News"], "image": None, "published_at": None},
]
FEED_ITEMS = [
    {"title": "Vid", "url": "https://y/1", "channel": "C", "video_id": "1", "published_at": "2026-10-20",
     "thumbnail_url": None, "views": 12, "ratio": 0.5},
    {"title": "Ünï", "url": "https://y/2", "channel": "C2", "nested": {"a": [1, 2.25, None, True]}},
]
EVENTS = [
    {"event_id": "e1", "name": "Ongoing", "url": "https://t/e1", "category": "Event", "image": None, "icon": "i",
     "lookup": "l", "starts_at": 1761000000, "ends_at": 1762000000},
    {"event_id": "e2", "name": "Soon", "url": "", "category": "Sale", "image": "img", "icon": None,
     "lookup": None, "starts_at": 1762000000, "ends_at": 1763000000},
    {"event_id": "e3", "name": "Past", "url": "", "category": "Event", "image": None, "icon": None,
     "lookup": None, "starts_at": 1700000000, "ends_at": 1700000001},
]


def feeds_cases() -> dict:
    news_docs = [types.SimpleNamespace(**{**n, "published_at": parse_iso(n["published_at"])}) for n in NEWS]

    async def ln(limit):
        return news_docs[:limit]
    S.trove_news.latest_news = ln
    news = client.get("/site/feeds/news", params={"limit": 3})
    videos = []
    for platform, feed in (("youtube", (FEED_ITEMS, dt.datetime(2026, 10, 22, 5, 6, 7, 891000, tzinfo=UTC))),
                           ("twitch", ([], dt.datetime(2026, 10, 22, tzinfo=UTC))),
                           ("bogus", None)):
        async def gf(p, feed=feed):
            return feed if feed is not None else ([], None)
        S.trove_feeds.get_feed = gf
        r = client.get("/site/feeds/videos", params={"platform": platform})
        videos.append({"platform": platform, "items": feed[0] if feed else [],
                       "fetched_at": iso(feed[1]) if feed else None, "body": r.text})
    now = 1761500000
    events = [S._calendar_event(types.SimpleNamespace(**e), now) for e in EVENTS]
    return {"news": NEWS, "news_body": news.text, "news_cache": news.headers["cache-control"],
            "videos": videos, "events": EVENTS, "now": now, "event_rows": dumps(events)}


def clubs_cases() -> list[dict]:
    from app.bot import models as bot_models
    from app.trove.leaderboards import pg_store
    clubs = [
        {"name": "  Alpha ", "description": "a", "banner_url": None, "avatar_url": "av", "discord_url": None,
         "website_url": "w"},
        {"name": "beta", "description": None, "banner_url": "b", "avatar_url": None, "discord_url": "d",
         "website_url": None},
        {"name": "Zed", "description": None, "banner_url": None, "avatar_url": None, "discord_url": None,
         "website_url": None},
        {"name": "aardvark", "description": None, "banner_url": None, "avatar_url": None, "discord_url": None,
         "website_url": None},
        {"name": "Gamma", "description": "g", "banner_url": None, "avatar_url": None, "discord_url": None,
         "website_url": None},
    ]
    entries = [{"player_name": "GAMMA", "rank": 1}, {"player_name": "alpha", "rank": 3},
               {"player_name": " gamma ", "rank": 9}, {"player_name": "", "rank": 4}, {"player_name": None, "rank": 5}]
    out = []
    for name, ents, board_ok in (("ranked", entries, True), ("no board", [], True), ("pg down", entries, False)):
        class Q:
            async def to_list(self):
                return [types.SimpleNamespace(**c) for c in clubs]

        class FakeClub:
            public = True

            @classmethod
            def find(cls, *a, **k):
                return Q()
        bot_models.Club = FakeClub

        async def la(uuid, ok=board_ok, ents=ents):
            if not ok:
                raise RuntimeError("pg")
            return 1761000000 if ents else None

        async def le(uuid, anchor, limit, offset, ents=ents):
            return ents, len(ents)
        pg_store.latest_anchor_for_board = la
        pg_store.list_entries = le
        res = run(clubs_page.public_clubs_ordered())
        out.append({"name": name, "clubs": clubs,
                    "entries": [[e["player_name"] or "", e["rank"]] for e in ents] if board_ok else [],
                    "body": dumps({"items": res, "count": len(res)})})
    return out


def sitemap_cases() -> list[dict]:
    from app.trove.modpacks import service as packs
    from app.trove.mods_hub import service as mods
    mod_cards = [{"handle": "h", "slug": f"m{i}", "updated_at": f"2026-10-{i + 1:02d}T00:00:00+00:00"} for i in range(3)]
    mod_cards += [{"handle": "a&b", "slug": "<x>", "updated_at": None}, {"handle": "", "slug": "nope"},
                  {"handle": "stray", "slug": "s", "updated_at": ""}]
    modders = [{"handle": "h", "updated_at": "2026-10-03T00:00:00+00:00"}, {"handle": "q", "updated_at": None}]
    pack_cards = [{"handle": "h", "slug": "p", "updated_at": "2026-10-04T00:00:00.500000+00:00"}]

    async def lp_mods(limit, offset):
        return mod_cards[offset:offset + limit], len(mod_cards)

    async def lp_packs(limit, offset):
        return pack_cards[offset:offset + limit], len(pack_cards)

    async def pmh():
        return modders

    mods.list_public = lp_mods
    mods.public_modder_handles = pmh
    packs.list_public = lp_packs
    out = []
    for name, flags in (("all on", {}), ("mods off", {features.MODS_HUB_FLAG: False}),
                        ("some off", {features.MARKET_FLAG: False, features.DRESSING_ROOM_FLAG: False,
                                      features.STAR_CHART_FLAG: False})):
        FLAGS.clear()
        FLAGS.update(flags)
        out.append({"name": name, "flags": flags, "body": run(S._render_sitemap())})
    FLAGS.clear()
    return {"mods": mod_cards, "modders": modders, "packs": pack_cards, "base": settings.app_url, "cases": out}


def screenshots_case() -> dict:
    names = ["b.png", "A.webp", "c.JPG", "d.jpeg", "e.gif", "f.txt", ".webp", "g.tar.gif", "README", "h.Png", "z.bmp"]
    with tempfile.TemporaryDirectory() as tmp:
        folder = Path(tmp) / "static" / "trove-screens"
        folder.mkdir(parents=True)
        for n in names:
            (folder / n).write_bytes(b"x")
        (folder / "sub.png").mkdir()
        old = settings.site_root
        settings.site_root = tmp
        r = client.get("/site/screenshots.json")
        settings.site_root = str(Path(tmp) / "missing")
        missing = client.get("/site/screenshots.json")
        settings.site_root = old
    return {"files": names, "dirs": ["sub.png"], "body": r.text, "cache_control": r.headers.get("cache-control"),
            "missing_root_body": missing.text}


BIG = 16_384


def shrink(v):
    """Large bodies are kept as their sha256 plus a head for debugging."""
    if isinstance(v, dict):
        out = {}
        for k, x in v.items():
            if k in ("body", "out_text") and isinstance(x, str) and len(x.encode()) > BIG:
                out["body_sha256"] = hashlib.sha256(x.encode()).hexdigest()
                out["body_len"] = len(x.encode())
                out["body_head"] = x[:1500]
            else:
                out[k] = shrink(x)
        return out
    if isinstance(v, list):
        return [shrink(x) for x in v]
    return v


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    files = {
        "flags.json": flag_cases(),
        "http.json": http_cases(),
        "search_index.json": search_index_cases(),
        "search_rows.json": row_cases(),
        "search.json": search_cases(),
        "rotations.json": rotation_cases(),
        "calendar.json": calendar_cases(),
        "classes.json": classes_case(),
        "gems.json": gem_cases(),
        "btt.json": btt_cases(),
        "site_changelog.json": site_changelog_cases(),
        "httpx_errors.json": status_messages(),
        "refresh.json": refresh_cases(),
        "feeds.json": feeds_cases(),
        "clubs.json": clubs_cases(),
        "sitemap.json": sitemap_cases(),
        "screenshots.json": screenshots_case(),
    }
    for name, data in files.items():
        text = json.dumps(shrink(data), indent=0, ensure_ascii=False, default=str)
        (OUT / name).write_text(text + "\n", encoding="utf-8")
    print(f"{len(files)} fixture files -> {OUT}")


if __name__ == "__main__":
    main()
