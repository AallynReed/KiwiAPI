"""Ephemeral store for .tmod files a partner site uploads to preview.

A partner (Trovesaurus) hosts its own mods, so the embed can't look them up in the
hub. Instead they POST the ``.tmod`` once from their backend and get a **token**
back, which their page puts in the iframe URL.

The token IS the file's SHA-256, so re-uploading the same mod is free and never
duplicates a blob. Retention is the interesting part:

  - bytes go into their OWN content-addressed store (``settings.embed_store_dir``),
    never the hub's - so purging an expired upload can't touch a real release,
  - a Redis key ``embed:tmod:<sha>`` is what makes a token *previewable*; it holds
    the display name and carries the TTL,
  - every successful load slides that TTL, so a mod page with live traffic keeps
    working indefinitely and a one-off upload ages out.

Redis is the source of truth for "is this token live". If Redis is down we fall
back to blob presence rather than blanking every partner embed - the store is
content-addressed and purge-gated, so the worst case is an expired preview
staying up until Redis returns.
"""

from __future__ import annotations

import asyncio
import json
import re
from pathlib import Path

from app.admin import runtime_config
from app.core.config import settings
from app.core.redis import get_redis
from app.trove.updates.cas import ContentStore

_SHA_RE = re.compile(r"^[0-9a-f]{64}$")
_KEY = "embed:tmod:"

_store = ContentStore(settings.embed_store_dir)


def _key(sha: str) -> str:
    return _KEY + sha


async def _ttl_seconds() -> int:
    hours = await runtime_config.get_setting("embed.upload_ttl_hours")
    return max(1, int(hours)) * 3600


def valid_token(token: str) -> bool:
    return bool(_SHA_RE.match(token or ""))


async def store(data: bytes, name: str) -> dict:
    """Persist an uploaded .tmod and mint (or refresh) its preview token.

    The caller has already validated that ``data`` parses as a .tmod - we only
    handle storage here. Returns ``{token, name, size, expires_in, reused}``.
    """
    sha, created = await asyncio.to_thread(_store.put, data)
    ttl = await _ttl_seconds()
    r = get_redis()
    reused = not created
    if r is not None:
        reused = bool(await r.exists(_key(sha))) or reused
        await r.set(_key(sha), json.dumps({"name": name, "size": len(data)}), ex=ttl)
    return {"token": sha, "name": name, "size": len(data),
            "expires_in": ttl, "reused": reused}


async def load(token: str) -> tuple[bytes, str] | None:
    """``(tmod_bytes, display_name)`` for a live token, else None. Slides the TTL."""
    if not valid_token(token):
        return None
    name = ""
    r = get_redis()
    if r is not None:
        raw = await r.get(_key(token))
        if raw is None:
            return None                       # expired / never uploaded
        try:
            name = str(json.loads(raw).get("name") or "")
        except (ValueError, AttributeError):
            name = ""
        await r.expire(_key(token), await _ttl_seconds())
    data = await asyncio.to_thread(_store.get, token)
    if data is None:
        return None
    return data, name


async def purge_expired() -> dict:
    """Delete stored blobs whose token has expired (admin action).

    Redis holds the live set, the filesystem holds the bytes; anything on disk
    without a live key is garbage. No-ops when Redis is unavailable - without the
    live set we cannot tell garbage from a valid upload, and deleting on a guess
    would break working partner embeds.
    """
    r = get_redis()
    if r is None:
        return {"purged": 0, "kept": 0, "freed_bytes": 0, "skipped": "redis-unavailable"}

    live: set[str] = set()
    async for key in r.scan_iter(match=_KEY + "*", count=500):
        live.add(key[len(_KEY):])

    def _sweep() -> dict:
        objects = Path(settings.embed_store_dir) / "objects"
        purged = kept = freed = 0
        if not objects.is_dir():
            return {"purged": 0, "kept": 0, "freed_bytes": 0}
        for shard in objects.iterdir():
            if not shard.is_dir():
                continue
            for blob in shard.iterdir():
                if not blob.is_file() or not _SHA_RE.match(blob.name):
                    continue
                if blob.name in live:
                    kept += 1
                    continue
                try:
                    size = blob.stat().st_size
                    blob.unlink()
                except OSError:
                    continue
                purged += 1
                freed += size
        return {"purged": purged, "kept": kept, "freed_bytes": freed}

    return await asyncio.to_thread(_sweep)


async def stats() -> dict:
    """Blob count + bytes on disk and how many tokens are still live (admin panel)."""
    def _walk() -> tuple[int, int]:
        objects = Path(settings.embed_store_dir) / "objects"
        count = total = 0
        if not objects.is_dir():
            return 0, 0
        for shard in objects.iterdir():
            if not shard.is_dir():
                continue
            for blob in shard.iterdir():
                if blob.is_file() and _SHA_RE.match(blob.name):
                    count += 1
                    total += blob.stat().st_size
        return count, total

    count, total = await asyncio.to_thread(_walk)
    live = 0
    r = get_redis()
    if r is not None:
        async for _ in r.scan_iter(match=_KEY + "*", count=500):
            live += 1
    return {"blobs": count, "bytes": total, "live_tokens": live,
            "redis": r is not None}
