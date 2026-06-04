import logging

import redis.asyncio as redis

from app.core.config import settings

logger = logging.getLogger("kiwi.redis")

_client: redis.Redis | None = None


async def init_redis() -> None:
    """Connect to Redis if configured. No-op (with a warning) if REDIS_URL is unset."""
    global _client
    if not settings.redis_url:
        logger.warning("REDIS_URL unset — running without Redis")
        return
    _client = redis.from_url(settings.redis_url, decode_responses=True)
    await _client.ping()


async def close_redis() -> None:
    global _client
    if _client is not None:
        await _client.aclose()
        _client = None


def get_redis() -> redis.Redis | None:
    return _client
