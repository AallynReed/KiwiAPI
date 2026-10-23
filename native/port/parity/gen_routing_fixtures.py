"""Starlette routing for kiwi-server and kiwi-web, captured from Python.

Writes native/kiwi-server/src/starlette/routes.json (both apps' route tables
in registration order, which the Rust routers fall back on) and
native/kiwi-server/testdata/routing/golden.json: unmatched paths, slash
redirects, 405s, decoded-path routing and the int-path answers, driven through
the real apps at the ASGI level (no lifespan, runtime_config at its defaults).

Run from the repo root with the production-pinned venv:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_routing_fixtures.py
"""

from __future__ import annotations

import asyncio
import base64
import json
import logging
import os
import subprocess
import sys
import tempfile
import warnings
from pathlib import Path
from urllib.parse import unquote

ROOT = Path(__file__).resolve().parents[3]
ROUTES = ROOT / "native" / "kiwi-server" / "src" / "starlette" / "routes.json"
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "routing" / "golden.json"
os.chdir(ROOT)
sys.path.insert(0, str(ROOT))
warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)

WEB_ENV = {
    "SITE_ROOT": str(ROOT / "site"),
    "APP_URL": "https://trove.aallyn.net",
    "API_URL": "https://api.aallyn.net",
    "INTERNAL_API_URL": "http://internal.test",
    "INTERNAL_KEY": "routing-fixture-internal-key",
    "SECRET_KEY": "routing-fixture-secret",
    "GEOIP_DIR": tempfile.mkdtemp(prefix="kiwi-routing-geo-"),
    "GAMEDATA_DIR": str(ROOT / "app" / "trove" / "gamedata"),
}


def table(app) -> list:
    from starlette.routing import Mount
    out = []
    for r in app.routes:
        if isinstance(r, Mount):
            out.append([r.path, None])
        else:
            out.append([r.path, sorted(r.methods)])
    return out


async def drive(app, case: dict) -> dict:
    raw_path, _, qs = case["path"].partition("?")
    scope = {
        "type": "http", "asgi": {"version": "3.0", "spec_version": "2.4"}, "http_version": "1.1",
        "method": case["method"], "scheme": case.get("scheme", "http"),
        "path": unquote(raw_path) if "%" in raw_path else raw_path,
        "raw_path": raw_path.encode("ascii"), "query_string": qs.encode("ascii"), "root_path": "",
        "headers": [(k.lower().encode("latin-1"), v.encode("latin-1")) for k, v in case["headers"]],
        "client": ("10.0.0.9", 50000), "server": ("testserver", 80), "state": {},
    }
    body = base64.b64decode(case.get("body_b64", ""))
    sent = False
    start: dict = {}
    chunks: list[bytes] = []

    async def receive():
        nonlocal sent
        if not sent:
            sent = True
            return {"type": "http.request", "body": body, "more_body": False}
        await asyncio.sleep(3600)

    async def send(msg):
        if msg["type"] == "http.response.start":
            start.update(msg)
        elif msg["type"] == "http.response.body":
            chunks.append(msg.get("body", b""))

    await app(scope, receive, send)
    data = b"" if case["method"] == "HEAD" else b"".join(chunks)
    hdrs = [[k.decode("latin-1"), v.decode("latin-1")] for k, v in start["headers"]]
    if any(k == "content-encoding" and v == "gzip" for k, v in hdrs) and data[:2] == b"\x1f\x8b":
        data = data[:9] + b"\x03" + data[10:]  # zlib's OS byte as on Linux
    return {"status": start["status"], "response_headers": hdrs, "body_b64": base64.b64encode(data).decode()}


def case(name, method, path, headers=(), **extra) -> dict:
    extra_h = [list(x) for x in headers]
    host = [] if any(k == "Host" for k, _ in extra_h) else [["Host", "kiwi-parity"]]
    h = [*host, ["X-Request-ID", f"rt-{name}"], *extra_h]
    return {"name": name, "method": method, "path": path, "headers": h, **extra}


HTML = ["Accept", "text/html,application/xhtml+xml,*/*;q=0.8"]
DOCS = ["Origin", "https://docs.aallyn.net"]
GZ = ["Accept-Encoding", "gzip"]
HTTPS = ["X-Forwarded-Proto", "https"]
BIG = "99999999999999999999999"

