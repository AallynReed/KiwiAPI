"""Golden fixtures for the website's reference and catalog pages, from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_web_catalog_fixtures.py
Writes native/kiwi-server/testdata/web_catalog/: golden.json and upstream.json.

/commands, /classes, /abilities, /allies, /market, /store, /codexes, /mods,
/mods/{handle}, /mods/{handle}/{slug}, /modpacks and /modpacks/{handle}/{slug}
driven through the real website app (raw ASGI) with internal_get answered from
upstream.json per scenario. Same site fingerprint as gen_web_fixtures.py, plus
the game data files the reference pages read.
"""
import asyncio
import base64
import hashlib
import json
import logging
import os
import shutil
import subprocess
import sys
import tempfile
import warnings
import zlib
from pathlib import Path

if os.environ.get("TZ") != "UTC0":
    # Production containers run in UTC; naive ISO stamps read as local time.
    sys.exit(subprocess.call([sys.executable, *sys.argv], env={**os.environ, "TZ": "UTC0"}))

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
sys.path.insert(0, os.getcwd())

ROOT = Path(os.getcwd())
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "web_catalog"
OUT.mkdir(parents=True, exist_ok=True)
GEO = Path(tempfile.mkdtemp(prefix="kiwi-web-catalog-geo-"))
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
    "PAGEVIEW_TRACKING_ENABLED": "false",
}
os.environ.update(ENV)

import app.core.internal_api as internal_api  # noqa: E402
import app.web.feature_flags as web_flags  # noqa: E402
import app.web.meta as web_meta  # noqa: E402
import app.web.pages as web_pages  # noqa: E402
import app.geo_maintenance.gate as geo_gate  # noqa: E402
import app.geo_maintenance.geoip as geoip  # noqa: E402
import app.embed.service as embed_service  # noqa: E402
import app.admin.runtime_config as runtime_config  # noqa: E402
from app.site import ssr  # noqa: E402
from app.web.main import app  # noqa: E402

FLAG_KEYS = list(web_flags._ALL_ATTRS)
OFF = {"mods_hub_enabled", "market_enabled", "store_enabled", "codexes_enabled", "commands_enabled",
       "classes_enabled", "abilities_enabled", "allies_enabled"}

LONG = ("A **bold** [link](https://x.example) and ![img](https://i.example/p.png) <b>tag</b>\n\n"
        "# Heading `code` ~strike~ | pipe > quote_under " + "word " * 60)

MOD_CARDS = [
    {"handle": "alice", "slug": "zeta-mod", "title": "Zakros UI - Chat", "summary": LONG, "author": "Alice A",
     "owner_username": "alice", "owner_avatar_url": "https://cdn.example/a.png",
     "tags": ["ui", 5, "chat", None, "qol", "extra"], "download_count": 1234567, "star_count": 12.5},
    {"handle": "bob", "slug": "alpha", "title": "\"Alpha\" & <Friends>", "summary": None,
     "owner_username": "bob", "download_count": None, "star_count": 0},
    {"handle": "carol", "slug": "ümlaut", "title": "", "summary": "short\u3000text\x1cend",
     "download_count": 1e16, "star_count": -3.25, "tags": "notalist"},
    {"handle": "", "slug": "nohandle", "title": "No handle"},
    {"handle": "dave", "slug": None, "title": "No slug"},
    {"handle": "erin", "slug": "floaty", "title": "Floaty", "download_count": 0.5, "star_count": True,
     "author": "", "owner_username": "erin_o"},
    "not a dict",
]

PACK_CARDS = [
    {"handle": "erin", "slug": "pack-1", "title": "Pack <One>", "summary": "Bundle " * 40,
     "owner_username": "erin", "download_count": 9001},
    {"handle": "frank", "slug": "pack-2", "title": None, "summary": "", "download_count": 2.75},
    {"handle": "", "slug": "pack-3"},
]

