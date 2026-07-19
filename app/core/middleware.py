from fastapi import FastAPI, Request
from starlette.responses import JSONResponse, RedirectResponse, Response
from starlette.types import ASGIApp, Receive, Scope, Send

from app.core.config import settings
from app.core.errors import ErrorCode, build_error_body

# The API serves JSON plus a few small, self-contained HTML pages (landing,
# verify-email, reset-password). Those use inline <style>/<script>, so inline is
# allowed, but everything external is locked down.
_API_CSP = (
    "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; "
    "connect-src 'self'; img-src 'self' data:; base-uri 'none'; frame-ancestors 'none'"
)

# The BetterTroveTools showcase site self-hosts all fonts and Font Awesome from
# /static/fonts/ (GDPR: no font/icon request ever leaves our origin to Google or
# cdnjs), and calls the Kiwi API for release data. The /login + /signup +
# /forgot-password pages also render a captcha widget (Turnstile or hCaptcha),
# whichever the API is configured for - both host their script + iframe under
# their own domains, so script-src + frame-src cover the union of providers so a
# toggle from one to the other doesn't require a CSP edit.
_SITE_CSP = (
    "default-src 'self'; "
    "script-src 'self' 'unsafe-inline' "
        "https://challenges.cloudflare.com https://hcaptcha.com https://*.hcaptcha.com; "
    "style-src 'self' 'unsafe-inline' "
        "https://hcaptcha.com https://*.hcaptcha.com; "
    "font-src 'self' data:; "
    # Allow any https image so user-content READMEs render badges + screenshots
    # (shields.io, github, imgur, …) like GitHub. Images can't execute, so this is
    # low-risk; `data:` covers inline, `cdn.discordapp.com` is already https.
    "img-src 'self' data: https:; "
    # Data-plane binary (mod artifacts, textures, blueprints, VFX assets) is
    # served from the API origin cross-origin; viewers fetch() it (connect-src),
    # but declare media-src too so any <audio>/<video> from the API isn't blocked
    # by the default-src fallback.
    "media-src 'self' https://api.aallyn.net; "
    "connect-src 'self' https://api.aallyn.net "
        "https://challenges.cloudflare.com https://hcaptcha.com https://*.hcaptcha.com; "
    "frame-src https://challenges.cloudflare.com https://hcaptcha.com https://*.hcaptcha.com; "
    "base-uri 'none'; frame-ancestors 'none'"
)


# Exact-matched showcase-site HTML page routes. (Everything else under the site
# is the /static/* asset mount or the /site/* JSON proxies.)
_PAGE_PATHS = frozenset({
    "/", "/app", "/browse", "/documentation", "/commands", "/leaderboards", "/updates",
    "/support", "/login", "/dashboard", "/market", "/store", "/codexes", "/codexes/crafting",
    "/status", "/giveaways",
    "/activity", "/class-activity", "/clubs", "/terms", "/privacy", "/accessibility", "/mods", "/modpacks",
    "/server-time", "/swf-docs", "/calendar", "/streams", "/releases", "/classes",
    "/star-chart", "/gem-simulator", "/gem-evaluator", "/gem-builds", "/calculators",
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
        h.setdefault("X-Frame-Options", "DENY")
        h.setdefault("Referrer-Policy", "strict-origin-when-cross-origin")
        h.setdefault("Strict-Transport-Security", "max-age=31536000; includeSubDomains")
        h.setdefault("Permissions-Policy", "geolocation=(), microphone=(), camera=()")
        # The showcase site pulls FontAwesome / GSAP from CDN and needs a looser CSP.
        h.setdefault("Content-Security-Policy", _SITE_CSP if _is_site_path(path) else _API_CSP)
        # Static assets + HTML pages always revalidate, so a deploy is picked up
        # immediately without ``?v=`` cache-busting. StaticFiles' ETag /
        # Last-Modified keep this to cheap 304s. The /site/ JSON proxies set
        # their own max-age and are intentionally excluded.
        if path.startswith("/static/") or path in _PAGE_PATHS or path.startswith(_PAGE_PREFIXES):
            h.setdefault("Cache-Control", "no-cache")
        return response


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


# The bare API host (api.aallyn.net) derived from settings, lower-cased for the
# Host-header compare. The one FastAPI app answers on every hostname, so a
# showcase page is reachable at api.aallyn.net/<page> as well as its real home on
# app_url - see add_api_host_redirect_middleware.
_API_HOST = settings.api_url.split("://", 1)[-1].split("/", 1)[0].lower()


def _is_site_page(path: str) -> bool:
    """A showcase-site HTML *page* - one in ``_PAGE_PATHS`` or a dynamic page
    subtree. These are the routes with a canonical home on ``app_url``.

    Deliberately narrower than ``_is_site_path``: it excludes ``/static`` and
    ``/site`` (assets + JSON proxies) and every api-native path (/v1, /health,
    /api-info, /openapi.json, /robots.txt), so only real indexable pages redirect.
    """
    return path in _PAGE_PATHS or path.startswith(_PAGE_PREFIXES)


def add_api_host_redirect_middleware(app: FastAPI) -> None:
    """301 showcase-site pages served on the API host to their canonical app_url home.

    api.aallyn.net/login serves the same page as trove.aallyn.net/login, and Google
    indexed the api copy. robots.txt ``Disallow: /`` on the api host does NOT deindex
    it - it only blocks the re-crawl that would let Google see the page's
    canonical/noindex - so the already-indexed URL is frozen there. A hard 301 to the
    app host is what actually drops it and consolidates the signal onto one URL.

    GET/HEAD only: page routes are GET, and a 301 must never silently turn a POST into
    a GET. Scoped to the API host; the JSON API, /api-info, robots.txt and static
    assets are untouched.
    """
    app_url = settings.app_url.rstrip("/")

    @app.middleware("http")
    async def api_host_redirect(request: Request, call_next):
        if (
            request.method in ("GET", "HEAD")
            and (request.url.hostname or "").lower() == _API_HOST
            and _is_site_page(request.url.path)
        ):
            target = app_url + request.url.path
            if request.url.query:
                target = f"{target}?{request.url.query}"
            return RedirectResponse(target, status_code=301)
        return await call_next(request)
