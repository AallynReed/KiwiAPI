from fastapi import FastAPI, Request
from starlette.responses import JSONResponse, Response

from app.core.config import settings
from app.core.errors import ErrorCode

# The API serves JSON plus a few small, self-contained HTML pages (landing,
# verify-email, reset-password). Those use inline <style>/<script>, so inline is
# allowed, but everything external is locked down.
_API_CSP = (
    "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; "
    "connect-src 'self'; img-src 'self' data:; base-uri 'none'; frame-ancestors 'none'"
)

# The BetterTroveTools showcase site pulls FontAwesome CSS from cdnjs, Space
# Grotesk + Inter from Google Fonts (their CSS lives on fonts.googleapis.com,
# the actual font files on fonts.gstatic.com - both need to be allowed or the
# page falls back to system fonts and the bold display look breaks), and calls
# the Kiwi API for release data. The /login + /signup + /forgot-password pages
# also render a captcha widget (Turnstile or hCaptcha), whichever the API
# is configured for - both host their script + iframe under their own
# domains, so script-src + frame-src cover the union of providers so a
# toggle from one to the other doesn't require a CSP edit.
_SITE_CSP = (
    "default-src 'self'; "
    "script-src 'self' 'unsafe-inline' https://cdnjs.cloudflare.com "
        "https://challenges.cloudflare.com https://hcaptcha.com https://*.hcaptcha.com; "
    "style-src 'self' 'unsafe-inline' https://cdnjs.cloudflare.com "
        "https://fonts.googleapis.com https://hcaptcha.com https://*.hcaptcha.com; "
    "font-src 'self' https://cdnjs.cloudflare.com https://fonts.gstatic.com data:; "
    # Allow any https image so user-content READMEs render badges + screenshots
    # (shields.io, github, imgur, …) like GitHub. Images can't execute, so this is
    # low-risk; `data:` covers inline, `cdn.discordapp.com` is already https.
    "img-src 'self' data: https:; "
    "connect-src 'self' https://api.aallyn.net "
        "https://challenges.cloudflare.com https://hcaptcha.com https://*.hcaptcha.com; "
    "frame-src https://challenges.cloudflare.com https://hcaptcha.com https://*.hcaptcha.com; "
    "base-uri 'none'; frame-ancestors 'none'"
)


# Exact-matched showcase-site HTML page routes. (Everything else under the site
# is the /static/* asset mount or the /site/* JSON proxies.)
_PAGE_PATHS = frozenset({
    "/", "/app", "/browse", "/documentation", "/commands", "/leaderboards", "/updates",
    "/support", "/login", "/dashboard", "/market", "/codexes", "/codexes/crafting",
    "/status", "/giveaways",
    "/activity", "/class-activity", "/clubs", "/terms", "/privacy", "/mods", "/modpacks",
    "/server-time", "/swf-docs", "/calendar", "/streams", "/releases", "/classes",
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
    # site_max_request_body_bytes used to govern /unlock_debug + /unlock_fps
    # uploads; those routes were removed 2026-06 after Trove shipped
    # anti-cheat. Setting is still defined in settings.py for parity but
    # not bound here so the middleware closure stays clean.
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
                    content={
                        "error": {
                            "code": ErrorCode.bad_request.value,
                            "message": f"Request body exceeds the {max_body}-byte limit",
                            "details": None,
                        }
                    },
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
