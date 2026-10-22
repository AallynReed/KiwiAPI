"""Golden fixtures for kiwi-server's middleware stack, captured from Python.

Builds a FastAPI app with the REAL middleware list of ``app.main.app`` (copied
from its ``user_middleware``) and error handlers, plus a table of fixed test
routes, and drives it at the ASGI level like uvicorn would (HEAD bodies
dropped). A few cases go to ``app.main.app`` itself. Redis is a dict and the
embed allowlist is fixed. The Rust test (src/middleware/tests.rs) builds the
same routes from the same table and must reproduce every response byte for
byte.

Run from the repo root with the project venv:
    .venv/Scripts/python.exe native/port/parity/gen_middleware_fixtures.py
"""

from __future__ import annotations

import asyncio
import base64
import gzip
import json
import os
import sys
from pathlib import Path
from urllib.parse import unquote

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "native" / "kiwi-server" / "src" / "middleware" / "fixtures.json"
os.chdir(ROOT)
sys.path.insert(0, str(ROOT))

from fastapi import FastAPI, Request  # noqa: E402
from starlette.responses import Response, StreamingResponse  # noqa: E402

import app.core.idempotency as idem  # noqa: E402
import app.embed.service as embed_service  # noqa: E402
import app.main as main  # noqa: E402
from app.core import csp, static_version  # noqa: E402
from app.core.errors import register_error_handlers  # noqa: E402
from app.core.middleware import API_SIDE_HOSTS  # noqa: E402

EMBED_ORIGINS = ["https://trovesaurus.com", "https://*.example.org"]


class FakeRedis:
    def __init__(self) -> None:
        self.store: dict[str, tuple[str, int | None]] = {}

    async def set(self, key, value, nx=False, ex=None):
        if nx and key in self.store:
            return None
        self.store[key] = (value, ex)
        return True

    async def get(self, key):
        v = self.store.get(key)
        return v[0] if v else None

    async def delete(self, key):
        self.store.pop(key, None)


REDIS = FakeRedis()
idem.get_redis = lambda: REDIS


async def _fixed_origins():
    return list(EMBED_ORIGINS)


embed_service.allowed_origins = _fixed_origins


def _json(obj) -> bytes:
    return json.dumps(obj, separators=(",", ":")).encode()


def _pregz() -> bytes:
    b = bytearray(gzip.compress(b'{"x":"' + b"x" * 3000 + b'"}', compresslevel=6, mtime=0))
    b[9] = 3
    return bytes(b)


JSON = ("content-type", "application/json")
HTML = ("content-type", "text/html; charset=utf-8")
TEXT = ("content-type", "text/plain; charset=utf-8")
BIG = _json({"items": list(range(700))})


def route(**spec) -> dict:
    return spec


