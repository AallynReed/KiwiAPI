"""Golden fixtures for the website container (app/web), from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_web_fixtures.py
Writes native/kiwi-server/testdata/web/: golden.json, upstream.json, the two
test .mmdb files and templates.json.

Drives the real website app (raw ASGI, no lifespan) with internal_get answered
from upstream.json per scenario, Python's mimetypes reduced to its built-in
table (what a Linux image without /etc/mime.types has), and gzip bodies given
Linux's header OS byte. The site tree's fingerprint is recorded: the Rust test
skips when templates or static files have changed since.
"""
import asyncio
import base64
import datetime as dt
import hashlib
import json
import logging
import mimetypes
import os
import struct
import sys
import tempfile
import warnings
import zlib
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
sys.path.insert(0, os.getcwd())

ROOT = Path(os.getcwd())
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "web"
OUT.mkdir(parents=True, exist_ok=True)
GEO = Path(tempfile.mkdtemp(prefix="kiwi-web-geo-"))

mimetypes.knownfiles = []
mimetypes._winreg = None
mimetypes._mimetypes_read_windows_registry = None
mimetypes.init()

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

# ---------------------------------------------------------------- mmdb writer


class U32(int):
    pass


class U64(int):
    pass


def _ctrl(t, size):
    if size < 29:
        sz, extra = size, b""
    elif size < 285:
        sz, extra = 29, bytes([size - 29])
    elif size < 65821:
        sz, extra = 30, (size - 285).to_bytes(2, "big")
    else:
        sz, extra = 31, (size - 65821).to_bytes(3, "big")
    if t <= 7:
        return bytes([(t << 5) | sz]) + extra
    return bytes([sz, t - 7]) + extra


