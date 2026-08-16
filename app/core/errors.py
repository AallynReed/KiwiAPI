import logging
from enum import Enum
from typing import Any

from fastapi import FastAPI, Request
from fastapi.encoders import jsonable_encoder
from fastapi.exceptions import RequestValidationError
from fastapi.responses import HTMLResponse, JSONResponse
from pydantic import BaseModel
from starlette.exceptions import HTTPException as StarletteHTTPException

from app.core.context import get_request_id

logger = logging.getLogger("kiwi.errors")

# Path prefixes that are data/API surfaces - a 404 there stays JSON even from a
# browser, so the showcase HTML 404 page only ever shows for front-facing pages.
_API_PREFIXES = ("/v1/", "/v1", "/admin", "/site/", "/git/", "/secret-scanning",
                 "/openapi", "/.well-known", "/health", "/config", "/metrics")

# Self-contained themed 404 for front-facing website pages (no template/context
# dependency, so it can never itself error). Fully inline palette + layout, and
# system-font fallbacks - no external font request (GDPR: no IP leak to Google),
# and no linked stylesheet that the stricter API CSP on unknown paths would block.
#
# The favicon is inline for the same reason: a 32x32 data: URI downscaled from
# site/static/assets/favicon.png. It can't be a /static href because the API
# container doesn't mount /static (the website container owns it), so a
# root-relative href 404s there - and an absolute app_url href would be BLOCKED by
# the API CSP (img-src 'self' data:) rather than fixed. data: satisfies both CSPs,
# and costs one fewer request on the website side too. Regenerate it from the
# source PNG if the brand mark changes; don't "tidy" it back into a /static href.
_NOT_FOUND_HTML = """<!DOCTYPE html>
<html lang="en" dir="ltr"><head>
<meta charset="UTF-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Page not found · Better Trove Tools</title>
<meta name="robots" content="noindex">
<meta name="theme-color" content="#0a0e14"><meta name="color-scheme" content="dark">
<link rel="icon" href="data:image/png;base64,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" type="image/png">
<style>
  :root{--bg:#0a0e14;--card:#11161f;--line:#222c3a;--text:#e6ebf2;--soft:#9aa6b8;--mute:#6b7688;--blue:#569cff}
  *{box-sizing:border-box}
  body{margin:0;min-height:100vh;display:grid;place-items:center;padding:32px;
    background:radial-gradient(1200px 600px at 50% -10%,rgba(86,156,255,.10),transparent),var(--bg);
    color:var(--text);font-family:'Inter',system-ui,sans-serif}
  .nf{max-width:520px;text-align:center}
  .nf-code{font:700 clamp(5rem,18vw,9rem)/1 'Space Grotesk','Inter',sans-serif;letter-spacing:-.04em;
    background:linear-gradient(135deg,#569cff,#a06bff 60%,#ff5e7e);-webkit-background-clip:text;
    background-clip:text;color:transparent;margin:0}
  .nf-title{font:700 1.5rem 'Space Grotesk','Inter',sans-serif;margin:6px 0 10px}
  .nf-text{color:var(--soft);line-height:1.6;margin:0 0 26px}
  .nf-actions{display:flex;gap:12px;justify-content:center;flex-wrap:wrap}
  .nf-btn{display:inline-flex;align-items:center;gap:8px;text-decoration:none;border-radius:10px;
    padding:11px 20px;font-weight:600;font-size:.95rem;border:1px solid var(--line);
    color:var(--text);background:var(--card);transition:border-color .15s,transform .12s}
  .nf-btn:hover{border-color:var(--blue);transform:translateY(-1px)}
  .nf-btn-primary{background:linear-gradient(135deg,#569cff,#4a7fe0);border-color:transparent;color:#04121f}
  .nf-mark{color:var(--mute);font-size:.82rem;margin-top:30px;letter-spacing:.04em;text-transform:uppercase}
</style></head>
<body><main class="nf">
  <p class="nf-code">404</p>
  <h1 class="nf-title">This page wandered off</h1>
  <p class="nf-text">The page you're looking for doesn't exist, moved, or never did. Let's get you back to solid ground.</p>
  <div class="nf-actions">
    <a class="nf-btn nf-btn-primary" href="/">&larr; Back home</a>
    <a class="nf-btn" href="/mods">Browse the Mods Hub</a>
  </div>
  <p class="nf-mark">Better Trove Tools</p>
</main></body></html>"""


def _wants_html_page(request: Request) -> bool:
    """A front-facing GET that a browser would navigate to (not an API/data path)."""
    if request.method not in ("GET", "HEAD"):
        return False
    if request.url.path.startswith(_API_PREFIXES):
        return False
    return "text/html" in request.headers.get("accept", "")


class ErrorCode(str, Enum):
    """Stable, machine-readable error slugs returned in every error body."""

    bad_request = "bad_request"
    validation_error = "validation_error"
    not_authenticated = "not_authenticated"
    invalid_credentials = "invalid_credentials"
    captcha_failed = "captcha_failed"
    forbidden = "forbidden"
    not_found = "not_found"
    not_public = "not_public"
    conflict = "conflict"
    email_taken = "email_taken"
    email_unverified = "email_unverified"
    disposable_email = "disposable_email"
    password_breached = "password_breached"
    account_locked = "account_locked"
    insufficient_scope = "insufficient_scope"
    ip_not_allowed = "ip_not_allowed"
    rate_limited = "rate_limited"
    method_not_allowed = "method_not_allowed"
    service_unavailable = "service_unavailable"
    internal_error = "internal_error"


