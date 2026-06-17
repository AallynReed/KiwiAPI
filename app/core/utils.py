from datetime import datetime, timezone

from starlette.requests import Request


def utcnow() -> datetime:
    """Timezone-aware UTC now - use everywhere instead of datetime.utcnow()."""
    return datetime.now(timezone.utc)


def countdown_bucket(target: int | None, now: int) -> tuple[str, int]:
    """Coarsen a future unix instant into a single-unit ``(unit, value)`` bucket:
    minutes under 1h, whole HOURS under a day, whole DAYS beyond. ``unit`` is one of
    ``"m" | "h" | "d" | "now" | "none"``.

    Used by BOTH the announcement image's countdown text and the bot's image
    refresh token, so a banner only changes (and the bot only re-edits the Discord
    message) when the shown value changes - "16h" stays "16h" for the whole hour
    (one edit/hour), dropping to per-minute "59m"…"0m" only under an hour. Sharing
    this one helper keeps the drawn text and the refresh cadence from drifting."""
    if not target:
        return ("none", 0)
    rem = int(target) - int(now)
    if rem <= 0:
        return ("now", 0)
    if rem < 3600:
        return ("m", rem // 60)
    if rem < 86400:
        return ("h", rem // 3600)
    return ("d", rem // 86400)


def client_ip(request: Request) -> str | None:
    """The client's IP, or None if unknown. uvicorn runs with --proxy-headers, so
    this reflects the real client once the reverse proxy sets X-Forwarded-For.
    Callers pick their own fallback (e.g. `or "unknown"` for a stable rate-limit key)."""
    return request.client.host if request.client else None
