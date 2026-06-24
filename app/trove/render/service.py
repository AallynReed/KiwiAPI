"""Render a blueprint to a PNG, cached in Redis (base64, TTL'd) so we don't
re-rasterize on every hit and don't spend disk while testing."""
from __future__ import annotations

import asyncio
import base64
import logging
from typing import Any

from app.core.config import settings
from app.core.redis import get_redis
from app.trove.render.source import get_blueprint_bytes
from app.trove.render.voxel import BlueprintError, render_blueprint_png

logger = logging.getLogger("kiwi.render")


def _key(branch: str, name: str, dim: int) -> str:
    norm = name.replace("\\", "/").lower()
    return f"render:bp:{branch}:{dim}:{norm}"


async def render_blueprint_cached(
    name: str, dim: int = 256, branch: str | None = None,
) -> bytes | None:
    """Rendered PNG bytes for a blueprint name, or None if the blueprint isn't
    found. Decoded-but-unrenderable blueprints raise ``BlueprintError``."""
    branch = branch or settings.trove_render_branch
    redis: Any = get_redis()
    key = _key(branch, name, dim)

    if redis is not None:
        try:
            cached = await redis.get(key)
            if cached:
                return base64.b64decode(cached)
        except Exception:  # noqa: BLE001 - cache is best-effort
            logger.warning("render: redis get failed for %s", key, exc_info=True)

    data = await get_blueprint_bytes(name, branch)
    if data is None:
        return None
    # rasterizing is CPU-bound numpy work -> keep it off the event loop.
    # contain=True: centred, frame-filling thumbnail (not catalog-faithful framing).
    png = await asyncio.to_thread(render_blueprint_png, data, dim, None, True)

    if redis is not None:
        try:
            await redis.set(key, base64.b64encode(png).decode("ascii"),
                            ex=settings.trove_render_cache_ttl)
        except Exception:  # noqa: BLE001
            logger.warning("render: redis set failed for %s", key, exc_info=True)
    return png


__all__ = ["render_blueprint_cached", "BlueprintError"]
