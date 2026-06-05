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


def add_security_middleware(app: FastAPI) -> None:
    """Reject oversized bodies early and attach security headers to every response."""
    default_max_body = settings.max_request_body_bytes
    mods_max_body = settings.mods_max_request_body_bytes

    @app.middleware("http")
    async def security(request: Request, call_next):
        # The mod tools exchange whole .tmod files, so they get a larger body cap.
        max_body = mods_max_body if request.url.path.startswith("/v1/mods/") else default_max_body
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
        h.setdefault("Content-Security-Policy", _API_CSP)
        return response
