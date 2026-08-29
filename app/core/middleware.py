import gzip
import re
import zlib

from fastapi import FastAPI, Request
from starlette.datastructures import Headers, MutableHeaders
from starlette.middleware.gzip import GZipMiddleware
from starlette.responses import JSONResponse, RedirectResponse, Response
from starlette.types import ASGIApp, Message, Receive, Scope, Send

from app.core.config import settings
from app.core.csp import API_CSP as _API_CSP
from app.core.csp import SITE_CSP as _SITE_CSP
from app.core.csp import embed_csp as _embed_csp
from app.core.errors import ErrorCode, build_error_body

# The embeddable viewer (app/embed) is the ONE surface that may be framed by another
# site. Its CSP is the site CSP with `frame-ancestors` swapped for the admin's
# allowlist (`embed.allowed_origins`) - and, because a legacy `X-Frame-Options: DENY`
# would veto that in every browser regardless of CSP, that header is dropped here too.
# Empty allowlist -> stays `frame-ancestors 'none'`: nobody can embed it until an
# origin is added, and the page still works when opened directly.
#
# Matched EXACTLY, not as a prefix. `/embed/status.svg` is the status badge (an
# <img> on other sites) and has no business carrying a frame-ancestors list, and
# `/embed/viewer/anything-else` is a 404 that should get the ordinary locked-down
# CSP. The edge proxy matches this same path exactly; keeping the two in step means
# there is no path where the app relaxes and the proxy doesn't, or vice versa.
_EMBED_PATH = "/embed/viewer"


# Exact-matched showcase-site HTML page routes. (Everything else under the site
# is the /static/* asset mount or the /site/* JSON proxies.)
_PAGE_PATHS = frozenset({
    "/", "/app", "/browse", "/documentation", "/commands", "/leaderboards", "/updates",
    "/support", "/login", "/dashboard", "/market", "/store", "/codexes", "/codexes/crafting",
    "/status", "/giveaways",
    "/activity", "/class-activity", "/clubs", "/terms", "/privacy", "/accessibility", "/changelog", "/mods", "/modpacks",
    "/server-time", "/swf-docs", "/calendar", "/streams", "/releases", "/classes",
    "/star-chart", "/gem-simulator", "/gem-evaluator", "/gem-builds", "/calculators",
    "/gems-guide", "/dressing-room", "/sound-studio", "/mod-workshop", "/tomes",
    "/blueprint-editor", "/unlock-debug", "/unlock_debug",
    "/search",
})

# Dynamic site page subtrees (parameterised routes like /mods/{slug},
# /modpacks/{handle}/{slug} and /player/{name}). Matched by prefix so the slug/name
# page gets the relaxed site CSP + no-cache, same as the bare listing pages above.
_PAGE_PREFIXES = ("/mods/", "/modpacks/", "/player/")


def _is_site_path(path: str) -> bool:
    """Showcase-site routes (everything served from `site_router` in app/site/).

    Page routes are exact-matched; ``/static/*``, ``/site/*`` and the dynamic
    page subtrees in ``_PAGE_PREFIXES`` get the relaxed CSP wholesale, so newly-
    added JSON proxies and assets don't need to be enumerated one-by-one as new
    pages land. Forgetting one of these wires up an API-CSP page that refuses to
    apply its own stylesheets - see /updates regression in 2026-06.
    """
    return (
        path in _PAGE_PATHS
        or path.startswith("/static/")
        or path.startswith("/site/")
        or path.startswith(_PAGE_PREFIXES)
    )


