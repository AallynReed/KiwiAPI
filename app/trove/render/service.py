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


def _creature_key(branch: str, prefab: str, dim: int) -> str:
    return f"render:rig:{branch}:{dim}:{prefab.replace(chr(92), '/').lower()}"


async def render_creature_cached(
    prefab: str, dim: int = 256, branch: str | None = None,
) -> bytes | None:
    """PNG of a creature assembled from ALL the blueprint parts its prefab binds, or
    None when that isn't possible.

    A mount or dragon is a set of parts on a skeleton, not one model. The single
    blueprint the prefab names first is a torso or a jaw, and even the game's own
    ``_ui`` icon is a small stand-in - a dragon's is ~600 voxels against ~3,500 for the
    creature itself. So when the rig map and the archive can supply every part, we draw
    the whole animal.

    All-or-nothing on purpose: a partial assembly is a creature missing its legs, which
    is worse than the single blueprint it would replace. Any shortfall returns None and
    the caller falls back.
    """
    branch = branch or settings.trove_render_branch
    redis: Any = get_redis()
    key = _creature_key(branch, prefab, dim)

    if redis is not None:
        try:
            cached = await redis.get(key)
            if cached:
                return base64.b64decode(cached) or None
        except Exception:  # noqa: BLE001 - cache is best-effort
            logger.warning("render: redis get failed for %s", key, exc_info=True)

    png = await _build_creature_png(prefab, dim, branch)
    if redis is not None:
        try:
            # Negative results are cached too (as empty): "this prefab has no assembly"
            # is stable until the next game sync, and re-deriving it costs many reads.
            await redis.set(key, base64.b64encode(png or b"").decode("ascii"),
                            ex=settings.trove_render_cache_ttl)
        except Exception:  # noqa: BLE001
            logger.warning("render: redis set failed for %s", key, exc_info=True)
    return png


async def _build_creature_png(prefab: str, dim: int, branch: str) -> bytes | None:
    from app.trove.mods_hub import assembly, rig_index
    from app.trove.render.source import blueprint_by_basename
    from app.trove.render.voxel import render_voxels

    skeleton, parts = await rig_index.creature_by_prefab(prefab, branch)
    if not skeleton or not parts or not assembly.has_baked_rig(skeleton):
        return None
    index = await blueprint_by_basename(branch)

    wanted: list[tuple[str, bytes]] = []
    for basename, ap_key in parts.items():
        path = index.get(basename.lower())
        if not path:
            return None                       # a part we can't locate -> incomplete
        data = await get_blueprint_bytes(path, branch)
        if data is None:
            return None
        wanted.append((ap_key, data))

    voxels = await asyncio.to_thread(assembly.assemble_voxels, wanted, skeleton)
    if not voxels:
        return None
    try:
        rgba = await asyncio.to_thread(render_voxels, voxels, dim, {"fit": "tight"})
    except BlueprintError:
        return None
    return await asyncio.to_thread(_encode_png, rgba, dim)


def _encode_png(rgba, dim: int) -> bytes:
    import io

    from PIL import Image

    from app.trove.render.voxel import _contain_square
    out = io.BytesIO()
    Image.fromarray(_contain_square(rgba, dim), "RGBA").save(out, format="PNG", optimize=True)
    return out.getvalue()


__all__ = ["render_blueprint_cached", "render_creature_cached", "BlueprintError"]