API_CASES = [
    case("nf_json", "GET", "/v1/nope"),
    case("nf_html", "GET", "/nope", [HTML]),
    case("nf_html_v1", "GET", "/v1/nope", [HTML]),
    case("nf_head_html", "HEAD", "/nope", [HTML]),
    case("nf_post", "POST", "/nope", [HTML]),
    case("nf_options", "OPTIONS", "/nope"),
    case("nf_cors", "GET", "/v1/nope", [DOCS, GZ]),
    case("nf_root", "GET", "/", [HTML]),
    case("nf_html_gzip", "GET", "/nope", [HTML, GZ]),
    case("nf_preflight", "OPTIONS", "/nope", [DOCS, ["Access-Control-Request-Method", "GET"]]),
    case("nf_preflight_evil", "OPTIONS", "/nope", [["Origin", "https://evil.example"],
                                                   ["Access-Control-Request-Method", "GET"]]),
    case("slash_health", "GET", "/health/"),
    case("slash_health_query", "GET", "/health/?a=1&b=%20c&d='x'&e=[1]&f=%2F", [DOCS]),
    case("slash_many", "GET", "/health///"),
    case("slash_https", "GET", "/config/", [HTTPS], scheme="https", peer="127.0.0.1"),
    case("slash_host_port", "GET", "/health/", [["Host", "Kiwi.Example:0443"]]),
    case("slash_add", "GET", "/v1/leaderboards/5/"),
    case("slash_unicode", "GET", "/v1/leaderboards/renames/%C3%A9%20x/"),
    case("slash_encoded", "GET", "/health%2F"),
    case("slash_docs", "GET", "/docs/"),
    case("slash_head", "HEAD", "/health/"),
    case("slash_post", "POST", "/health/"),
    case("slash_git", "GET", "/git/mods/a/b.git/info/refs/"),
    case("int_word", "GET", "/v1/leaderboards/abc"),
    case("int_word_slash", "GET", "/v1/leaderboards/abc/"),
    case("int_word_entries", "GET", "/v1/leaderboards/abc/entries?created_at=1"),
    case("int_signed", "GET", "/v1/leaderboards/+5"),
    case("int_post", "POST", "/v1/leaderboards/5"),
    case("int_delete", "DELETE", "/v1/leaderboards/5/health"),
    case("git_no_suffix", "GET", "/git/mods/a/b/info/refs"),
    case("git_bare_suffix", "GET", "/git/mods/a/.git/info/refs"),
    case("git_post_refs", "POST", "/git/mods/a/b.git/info/refs"),
    case("method_put", "PUT", "/health"),
    case("method_options", "OPTIONS", "/health"),
    case("method_cors", "DELETE", "/health", [DOCS]),
    case("method_head_post_only", "HEAD", "/v1/leaderboards/insert"),
    case("empty_segment", "GET", "/v1//stats"),
    case("empty_param", "GET", "/v1/leaderboards//entries"),
    case("leading_double", "GET", "//health"),
    case("decoded_literal", "GET", "/h%65alth"),
    case("decoded_slash_split", "GET", "/v1/leaderboards%2F5"),
    case("newline_literal", "GET", "/health%0A"),
    case("newline_slash", "GET", "/health/%0A"),
    case("events_head", "HEAD", "/v1/events/stream"),
    case("events_head_slash", "HEAD", "/v1/events/stream/"),
    case("events_head_other", "HEAD", "/v1/events/nope"),
    case("events_post", "POST", "/v1/events/stream"),
    case("site_lb_word", "GET", "/site/leaderboards/abc/entries"),
    case("site_lb_word_query", "GET", "/site/leaderboards/1.5/entries?created_at=x&limit=0"),
    case("site_lb_health_word", "GET", "/site/leaderboards/%20/health"),
    case("site_lb_history_word", "GET", "/site/leaderboards/1e3/history?days=99"),
    case("site_lb_big_entries", "GET", f"/site/leaderboards/{BIG}/entries?created_at=4102444800"),
    case("site_lb_big_history", "GET", "/site/leaderboards/2147483648/history"),
    case("site_lb_big_health", "GET", f"/site/leaderboards/-{BIG}/health"),
    case("site_lb_big_guarded", "GET", "/site/leaderboards/2147483648/entries?created_at=100"),
    case("discord_big_anon", "GET", f"/v1/site-auth/discord/guilds/{BIG}"),
    case("discord_word_anon", "GET", "/v1/site-auth/discord/guilds/abc/clubs"),
    case("api_static", "GET", "/static/app.js", [HTML]),
]

