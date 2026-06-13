import logging
from enum import Enum
from typing import Any

from fastapi import FastAPI, Request
from fastapi.encoders import jsonable_encoder
from fastapi.exceptions import RequestValidationError
from fastapi.responses import JSONResponse
from pydantic import BaseModel
from starlette.exceptions import HTTPException as StarletteHTTPException

from app.core.context import get_request_id

logger = logging.getLogger("kiwi.errors")


class ErrorCode(str, Enum):
    """Stable, machine-readable error slugs returned in every error body."""

    bad_request = "bad_request"
    validation_error = "validation_error"
    not_authenticated = "not_authenticated"
    invalid_credentials = "invalid_credentials"
    captcha_failed = "captcha_failed"
    forbidden = "forbidden"
    not_found = "not_found"
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


def _envelope(code: str, message: str, details: Any | None = None) -> dict:
    return {
        "error": {
            "code": code,
            "message": message,
            "details": details,
            "request_id": get_request_id(),
        }
    }


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
) -> JSONResponse:
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