def add_security_middleware(app: FastAPI) -> None:
    """Reject oversized bodies early and attach security headers to every response."""
    default_max_body = settings.max_request_body_bytes
    mods_max_body = settings.mods_max_request_body_bytes
    leaderboards_max_body = settings.leaderboards_max_request_body_bytes
    market_max_body = settings.market_max_request_body_bytes
    ocr_max_body = settings.ocr_max_request_body_bytes
    git_max_body = settings.mods_git_max_body_bytes

    @app.middleware("http")
    async def security(request: Request, call_next):
        path = request.url.path
        # Per-surface body caps: mod tools accept .tmod uploads, the
        # leaderboards + market ingests accept the bot's raw cfg dumps,
        # everything else uses the default.
        if path.startswith("/git/"):
            max_body = git_max_body          # git push packfiles
        elif path.startswith("/v1/mods/"):
            max_body = mods_max_body
        elif path == "/v1/leaderboards/insert":
            max_body = leaderboards_max_body
        elif path == "/v1/market/insert":
            max_body = market_max_body
        elif path == "/v1/ocr/character":
            max_body = ocr_max_body
        elif path == "/site/sound-studio/build":
            # Replacement audio arrives as raw PCM (the browser decodes whatever
            # the user picked and sends samples), which is bulky by nature. The
            # endpoint caps each clip and the total itself.
            max_body = settings.sound_studio_max_request_body_bytes
        elif path.startswith("/site/blueprint-editor/"):
            # The blueprint itself is small (the biggest in the game is 515 KB), but a
            # save also carries the edit list, and repainting every voxel of a large
            # model is one JSON entry each - which outgrows the default cap on its own.
            # A .qb import is bulkier still: four grids rather than one packed model.
            max_body = 64 * 1024 * 1024
        elif path == "/site/unlock-debug":
            # A whole game executable arrives so seven bytes of it can change.
            max_body = settings.unlock_debug_max_request_body_bytes
        elif path.startswith("/site/mod-workshop/"):
            # A whole mod is uploaded in one request (loose files, a .zip, or a
            # .tmod being repaired). The unpacked-size cap lives in workshop.py.
            max_body = settings.mod_workshop_max_request_body_bytes
        elif path == "/v1/misc/feedback":
            # 4 attachments × 5 MB + form fields. The endpoint also caps
            # per-file size + count itself, so this is a generous gate
            # that lets a valid 22 MB submission through.
            max_body = 24 * 1024 * 1024
        else:
            max_body = default_max_body
        content_length = request.headers.get("content-length")
        if content_length is not None:
            try:
                too_big = int(content_length) > max_body
            except ValueError:
                too_big = False
            if too_big:
                return JSONResponse(
                    status_code=413,
                    content=build_error_body(
                        ErrorCode.bad_request.value,
                        f"Request body exceeds the {max_body}-byte limit",
                    ),
                )

        response: Response = await call_next(request)
        h = response.headers
        h.setdefault("X-Content-Type-Options", "nosniff")
        h.setdefault("Referrer-Policy", "strict-origin-when-cross-origin")
        h.setdefault("Strict-Transport-Security", "max-age=31536000; includeSubDomains")
        h.setdefault("Permissions-Policy", "geolocation=(), microphone=(), camera=()")
        if path == _EMBED_PATH:
            # The one framable surface: CSP carries the allowlist and X-Frame-Options
            # must be ABSENT (a legacy DENY beats frame-ancestors in every browser -
            # Firefox reports it as NS_ERROR_XFO_VIOLATION).
            # Imported here, not at module scope: app.embed.service pulls in the whole
            # Mods Hub service chain, and middleware is imported by everything.
            from app.embed.service import allowed_origins
            # Deleted, not merely un-set: an inner layer may have added one, and this
            # makes the ORIGIN's answer unambiguous when debugging a proxy that injects
            # its own (nginx `add_header`, a Cloudflare managed transform).
            del h["X-Frame-Options"]
            h.setdefault("Content-Security-Policy", _embed_csp(await allowed_origins()))
            h.setdefault("Cache-Control", "no-cache")
            return response
        h.setdefault("X-Frame-Options", "DENY")
        # The showcase site pulls FontAwesome / GSAP from CDN and needs a looser CSP.
        h.setdefault("Content-Security-Policy", _SITE_CSP if _is_site_path(path) else _API_CSP)
        # Static assets + HTML pages always revalidate, so a deploy is picked up
        # immediately without ``?v=`` cache-busting. StaticFiles' ETag /
        # Last-Modified keep this to cheap 304s. The /site/ JSON proxies set
        # their own max-age and are intentionally excluded.
        if path.startswith("/static/") or path in _PAGE_PATHS or path.startswith(_PAGE_PREFIXES):
            h.setdefault("Cache-Control", "no-cache")
        # A response produced for a CREDENTIAL is per-user and must never be
        # stored by a shared cache. Enforced here, not per-route, for two
        # reasons: a handler cannot forget it, and a handler cannot get it wrong
        # (a viewer-dependent body was being returned as `public, max-age=30`).
        # This deliberately OVERWRITES whatever the route set - the edge keys its
        # cache on the URL and does not see the cookie, so "public" on any path
        # that reads a session is a cross-user leak waiting to happen.
        if _has_credential(request) and not _is_public_asset(path):
            h["Cache-Control"] = "private, no-store"
            _add_vary(h, "Cookie", "Authorization")
        return response