WEB_CASES = [
    case("web_nf_html", "GET", "/nope", [HTML]),
    case("web_nf_json", "GET", "/nope"),
    case("web_nf_post", "POST", "/nope"),
    case("web_slash_page", "GET", "/terms/", [HTML]),
    case("web_slash_page_https", "GET", "/terms/?x=1", [HTTPS], scheme="https", peer="127.0.0.1"),
    case("web_slash_param", "GET", "/swf-docs/abc/"),
    case("web_static_bare", "GET", "/static"),
    case("web_static_bare_head", "HEAD", "/static?v=1"),
    case("web_static_dir", "GET", "/static/", [HTML]),
    case("web_static_dir_json", "GET", "/static/"),
    case("web_static_post", "POST", "/static/app.js"),
    case("web_method_post", "POST", "/terms"),
    case("web_method_options", "OPTIONS", "/terms"),
    case("web_decoded_slash", "GET", "/t%65rms/"),
    case("web_encoded_split", "GET", "/swf-docs/a%2Fb"),
    case("web_double", "GET", "//terms", [HTML]),
    case("web_empty_param", "GET", "/drop/"),
    case("web_gamedata_slash", "GET", "/gamedata/pvp.json/"),
    case("web_health_slash", "GET", "/health/"),
    case("web_robots_slash", "GET", "/robots.txt/"),
    case("web_events_head", "HEAD", "/v1/events/stream"),
    case("web_head_nf", "HEAD", "/nope", [HTML]),
    case("web_nf_gzip", "GET", "/nope", [HTML, GZ]),
    case("web_static_bare_gzip", "GET", "/static", [GZ]),
]


def patch_common():
    import app.admin.runtime_config as rc

    async def defaults(key):
        return rc._spec(key).default

    rc.get_setting = defaults


def patch_pg():
    """Postgres that refuses an int past int32, like asyncpg against INTEGER."""
    import asyncpg
    import app.trove.leaderboards.pg_store as pg

    class Con:
        def _check(self, args):
            for i, a in enumerate(args, 1):
                if isinstance(a, int) and not isinstance(a, bool) and not -2**31 <= a < 2**31:
                    raise asyncpg.exceptions.DataError(
                        f"invalid input for query argument ${i}: {a} (value out of int32 range)")

        async def fetchrow(self, sql, *args):
            self._check(args)
            return None

        async def fetch(self, sql, *args):
            self._check(args)
            return []

        async def fetchval(self, sql, *args):
            self._check(args)
            return 0

    class Acquire:
        async def __aenter__(self):
            return Con()

        async def __aexit__(self, *a):
            return False

    pg.acquire = lambda: Acquire()


async def run_api() -> dict:
    patch_common()
    patch_pg()
    import app.main as main
    cases = []
    for c in API_CASES:
        cases.append({**c, **await drive(main.app, c)})
    return {"table": table(main.app), "cases": cases}


async def run_web() -> dict:
    patch_common()
    import app.core.internal_api as internal_api
    import app.geo_maintenance.gate as geo_gate
    import app.web.feature_flags as web_flags
    import app.web.meta as web_meta
    import app.web.pages as web_pages

    async def down(path, params=None, *, timeout=8.0, headers=None):
        return None

    for mod in (internal_api, web_flags, web_meta, web_pages, geo_gate):
        mod.internal_get = down
    from app.web.main import app
    cases = []
    for c in WEB_CASES:
        c = {**c, "headers": [["Host", "trove.aallyn.net"] if h[0] == "Host" else h for h in c["headers"]]}
        cases.append({**c, **await drive(app, c)})
    keep = ("APP_URL", "API_URL", "INTERNAL_KEY", "SECRET_KEY")
    return {"table": table(app), "cases": cases, "env": {k: WEB_ENV[k] for k in keep}}


