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
# dependency, so it can never itself error). Pulls in the site stylesheet for
# fonts + palette and adds a small inline layout.
_NOT_FOUND_HTML = """<!DOCTYPE html>
<html lang="en" dir="ltr"><head>
<meta charset="UTF-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>Page not found · Better Trove Tools</title>
<meta name="robots" content="noindex">
<meta name="theme-color" content="#0a0e14"><meta name="color-scheme" content="dark">
<link rel="icon" href="/static/assets/favicon.png" type="image/png">
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Space+Grotesk:wght@600;700&family=Inter:wght@400;500;600&display=swap">
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