def _has_credential(request: Request) -> bool:
    """Whether the CALLER presented anything that could make this response
    user-specific. Checked on the request, not the response, so it also covers a
    route that reads the session and then happens to return an identical body."""
    if request.headers.get("authorization"):
        return True
    # Named literally rather than imported: app.site_auth pulls in the Beanie
    # models, and middleware is imported by everything. Kept in step with
    # app/site_auth/cookies.py ACCESS_COOKIE.
    return "kiwi_site_access" in request.cookies


def _add_vary(headers, *fields: str) -> None:
    """Merge into any existing Vary (GZipMiddleware sets Accept-Encoding)."""
    have = {v.strip().lower() for v in headers.get("Vary", "").split(",") if v.strip()}
    merged = [f for f in fields if f.lower() not in have]
    if merged:
        existing = headers.get("Vary")
        headers["Vary"] = f"{existing}, {', '.join(merged)}" if existing else ", ".join(merged)


# The assets the 3D viewers fetch while they run: baked rigs (clips + animation
# graph), the dressing-room catalogue and assembled models, and the BRDF lighting
# map. Every one is public, tokenless and takes no viewer identity - no cookie, no
# Authorization header, pure query params - which is what makes them safe to hand to
# any origin.
_PUBLIC_ASSET_PREFIXES = ("/site/rigs/", "/site/dressing/")
_PUBLIC_ASSET_PATHS = frozenset({"/site/render/brdf-map.png"})


def _is_public_asset(path: str) -> bool:
    return path.startswith(_PUBLIC_ASSET_PREFIXES) or path in _PUBLIC_ASSET_PATHS


# Same test the credentialed CORS layer applies (Starlette matches the regex with
# fullmatch), so the two layers cannot disagree about who owns an origin.
_ORIGIN_RE = re.compile(settings.cors_origin_regex) if settings.cors_origin_regex else None


def _is_own_origin(origin: str) -> bool:
    if not origin:
        return False
    return origin in settings.cors_origins or bool(_ORIGIN_RE and _ORIGIN_RE.fullmatch(origin))