# kind: full | stream | raise. py/rs: the route path in each router syntax.
ROUTES = [
    route(name="small", methods=["GET"], py="/__mw/small", status=200, headers=[JSON], body=b'{"ok":true}'),
    route(name="big", methods=["GET"], py="/__mw/big", status=200, headers=[JSON], body=BIG),
    route(name="html", methods=["GET"], py="/__mw/html", status=200, headers=[HTML],
         body=("<!doctype html><title>t</title>" + "<p>hello world</p>" * 120).encode()),
    route(name="png", methods=["GET"], py="/__mw/png", status=200, headers=[("content-type", "image/png")],
         body=b"\x89PNG\r\n\x1a\n" + bytes(range(256)) * 8),
    route(name="ndjson", methods=["GET"], py="/__mw/stream", status=200,
         headers=[("content-type", "application/x-ndjson")], chunks=[b'{"a":1}\n', b'{"b":2}\n']),
    route(name="sse", methods=["GET"], py="/__mw/sse", status=200,
         headers=[("content-type", "text/event-stream"), ("cache-control", "no-cache")],
         chunks=[b"data: 1\n\n", b"data: 2\n\n"]),
    route(name="cached", methods=["GET"], py="/__mw/cached", status=200,
         headers=[("cache-control", "public, max-age=30"), ("vary", "Accept-Language"), JSON], body=BIG),
    route(name="own", methods=["GET"], py="/__mw/own", status=200,
         headers=[("content-security-policy", "default-src 'self'"), ("x-frame-options", "SAMEORIGIN"),
                  ("referrer-policy", "no-referrer"), ("x-request-id", "inner"), JSON], body=b'{"own":1}'),
    route(name="echo", methods=["POST"], py="/__mw/echo", status=201,
         headers=[JSON, ("x-ratelimit-limit", "60")], body=b'{"created":true}'),
    route(name="fail", methods=["POST"], py="/__mw/fail", status=400, headers=[JSON], body=b'{"bad":1}'),
    route(name="cookies", methods=["POST"], py="/__mw/cookies", status=200,
         headers=[("set-cookie", "a=1; Path=/"), ("set-cookie", "b=2; Path=/"), TEXT], body=b"ok"),
    route(name="nocontent", methods=["GET"], py="/__mw/nocontent", status=204, headers=[], body=b""),
    route(name="boom", methods=["GET"], py="/__mw/boom", kind="raise"),
    route(name="pregz", methods=["GET"], py="/__mw/pregz", status=200,
         headers=[JSON, ("content-encoding", "gzip")], body=_pregz()),
    route(name="partial", methods=["GET"], py="/__mw/partial", status=206,
         headers=[TEXT, ("content-range", "bytes 0-1999/5000")], body=b"y" * 2000),
    route(name="text", methods=["GET"], py="/__mw/text", status=200, headers=[TEXT], body=b"z" * 1500),
    route(name="rig", methods=["GET"], py="/site/rigs/{name}", rs="/site/rigs/{name}", status=200,
         headers=[("cache-control", "public, max-age=86400"), JSON], body=BIG),
    route(name="site", methods=["GET"], py="/site/data", status=200,
         headers=[("cache-control", "public, max-age=60"), JSON], body=b'{"d":1}'),
    route(name="login", methods=["GET"], py="/login", status=200, headers=[HTML], body=b"<html>login</html>"),
    route(name="modpage", methods=["GET"], py="/mods/{slug}", rs="/mods/{slug}", status=200, headers=[HTML],
         body=b"<html>mod</html>"),
    route(name="static", methods=["GET"], py="/static/{file:path}", rs="/static/{*file}", status=200,
         headers=[("content-type", "text/javascript; charset=utf-8")], body=b"/*js*/"),
    route(name="embed", methods=["GET"], py="/embed/viewer", status=200, headers=[HTML], body=b"<html>embed</html>"),
    route(name="events", methods=["GET"], py="/v1/events/stream", status=200,
         headers=[("content-type", "text/event-stream")], chunks=[b": hi\n\n"]),
    route(name="health", methods=["GET"], py="/health", status=200, headers=[JSON], body=b'{"status":"ok"}'),
    route(name="caps", methods=["POST"], py="/v1/mods/upload", status=200, headers=[JSON], body=b"{}"),
    route(name="caps_git", methods=["POST"], py="/git/mods/x.git/git-receive-pack", status=200, headers=[JSON], body=b"{}"),
    route(name="caps_lb", methods=["POST"], py="/v1/leaderboards/insert", status=200, headers=[JSON], body=b"{}"),
    route(name="caps_drops", methods=["POST"], py="/site/drops/abc", status=200, headers=[JSON], body=b"{}"),
    route(name="caps_feedback", methods=["POST"], py="/v1/misc/feedback", status=200, headers=[JSON], body=b"{}"),
    route(name="caps_bp", methods=["POST"], py="/site/blueprint-editor/save", status=200, headers=[JSON], body=b"{}"),
    route(name="caps_unlock", methods=["POST"], py="/site/unlock-debug", status=200, headers=[JSON], body=b"{}"),
    route(name="put", methods=["PUT"], py="/__mw/put", status=200, headers=[JSON], body=b'{"put":1}'),
    route(name="dress_boom", methods=["GET"], py="/site/dressing/boom", kind="raise"),
    route(name="stats_any", methods=["GET"], py="/v1/stats/anything", status=200, headers=[JSON], body=b"{}"),
]