PROJECT = {
    "handle": "alice", "slug": "zeta-mod", "title": "Zakros UI - Chat", "summary": "  Better chat. ",
    "description": LONG * 6, "owner_username": "alice", "author": "Alice A",
    "owner_avatar_url": "https://cdn.example/a b.png", "banner_sha": "abc/def 1",
    "preview_shas": ["p1", "p2"], "source_visible": True, "is_stray": False,
    "tags": ["ui", "chat", 3, "a", "b", "c", "d", "e", "f", "g"], "download_count": 4321, "star_count": 7,
    "created_at": "2026-01-02T03:04:05Z", "updated_at": "2026-10-01T10:00:00.123456+02:00",
    "last_release_at": None,
    "releases": [
        {"id": "draft1", "status": "draft", "title": "Draft"},
        {"id": 42, "tag": "v1.2", "title": "", "published_at": 1760000000, "changelog": "Fixed <things>.",
         "tmod_filename": "Zakros UI - Chat.tmod", "tmod_size": 3_456_789},
        {"id": "r2", "tag": "v1.1", "published_at": "2026-05-05", "changelog": None, "status": "published"},
        "junk",
        {"tag": "v1.0", "published_at": "not a date"},
    ],
}

PROFILE_ALICE = {
    "username": "alice", "display_name": "Alice <A>", "tagline": "", "readme": LONG,
    "avatar_url": "https://cdn.example/av.png", "banner_url": None, "website_url": "https://alice.example",
    "joined_at": "2025-12-01T00:00:00Z",
    "mods": [
        {"handle": "alice", "slug": "zeta-mod", "title": "Zakros UI - Chat", "download_count": 99},
        {"handle": "alice", "slug": "other", "title": "Other Thing", "download_count": 500},
        {"handle": "alice", "slug": "nameplate", "title": "Zakros UI - Nameplate", "download_count": 10},
        {"handle": "alice", "slug": "frames", "title": "Zakros UI - Frames", "download_count": 20.5},
        {"handle": "alice", "slug": "notitle", "title": None, "download_count": None},
        {"handle": "alice", "slug": "big", "title": "Big", "download_count": 100000},
        {"handle": "alice", "slug": "m7", "title": "M7"}, {"handle": "alice", "slug": "m8", "title": "M8"},
        {"handle": "alice", "slug": "m9", "title": "M9", "download_count": 1},
        {"handle": "alice", "slug": "m10", "title": "M10", "download_count": 2},
        {"handle": None, "slug": "nohandle", "title": "x"},
    ],
}

PACK = {
    "handle": "erin", "slug": "pack-1", "title": "Pack <One>", "summary": "", "description": LONG,
    "owner_username": "erin", "download_count": 12, "updated_at": "2026-09-09T09:09:09+00:00",
    "banner_sha": None, "preview_shas": ["prev0"],
    "variants": [
        {"label": "Full", "mods": [
            {"handle": "alice", "slug": "zeta-mod", "title": "Zakros UI - Chat", "author": "Alice"},
            {"handle": "bob", "slug": "alpha", "title": None},
            {"title": "Loose file", "author": "?"},
            {"title": "Loose file 2"},
            "junk",
        ]},
        {"name": "Lite", "mods": [
            {"handle": "alice", "slug": "zeta-mod", "title": "dupe"},
            {"handle": "carol", "slug": "c", "title": "", "author": None},
        ]},
        {"mods": "nope"},
    ],
}