class PublicAssetCorsMiddleware:
    """Let ANY origin read the viewer's assets, without credentials.

    The ordinary CORS layer runs ``allow_credentials=True`` against an allowlist, so
    everything it permits can also make cookie-bearing calls and read a signed-in
    user's responses. That is the right policy for our own front-ends and the wrong
    one to hand a partner site that only needs geometry: a page on their domain would
    inherit the visitor's session.

    So these paths get their own answer - ``Access-Control-Allow-Origin: *`` with no
    credentials. A partner points the viewer's ``apiBase`` at the API and it works,
    with no per-partner allowlist to maintain and nothing new exposed: the browser
    refuses to attach cookies to a wildcard origin, and these routes never read them
    anyway. They are rate-limited per IP as tokenless endpoints either way.

    Registered LAST in main.py so it sits outside the credentialed CORS middleware and
    can replace what that one decided. Both layers emitting an allow-origin header
    would be two of them, which every browser treats as no CORS at all.

    OUR OWN origins are handed straight through instead. The site's fetch wrapper
    (`_site_util.js`) rewrites every `/site/*` call to the API host and sets
    `credentials: 'include'` so the session cookie survives the hop - and a browser
    rejects a wildcard allow-origin outright on a credentialed request. Substituting
    `*` here therefore took the dressing room down on our own front-end while working
    perfectly for the partner it was written for. A partner is, by definition, an
    origin the credentialed allowlist does NOT cover, so deciding on that one test
    serves both: allowlisted origins keep the credentialed answer, everyone else gets
    the wildcard.
    """

    _STRIP = frozenset({b"access-control-allow-origin", b"access-control-allow-credentials",
                        b"access-control-expose-headers"})
    _PREFLIGHT = (
        (b"access-control-allow-origin", b"*"),
        (b"access-control-allow-methods", b"GET, HEAD, OPTIONS"),
        (b"access-control-allow-headers", b"*"),
        (b"access-control-max-age", b"86400"),
    )

    def __init__(self, app: ASGIApp) -> None:
        self.app = app

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] != "http" or not _is_public_asset(scope.get("path", "")):
            await self.app(scope, receive, send)
            return
        origin = next((v for k, v in scope.get("headers", ()) if k == b"origin"), b"")
        if _is_own_origin(origin.decode("latin-1")):
            await self.app(scope, receive, send)       # credentialed layer owns this one
            return
        if scope["method"] == "OPTIONS":
            # a plain GET is never preflighted; this is for a partner that adds a
            # header of its own and turns it into one
            headers = {k.decode(): v.decode() for k, v in self._PREFLIGHT}
            await Response(status_code=204, headers=headers)(scope, receive, send)
            return

        async def send_wrapper(message: dict) -> None:
            if message["type"] == "http.response.start":
                vary = b", ".join(v for k, v in message["headers"] if k.lower() == b"vary")
                if b"origin" not in vary.lower():
                    vary = vary + b", Origin" if vary else b"Origin"
                headers = [(k, v) for k, v in message["headers"]
                           if k.lower() not in self._STRIP and k.lower() != b"vary"]
                # This layer answers our own origins and everybody else differently, so
                # any shared cache in front of it (the edge proxy, Cloudflare) has to
                # key on the origin or it hands one of them the other's answer - and a
                # cached `*` breaks our own credentialed fetch of the very same file.
                headers.append((b"vary", vary))
                headers.append((b"access-control-allow-origin", b"*"))
                # a cross-origin caller cannot read a header it is not handed: without
                # this the dressing room's "your hat isn't on this model" is invisible
                # to exactly the partner it was written for
                headers.append((b"access-control-expose-headers", b"X-Dressing-Dropped"))
                message = {**message, "headers": headers}
            await send(message)

        await self.app(scope, receive, send_wrapper)


def add_public_asset_cors_middleware(app: FastAPI) -> None:
    """Open the viewer's public assets to every origin (see PublicAssetCorsMiddleware).

    Register LAST in main.py, so it is the outermost layer and its answer is the one
    that reaches the browser."""
    app.add_middleware(PublicAssetCorsMiddleware)


class HeadMethodMiddleware:
    """Serve ``HEAD`` by running the matching ``GET`` route, body suppressed.

    FastAPI's ``APIRoute`` registers only the literal method passed to
    ``@router.get`` - unlike Starlette's plain ``Route`` it does NOT pair GET
    with HEAD - so every showcase page (plus ``/robots.txt`` and
    ``/sitemap.xml``) answers a bare ``HEAD`` with ``405 Method Not Allowed``.
    Crawlers, link-checkers and uptime monitors that probe with HEAD then read
    the URL as broken.

    Flipping the method to GET *for the app's routing only* fixes this: uvicorn
    keeps its own scope (still HEAD) and so already drops the response body and
    frames the correct ``Content-Length`` per RFC 9110 §9.3.2. We therefore copy
    the scope and never mutate it in place - mutating it would flip uvicorn's own
    method too, and it would then try to write the full GET body onto a HEAD
    response.

    Registered innermost (added first in main.py), so the pageview + usage
    middleware still observe the real HEAD and skip their GET-only accounting.
    ``/v1/events`` is excluded: rewriting a HEAD there to GET would open an
    unbounded SSE stream that never completes.
    """

    _EXCLUDE_PREFIXES = ("/v1/events",)

    def __init__(self, app: ASGIApp) -> None:
        self.app = app

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if (
            scope["type"] == "http"
            and scope["method"] == "HEAD"
            and not scope["path"].startswith(self._EXCLUDE_PREFIXES)
        ):
            scope = {**scope, "method": "GET"}
        await self.app(scope, receive, send)