async def discord_big() -> list:
    """The bot dashboard's handlers with a guild id past int64, Discord answered
    from a fixed world."""
    from app.bot import discord_rest as dr
    from app.bot import router as br
    from app.core.errors import APIError

    g, uid = "99999999999999999999", 312345678901234567
    found_guild = {"id": g, "owner_id": "1", "roles": [{"id": g, "permissions": "0"}]}
    member = {"user": {"id": str(uid)}, "roles": []}
    worlds = {
        "missing": {"paths": {}, "errors": {}},
        "down": {"paths": {}, "errors": {f"/guilds/{g}": "DISCORD_BOT_TOKEN is not configured."}},
        "member": {"paths": {f"/guilds/{g}": found_guild, f"/guilds/{g}/members/{uid}": member}, "errors": {}},
        "admin": {"paths": {f"/guilds/{g}": {**found_guild, "owner_id": str(uid)},
                            f"/guilds/{g}/members/{uid}": member, f"/guilds/{g}/roles": []}, "errors": {}},
        "admin_roles_down": {"paths": {f"/guilds/{g}": {**found_guild, "owner_id": str(uid)},
                                       f"/guilds/{g}/members/{uid}": member},
                             "errors": {f"/guilds/{g}/roles": f"GET /guilds/{g}/roles -> 400: bad"}},
    }
    world = {}

    async def fake_get(client, path):
        if path in world["errors"]:
            raise dr.DiscordRestError(world["errors"][path])
        return world["paths"].get(path)

    dr._get = fake_get

    class User:
        def __init__(self, discord_id):
            self.discord_id = discord_id

    detail = ("/guilds/{guild_id}", lambda u: br.guild_detail(int(g), u))
    perms = ("/guilds/{guild_id}/permissions",
             lambda u: br.set_permissions(int(g), br.PermissionsUpdate(permissions={}), u))
    template = ("/guilds/{guild_id}/announcements/{key}/template",
                lambda u: br.set_announcement_template(int(g), "nope", br.AnnouncementTemplateUpdate(), u))
    action = ("/guilds/{guild_id}/clubs/{club_id}/roster/{member_id}/{action}",
              lambda u: br.club_roster_action(int(g), "c", 5, "fly", u))
    clubs = ("/guilds/{guild_id}/clubs", lambda u: br.list_clubs(int(g), u))
    params = {detail[0]: {}, perms[0]: {}, template[0]: {"key": "nope"},
              action[0]: {"club_id": "c", "member_id": "5", "action": "fly"}, clubs[0]: {}}
    out = []
    for wname, (route, call), linked in [
        ("missing", detail, False), ("missing", detail, True), ("down", detail, True), ("member", detail, True),
        ("member", clubs, True), ("member", perms, True), ("admin", perms, True), ("admin_roles_down", perms, True),
        ("missing", template, True), ("missing", action, True),
    ]:
        world.clear()
        world.update(worlds[wname])
        try:
            await call(User(uid if linked else None))
            want = {"status": 200}
        except APIError as e:
            want = {"status": e.status_code, "code": e.code, "message": e.message}
        except Exception:  # noqa: BLE001 - Python's unhandled 500
            want = {"status": 500}
        out.append({"world": worlds[wname], "route": route, "linked": linked,
                    "params": [["guild_id", f"+{g}"], *params[route].items()], "want": want})
    return out


def pyints() -> list:
    from pydantic import TypeAdapter, ValidationError
    ta = TypeAdapter(int)
    out = []
    for s in ["5", "+5", "-5", " 7 ", "0005", "1_000", "1__0", "1.0", "1.5", "1e3", "", " ", "٣", "-0",
              "2147483647", "2147483648", "-2147483649", BIG, "0x10", "1_", "_1", "+", "1.", ".0"]:
        try:
            out.append([s, str(ta.validate_python(s))])
        except ValidationError as e:
            out.append([s, e.errors()[0]["type"]])
    return out


if __name__ == "__main__":
    if sys.argv[1:] == ["web"]:
        print(json.dumps(asyncio.run(run_web()), ensure_ascii=False))
        sys.exit(0)
    os.environ.setdefault("SECRET_KEY", "routing-fixture-secret")
    web = subprocess.run([sys.executable, __file__, "web"], env={**os.environ, **WEB_ENV},
                         capture_output=True, check=True).stdout
    web = json.loads(web)
    api = asyncio.run(run_api())
    ROUTES.parent.mkdir(parents=True, exist_ok=True)
    def rows(t):
        return ",\n".join(json.dumps(r, separators=(",", ":")) for r in t)
    ROUTES.write_text('{"api": [\n' + rows(api["table"]) + '\n],\n"web": [\n' + rows(web["table"]) + "\n]}\n",
                      encoding="utf-8", newline="\n")
    OUT.parent.mkdir(parents=True, exist_ok=True)
    golden = {"api": api["cases"], "web": web["cases"], "web_env": web["env"], "pyint": pyints(),
              "discord_big": asyncio.run(discord_big())}
    OUT.write_text(json.dumps(golden, indent=1, ensure_ascii=False) + "\n", encoding="utf-8", newline="\n")
    print(f"wrote {len(api['table'])}+{len(web['table'])} routes, "
          f"{len(api['cases'])}+{len(web['cases'])} cases")
