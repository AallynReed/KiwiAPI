"""Transient holding area for .tmod files a partner site uploads to preview.

A partner (Trovesaurus) hosts its own mods, so the embed can't look them up in the
hub. They POST the ``.tmod`` and get a **token** back, which their page puts in the
iframe URL.

**Nothing is written to disk, and nothing outlives its TTL.** The bytes sit in Redis
under a short expiry and that's the whole lifecycle - there is no store to grow, no
cleanup job, and no copy of a partner's mod on our filesystem. The only reason the
file has to persist past the upload at all is that one page view makes several calls
against it (manifest, then a model, then a VFX manifest and one request per texture).

Two consequences worth knowing:

  - **The TTL is fixed, not slid.** A token is good for ``embed.upload_ttl_minutes``
    from the upload, full stop. The intended flow is that a partner POSTs the mod as
    their page renders and embeds the fresh token, so nothing accumulates - and a
    visitor who sits on a page past the expiry just reloads it.
  - **Redis is required.** Without it there is nowhere to put the bytes, so uploads
    fail loudly rather than silently falling back to storage.

The token is the file's SHA-256, so re-posting the same mod reuses the same key
instead of holding a second copy. Bytes are base64'd because the shared Redis client
runs with ``decode_responses=True`` - same approach as the blueprint PNG cache in
``app/trove/render/service.py``.
"""

from __future__ import annotations

import base64
import re

from app.admin import runtime_config
from app.core.errors import APIError, ErrorCode
from app.core.redis import get_redis

_SHA_RE = re.compile(r"^[0-9a-f]{64}$")
_KEY = "embed:tmod:"


def _key(sha: str) -> str:
    return _KEY + sha


async def _ttl_seconds() -> int:
    minutes = await runtime_config.get_setting("embed.upload_ttl_minutes")
    return max(1, int(minutes)) * 60


def valid_token(token: str) -> bool:
    return bool(_SHA_RE.match(token or ""))


async def store(data: bytes, sha: str) -> dict:
    """Hold an uploaded .tmod for the TTL and return its preview token.

    The caller has already validated that ``data`` parses as a .tmod and computed
    its hash. Returns ``{token, size, expires_in, reused}``.
    """
    r = get_redis()
    if r is None:
        raise APIError(
            503, ErrorCode.service_unavailable,
            "Mod previews are temporarily unavailable. Try again shortly.",
        )
    ttl = await _ttl_seconds()
    reused = bool(await r.exists(_key(sha)))
    await r.set(_key(sha), base64.b64encode(data).decode("ascii"), ex=ttl)
    return {"token": sha, "size": len(data), "expires_in": ttl, "reused": reused}


async def load(token: str) -> bytes | None:
    """The held .tmod for a live token, else None. Does NOT extend the expiry."""
    if not valid_token(token):
        return None
    r = get_redis()
    if r is None:
        return None
    raw = await r.get(_key(token))
    if raw is None:
        return None                              # expired / never uploaded
    try:
        return base64.b64decode(raw)
    except ValueError:
        return None                              # corrupt value - treat as gone


async def stats() -> dict:
    """How many previews are live right now and what they weigh, for the admin panel.

    There is no store to report on - this is a point-in-time look at Redis, and it
    drops to zero on its own. Sizes come from ``STRLEN`` (the base64 length), so this
    doesn't pull megabytes of mod back through the client just to measure them.
    """
    r = get_redis()
    if r is None:
        return {"live": 0, "bytes": 0, "redis": False}

    keys = [k async for k in r.scan_iter(match=_KEY + "*", count=500)]
    encoded = 0
    for chunk in (keys[i:i + 100] for i in range(0, len(keys), 100)):
        pipe = r.pipeline()
        for k in chunk:
            pipe.strlen(k)
        encoded += sum(await pipe.execute())
    # base64 is 4 bytes per 3 - report what the mods actually weigh.
    return {"live": len(keys), "bytes": encoded * 3 // 4, "redis": True}
