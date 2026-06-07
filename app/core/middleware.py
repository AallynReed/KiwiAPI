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

# The BetterTroveTools showcase site pulls FontAwesome CSS + GSAP from CDN and
# calls the Kiwi API for release data, so its CSP is broader than the API's.
_SITE_CSP = (
    "default-src 'self'; "
    "script-src 'self' 'unsafe-inline' https://cdnjs.cloudflare.com; "
    "style-src 'self' 'unsafe-inline' https://cdnjs.cloudflare.com; "
    "font-src 'self' https://cdnjs.cloudflare.com data:; "
    "img-src 'self' data:; "
    "connect-src 'self' https://api.aallyn.net; "
    "base-uri 'none'; frame-ancestors 'none'"
)


def _is_site_path(path: str) -> bool:
    """Showcase-site routes (everything served from `site_router` in app/site/)."""
    return path == "/" or path in {
        "/documentation", "/unlock_debug", "/unlock_fps",
    } or path.startswith("/static/")


def add_security_middleware(app: FastAPI) -> None:
    """Reject oversized bodies early and attach security headers to every response."""
    default_max_body = settings.max_request_body_bytes
    mods_max_body = settings.mods_max_request_body_bytes
    site_max_body = settings.site_max_request_body_bytes
    leaderboards_max_body = settings.leaderboards_max_request_body_bytes
    market_max_body = settings.market_max_request_body_bytes

    @app.middleware("http")
    async def security(request: Request, call_next):
        path = request.url.path
        # Per-surface body caps: mod tools accept .tmod uploads, the leaderboards
        # + market ingests accept the bot's raw cfg dumps, and the site's
        # /unlock_* tools accept the whole Trove.exe (~100 MB).
        if path.startswith("/v1/mods/"):
            max_body = mods_max_body
        elif path == "/v1/leaderboards/insert":
            max_body = leaderboards_max_body
        elif path == "/v1/market/insert":
            max_body = market_max_body
        elif path.startswith("/unlock_"):
            max_body = site_max_body
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
        return response