ON = {
    "/site/feature-flags": {k: True for k in FLAG_KEYS},
    "/site/embed/allowed-origins": {"origins": ["https://partner.example"]},
    "/internal/geo-maintenance": {"countries": []},
    "/site/mods/projects?sort=recent&limit=24": {"items": MOD_CARDS, "total": 77},
    "/site/modpacks/projects?limit=24": {"items": PACK_CARDS, "total": "3"},
    "/site/codexes/types": {"items": [{"type": "ally", "count": 1200}, {"type": "mount", "count": 2.5},
                                      {"type": "", "count": 4}, {"type": "fish"}]},
    "/site/codexes/search?sort=name&limit=24": {"items": [
        {"name": "Ancient Hero", "type": "ally", "category": "Pet"},
        {"name": "Bat <Wing>", "type": "mount", "category": None},
        {"name": "", "type": "x"}, {"name": "Cod"}], "total": 9876},
    "/site/market/items": {"items": ["Flux", "Gem Box", 7, None, "Ünique"]},
    "/site/market/listings?limit=24&sort=-last_seen": {"items": [
        {"name": "Flux", "price_each": 1234.5, "stack": 999, "price": 1233265.5, "last_seen": 1760000000},
        {"name": "Gem Box", "price_each": 0.125, "stack": True, "price": "12", "last_seen": "2026-10-01T10:00:00Z"},
        {"name": "Naive", "price_each": -7, "stack": None, "price": 1e16, "last_seen": "2026-10-01T10:00:00"},
        {"name": "Date only", "price_each": 2.999, "stack": 1.0, "price": 0, "last_seen": "2026-10-01"},
        {"name": "Bad date", "price_each": 1000000.004, "last_seen": "yesterday"},
        {"name": "Float ts", "price_each": 12.345, "last_seen": 1760000000.9},
        {"name": "", "price_each": 1},
    ]},
    "/site/store/categories": {"items": [{"label": "$StoreCategory_Mounts"}, {"name": "StoreCategory_Allies"},
                                         {"label": "$Plain"}, {"label": "", "name": ""}, {"name": "Raw"}]},
    "/site/store/products?limit=24&active=true": {"items": [
        {"name": "Starter Pack", "price_string": "$9.99", "kind": "pack"},
        {"code": "CODE_ONLY", "price_string": None}, {"name": "", "code": ""}], "total": 3},
    "/site/mods/projects/alice/zeta-mod": PROJECT,
    "/site/mods/profile/alice": PROFILE_ALICE,
    "/site/mods/projects/bob/plain": {"slug": "plain", "title": None, "summary": "",
                                      "description": "<p>Only <i>html</i> here</p>", "owner_username": "",
                                      "preview_shas": ["prev-sha"], "download_count": True,
                                      "updated_at": "2026-02-03T04:05:06Z", "releases": None},
    "/site/mods/profile/bob": {"username": "bob", "display_name": None, "tagline": "  Bob makes mods  ",
                               "readme": None, "banner_url": "https://cdn.example/bb.png", "mods": "x"},
    "/site/mods/projects/carol/%C3%BCmlaut": {"slug": "ümlaut", "title": "Ümlaut ünïcode", "summary": "",
                                              "description": "", "owner_username": "carol", "is_stray": True,
                                              "owner_avatar_url": "http://insecure.example/a.png",
                                              "download_count": 3.0, "last_release_at": "2026-03-03"},
    "/site/mods/projects/dan/empty": {},
    "/site/mods/profile/dan": {},
    "/site/mods/projects/eve/x%20y%22%3C": {"title": "Eve's <mod>", "owner_username": "eve",
                                           "summary": "", "description": "", "preview_shas": "sha-string",
                                           "download_count": 5, "releases": [{"id": 0, "tag": "t0"}]},
    "/site/modpacks/projects/erin/pack-1": PACK,
    "/site/modpacks/projects/frank/bare": {"title": "", "slug": "bare", "owner_username": None,
                                          "banner_sha": "bsha", "summary": "  Frank's pack  "},
}

SCENARIOS = {
    "on": ON,
    "off": {
        "/site/feature-flags": {**{k: k not in OFF for k in FLAG_KEYS}},
        "/internal/geo-maintenance": {"countries": []},
    },
    "down": {},
}
(OUT / "upstream.json").write_text(json.dumps(SCENARIOS, indent=1, ensure_ascii=False), encoding="utf-8")

CURRENT = {"name": "on"}


async def fake_internal_get(path, params=None, *, timeout=8.0, headers=None):
    key = path
    if params:
        key += "?" + "&".join(f"{k}={v}" for k, v in params.items())
    if CURRENT["name"] == "down":
        return None
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


geoip._COUNTRY._load(geoip._COUNTRY.path())
geoip._ASN._load(geoip._ASN.path())


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

PAGES = ["/commands", "/classes", "/abilities", "/allies", "/market", "/store", "/codexes", "/mods", "/modpacks",
         "/mods/alice", "/mods/alice/zeta-mod", "/modpacks/erin/pack-1"]