def add_head_method_middleware(app: FastAPI) -> None:
    """Answer HEAD requests via the matching GET route (see HeadMethodMiddleware).

    Register FIRST in main.py so it sits innermost - just outside routing but
    inside the analytics middleware, which gate on the real (HEAD) method.
    """
    app.add_middleware(HeadMethodMiddleware)


class StaticCompressionMiddleware:
    """gzip the text assets under ``/static``, which nothing else compresses.

    The edge proxy runs nginx's stock ``gzip_types``, i.e. ``text/html`` only, so
    pages arrived compressed while every stylesheet, script and locale file went
    out raw - 53 KB of ``style.min.css`` and a ~300 KB locale JSON per non-English
    page load, all of it on the render-blocking path.

    Scoped by extension rather than handed the whole app: woff2, png and webp are
    already compressed (gzipping them burns CPU to add bytes), and range requests
    on them would break under a body rewrite. Streaming endpoints - ``/v1/events``
    above all - never match, so nothing here can stall an SSE connection.
    """

    _SUFFIXES = (".css", ".js", ".json", ".svg", ".txt", ".map", ".md")

    def __init__(self, app: ASGIApp) -> None:
        self.app = app
        self.gzip = GZipMiddleware(app, minimum_size=1024)

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] == "http" and scope["path"].endswith(self._SUFFIXES):
            await self.gzip(scope, receive, send)
            return
        await self.app(scope, receive, send)


def add_static_compression_middleware(app: FastAPI) -> None:
    """gzip served text assets (see StaticCompressionMiddleware).

    Register LAST in main.py so it sits outermost and compresses the finished
    response, security headers and all.
    """
    app.add_middleware(StaticCompressionMiddleware)


def _api_side_hosts() -> frozenset[str]:
    """Hosts on which a showcase page is a DUPLICATE of its canonical ``app_url`` copy.

    The one FastAPI app answers on every hostname the proxy sends it, and the proxy
    points the API host, the apex AND its ``www`` at this same container - so
    api.aallyn.net/login, aallyn.net/login and www.aallyn.net/login all serve the page
    whose real home is trove.aallyn.net/login. All three redirect (see
    ``add_api_host_redirect_middleware``) and all three must stay crawlable (see
    ``robots_body``) so Google can follow the 301 and drop the stale entry.

    The apex is derived rather than configured: a host with a subdomain to strip
    (``api.example.net``) yields ``example.net`` + ``www.example.net``; a bare host
    (``localhost``) yields nothing extra. The app host is always excluded - including
    it would point the canonical page at itself and loop.
    """
    host = settings.api_url.split("://", 1)[-1].split("/", 1)[0].lower()
    app_host = settings.app_url.split("://", 1)[-1].split("/", 1)[0].lower()
    hosts = {host}
    labels = host.split(".")
    if len(labels) > 2:
        apex = ".".join(labels[1:])
        hosts |= {apex, f"www.{apex}"}
    return frozenset(hosts - {app_host})


API_SIDE_HOSTS = _api_side_hosts()


def _is_site_page(path: str) -> bool:
    """A showcase-site HTML *page* - one in ``_PAGE_PATHS`` or a dynamic page
    subtree. These are the routes with a canonical home on ``app_url``.

    Deliberately narrower than ``_is_site_path``: it excludes ``/static`` and
    ``/site`` (assets + JSON proxies) and every api-native path (/v1, /health,
    /api-info, /openapi.json, /robots.txt), so only real indexable pages redirect.
    """
    return path in _PAGE_PATHS or path.startswith(_PAGE_PREFIXES)


