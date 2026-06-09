from datetime import datetime, timezone

from starlette.requests import Request


def utcnow() -> datetime:
    """Timezone-aware UTC now - use everywhere instead of datetime.utcnow()."""
    return datetime.now(timezone.utc)


def client_ip(request: Request) -> str | None:
    """The client's IP, or None if unknown. uvicorn runs with --proxy-headers, so
    this reflects the real client once the reverse proxy sets X-Forwarded-For.
    Callers pick their own fallback (e.g. `or "unknown"` for a stable rate-limit key)."""
    return request.client.host if request.client else None