def requests_for(scenario):
    reqs = []

    def add(name, path, method="GET", headers=None):
        h = [("host", "trove.aallyn.net"), ("x-request-id", f"cat-{scenario}-{len(reqs)}")]
        h += list(headers or [])
        reqs.append({"name": name, "method": method, "path": path, "headers": h, "body_b64": ""})

    for p in PAGES:
        add("page " + p, p, headers=[("accept", HTML)])
    add("mods json accept", "/mods/alice/zeta-mod", headers=[("accept", "application/json")])
    add("profile missing html", "/mods/nobody", headers=[("accept", HTML)])
    add("profile missing json", "/mods/nobody")
    add("head mods", "/mods", method="HEAD", headers=[("accept", HTML)])
    if scenario != "on":
        add("project missing", "/mods/x/missing", headers=[("accept", HTML)])
        return reqs
    add("abilities rings", "/abilities?tab=rings", headers=[("cookie", "btt_theme=dark")])
    add("abilities classes", "/abilities?tab=classes")
    add("abilities star", "/abilities?tab=star-chart")
    add("abilities allies", "/abilities?tab=allies&x=1")
    add("abilities bogus", "/abilities?tab=bogus")
    add("abilities blank", "/abilities?tab=")
    add("abilities repeat", "/abilities?tab=allies&tab=rings")
    add("abilities encoded", "/abilities?tab=star%2Dchart")
    add("commands classic", "/commands", headers=[("cookie", "btt_design=classic; btt_theme=light")])
    add("classes again", "/classes", headers=[("accept", HTML)])
    add("mods again", "/mods", headers=[("accept", HTML)])
    add("profile bob", "/mods/bob", headers=[("accept", HTML)])
    add("profile dan empty", "/mods/dan", headers=[("accept", HTML)])
    add("project bob", "/mods/bob/plain", headers=[("accept", HTML)])
    add("project carol", "/mods/carol/%C3%BCmlaut", headers=[("accept", HTML)])
    add("project dan empty", "/mods/dan/empty", headers=[("accept", HTML)])
    add("project eve quoted", "/mods/eve/x%20y%22%3C", headers=[("accept", HTML)])
    add("project missing", "/mods/x/missing", headers=[("accept", HTML)])
    add("project why child", "/mods/why/thing", headers=[("accept", HTML)])
    add("profile why", "/mods/why", headers=[("accept", HTML)])
    add("pack bare", "/modpacks/frank/bare", headers=[("accept", HTML)])
    add("pack missing", "/modpacks/x/missing", headers=[("accept", HTML)])
    add("head project", "/mods/alice/zeta-mod", method="HEAD")
    add("post mods", "/mods", method="POST")
    return reqs


async def run():
    golden = []
    for scenario in SCENARIOS:
        CURRENT["name"] = scenario
        reset()
        for r in requests_for(scenario):
            res = await call(r["method"], r["path"], r["headers"])
            golden.append({
                "scenario": scenario, **r,
                "status": res["status"], "resp_headers": res["headers"],
                "resp_body_z": base64.b64encode(zlib.compress(res["body"], 9)).decode(),
            })
    return golden


GAMEDATA = ("classes.json", "class_abilities.json", "gem_abilities.json", "ring_abilities.json", "star_chart.json",
            "ally_abilities.json")


def fingerprint():
    h = hashlib.sha256()
    for sub in ("templates", "static"):
        base = ROOT / "site" / sub
        for p in sorted(base.rglob("*")):
            if p.is_file():
                h.update(p.relative_to(ROOT / "site").as_posix().encode() + b"\0")
                h.update(p.read_bytes())
    for name in GAMEDATA:
        h.update(name.encode() + b"\0")
        h.update((ROOT / "app" / "trove" / "gamedata" / name).read_bytes())
    return h.hexdigest()


async def main():
    golden = await run()
    data = {"fingerprint": fingerprint(), "gamedata": list(GAMEDATA), "env": ENV, "requests": golden}
    for k in ("SITE_ROOT", "GEOIP_DIR", "GAMEDATA_DIR"):
        data["env"] = {**data["env"], k: None}
    (OUT / "golden.json").write_text(json.dumps(data, indent=0, ensure_ascii=False), encoding="utf-8")
    sizes = sorted(((len(zlib.decompress(base64.b64decode(r["resp_body_z"]))), r["name"], r["scenario"], r["status"])
                    for r in golden), reverse=True)
    for s in sizes[:8]:
        print(s)
    print(f"{len(golden)} requests")
    cases = view_cases()
    (OUT / "views.json").write_text(json.dumps(cases, indent=1, ensure_ascii=False), encoding="utf-8")
    for c in cases["sets"]:
        print(c["name"], {k: ("ok" if "ok" in v else v["raises"]) for k, v in c["results"].items()})