def add_api_host_redirect_middleware(app: FastAPI) -> None:
    """301 showcase-site pages served on an api-side host to their canonical app_url home.

    api.aallyn.net/login serves the same page as trove.aallyn.net/login, and Google
    indexed the api copy. robots.txt ``Disallow: /`` on the api host does NOT deindex
    it - it only blocks the re-crawl that would let Google see the page's
    canonical/noindex - so the already-indexed URL is frozen there. A hard 301 to the
    app host is what actually drops it and consolidates the signal onto one URL.

    Applies to every host in ``API_SIDE_HOSTS`` - the api host AND the apex/www, which
    the proxy also points at this container. Until the apex was included it kept
    serving the page routes for real, which is why the duplicate handlers in
    ``app/site/router.py`` could not be deleted.

    GET/HEAD only: page routes are GET, and a 301 must never silently turn a POST into
    a GET. The JSON API, /api-info, robots.txt and static assets are untouched.
    """
    app_url = settings.app_url.rstrip("/")

    @app.middleware("http")
    async def api_host_redirect(request: Request, call_next):
        if (
            request.method in ("GET", "HEAD")
            and (request.url.hostname or "").lower() in API_SIDE_HOSTS
            and _is_site_page(request.url.path)
        ):
            target = app_url + request.url.path
            if request.url.query:
                target = f"{target}?{request.url.query}"
            return RedirectResponse(target, status_code=301)
        return await call_next(request)


# ── response compression (JSON) ────────────────────────────────────────────
# Content types worth gzipping. Chosen off the RESPONSE's content-type rather
# than the request path because /v1 and /site serve JSON and raw game blobs out
# of the same prefixes, and only the former compresses usefully. `text/event-stream`
# is deliberately absent: the live event stream must reach the client unbuffered.
_COMPRESSIBLE_TYPES = frozenset({
    "application/javascript",
    "application/json",
    "application/manifest+json",
    "application/x-ndjson",
    "application/xml",
    "image/svg+xml",
    "text/css",
    "text/csv",
    "text/html",
    "text/javascript",
    "text/markdown",
    "text/plain",
    "text/xml",
})
# Below this a gzip frame costs more than it saves, and the CPU is pure waste.
_COMPRESS_MIN_BYTES = 1024
# Level 5, not gzip's default 9: these bodies are compressed on the event loop,
# and on a multi-megabyte tree listing level 9 costs several times the CPU of
# level 5 for a couple of percent of size. See the warmer's event-loop note.
_COMPRESS_LEVEL = 5


def _accepts_gzip(scope: Scope) -> bool:
    for key, value in scope.get("headers", []):
        if key == b"accept-encoding":
            return b"gzip" in value.lower()
    return False


def _is_compressible(message: Message) -> bool:
    """Whether this ``http.response.start`` describes a body we should gzip."""
    status = message["status"]
    # 204/304 have no body; 206 is a byte range whose offsets a re-encode
    # would invalidate; errors are small enough not to bother.
    if status < 200 or status in (204, 206, 304):
        return False
    headers = Headers(raw=message["headers"])
    # Already encoded - notably the pre-gzipped blueprint/tmod cache, which
    # hands out stored gzip bytes verbatim. Never double-compress.
    if "content-encoding" in headers:
        return False
    if headers.get("content-range"):
        return False
    media_type = headers.get("content-type", "").split(";")[0].strip().lower()
    if media_type not in _COMPRESSIBLE_TYPES:
        return False
    # A declared length under the floor settles it without buffering anything.
    length = headers.get("content-length")
    return not (length is not None and length.isdigit() and int(length) < _COMPRESS_MIN_BYTES)


