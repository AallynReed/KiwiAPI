import time
from dataclasses import dataclass
from datetime import datetime, timezone

from pymongo import ReturnDocument

from app.core.database import RATE_LIMIT_COLLECTION, get_db
from app.core.errors import APIError, ErrorCode
from app.core.redis import get_redis


@dataclass
class RateLimitInfo:
    limit: int
    remaining: int
    reset: datetime  # when the current window ends


def rate_limit_headers(info: RateLimitInfo) -> dict[str, str]:
    """Standard rate-limit headers, set on every rate-limited response."""
    return {
        "X-RateLimit-Limit": str(info.limit),
        "X-RateLimit-Remaining": str(info.remaining),
        "X-RateLimit-Reset": str(int(info.reset.timestamp())),
    }


def _too_many(info: RateLimitInfo, window_seconds: int, now: datetime) -> APIError:
    retry_after = max(1, int((info.reset - now).total_seconds()))
    return APIError(
        status_code=429,
        code=ErrorCode.rate_limited,
        message="Rate limit exceeded. Slow down and try again later.",
        details={"limit": info.limit, "window_seconds": window_seconds},
        headers={**rate_limit_headers(info), "Retry-After": str(retry_after)},
    )


async def check_rate_limit(
    key: str,
    max_requests: int,
    window_seconds: int,
) -> RateLimitInfo:
    """Rate-limit ``key`` to ``max_requests`` per ``window_seconds``.

    Uses a Redis sliding-window counter when Redis is available (smoother and
    cheaper than per-request database writes); otherwise falls back to an atomic
    Mongo fixed-window counter. Both raise a 429 APIError with Retry-After +
    X-RateLimit-* headers once the budget is exhausted.
    """
    redis = get_redis()
    if redis is not None:
        return await _redis_sliding_window(redis, key, max_requests, window_seconds)
    return await _mongo_fixed_window(key, max_requests, window_seconds)


# --- Redis sliding window (preferred) --------------------------------------
#
# A sorted set per key holds one member per request, scored by timestamp. Each
# call trims members older than the window, adds itself, and counts what's left
# — a true rolling window with no fixed-window edge bursts. The key self-expires.

_SLIDING_WINDOW_LUA = """
local key = KEYS[1]
local now = tonumber(ARGV[1])
local window = tonumber(ARGV[2])
local limit = tonumber(ARGV[3])
local member = ARGV[4]
redis.call('ZREMRANGEBYSCORE', key, 0, now - window)
redis.call('ZADD', key, now, member)
local count = redis.call('ZCARD', key)
redis.call('PEXPIRE', key, math.ceil(window * 1000))
if count > limit then
  -- This request is over budget; don't let it count against future windows.
  redis.call('ZREM', key, member)
end
return count
"""


async def _redis_sliding_window(
    redis, key: str, max_requests: int, window_seconds: int
) -> RateLimitInfo:
    now = time.time()
    member = f"{now:.6f}:{_counter()}"
    count = await redis.eval(
        _SLIDING_WINDOW_LUA,
        1,
        f"rl:{key}",
        str(now),
        str(window_seconds),
        str(max_requests),
        member,
    )
    count = int(count)
    reset = datetime.fromtimestamp(now + window_seconds, tz=timezone.utc)
    info = RateLimitInfo(
        limit=max_requests,
        remaining=max(0, max_requests - count),
        reset=reset,
    )
    if count > max_requests:
        raise _too_many(info, window_seconds, datetime.now(timezone.utc))
    return info


_seq = 0


def _counter() -> int:
    """Process-local monotonic suffix so concurrent calls get unique members."""
    global _seq
    _seq = (_seq + 1) % 1_000_000
    return _seq


# --- Mongo fixed window (fallback when Redis is unavailable) ----------------

async def _mongo_fixed_window(
    key: str, max_requests: int, window_seconds: int
) -> RateLimitInfo:
    now = datetime.now(timezone.utc)
    window_start = int(now.timestamp() // window_seconds) * window_seconds
    reset = datetime.fromtimestamp(window_start + window_seconds, tz=timezone.utc)
    bucket_id = f"{key}:{window_start}"

    collection = get_db()[RATE_LIMIT_COLLECTION]
    doc = await collection.find_one_and_update(
        {"_id": bucket_id},
        {"$inc": {"count": 1}, "$setOnInsert": {"expires_at": reset}},
        upsert=True,
        return_document=ReturnDocument.AFTER,
    )
    # upsert=True + AFTER always returns the document; default defensively anyway
    # (so this is correct even under `python -O`, which strips asserts).
    count = doc["count"] if doc else 1
    info = RateLimitInfo(
        limit=max_requests,
        remaining=max(0, max_requests - count),
        reset=reset,
    )
    if count > max_requests:
        raise _too_many(info, window_seconds, now)
    return info
