"""Per-account login lockout, backed by Redis. Gracefully no-ops if Redis is
unavailable (the IP-based rate limit still applies)."""

from app.core.config import settings
from app.core.redis import get_redis


def _fail_key(email: str) -> str:
    return f"loginfail:{email.lower()}"


def _lock_key(email: str) -> str:
    return f"loginlock:{email.lower()}"


async def lock_ttl(email: str) -> int:
    """Seconds remaining on the lock, or 0 if not locked."""
    r = get_redis()
    if r is None:
        return 0
    ttl = await r.ttl(_lock_key(email))
    return ttl if ttl and ttl > 0 else 0


async def record_failure(email: str) -> None:
    """Count a failed attempt; lock the account once the threshold is hit."""
    r = get_redis()
    if r is None:
        return
    count = await r.incr(_fail_key(email))
    if count == 1:
        await r.expire(_fail_key(email), settings.login_attempt_window_seconds)
    if count >= settings.login_max_attempts:
        await r.set(_lock_key(email), "1", ex=settings.login_lockout_seconds)


async def clear(email: str) -> None:
    """Reset on a successful login."""
    r = get_redis()
    if r is None:
        return
    await r.delete(_fail_key(email), _lock_key(email))