class ResponseCompressionMiddleware:
    """gzip JSON (and other text) responses that leave the API.

    The edge proxy runs nginx's stock ``gzip_types``, i.e. ``text/html`` only, so
    every JSON body went out raw. One ``/updates`` folder listing is 8.6 MB of
    extremely repetitive JSON - the wire time for it dwarfed the ~0.5 s the server
    spent building it, and that is what "fetching updates is slow" was.

    A fully-materialised response (the JSON case) is compressed in one shot and
    keeps an exact ``Content-Length``, so the edge cache still stores a sized
    entry. A response that arrives in several chunks is compressed incrementally
    and goes out chunked - so nothing is ever buffered whole here, and a stream
    cannot be stalled by this layer even if it does carry a compressible type.
    """

    def __init__(self, app: ASGIApp) -> None:
        self.app = app

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] != "http" or not _accepts_gzip(scope):
            await self.app(scope, receive, send)
            return
        await _GzipResponder(self.app, send)(scope, receive)


class _GzipResponder:
    """Per-request state for ``ResponseCompressionMiddleware``.

    ``http.response.start`` is held back until the first body chunk arrives: only
    then do we know whether the body is complete (compress in one shot, exact
    Content-Length) or streamed (compress incrementally, drop Content-Length).
    """

    def __init__(self, app: ASGIApp, send: Send) -> None:
        self.app = app
        self.send = send
        self.start: Message | None = None
        self.compress = False
        self.stream = None  # zlib compressobj, set on the first of several chunks

    async def __call__(self, scope: Scope, receive: Receive) -> None:
        await self.app(scope, receive, self._send)

    async def _send(self, message: Message) -> None:
        if message["type"] == "http.response.start":
            self.start = message
            self.compress = _is_compressible(message)
            if not self.compress:
                await self._flush_start()
            return

        if message["type"] != "http.response.body" or not self.compress:
            await self.send(message)
            return

        body = message.get("body", b"")
        more = message.get("more_body", False)

        if self.stream is not None:            # already streaming - keep going
            await self._send_stream_chunk(body, more)
            return

        if more:                               # first of several chunks
            self.stream = zlib.compressobj(
                _COMPRESS_LEVEL, zlib.DEFLATED, zlib.MAX_WBITS | 16,
            )
            self._mark_encoded(chunked=True)
            await self._flush_start()
            await self._send_stream_chunk(body, more)
            return

        # The whole body in one message: the common JSON path.
        if len(body) < _COMPRESS_MIN_BYTES:
            self.compress = False
            await self._flush_start()
            await self.send(message)
            return
        packed = gzip.compress(body, compresslevel=_COMPRESS_LEVEL)
        if len(packed) >= len(body):           # incompressible - ship the original
            self.compress = False
            await self._flush_start()
            await self.send(message)
            return
        self._mark_encoded(length=len(packed))
        await self._flush_start()
        await self.send({"type": "http.response.body", "body": packed, "more_body": False})

    def _mark_encoded(self, *, length: int | None = None, chunked: bool = False) -> None:
        assert self.start is not None
        headers = MutableHeaders(raw=self.start["headers"])
        headers["Content-Encoding"] = "gzip"
        _add_vary(headers, "Accept-Encoding")
        if chunked:
            del headers["Content-Length"]      # length is unknown until the end
        else:
            headers["Content-Length"] = str(length)

    async def _flush_start(self) -> None:
        if self.start is not None:
            await self.send(self.start)
            self.start = None

    async def _send_stream_chunk(self, body: bytes, more: bool) -> None:
        assert self.stream is not None
        chunk = self.stream.compress(body)
        if not more:
            chunk += self.stream.flush(zlib.Z_FINISH)
        # A zero-length chunk mid-stream is legal but pointless; the final one is
        # sent regardless so the client sees more_body=False.
        if chunk or not more:
            await self.send({"type": "http.response.body", "body": chunk, "more_body": more})


def add_response_compression_middleware(app: FastAPI) -> None:
    """gzip compressible response bodies (see ResponseCompressionMiddleware).

    Register LAST in main.py so it sits outermost and compresses the finished
    response, security headers and all.
    """
    app.add_middleware(ResponseCompressionMiddleware)