def build_harness() -> FastAPI:
    app = FastAPI()
    register_error_handlers(app)
    app.user_middleware = list(main.app.user_middleware)

    for r in ROUTES:
        def endpoint(request: Request, _r=r):
            if _r.get("kind") == "raise":
                raise RuntimeError("boom")
            raw = [(k.encode("latin-1"), v.encode("latin-1")) for k, v in _r["headers"]]
            if "chunks" in _r:
                async def gen(chunks=_r["chunks"]):
                    for c in chunks:
                        yield c
                resp = StreamingResponse(gen(), status_code=_r["status"])
                resp.raw_headers = [h for h in resp.raw_headers if h[0] != b"content-type"] + raw
                return resp
            resp = Response(content=_r["body"], status_code=_r["status"])
            resp.raw_headers = resp.raw_headers + raw
            return resp
        app.add_api_route(r["py"], endpoint, methods=r["methods"], name=r["name"])
    return app


async def drive(app, method: str, target: str, headers: list[list[str]], body: bytes = b"") -> dict:
    raw_path, _, qs = target.partition("?")
    path = unquote(raw_path) if "%" in raw_path else raw_path
    scope = {
        "type": "http", "asgi": {"version": "3.0", "spec_version": "2.4"}, "http_version": "1.1",
        "method": method, "scheme": "http", "path": path, "raw_path": raw_path.encode("ascii"),
        "query_string": qs.encode("ascii"), "root_path": "",
        "headers": [(k.lower().encode("latin-1"), v.encode("latin-1")) for k, v in headers],
        "client": ("10.0.0.9", 50000), "server": ("testserver", 80), "state": {},
    }
    done = asyncio.Event()
    sent = False
    start: dict = {}
    chunks: list[bytes] = []

    async def receive():
        nonlocal sent
        if not sent:
            sent = True
            return {"type": "http.request", "body": body, "more_body": False}
        await done.wait()
        return {"type": "http.disconnect"}

    async def send(msg):
        if msg["type"] == "http.response.start":
            start.update(msg)
        elif msg["type"] == "http.response.body":
            chunks.append(msg.get("body", b""))
            if not msg.get("more_body", False):
                done.set()

    await app(scope, receive, send)
    data = b"" if method == "HEAD" else b"".join(chunks)
    hdrs = [[k.decode("latin-1"), v.decode("latin-1")] for k, v in start["headers"]]
    if any(k == "content-encoding" and v == "gzip" for k, v in hdrs) and data[:2] == b"\x1f\x8b" and data[9:10] != b"\x03":
        data = data[:9] + b"\x03" + data[10:]  # zlib's OS byte as on Linux
    return {"status": start["status"], "response_headers": hdrs, "body_b64": base64.b64encode(data).decode()}


GZ = ["Accept-Encoding", "gzip, deflate, br"]
RID = ["X-Request-ID", "rid-fixed"]
DOCS = ["Origin", "https://docs.aallyn.net"]
AUTH = ["Authorization", "Bearer kiwi_abc"]
APP_JS_V = static_version.version("/static/app.js")


def idem_key(auth: str, method: str, path: str, key: str) -> str:
    import hashlib
    cred = hashlib.sha256(auth.encode()).hexdigest()[:16]
    return "idem:" + hashlib.sha256(f"{cred}:{method}:{path}:{key}".encode()).hexdigest()


