from datetime import datetime, timezone

from beanie import PydanticObjectId
from starlette.requests import Request


def utcnow() -> datetime:
    """Timezone-aware UTC now - use everywhere instead of datetime.utcnow()."""
    return datetime.now(timezone.utc)


def iso(dt: datetime | None) -> str | None:
    """None-safe ISO-8601 serialization. Plain JSONResponse can't encode a raw
    datetime, so hand-built response bodies coerce through this."""
    return dt.isoformat() if dt else None


def to_oid(value: str | None) -> PydanticObjectId | None:
    """Parse an ObjectId, or None if malformed - for by-id lookups where a bad id
    should 404 rather than raise."""
    try:
        return PydanticObjectId(value)
    except Exception:
        return None


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


def device_label(user_agent: str | None) -> str | None:
    """A coarse ``"Browser on OS"`` label from a User-Agent string, for the
    "your active sessions" list. Deliberately lossy: we store THIS, never the raw
    User-Agent (data minimization), and never the IP. ``None`` if no UA."""
    if not user_agent:
        return None
    ua = user_agent
    # Order matters - Edge/Opera/Brave embed "Chrome" in their UA.
    if "Edg" in ua:
        browser = "Edge"
    elif "OPR" in ua or "Opera" in ua:
        browser = "Opera"
    elif "Firefox" in ua or "FxiOS" in ua:
        browser = "Firefox"
    elif "Chrome" in ua or "CriOS" in ua:
        browser = "Chrome"
    elif "Safari" in ua:
        browser = "Safari"
    else:
        browser = "Browser"
    if "Windows" in ua:
        os_name = "Windows"
    elif "Android" in ua:
        os_name = "Android"
    elif "iPhone" in ua or "iPad" in ua or "iOS" in ua:
        os_name = "iOS"
    elif "Mac OS" in ua or "Macintosh" in ua:
        os_name = "macOS"
    elif "Linux" in ua:
        os_name = "Linux"
    else:
        os_name = ""
    return f"{browser} on {os_name}" if os_name else browser
