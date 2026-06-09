"""Coalesced "token was used" accounting.

Naively, every authenticated API request would issue a Mongo write to bump the
token's ``request_count`` / ``last_used_at``. Under load that's a write per
request purely for bookkeeping. When Redis is available we debounce it: requests
increment a Redis counter, and only the first request in each interval flushes
the accumulated delta to Mongo. ``request_count`` stays eventually-exact and
``last_used_at`` is fresh to within one interval. Without Redis we fall back to
writing every request (correct, just chattier).
"""

import logging
from typing import Any

from beanie.operators import Inc, Set

from app.core.config import settings
from app.core.redis import get_redis
from app.core.utils import utcnow
from app.tokens.models import ApiToken

logger = logging.getLogger("kiwi.tokens")

# Atomically: increment the pending counter; if no flush has happened this
# interval, claim the interval, reset the counter to 0, and return the delta to
# flush. Otherwise return -1 (another request already owns this interval).
_FLUSH_LUA = """
local pending = KEYS[1]
local gate = KEYS[2]
local interval = tonumber(ARGV[1])
local n = redis.call('INCR', pending)
local claimed = redis.call('SET', gate, '1', 'NX', 'EX', interval)
if claimed then
  redis.call('SET', pending, '0')
  redis.call('EXPIRE', pending, interval * 4)
  return n
end
return -1
"""


async def record_token_use(token: ApiToken, ip: str | None) -> None:
    # ``ip`` is accepted for backwards compatibility with existing call sites
    # but intentionally NOT persisted - see ``app/tokens/models.py`` for the
    # reasoning (plaintext IPs leak; hashing them defeats the field's purpose).
    del ip  # explicitly ignored

    redis: Any = get_redis()  # Any: redis-py's command stubs aren't async-typed
    now = utcnow()

    if redis is None:
        await token.update(
            Inc({ApiToken.request_count: 1}),
            Set({ApiToken.last_used_at: now}),
        )
        return

    interval = settings.token_touch_interval_seconds
    try:
        delta = int(
            await redis.eval(
                _FLUSH_LUA,
                2,
                f"tok:pending:{token.id}",
                f"tok:gate:{token.id}",
                str(interval),
            )
        )
    except Exception:
        # If Redis hiccups, fall back to a direct write so accounting isn't lost.
        logger.warning("token-use debounce failed; writing directly", exc_info=True)
        delta = 1

    if delta > 0:
        await token.update(
            Inc({ApiToken.request_count: delta}),
            Set({ApiToken.last_used_at: now}),
        )