# ---------------------------------------------------------------- view models over synthetic game data

RICH = {
    "classes.json": [
        {"name": "Knight", "qualified_name": "knight", "shorts": ["KN", "K"], "damage_type": "Physical",
         "weapons": ["Sword"],
         "stats": [{"name": "Physical Damage", "value": 100, "percentage": False},
                   {"name": "Critical Hit", "value": 12.5, "percentage": True},
                   {"name": "Attack Speed", "value": None}, {"name": "Mystery", "value": "x"},
                   {"name": "Flag", "value": True}, {"name": "Big", "value": 1234567.891},
                   {"name": "Regen", "value": 0.6, "percentage": True}],
         "bonuses": [{"name": "Physical Damage", "value": 0, "percentage": True}, {"name": "Health", "value": 5},
                     {"name": "Speed", "value": -2.5}, {"name": "Fly", "value": 7, "absolute": True},
                     {"name": "", "value": 3}],
         "levels": {"10": [{"name": "Physical Damage", "value": 10}],
                    "2": [{"name": "Regen", "value": 0.1, "percentage": True}, {"name": "Regen", "value": 0.2},
                          {"name": "Regen", "value": 0.3}],
                    "3": [{"name": "Critical Hit", "value": 0.25}, {"name": "Physical Damage", "value": 1.5}],
                    "30": [{"name": "Big", "value": 0.001}, {"name": "Critical Hit", "value": None}]},
         "subclass": {"name": "Iron Will", "description": "Tough.",
                      "level": {"20": [{"name": "Armor", "value": 5}],
                                "1": [{"name": "", "value": 0}, {"name": "  ", "value": 0}],
                                "5": [{"name": " Padded ", "value": 0},
                                      {"name": "", "value": 2.5, "percentage": True}]}},
         "abilities": [{"name": "Slash", "description": "Cuts.", "type": "Active",
                        "stages": [{"name": "Hit", "multiplier": 3.5, "base": 50},
                                   {"name": "Zero", "multiplier": 0, "base": 0}, {"name": "Flat", "base": 12.25},
                                   {"name": "Int", "multiplier": 2}, {"name": "Tiny", "multiplier": 0.000001}]},
                       {"name": "Old", "description": "Gone.", "active": False, "stages": []},
                       {"name": "", "description": ""},
                       {"name": "", "description": "Desc only", "type": "Passive", "active": 0}]},
        {"name": "Dupe", "qualified_name": "dupe", "damage_type": None},
        {"name": "Ice Sage", "qualified_name": "icemage", "damage_type": "Magic", "abilities": []},
        {"name": "Dupe Two", "qualified_name": "dupe", "damage_type": "magic", "levels": {}},
    ],
    "class_abilities.json": [
        {"name": "Ice Sage", "abilities": [
            {"name": "Frost", "description": "Cold.", "type": "Active",
             "stages": [{"name": "Shard", "multiplier": 1.25, "base": 0, "prefab": "abilities/icemage/shard"}],
             "stats": [{"name": "Movement Speed", "op": "Add", "value": -20, "prefab": "abilities/icemage/slow"},
                       {"name": "Attack Speed", "op": "MultiplySum", "value": 15,
                        "prefab": "abilities/icemage/haste_big"}]},
            {"name": "Legacy Bolt", "active": False, "description": "Old bolt."}]},
        {"name": "Knight", "abilities": [{"name": "Slash", "type": "Active", "description": "Cuts."}]},
        {"name": "", "abilities": [{"name": "Orphan", "description": "No owner"}]},
    ],
    "gem_abilities.json": [
        {"element": "Air", "slug": "spirittank", "name": "Aegis Assault",
         "description": "Empowered Air Gem for the Revenant. Bash loses its cooldown.", "tiers": [110, 111],
         "stats": [{"name": "Health Regen", "op": "Multiply", "value": -100.0, "prefab": "abilities/a/leech"}],
         "stages": [{"name": "Smash", "multiplier": 2.5, "base": 10, "prefab": "abilities/a/smash"}]},
        {"element": "Fire", "slug": "spirittank", "name": "Aegis Assault",
         "description": "Empowered Fire Gem for the Revenant. Bash loses its cooldown.", "tiers": [112, 110],
         "stats": [{"name": "Health Regen", "op": "Multiply", "value": -100.0, "prefab": "abilities/a/leech"},
                   {"name": "Damage", "op": "MultiplySum", "value": 20, "prefab": "abilities/a/leech"}],
         "stages": []},
        {"element": "Prismatic", "slug": "mastery", "name": "Masterful",
         "description": "Empowered Prismatic Gem; can be socketed anywhere."},
        {"element": "Cosmic", "slug": "any", "name": "Star Power",
         "description": "Empowered Cosmic Gem for any class; shoots stars.", "tiers": [5]},
        {"element": "Water", "slug": "x", "name": "plain", "description": "No lead here.", "tiers": []},
        {"element": "Prismatic", "slug": "y", "name": "Berserk", "description": "Empowered Prismatic Gem. Rage.",
         "stats": [{"name": "Attack Speed", "op": "Add", "value": 25, "prefab": "abilities/gems/battle_frenzy_small"},
                   {"name": "Attack Speed", "op": "Add", "value": 50, "prefab": "abilities/gems/battle_frenzy_large"},
                   {"name": "Crit", "op": "Multiply", "value": 5, "amount": 1.5,
                    "prefab": "abilities/gems/battle_frenzy_large"},
                   {"name": "Shield", "op": "Other", "value": 0, "prefab": ""}]},
        {"element": "Air", "slug": "z", "name": "", "description": "nameless", "tiers": [1]},
        {"element": "Unknown", "slug": "u", "name": "Weird Element",
         "description": "EMPOWERED  air  gem FOR THE   all classes .  Lower case lead.", "tiers": [3, 1, 2, 1]},
        {"element": "Water", "slug": "v", "name": "Weird Element", "description": "", "tiers": [9],
         "stats": [{"name": "X", "op": "Add", "value": 1}, {"name": "X", "op": "Add", "value": 1}]},
    ],
    "ring_abilities.json": [
        {"class": "Bard", "name": "Crystal-Teleport", "description": "Teleport.", "stats": [],
         "stages": [{"name": "Pop", "multiplier": 0.5, "base": 0, "prefab": "abilities/bard/pop"}]},
        {"class": "", "name": "Classless", "description": "None"},
        {"class": "Archer", "name": "b ring", "description": "x",
         "stats": [{"name": "Speed", "op": "Add", "value": 1e-5}]},
        {"class": "Archer", "name": "A ring", "description": "y"},
    ],
    "star_chart.json": {
        "Combat": {"Path": "combat", "Name": "Combat", "Constellation": "Combat", "Type": "Root", "Abilities": [],
                   "Stars": [{"Path": "combat.a", "Constellation": "Combat", "Type": "Major", "Name": "Lucky",
                              "Abilities": ["5% chance ", " to crit"],
                              "Ability_Values": [{"name": "Crit", "op": "MultiplySum", "value": 5}],
                              "Stars": [{"Path": "combat.a.0", "Constellation": "Combat", "Type": "", "Name": "",
                                         "Abilities": ["Deep node"]}]}]},
        "Gathering": {"Path": "gathering", "Name": "Gathering", "Constellation": "", "Abilities": ["Root ability"],
                      "Stars": None},
    },
    "ally_abilities.json": [
        {"slug": "a1", "name": "Zeta Pet",
         "stats": [{"name": "Light", "op": "Add", "value": 150.0, "amount": 150.0},
                   {"name": "Energy Regen Bonus", "op": "MultiplySum", "value": 15.0, "amount": 0.15},
                   {"name": "$Hidden", "op": "Add", "value": 1}, {"name": "Light", "op": "Add", "value": -200}],
         "abilities": [{"text": "Triggers Chaos. "}, {"text": ""}, {"ref": "x"}]},
        {"slug": "a2", "name": "alpha pet",
         "stats": [{"name": "Maximum Health %", "op": "Multiply", "value": 10, "amount": 1.1},
                   {"name": "Speed", "op": "Multiply", "value": 7, "amount": 0}], "abilities": []},
        {"slug": "no_stats", "name": "Nothing", "stats": [], "abilities": [{"text": "Only ability"}]},
        {"slug": "slug_only", "stats": [{"name": "Physical Damage", "op": "Add", "value": 400}]},
        {"slug": "blank", "name": "Blank Stat", "stats": [{"name": "", "value": 5}, {"value": 3}]},
    ],
}