CASES = [
    ("small", "GET", "/__mw/small", [RID]),
    ("small_new_rid", "GET", "/__mw/small", []),
    ("small_empty_rid", "GET", "/__mw/small", [["X-Request-ID", ""]]),
    ("small_gzip", "GET", "/__mw/small", [RID, GZ]),
    ("big_gzip", "GET", "/__mw/big", [RID, GZ]),
    ("big_gzip_upper", "GET", "/__mw/big", [RID, ["Accept-Encoding", "deflate, GZIP"]]),
    ("big_identity", "GET", "/__mw/big", [RID]),
    ("big_br_only", "GET", "/__mw/big", [RID, ["Accept-Encoding", "br"]]),
    ("html_gzip", "GET", "/__mw/html", [RID, GZ]),
    ("png_gzip", "GET", "/__mw/png", [RID, GZ]),
    ("ndjson_gzip", "GET", "/__mw/stream", [RID, GZ]),
    ("ndjson_plain", "GET", "/__mw/stream", [RID]),
    ("sse_gzip", "GET", "/__mw/sse", [RID, GZ, DOCS]),
    ("text_gzip", "GET", "/__mw/text", [RID, GZ]),
    ("nocontent_gzip", "GET", "/__mw/nocontent", [RID, GZ]),
    ("pregz_gzip", "GET", "/__mw/pregz", [RID, GZ]),
    ("partial_gzip", "GET", "/__mw/partial", [RID, GZ]),
    ("cached_anon", "GET", "/__mw/cached", [RID]),
    ("cached_bearer", "GET", "/__mw/cached", [RID, AUTH]),
    ("cached_basic", "GET", "/__mw/cached", [RID, ["Authorization", "Basic eDp5"]]),
    ("cached_empty_auth", "GET", "/__mw/cached", [RID, ["Authorization", ""]]),
    ("cached_cookie", "GET", "/__mw/cached", [RID, ["Cookie", "a=1; kiwi_site_access=tok"]]),
    ("cached_cookie_blank", "GET", "/__mw/cached", [RID, ["Cookie", "kiwi_site_access="]]),
    ("cached_cookie_bare", "GET", "/__mw/cached", [RID, ["Cookie", "kiwi_site_access"]]),
    ("cached_other_cookie", "GET", "/__mw/cached", [RID, ["Cookie", "kiwi_site_accessx=1"]]),
    ("cached_all", "GET", "/__mw/cached", [RID, AUTH, DOCS, GZ]),
    ("own_headers", "GET", "/__mw/own", [RID]),
    ("cors_docs", "GET", "/__mw/small", [RID, DOCS]),
    ("cors_evil", "GET", "/__mw/small", [RID, ["Origin", "https://evil.example"]]),
    ("cors_sub", "GET", "/__mw/small", [RID, ["Origin", "https://trove.aallyn.net"]]),
    ("cors_localhost", "GET", "/__mw/small", [RID, ["Origin", "http://localhost:9999"]]),
    ("cors_suffix_trick", "GET", "/__mw/small", [RID, ["Origin", "https://aallyn.net.evil.com"]]),
    ("cors_empty_origin", "GET", "/__mw/small", [RID, ["Origin", ""]]),
    ("preflight_ok", "OPTIONS", "/__mw/echo",
     [DOCS, ["Access-Control-Request-Method", "POST"], ["Access-Control-Request-Headers", "content-type, authorization"]]),
    ("preflight_no_headers", "OPTIONS", "/__mw/echo", [DOCS, ["Access-Control-Request-Method", "GET"]]),
    ("preflight_evil", "OPTIONS", "/__mw/echo", [["Origin", "https://evil.example"], ["Access-Control-Request-Method", "POST"]]),
    ("preflight_bad_method", "OPTIONS", "/__mw/echo", [DOCS, ["Access-Control-Request-Method", "post"]]),
    ("preflight_evil_bad_method", "OPTIONS", "/__mw/echo", [["Origin", "null"], ["Access-Control-Request-Method", "FOO"]]),
    ("preflight_private_net", "OPTIONS", "/__mw/echo",
     [DOCS, ["Access-Control-Request-Method", "POST"], ["Access-Control-Request-Private-Network", "true"]]),
    ("preflight_gzip", "OPTIONS", "/__mw/echo", [DOCS, ["Access-Control-Request-Method", "POST"], GZ]),
    ("options_no_acrm", "OPTIONS", "/__mw/small", [RID, DOCS]),
    ("options_no_origin", "OPTIONS", "/__mw/small", [RID]),
    ("method_not_allowed", "DELETE", "/__mw/small", [RID]),
    ("method_not_allowed_post", "GET", "/__mw/echo", [RID, DOCS]),
    ("rig_partner", "GET", "/site/rigs/a", [RID, ["Origin", "https://partner.example"]]),
    ("rig_partner_gzip", "GET", "/site/rigs/a", [RID, ["Origin", "https://partner.example"], GZ]),
    ("rig_own_origin", "GET", "/site/rigs/a", [RID, ["Origin", "https://trove.aallyn.net"]]),
    ("rig_no_origin", "GET", "/site/rigs/a", [RID]),
    ("rig_auth", "GET", "/site/rigs/a", [RID, AUTH]),
    ("rig_preflight_partner", "OPTIONS", "/site/rigs/a",
     [["Origin", "https://partner.example"], ["Access-Control-Request-Method", "GET"]]),
    ("rig_options_bare", "OPTIONS", "/site/rigs/a", []),
    ("rig_preflight_own", "OPTIONS", "/site/rigs/a", [DOCS, ["Access-Control-Request-Method", "GET"]]),
    ("site_anon", "GET", "/site/data", [RID]),
    ("site_auth", "GET", "/site/data", [RID, AUTH]),
    ("page_login", "GET", "/login", [RID]),
    ("page_mod", "GET", "/mods/foo", [RID]),
    ("page_login_auth", "GET", "/login", [RID, ["Cookie", "kiwi_site_access=x"]]),
    ("redirect_api", "GET", "/login", [RID, ["Host", "api.aallyn.net"]]),
    ("redirect_apex_query", "GET", "/mods/foo?x=1&y=%20z", [RID, ["Host", "aallyn.net"]]),
    ("redirect_www_port", "GET", "/login", [RID, ["Host", "www.aallyn.net:443"]]),
    ("redirect_upper", "GET", "/login", [RID, ["Host", "API.AALLYN.NET"], DOCS]),
    ("redirect_space", "GET", "/mods/a%20b%C3%BC", [RID, ["Host", "api.aallyn.net"]]),
    ("redirect_head", "HEAD", "/login", [RID, ["Host", "api.aallyn.net"]]),
    ("redirect_post", "POST", "/login", [RID, ["Host", "api.aallyn.net"]]),
    ("redirect_trove_host", "GET", "/login", [RID, ["Host", "trove.aallyn.net"]]),
    ("redirect_not_page", "GET", "/__mw/small", [RID, ["Host", "api.aallyn.net"]]),
    ("cap_default", "POST", "/__mw/echo", [RID, ["Content-Length", "9000000"]]),
    ("cap_default_cors", "POST", "/__mw/echo", [RID, ["Content-Length", "9000000"], DOCS, GZ]),
    ("cap_at_limit", "POST", "/__mw/echo", [RID, ["Content-Length", "8388608"]]),
    ("cap_garbage", "POST", "/__mw/echo", [RID, ["Content-Length", "12abc"]]),
    ("cap_underscore", "POST", "/__mw/echo", [RID, ["Content-Length", "9_000_000"]]),
    ("cap_mods", "POST", "/v1/mods/upload", [RID, ["Content-Length", "20971521"]]),
    ("cap_mods_ok", "POST", "/v1/mods/upload", [RID, ["Content-Length", "20971520"]]),
    ("cap_git", "POST", "/git/mods/x.git/git-receive-pack", [RID, ["Content-Length", "104857601"]]),
    ("cap_lb", "POST", "/v1/leaderboards/insert", [RID, ["Content-Length", "20971521"]]),
    ("cap_drops", "POST", "/site/drops/abc", [RID, ["Content-Length", "536870913"]]),
    ("cap_feedback", "POST", "/v1/misc/feedback", [RID, ["Content-Length", "25165825"]]),
    ("cap_bp", "POST", "/site/blueprint-editor/save", [RID, ["Content-Length", "67108865"]]),
    ("cap_unlock", "POST", "/site/unlock-debug", [RID, ["Content-Length", "167772161"]]),
    ("cap_rig_auth", "POST", "/site/rigs/a", [RID, ["Content-Length", "9000000"], ["Origin", "https://partner.example"]]),
    ("embed", "GET", "/embed/viewer", [RID]),
    ("embed_auth", "GET", "/embed/viewer", [RID, AUTH]),
    ("embed_sub", "GET", "/embed/viewer/x", [RID]),
    ("static_versioned", "GET", f"/static/app.js?v={APP_JS_V}", [RID, AUTH]),
    ("static_versioned_last", "GET", f"/static/app.js?v=nope&v={APP_JS_V}", [RID]),
    ("static_wrong_v", "GET", "/static/app.js?v=0000000000", [RID]),
    ("static_font", "GET", "/static/fonts/missing.woff2", [RID]),
    ("static_escape", "GET", "/static/../README.md?v=x", [RID]),
    ("head_big", "HEAD", "/__mw/big", [RID]),
    ("head_big_gzip", "HEAD", "/__mw/big", [RID, GZ]),
    ("head_events", "HEAD", "/v1/events/stream", [RID]),
    ("head_post_only", "HEAD", "/__mw/echo", [RID]),
    ("boom", "GET", "/__mw/boom", [RID, DOCS]),
    ("boom_public_asset", "GET", "/site/dressing/boom", [RID]),
    ("health", "GET", "/health", [RID, GZ, DOCS]),
]