def _enc(v):
    if isinstance(v, bool):
        return _ctrl(14, int(v))
    if isinstance(v, str):
        b = v.encode()
        return _ctrl(2, len(b)) + b
    if isinstance(v, int):
        b = v.to_bytes((v.bit_length() + 7) // 8, "big") if v else b""
        if isinstance(v, U64):
            return _ctrl(9, len(b)) + b
        if isinstance(v, U32) or v >= 1 << 16:
            return _ctrl(6, len(b)) + b
        return _ctrl(5, len(b)) + b
    if isinstance(v, list):
        return _ctrl(11, len(v)) + b"".join(_enc(x) for x in v)
    if isinstance(v, dict):
        return _ctrl(7, len(v)) + b"".join(_enc(k) + _enc(x) for k, x in v.items())
    raise TypeError(v)


def write_mmdb(path, db_type, nets):
    """An IPv4 MMDB (record size 24) mapping each /24 in ``nets`` to its record."""
    data = b""
    offsets = []
    for _, rec in nets:
        offsets.append(len(data))
        data += _enc(rec)
    nodes = [[None, None]]
    for (prefix, _), off in zip(nets, offsets):
        bits = int.from_bytes(bytes(int(x) for x in prefix.split(".")), "big") >> 8
        n = 0
        for i in range(24):
            bit = (bits >> (23 - i)) & 1
            if i == 23:
                nodes[n][bit] = ("data", off)
            else:
                if nodes[n][bit] is None:
                    nodes.append([None, None])
                    nodes[n][bit] = ("node", len(nodes) - 1)
                n = nodes[n][bit][1]
    count = len(nodes)
    tree = b""
    for left, right in nodes:
        vals = []
        for r in (left, right):
            if r is None:
                vals.append(count)
            elif r[0] == "node":
                vals.append(r[1])
            else:
                vals.append(count + 16 + r[1])
        tree += vals[0].to_bytes(3, "big") + vals[1].to_bytes(3, "big")
    meta = _enc({
        "binary_format_major_version": 2, "binary_format_minor_version": 0,
        "build_epoch": U64(1760000000), "database_type": db_type,
        "description": {"en": "kiwi-web test database"}, "ip_version": 4,
        "languages": ["en"], "node_count": U32(count), "record_size": 24,
    })
    path.write_bytes(tree + b"\x00" * 16 + data + b"\xab\xcd\xefMaxMind.com" + meta)


write_mmdb(GEO / "dbip-country-lite.mmdb", "DBIP-Country-Lite", [
    ("203.0.113.0", {"country": {"iso_code": "ZZ"}}),
    ("198.51.100.0", {"country": {"iso_code": "YY"}}),
])
write_mmdb(GEO / "dbip-asn-lite.mmdb", "DBIP-ASN-Lite", [
    ("203.0.113.0", {"autonomous_system_number": U32(64500), "autonomous_system_organization": "Doc Net"}),
    ("198.51.100.0", {"autonomous_system_number": U32(16509), "autonomous_system_organization": "Cloud"}),
])
for name in ("dbip-country-lite.mmdb", "dbip-asn-lite.mmdb"):
    (OUT / name).write_bytes((GEO / name).read_bytes())

# ---------------------------------------------------------------- the app

import app.core.internal_api as internal_api  # noqa: E402
import app.web.feature_flags as web_flags  # noqa: E402
import app.web.meta as web_meta  # noqa: E402
import app.web.pages as web_pages  # noqa: E402
import app.geo_maintenance.gate as geo_gate  # noqa: E402
import app.geo_maintenance.geoip as geoip  # noqa: E402
import app.embed.service as embed_service  # noqa: E402
import app.admin.runtime_config as runtime_config  # noqa: E402
import app.pageviews.collect as collect  # noqa: E402
from app.pageviews.beacon import beacon  # noqa: E402
from app.web.main import app  # noqa: E402

FLAG_KEYS = list(web_flags._ALL_ATTRS)
OFF = {"mods_hub_enabled", "file_drops_enabled", "unlock_debug_enabled", "server_status_enabled",
       "player_activity_enabled", "guides_enabled", "abilities_enabled", "gem_tools_enabled",
       "dressing_room_page_enabled", "codexes_enabled", "tomes_enabled", "calculators_enabled",
       "gems_guide_enabled", "leaderboards_enabled", "community_sites_enabled"}

MOD_CARDS = [
    {"handle": "alice", "slug": "zeta-mod", "title": "Zeta Mod", "updated_at": "2026-10-01T10:00:00Z"},
    {"handle": "bob", "slug": "alpha", "title": "\"Alpha\" & <Friends>", "updated_at": None},
    {"handle": "carol", "slug": "ümlaut", "title": "— Über", "updated_at": "2026-09-01T00:00:00+00:00"},
    {"handle": "", "slug": "nohandle", "title": "No handle"},
    {"handle": "dave", "slug": "untitled", "title": ""},
]
PACK_CARDS = [{"handle": "erin", "slug": f"pack-{i}", "title": f"Pack {i:03d}"} for i in range(130)]

SCENARIOS = {
    "on": {
        "/site/feature-flags": {k: True for k in FLAG_KEYS},
        "/v1/misc/supporters": {"supporters": ["Zed", "Ann <b>bold</b>", "O'Neil"]},
        "/site/clubs": {"items": [
            {"name": "Kiwi Club", "rank": 1, "description": "A \"club\" & more", "discord_url": "https://discord.gg/x",
             "website_url": None, "avatar_url": None, "banner_url": "https://cdn.example/b.png"},
            {"name": "", "rank": None, "description": "", "discord_url": None, "website_url": "https://ex.example",
             "avatar_url": "https://cdn.example/a.png", "banner_url": None},
            {"name": "émeraude", "rank": 12},
        ]},
        "/site/mods/projects?limit=100&offset=0": {"items": MOD_CARDS, "total": len(MOD_CARDS)},
        "/site/modpacks/projects?limit=100&offset=0": {"items": PACK_CARDS[:100], "total": len(PACK_CARDS)},
        "/site/modpacks/projects?limit=100&offset=100": {"items": PACK_CARDS[100:], "total": len(PACK_CARDS)},
        "/site/mods/modders": {"items": [{"handle": "alice", "updated_at": "2026-10-02"}, {"handle": "bob"},
                                         {"handle": ""}]},
        "/site/embed/allowed-origins": {"origins": ["https://partner.example", "https://bad/path", "http://*.ok.example:8080"]},
        "/internal/geo-maintenance": {"countries": []},
    },
    "off": {
        "/site/feature-flags": {**{k: k not in OFF for k in FLAG_KEYS}, "extra_flag": False},
        "/site/mods/modders": {"items": [{"handle": "alice"}]},
        "/internal/geo-maintenance": {"countries": []},
    },
    "down": {},
    "geo": {
        "/site/feature-flags": {k: True for k in FLAG_KEYS},
        "/internal/geo-maintenance": {"countries": ["ZZ", 5]},
    },
}
(OUT / "upstream.json").write_text(json.dumps(SCENARIOS, indent=1, ensure_ascii=False), encoding="utf-8")

CURRENT = {"name": "on"}


async def fake_internal_get(path, params=None, *, timeout=8.0, headers=None):
    key = path
    if params:
        key += "?" + "&".join(f"{k}={v}" for k, v in params.items())
    data = SCENARIOS[CURRENT["name"]].get(key)
    return json.loads(json.dumps(data)) if data is not None else None


async def no_db(*a, **k):
    raise RuntimeError("no database in the website container")


for mod in (internal_api, web_flags, web_meta, web_pages, geo_gate):
    mod.internal_get = fake_internal_get
runtime_config.get_setting = no_db

EVENTS = []
beacon.record = lambda event: EVENTS.append(event)
FIXED_NOW = dt.datetime(2026, 10, 23, 12, 0, 0, tzinfo=dt.timezone.utc)
collect.utcnow = lambda: FIXED_NOW


def reset():
    web_flags._cache.update({"at": -1e9, "flags": None})
    web_meta._SITEMAP_CACHE.update({"body": None, "at": -1e9})
    geo_gate._cache.update({"at": -1e9, "countries": frozenset()})
    embed_service._origins_cache.update({"at": -1e9, "value": None})
    collect._rate.clear()


geoip._COUNTRY._load(geoip._COUNTRY.path())
geoip._ASN._load(geoip._ASN.path())


def etag_of(rel):
    st = (ROOT / "site" / "static" / rel).stat()
    base = str(st.st_mtime) + "-" + str(st.st_size)
    return '"' + hashlib.md5(base.encode()).hexdigest() + '"'


def version_of(rel):
    return hashlib.sha256((ROOT / "site" / "static" / rel).read_bytes()).hexdigest()[:10]


async def call(method, raw_path, headers, body=b"", client=None):
    path_part, _, query = raw_path.partition("?")
    from urllib.parse import unquote
    scope = {
        "type": "http", "asgi": {"version": "3.0", "spec_version": "2.4"}, "http_version": "1.1",
        "method": method, "scheme": "http", "path": unquote(path_part), "raw_path": path_part.encode(),
        "query_string": query.encode(), "root_path": "",
        "headers": [(k.lower().encode("latin-1"), v.encode("latin-1")) for k, v in headers],
        "client": (client or "203.0.113.7", 50000), "server": ("testserver", 80),
    }
    sent = False

    async def receive():
        nonlocal sent
        if not sent:
            sent = True
            return {"type": "http.request", "body": body, "more_body": False}
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
UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36"

PAGES = ["/embed/viewer", "/app", "/browse", "/documentation", "/swf-docs", "/support", "/embed/status.svg",
         "/status/og.png", "/tomes", "/server-time", "/search", "/star-chart", "/sound-studio",
         "/blueprint-editor", "/mod-workshop", "/unlock-debug", "/unlock_debug", "/drop/abc123",
         "/zakros-ui-requests", "/gems-guide", "/guides", "/gem-tools", "/gem-abilities", "/gear-guide",
         "/fishing-guide", "/loot-collector-guide", "/community-sites", "/dressing-room", "/gem-simulator",
         "/gem-evaluator", "/gem-builds", "/calculators", "/terms", "/privacy", "/accessibility", "/changelog",
         "/login", "/dashboard", "/codexes/crafting", "/mods/why", "/clubs", "/activity/og.png"]


def requests_for(scenario):
    reqs = []

    def add(name, path, method="GET", headers=None, body=None, **extra):
        h = [("host", "trove.aallyn.net"), ("x-request-id", f"web-{scenario}-{len(reqs)}")]
        h += list(headers or [])
        reqs.append({"name": name, "method": method, "path": path, "headers": h,
                     "body_b64": base64.b64encode(body or b"").decode(), **extra})

    for p in PAGES:
        add("page " + p, p, headers=[("accept", HTML)])
    if scenario == "geo":
        add("geo static", "/static/appearance.css")
        add("geo health", "/health")
        add("geo other ip", "/terms", headers=[("accept", HTML)], client="198.51.100.9")
        return reqs
    add("terms json 404 gate", "/terms", headers=[("accept", "application/json")])
    add("classic", "/terms", headers=[("accept", HTML), ("cookie", "btt_design=classic; btt_theme=light")])
    add("dark", "/app", headers=[("accept", HTML), ("cookie", "btt_theme=dark")])
    add("junk cookies", "/app", headers=[("accept", HTML), ("cookie", "btt_design=neon; btt_theme=\"auto\"")])
    add("credential", "/terms", headers=[("accept", HTML), ("cookie", "kiwi_site_access=abc")])
    add("bearer", "/support", headers=[("authorization", "Bearer x")])
    add("embed full", "/embed/viewer?release=r1&tmod=t&game=g&prefab=p&dress=d&path=a/b%22%3C%3E&sound=s"
        "&backdrop=hat&mode=vfx&theme=light&mode=bogus", headers=[("cookie", "btt_theme=dark")])
    add("embed defaults", "/embed/viewer?backdrop=nope&theme=&mode=")
    add("activity og 1m", "/activity/og.png?period=1M")
    add("activity og bad", "/activity/og.png?period=%3Cx%3E")
    add("drop encoded", "/drop/a%20b%C3%A9", headers=[("accept", HTML)])
    add("swf readme", "/swf-docs/README")
    add("swf readme lower", "/swf-docs/readme")
    add("swf atlas", "/swf-docs/atlas", headers=[("accept", HTML)])
    add("swf atlas upper", "/swf-docs/ATLAS")
    add("swf shared", "/swf-docs/SHARED_LIBRARIES")
    add("swf nope html", "/swf-docs/nope", headers=[("accept", HTML)])
    add("swf nope json", "/swf-docs/nope")
    add("swf md path gzip", "/swf-docs/atlas.md", headers=[("accept", HTML), ("accept-encoding", "gzip, br")])
    add("swf md path identity", "/swf-docs/atlas.md", headers=[("accept", HTML)])
    for host in ("trove.aallyn.net", "api.aallyn.net", "aallyn.net", "www.aallyn.net", "dev.aallyn.net",
                 "Dev.Aallyn.net:443", "elsewhere.example"):
        reqs.append({"name": "robots " + host, "method": "GET", "path": "/robots.txt",
                     "headers": [("host", host), ("x-request-id", f"web-{scenario}-{len(reqs)}"),
                                 ("accept-encoding", "gzip")], "body_b64": ""})
    add("bing", "/BingSiteAuth.xml")
    add("sitemap", "/sitemap.xml", headers=[("accept-encoding", "gzip")])
    add("health", "/health")
    add("head terms", "/terms", method="HEAD", headers=[("accept", HTML)])
    add("post terms", "/terms", method="POST")
    add("options terms", "/terms", method="OPTIONS")
    if scenario != "on":
        return reqs
    add("gamedata pvp gzip", "/gamedata/pvp.json", headers=[("accept-encoding", "gzip")])
    add("gamedata gear gzip", "/gamedata/gear.json", headers=[("accept-encoding", "gzip")])
    add("gamedata gear identity", "/gamedata/gear.json")
    add("gamedata range", "/gamedata/gear.json", headers=[("range", "bytes=0-99"), ("accept-encoding", "gzip")])
    add("gamedata nope html", "/gamedata/nope.json", headers=[("accept", HTML)])
    add("gamedata nope json", "/gamedata/nope.json")
    add("gamedata head", "/gamedata/pvp.json", method="HEAD")
    add("static css small", "/static/appearance.css", headers=[("accept-encoding", "gzip")])
    add("static css big gzip", "/static/style.css", headers=[("accept-encoding", "gzip")])
    add("static css big identity", "/static/style.css")
    add("static css versioned", "/static/style.css?v=" + version_of("style.css"))
    add("static css stale version", "/static/style.css?v=0000000000")
    add("static js stream gzip", "/static/blueprint-editor.js", headers=[("accept-encoding", "gzip")])
    add("static js stream identity", "/static/blueprint-editor.js")
    add("static json gzip", "/static/commands.json", headers=[("accept-encoding", "deflate, gzip")])
    add("static woff2", "/static/fonts/inter-greek-400.woff2", headers=[("accept-encoding", "gzip")])
    add("static png", "/static/class-icons/bard.png", headers=[("accept-encoding", "gzip")])
    add("static webp", "/static/trove-screens/loading_delve_01.webp")
    add("static txt", "/static/vendor/lame.LICENSE.txt", headers=[("accept-encoding", "gzip")])
    add("static md", "/static/swf-docs/atlas.md", headers=[("accept-encoding", "gzip")])
    add("static 304", "/static/style.css", headers=[("if-none-match", "W/" + etag_of("style.css"))],
        etag_of="style.css")
    add("static 304 star", "/static/style.css", headers=[("if-none-match", "*")])
    add("static ims", "/static/style.css", headers=[("if-modified-since", "Fri, 01 Jan 2100 00:00:00 GMT")])
    add("static ims old", "/static/style.css", headers=[("if-modified-since", "Thu, 01 Jan 1970 00:00:00 GMT")])
    add("static range", "/static/style.css", headers=[("range", "bytes=10-19"), ("accept-encoding", "gzip")])
    add("static bad range", "/static/style.css", headers=[("range", "bytes=999999999-")])
    add("static head", "/static/style.css", method="HEAD", headers=[("accept-encoding", "gzip")])
    add("static post", "/static/style.css", method="POST")
    add("static missing html", "/static/nope.css", headers=[("accept", HTML), ("accept-encoding", "gzip")])
    add("static missing json", "/static/nope.css")
    add("static dir", "/static/fonts", headers=[("accept", HTML)])
    add("static dir slash", "/static/fonts/")
    add("static traversal", "/static/../templates/terms.html")
    add("static dotdot inside", "/static/fonts/../appearance.css")
    add("static encoded", "/static/appearance%2Ecss")
    add("413", "/s", method="POST", headers=[("content-length", "99999999")])

    def beacon(name, data, headers=(), raw=None, client=None):
        body = raw if raw is not None else json.dumps(data).encode()
        h = dict([("user-agent", UA), ("origin", "https://trove.aallyn.net"), ("content-type", "text/plain")])
        h.update(dict(headers))
        add("beacon " + name, "/s", method="POST", headers=list(h.items()), body=body,
            **({"client": client} if client else {}))

    pv = {"t": "pv", "id": "pid-0001-abcd", "u": "https://www.trove.aallyn.net/mods/alice/zeta?utm_source=Discord",
          "r": "", "w": 1280, "tp": 0, "l": "de"}
    beacon("pv", pv)
    beacon("pv player", {**pv, "u": "https://trove.aallyn.net/player/Some%20One", "r": "https://www.google.com/"})
    beacon("pv swf", {**pv, "u": "https://trove.aallyn.net/swf-docs/atlas", "r": "https://api.aallyn.net/x", "l": "xx"})
    beacon("pv drop", {**pv, "u": "http://localhost:8000/drop/x", "w": "320", "tp": 3.9, "l": None})
    beacon("pv static", {**pv, "u": "https://trove.aallyn.net/static/x.css"})
    beacon("pv og", {**pv, "u": "https://trove.aallyn.net/activity/og.png"})
    beacon("pv foreign", {**pv, "u": "https://evil.example/terms"})
    beacon("pv bad ipv6", {**pv, "u": "https://[::1/terms"})
    beacon("pv lang list", {**pv, "l": ["de"]})
    beacon("pv int id", {**pv, "id": 123456789})
    beacon("pv id newline", {**pv, "id": "abcdefgh\n"})
    beacon("pv utm medium", {**pv, "u": "https://trove.aallyn.net/?utm_source=News%20Letter!&utm_medium=email"})
    beacon("pv trailing newline", {**pv, "u": "https://trove.aallyn.net/terms%0A"})
    beacon("end", {"t": "end", "id": "pid-0001-abcd", "s": "123"})
    beacon("end huge", {"t": "end", "id": "pid-0001-abcd", "s": 1e30})
    beacon("perf", {"t": "perf", "id": "pid-0001-abcd", "ttfb": 120.7, "fcp": 0, "lcp": -5, "dcl": "x", "load": 999999})
    beacon("perf empty", {"t": "perf", "id": "pid-0001-abcd"})
    beacon("cls", {"t": "cls", "id": "pid-0001-abcd", "c": 37})
    beacon("cls negative", {"t": "cls", "id": "pid-0001-abcd", "c": -3})
    beacon("short id", {"t": "end", "id": "abc"})
    beacon("bot", pv, headers=[("user-agent", "Mozilla/5.0 (compatible; Googlebot/2.1)")])
    beacon("foreign origin", pv, headers=[("origin", "https://evil.example")])
    beacon("bad json", None, raw=b"{nope")
    beacon("array json", None, raw=b"[1, 2]")
    beacon("empty body", None, raw=b"")
    beacon("big body", {**pv, "pad": "x" * 5000})
    beacon("cloud asn", pv, client="198.51.100.20")
    beacon("no origin", pv, headers=[("origin", "")])
    beacon("no ua", pv, headers=[("user-agent", "")])
    return reqs


async def run():
    golden = []
    for scenario in SCENARIOS:
        CURRENT["name"] = scenario
        reset()
        for r in requests_for(scenario):
            EVENTS.clear()
            body = base64.b64decode(r["body_b64"])
            res = await call(r["method"], r["path"], r["headers"], body, r.get("client"))
            rbody = res["body"]
            enc = dict((k, v) for k, v in res["headers"]).get("content-encoding")
            if enc == "gzip" and len(rbody) >= 10:
                rbody = rbody[:9] + b"\x03" + rbody[10:]
            golden.append({
                "scenario": scenario, **r,
                "status": res["status"], "resp_headers": res["headers"],
                "resp_body_z": base64.b64encode(zlib.compress(rbody, 9)).decode(),
                "events": list(EVENTS),
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


def template_cases():
    """Jinja2 (Starlette's env) over small templates: escaping, filters, methods."""
    import jinja2
    env = jinja2.Environment(loader=jinja2.DictLoader({}), autoescape=jinja2.select_autoescape())
    ctx = {
        "s": "a<b>&\"'/c", "n": None, "t": True, "f": False, "i": 42, "x": 1.5, "big": 1e16, "small": 1e-05,
        "neg": -0.0, "lst": ["a", 1, None, 2.5, True], "d": {"k": "v", "n": [1, {"z": None}]},
        "path": "/mods/a b", "e": "", "uni": "ü€😀", "name": "émile", "items_d": {"items": [1, 2]},
    }
    cases = [
        "{{ s }}|{{ n }}|{{ t }}|{{ f }}|{{ i }}|{{ x }}|{{ big }}|{{ small }}|{{ neg }}",
        "{{ lst }}|{{ d }}|{{ undefined_thing }}|{{ uni }}",
        "{{ s | safe }}|{{ lst | join(', ') }}|{{ lst | join }}|{{ lst | length }}|{{ s | length }}",
        "{{ undefined_thing | default('dflt') }}|{{ n | default('x') }}|{{ e | default('y', true) }}",
        "{{ d | tojson }}|{{ s | tojson }}|{{ lst | tojson }}|{{ uni | tojson }}",
        "{{ path | urlencode }}|{{ uni | urlencode }}|{{ {'a b': 'c&d'} | urlencode }}",
        "{{ name | upper }}|{{ (name or '?')[0] | upper }}|{{ (e or '?')[0] | upper }}",
        "{{ 'x' if path.startswith('/mods/') else 'y' }}|{{ path.startswith(('/a', '/mods')) }}",
        "{% for k, v in d.items() %}{{ k }}={{ v }};{% endfor %}|{{ items_d['items'] }}",
        "{{ 'checked' if t not in ('light', 'dark') else '' }}|{{ s | replace('a', '<') }}",
        "{%- if n %}A{% else %}B{% endif -%}\n  {{ i }}  \n{% set q = i %}{{ q }}\n",
        "{{ ['#7a', '#e7'][1 % 2] }}|{% for z in lst %}{{ loop.index0 }}{% endfor %}",
    ]
    out = []
    for src in cases:
        env.loader.mapping["t.html"] = src
        env.cache.clear() if env.cache is not None else None
        out.append({"src": src, "out": env.get_template("t.html").render(ctx)})
    return {"context": ctx, "cases": out}


async def main():
    golden = await run()
    routes = [[getattr(r, "path", ""), sorted(m for m in (getattr(r, "methods", None) or []) if m != "HEAD")]
              for r in app.router.routes]
    data = {"fingerprint": fingerprint(), "env": ENV, "routes": routes, "requests": golden}
    for k in ("SITE_ROOT", "GEOIP_DIR", "GAMEDATA_DIR"):
        data["env"] = {**data["env"], k: None}
    (OUT / "golden.json").write_text(json.dumps(data, indent=0, ensure_ascii=False), encoding="utf-8")
    (OUT / "templates.json").write_text(json.dumps(template_cases(), indent=1, ensure_ascii=False),
                                        encoding="utf-8")
    from markdown_it import MarkdownIt
    md = MarkdownIt("commonmark", {"html": False}).enable("table")
    docs, sums = {}, {}
    for f in sorted((ROOT / "site" / "static" / "swf-docs").glob("*.md")):
        src = f.read_text(encoding="utf-8")
        docs[f.name] = base64.b64encode(zlib.compress(md.render(src).encode(), 9)).decode()
        sums[f.name] = hashlib.sha256(src.encode()).hexdigest()
    (OUT / "swf_docs.json").write_text(json.dumps({"sha256": sums, "docs": docs}, indent=0, ensure_ascii=False),
                                       encoding="utf-8")
    events = [{"name": r["name"], "scenario": r["scenario"], "events": r["events"]} for r in golden if r["events"]]
    print(f"{len(golden)} requests, {len(events)} with beacon events")


asyncio.run(main())