COMMANDS = {
    "intro": {"title": {"en": "T", "fr": "F"}, "subtitle": {"fr": "only fr"}, "search_placeholder": None,
              "source_note": {"en": "S"}, "placeholder_warning": {"en": "W <b>"},
              "rules": {"fr": ["r"], "en": ["one", "two"]}},
    "categories": [{"key": "chat", "name": {"en": "Chat"}}, {"key": "empty", "name": {"en": "Empty"}},
                   {"key": "misc"}],
    "commands": [{"syntax": "/1", "category": "chat", "description": {"en": "Global."}},
                 {"syntax": "/w <player>", "category": "chat", "aliases": ["/whisper", "/tell"],
                  "description": {"en": "Whisper."}, "note": {"en": "Note Ü"}},
                 {"syntax": "/x <>", "category": "misc", "description": {}, "note": {"fr": "only fr"}}],
}


def with_file(name, data):
    return {**RICH, name: data}


SETS = [
    {"name": "rich", "files": RICH, "views": ["classes", "abilities", "allies"]},
    {"name": "empty", "files": {}, "views": ["classes", "abilities", "allies"]},
    {"name": "bad level key", "views": ["classes"], "files": with_file("classes.json", [
        {"name": "K", "qualified_name": "k", "levels": {"x": [{"name": "A", "value": 1}]}}])},
    {"name": "stat not a dict", "views": ["classes"], "files": with_file("classes.json", [
        {"name": "K", "qualified_name": "k", "stats": ["oops"]}])},
    {"name": "no qualified name", "views": ["classes"], "files": with_file("classes.json", [{"name": "K"}])},
    {"name": "ally stat name not str", "views": ["abilities", "allies"], "files": with_file("ally_abilities.json", [
        {"name": "A", "stats": [{"name": 5, "value": 1}], "abilities": [{"text": "t"}]}])},
    {"name": "gem description not str", "views": ["abilities"], "files": with_file("gem_abilities.json", [
        {"element": "Air", "slug": "s", "name": "G", "description": 5}])},
    {"name": "stage text value", "views": ["abilities"], "files": with_file("ring_abilities.json", [
        {"class": "Bard", "name": "R", "description": "d", "stages": [{"name": "s", "multiplier": "3"}]}])},
]


def view_cases():
    from app.site import abilities_page, allies_page, classes_page, commands_page
    from app.trove.decode import store

    abilities_page.gem_lookups = lambda: {"empowered_gems": store.load("gem_abilities.json", [])}
    out = []
    for case in SETS:
        syn = Path(tempfile.mkdtemp(prefix="kiwi-web-catalog-syn-"))
        for name, data in case["files"].items():
            (syn / name).write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
        store.runtime_dir = lambda syn=syn: syn
        store.BASELINE_DIR = syn
        views = {"classes": classes_page.classes_view,
                 "abilities": lambda: abilities_page.abilities_view("rings")["tabs"],
                 "allies": allies_page.allies_view}
        results = {}
        for view in case["views"]:
            try:
                results[view] = {"ok": json.loads(json.dumps(views[view]()))}
            except Exception as e:  # noqa: BLE001
                results[view] = {"raises": type(e).__name__}
        out.append({"name": case["name"], "files": case["files"], "results": results})
    commands_page.load_static_json = lambda rel: json.loads(json.dumps(COMMANDS))
    return {"sets": out, "commands": {"input": COMMANDS, "ok": commands_page.commands_view()}}


asyncio.run(main())