IDEM_CASES = [
    # (name, method, path, headers, redis_before)
    ("idem_first", "POST", "/__mw/echo", [RID, AUTH, ["Idempotency-Key", "k1"]], None),
    ("idem_replay", "POST", "/__mw/echo", [RID, AUTH, ["Idempotency-Key", "k1"], DOCS], "keep"),
    ("idem_other_cred", "POST", "/__mw/echo", [RID, ["Idempotency-Key", "k1"]], "keep"),
    ("idem_fail_released", "POST", "/__mw/fail", [RID, AUTH, ["Idempotency-Key", "k2"]], {}),
    ("idem_in_flight", "POST", "/__mw/echo", [RID, AUTH, ["Idempotency-Key", "k3"]],
     {idem_key("Bearer kiwi_abc", "POST", "/__mw/echo", "k3"): ['{"s":"lock"}', 60]}),
    ("idem_compact_record", "POST", "/__mw/echo", [RID, ["Idempotency-Key", "k4"]],
     {idem_key("", "POST", "/__mw/echo", "k4"): ['{"s":"done","c":202,"ct":"text/plain","b":"aGk="}', 86400]}),
    ("idem_null_ct", "POST", "/__mw/echo", [RID, ["Idempotency-Key", "k5"]],
     {idem_key("", "POST", "/__mw/echo", "k5"): ['{"s": "done", "c": 200, "ct": null, "b": ""}', 86400]}),
    ("idem_corrupt", "POST", "/__mw/echo", [RID, ["Idempotency-Key", "k6"]],
     {idem_key("", "POST", "/__mw/echo", "k6"): ["not json", 86400]}),
    ("idem_get_ignored", "GET", "/__mw/small", [RID, ["Idempotency-Key", "k7"]], {}),
    ("idem_long_key", "POST", "/__mw/echo", [RID, ["Idempotency-Key", "x" * 201]], {}),
    ("idem_max_key", "POST", "/__mw/echo", [RID, ["Idempotency-Key", "y" * 200]], {}),
    ("idem_put", "PUT", "/__mw/put", [RID, ["Idempotency-Key", "k8"], ["Authorization", "Bearer été"]], {}),
    ("idem_cookies", "POST", "/__mw/cookies", [RID, ["Idempotency-Key", "k9"]], {}),
    ("idem_cookies_replay", "POST", "/__mw/cookies", [RID, ["Idempotency-Key", "k9"]], "keep"),
]

