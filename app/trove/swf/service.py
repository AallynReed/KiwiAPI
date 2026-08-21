"""Turn a ``.swf`` blob into a browsable, cached asset manifest.

Extraction is expensive (a second or more for an art-heavy movie) and perfectly
deterministic, so it happens once per distinct file and is remembered:

* every extracted image goes into the shared content store, so the *same* icon
  shipped in fifty game versions is one file on disk;
* the manifest - ids, names, sizes and the store hashes - is cached by
  :mod:`app.trove.render.bp_cache` under the movie's own content hash.

The manifest is what the gallery lists; the bytes are fetched per asset, and only
for the ones actually looked at.
"""

from __future__ import annotations

import asyncio
import io
import logging
import zipfile

from app.core.config import settings
from app.core.errors import APIError, ErrorCode
from app.trove.render import bp_cache
from app.trove.swf import decompile
from app.trove.swf.extract import SwfError, extract_images
from app.trove.updates.cas import ContentStore

logger = logging.getLogger("kiwi.swf")

_store = ContentStore(settings.mods_store_dir)

MAX_ZIP_BYTES = 256 * 1024 * 1024

# Decompiling spawns a JVM with its own heap, so the cost of a code-view open is
# nothing like that of an asset extraction. Bound how many can run at once - the
# work is cached, so the queue behind this only ever forms on cold movies.
_decompile_gate = asyncio.Semaphore(settings.ffdec_max_concurrent)


def _build_sync(raw: bytes) -> dict:
    """Extract, store the bytes, return the manifest. CPU-bound - run in a thread."""
    header, images, inventory = extract_images(raw)
    assets = []
    for img in images:
        sha, _ = _store.put(img.data)
        thumb_sha = _store.put(img.thumb)[0] if img.thumb else None
        assets.append({
            "id": img.char_id,
            "name": img.name,
            "source": img.source,
            "codec": img.codec,
            "width": img.width,
            "height": img.height,
            "mime": img.mime,
            "bytes": len(img.data),
            "sha": sha,
            "thumb_sha": thumb_sha,
        })
    return {
        "swf": {
            "version": header.version,
            "compression": header.compression,
            "width": header.width,
            "height": header.height,
            "frame_rate": round(header.frame_rate, 2),
            "frame_count": header.frame_count,
        },
        "inventory": inventory,
        "assets": assets,
        "count": len(assets),
    }


async def manifest(raw: bytes, content_sha: str) -> dict:
    """The cached asset manifest for one movie, extracting it on a miss."""

    async def build() -> dict:
        try:
            return await asyncio.to_thread(_build_sync, raw)
        except SwfError as exc:
            logger.info("swf: cannot read movie %s: %s", content_sha[:12], exc)
            raise APIError(422, ErrorCode.bad_request,
                           "This file could not be read as a Flash movie.") from None

    cached = await bp_cache.get_or_build(bp_cache.key_for_swf(content_sha), build)
    return await asyncio.to_thread(cached.payload)


async def decompile_throttle(request) -> None:
    """Per-IP bucket for the code view, shared by the ``/v1`` route and the
    same-origin ``/site`` proxy so one visitor can't spend both budgets.

    A movie nobody has opened yet costs a JVM, which is unlike every other read on
    a release - hence its own bucket rather than the shared anonymous budget. Only
    the first open of a given movie pays it; the rest are cache reads.
    """
    from app.admin import runtime_config
    from app.core.ratelimit import check_rate_limit
    from app.core.utils import client_ip

    max_, window = await runtime_config.get_rate_limit("swf_decompile_rate_limit")
    await check_rate_limit(f"swfcode:{client_ip(request) or 'unknown'}", max_, window)


async def scripts(raw: bytes, content_sha: str) -> dict:
    """The decompiled ActionScript of one movie, decompiling it on a miss.

    Cached under the movie's own content hash, so the JVM runs once per distinct
    ``.swf`` no matter how many releases ship it or how often it is opened.
    """
    if not decompile.available():
        raise APIError(503, ErrorCode.service_unavailable,
                       "The Flash decompiler is not available on this server.")

    async def build() -> dict:
        async with _decompile_gate:
            try:
                return await asyncio.to_thread(decompile.decompile_scripts, raw)
            except decompile.DecompilerUnavailable as exc:
                logger.warning("swf: decompiler unavailable: %s", exc)
                raise APIError(503, ErrorCode.service_unavailable,
                               "The Flash decompiler is not available on this "
                               "server.") from None
            except decompile.DecompileError as exc:
                logger.info("swf: cannot decompile %s: %s", content_sha[:12], exc)
                raise APIError(422, ErrorCode.bad_request, str(exc)) from None

    cached = await bp_cache.get_or_build(bp_cache.key_for_swf_scripts(content_sha), build)
    return await asyncio.to_thread(cached.payload)


async def asset_bytes(sha: str) -> bytes | None:
    """Raw bytes of one extracted asset, straight out of the store."""
    return await asyncio.to_thread(_store.get, sha)


def _zip_name(asset: dict, taken: set[str]) -> str:
    ext = {"image/jpeg": "jpg", "image/gif": "gif"}.get(asset["mime"], "png")
    stem = asset.get("name") or f"asset_{asset['id']}"
    stem = "".join(c if c.isalnum() or c in "._- " else "_" for c in stem).strip() or "asset"
    base = f"{asset['id']:04d}_{stem}"
    name = f"{base}.{ext}"
    n = 2
    while name in taken:
        name = f"{base}_{n}.{ext}"
        n += 1
    taken.add(name)
    return name


def build_zip(assets: list[dict]) -> bytes:
    """Every asset in one archive, named ``<id>_<symbol>.<ext>``.

    The id prefix keeps the order stable and guarantees uniqueness even when two
    bitmaps resolved to the same symbol name, which happens whenever a sprite
    holds several frames of the same artwork.
    """
    out = io.BytesIO()
    taken: set[str] = set()
    total = 0
    with zipfile.ZipFile(out, "w", zipfile.ZIP_STORED) as zf:
        for asset in assets:
            data = _store.get(asset["sha"])
            if data is None:
                continue
            total += len(data)
            if total > MAX_ZIP_BYTES:
                raise APIError(413, ErrorCode.bad_request,
                               "These assets are too large to download as one archive.")
            zf.writestr(_zip_name(asset, taken), data)
    return out.getvalue()