# --- Response shape (also documented in OpenAPI) ---------------------------

class ErrorBody(BaseModel):
    code: str
    message: str
    details: Any | None = None
    request_id: str | None = None  # correlate with server logs (also in X-Request-ID)


class ErrorResponse(BaseModel):
    error: ErrorBody

    model_config = {
        "json_schema_extra": {
            "example": {
                "error": {
                    "code": "rate_limited",
                    "message": "Rate limit exceeded. Slow down and try again later.",
                    "details": {"limit": 120, "window_seconds": 60},
                }
            }
        }
    }


# Reusable OpenAPI `responses` so every endpoint documents the shared error
# envelope (not FastAPI's default bare `{"detail": ...}`). Attach per-router.
_ERROR_DESCRIPTIONS: dict[int, str] = {
    400: "Bad request",
    401: "Authentication required or invalid",
    403: "Forbidden (insufficient scope / privileges / IP not allowed)",
    404: "Not found",
    409: "Conflict",
    422: "Validation error",
    429: "Rate limit exceeded",
    500: "Internal server error",
}


def error_responses(*status_codes: int) -> dict:
    """Build an OpenAPI `responses` map pointing the given statuses at the
    standard error envelope. With no args, documents the common set."""
    codes = status_codes or (400, 401, 403, 404, 422, 429)
    return {
        code: {"model": ErrorResponse, "description": _ERROR_DESCRIPTIONS.get(code, "Error")}
        for code in codes
    }


# The default envelope documentation reused across data + management routers.
COMMON_ERROR_RESPONSES = error_responses()


# --- The one exception type the app raises ---------------------------------

class APIError(Exception):
    """Raise this anywhere; it renders as the standard error envelope."""

    def __init__(
        self,
        status_code: int,
        code: ErrorCode | str,
        message: str,
        *,
        details: Any | None = None,
        headers: dict[str, str] | None = None,
    ) -> None:
        self.status_code = status_code
        self.code = code.value if isinstance(code, ErrorCode) else code
        self.message = message
        self.details = details
        self.headers = headers or {}
        super().__init__(message)


def build_error_body(code: str, message: str, details: Any | None = None) -> dict:
    """The standard error envelope. Use anywhere a body is built by hand (proxies,
    middleware, idempotency replays) so ``request_id`` and shape stay consistent."""
    return {
        "error": {
            "code": code,
            "message": message,
            "details": details,
            "request_id": get_request_id(),
        }
    }


_envelope = build_error_body


def raise_from_value_error(exc: ValueError) -> None:
    """Map a service-layer ValueError to the right APIError: a message mentioning
    'already exists' is a 409 conflict, otherwise a 400 bad request."""
    message = str(exc)
    if "already exists" in message:
        raise APIError(status_code=409, code=ErrorCode.conflict, message=message)
    raise APIError(status_code=400, code=ErrorCode.bad_request, message=message)


# Fallback mapping for framework-raised HTTPExceptions (no explicit code).
_STATUS_TO_CODE: dict[int, ErrorCode] = {
    400: ErrorCode.bad_request,
    401: ErrorCode.not_authenticated,
    403: ErrorCode.forbidden,
    404: ErrorCode.not_found,
    405: ErrorCode.method_not_allowed,
    409: ErrorCode.conflict,
    422: ErrorCode.validation_error,
    429: ErrorCode.rate_limited,
    503: ErrorCode.service_unavailable,
}


async def _api_error_handler(request: Request, exc: APIError) -> JSONResponse:
    return JSONResponse(
        status_code=exc.status_code,
        content=_envelope(exc.code, exc.message, exc.details),
        headers=exc.headers,
    )


async def _http_exception_handler(
    request: Request, exc: StarletteHTTPException
) -> JSONResponse | HTMLResponse:
    # A 404 from a browser navigating to a front-facing page gets the friendly HTML
    # page; data/API 404s (and everything programmatic) stay JSON.
    if exc.status_code == 404 and _wants_html_page(request):
        return HTMLResponse(_NOT_FOUND_HTML, status_code=404)
    code = _STATUS_TO_CODE.get(exc.status_code, ErrorCode.internal_error)
    return JSONResponse(
        status_code=exc.status_code,
        content=_envelope(code.value, str(exc.detail)),
        headers=getattr(exc, "headers", None),
    )


async def _validation_exception_handler(
    request: Request, exc: RequestValidationError
) -> JSONResponse:
    return JSONResponse(
        status_code=422,
        content=_envelope(
            ErrorCode.validation_error.value,
            "Request validation failed",
            jsonable_encoder(exc.errors()),
        ),
    )


async def _unhandled_exception_handler(request: Request, exc: Exception) -> JSONResponse:
    logger.exception("Unhandled error on %s %s", request.method, request.url.path)
    return JSONResponse(
        status_code=500,
        content=_envelope(ErrorCode.internal_error.value, "Internal server error"),
    )


def register_error_handlers(app: FastAPI) -> None:
    # Starlette types handlers as taking the base Exception; ours are narrowed to
    # the specific exception type they handle (correct at runtime).
    app.add_exception_handler(APIError, _api_error_handler)  # pyright: ignore[reportArgumentType]
    app.add_exception_handler(RequestValidationError, _validation_exception_handler)  # pyright: ignore[reportArgumentType]
    app.add_exception_handler(StarletteHTTPException, _http_exception_handler)  # pyright: ignore[reportArgumentType]
    app.add_exception_handler(Exception, _unhandled_exception_handler)