REAL_CASES = [
    ("real_health", "GET", "/health", [RID, DOCS]),
    ("real_preflight", "OPTIONS", "/v1/stats/anything",
     [DOCS, ["Access-Control-Request-Method", "GET"], ["Access-Control-Request-Headers", "authorization"]]),
    ("real_cap_mods", "POST", "/v1/mods/upload", [RID, ["Content-Length", "20971521"]]),
    ("real_redirect", "GET", "/login?a=b", [RID, ["Host", "api.aallyn.net"]]),
    ("real_head_events", "HEAD", "/v1/events/stream", [RID]),
    ("real_rig_options", "OPTIONS", "/site/rigs/whatever", [["Origin", "https://partner.example"]]),
]


async def run() -> dict:
    harness = build_harness()
    out_cases = []

    async def one(app, name, method, target, headers, kind):
        res = await drive(app, method, target, headers)
        out_cases.append({"name": name, "app": kind, "method": method, "path": target,
                          "request_headers": headers, **res,
                          "redis_after": {k: [v, ex] for k, (v, ex) in sorted(REDIS.store.items())}})

    for name, method, target, headers in CASES:
        REDIS.store.clear()
        await one(harness, name, method, target, headers, "harness")
    REDIS.store.clear()
    for name, method, target, headers, before in IDEM_CASES:
        if before != "keep":
            REDIS.store.clear()
            for k, (v, ex) in (before or {}).items():
                REDIS.store[k] = (v, ex)
        seed = {k: [v, ex] for k, (v, ex) in sorted(REDIS.store.items())}
        await one(harness, name, method, target, headers, "harness")
        out_cases[-1]["redis_before"] = seed
    for name, method, target, headers in REAL_CASES:
        REDIS.store.clear()
        await one(main.app, name, method, target, headers, "real")
        out_cases[-1]["redis_before"] = {}

    routes = []
    for r in ROUTES:
        e = {"name": r["name"], "methods": r["methods"], "path": r.get("rs", r["py"]),
             "kind": r.get("kind", "stream" if "chunks" in r else "full")}
        if e["kind"] != "raise":
            e["status"] = r["status"]
            e["headers"] = [list(h) for h in r["headers"]]
        if "body" in r:
            e["body_b64"] = base64.b64encode(r["body"]).decode()
        if "chunks" in r:
            e["chunks_b64"] = [base64.b64encode(c).decode() for c in r["chunks"]]
        routes.append(e)

    constants = {
        "api_csp": csp.API_CSP,
        "site_csp": csp.SITE_CSP,
        "embed_csp": csp.embed_csp(EMBED_ORIGINS),
        "embed_csp_none": csp.embed_csp([]),
        "api_side_hosts": sorted(API_SIDE_HOSTS),
        "embed_origins": EMBED_ORIGINS,
        "parse_origins": {
            raw: embed_service._parse_origins(raw)
            for raw in ["", "https://a.com https://a.com/", "https://*.b.org,http://c.net:8080 ftp://x",
                        "HTTPS://UP.COM\thttps://bad_host.com https://ok.com/path", "https://ok.com\x1fhttps://two.com"]
        },
        "idem_keys": [
            [a, m, p, k, idem_key(a, m, p, k)]
            for a, m, p, k in [("", "POST", "/v1/x", "k"), ("Bearer t", "DELETE", "/v1/a b", "é")]
        ],
    }
    return {"routes": routes, "cases": out_cases, "constants": constants}


if __name__ == "__main__":
    data = asyncio.run(run())
    OUT.write_text(json.dumps(data, indent=1, ensure_ascii=False) + "\n", encoding="utf-8", newline="\n")
    print(f"wrote {len(data['cases'])} cases, {len(data['routes'])} routes -> {OUT.relative_to(ROOT)}")
