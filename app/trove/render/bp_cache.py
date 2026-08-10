"""Cache of decoded ``.blueprint`` voxel payloads.

Decoding is the expensive half of every 3D preview: read the whole ``.tmod`` out
of the store, parse the archive, decode the voxels, then serialise a payload that
runs to megabytes for a big model - all of it repeated on every single viewer
open, for a result that never changes.

That payload is a pure function of the blueprint's bytes, so it is cached in two
levels:

  index   ``BlueprintCacheEntry`` - keyed on something the caller knows *without*
          doing the expensive work (the container's content hash + the path
          inside it), holding the store sha of the payload.
  blob    the gzipped JSON body, in the content-addressed store - so the same
          model shipped in twenty releases collapses to one copy on disk.

Keys are content hashes, so entries are immutable and nothing is ever
invalidated. Bumping ``PACK_VERSION`` opens a fresh generation and orphans the
old rows (prunable by key prefix). Failures are cached too - an empty
placeholder or an over-cap model answers instantly instead of being re-decoded
on every hit.

The body is stored already-compressed and served verbatim with the blob sha as
its ``ETag``, so a warm open costs one indexed lookup plus one file read, and a
*repeat* open usually costs a 304 with no body at all.
"""

from __future__ import annotations

import asyncio
import gzip
import json
import logging
import time
from collections.abc import Awaitable, Callable
from dataclasses import dataclass

from fastapi import Request, Response
from pymongo.errors import DuplicateKeyError

from app.core.config import settings
from app.core.errors import APIError
from app.trove.render import binpack
from app.trove.render.models import BlueprintCacheEntry
from app.trove.updates.cas import ContentStore

logger = logging.getLogger("kiwi.render")

# Bump when ``pack_blueprint``'s output shape or the voxel/material mapping
# changes - it namespaces every key, so a new generation is built from scratch
# and the old rows fall out of use (delete by ``key`` prefix to reclaim).
PACK_VERSION = "v2"

# Bump when the baked rigs (``mods_hub/rigs``) or ``mods_hub/assembly.py`` change
# the shape of an assembled creature. The live rig map has its own signature in the
# key (see ``key_for_assembly``); this covers what ships in the repo.
ASSEMBLY_VERSION = "a6"      # a6: measured 1/12 voxel size; head slots keep their 2x-art halving

# Derived payloads share the mods content store: same sharded, atomic, dedupe-by-
# content primitive, already bind-mounted, and a cache blob that goes missing is
# simply rebuilt on the next hit.
_store = ContentStore(settings.mods_store_dir)

# Decode outcomes worth remembering. A 404 is deliberately NOT cached: it can mean
# a blob that's temporarily missing rather than a file that can't be decoded.
_CACHEABLE_ERRORS = (413, 422)


# ── keys ───────────────────────────────────────────────────────────────────

def _norm(path: str) -> str:
    return (path or "").strip().replace("\\", "/").lstrip("/").lower()


def key_for_tmod(tmod_sha: str, path: str) -> str:
    """Key for one blueprint inside a ``.tmod`` artifact we host (a hub release).

    NOT for a partner's embed upload: those bytes are held in Redis for a short
    TTL and never written to disk, and a decoded copy of a mod is still the mod -
    see ``build_uncached``."""
    return f"{PACK_VERSION}:tmod:{tmod_sha}:{_norm(path)}"


def key_for_file(content_sha: str, path: str) -> str:
    """Key for a standalone ``.blueprint`` blob (the updates archive / game tree)."""
    return f"{PACK_VERSION}:file:{content_sha}:{_norm(path)}"


def key_for_assembly(rig_sig: str, ident: str) -> str:
    """Key for an assembled creature - many parts on a baked rig.

    Two things decide that payload beyond the parts themselves: the binfab-derived
    rig map (``rig_index.index_signature`` - a reindex can move a part to a
    different attach point) and our own baked rigs + assembler (``ASSEMBLY_VERSION``).
    Both are in the key, so neither can go stale silently. ``ident`` names the
    source: ``tmod:<sha>`` for a release, ``game:<skeleton>:<folder>`` for a native
    creature."""
    return f"{PACK_VERSION}:{ASSEMBLY_VERSION}:asm:{rig_sig}:{_norm(ident)}"


# ── the cached body ────────────────────────────────────────────────────────

class NoPayload(Exception):
    """Raised by a builder that found nothing to serve - a mod whose parts don't
    place on any known rig, say. Not an error and not cached: the caller turns it
    into its own "no model" answer (a 404, usually)."""


@dataclass(frozen=True)
class Cached:
    """A ready-to-serve payload: ``gz`` is the gzipped JSON body, ``etag`` its
    content hash. ``payload()`` is only for callers that need the dict back.

    ``hit``/``build_ms`` exist so the cache can be *seen* working: they become
    ``X-Kiwi-Cache`` and ``Server-Timing`` on the response, which is the only way
    to tell a warm open from a cold one without instrumenting the client. ``off``
    means nothing was stored (index unreachable, or a source we don't keep)."""

    etag: str
    gz: bytes
    count: int
    hit: bool = False
    build_ms: float = 0.0
    stored: bool = True
    media_type: str = "application/json"

    @property
    def state(self) -> str:
        return "hit" if self.hit else ("miss" if self.stored else "off")

    def payload(self) -> dict:
        """The decoded dict, whichever wire format this was built in. CPU-bound for
        a large model - call via ``asyncio.to_thread``."""
        raw = gzip.decompress(self.gz)
        if raw[:4] == binpack.MAGIC:
            return binpack.decode(raw)
        return json.loads(raw)


def _encode(payload: dict) -> bytes:
    """JSON-serialise + compress. CPU-bound; runs in a thread."""
    body = json.dumps(payload, separators=(",", ":")).encode("utf-8")
    return gzip.compress(body, 6)


def _encode_bin(payload: dict) -> bytes:
    """Pack into the KVX1 binary container + compress. CPU-bound; runs in a thread."""
    return gzip.compress(binpack.encode(payload), 6)


# Wire formats a caller can ask for. ``bin`` is what the 3D viewers fetch - same
# object shape on arrival, but typed arrays instead of millions of parsed JS
# numbers. ``json`` stays the default so the documented API contract is unchanged.
FORMATS: dict[str, tuple[Callable[[dict], bytes], str]] = {
    "json": (_encode, "application/json"),
    "bin": (_encode_bin, "application/x-kiwi-voxel"),
}


# ── build / lookup ─────────────────────────────────────────────────────────

async def get_or_build(key: str, build: Callable[[], Awaitable[dict]], fmt: str = "json") -> Cached:
    """The cached payload for ``key`` in wire format ``fmt``, building it with
    ``build()`` on a miss.

    ``build`` is only awaited when there's nothing cached, so the caller can put
    everything expensive (fetching the .tmod, parsing it, decoding) inside it.
    An ``APIError`` it raises with a decode status (413/422) is remembered and
    re-raised on later hits.

    Each format is stored under its own key: the model is decoded once per format
    that's actually requested, and in practice a given model is only ever fetched
    one way (the viewers ask for ``bin``, the documented API serves ``json``).
    """
    encode, media_type = FORMATS[fmt]
    key = key if fmt == "json" else f"{key}:{fmt}"
    entry, usable = await _lookup(key)
    if entry is not None:
        if entry.err_status:
            raise APIError(entry.err_status, entry.err_code or "bad_request",
                           entry.err_msg or "This blueprint can't be previewed.")
        if entry.blob_sha:
            gz = await _read_blob(entry.blob_sha)
            if gz is not None:
                return Cached(entry.blob_sha, gz, entry.voxel_count,
                              hit=True, media_type=media_type)
            # Blob pruned out from under the index - drop the row and rebuild.
            logger.info("bp_cache: blob %s missing for %s, rebuilding", entry.blob_sha, key)
            await _forget(entry)

    started = time.perf_counter()
    try:
        payload = await build()
    except APIError as exc:
        if usable and exc.status_code in _CACHEABLE_ERRORS:
            await _remember(BlueprintCacheEntry(
                key=key, err_status=exc.status_code, err_code=exc.code, err_msg=exc.message,
            ))
        raise

    gz = await asyncio.to_thread(encode, payload)
    count = int(payload.get("count") or 0)
    ms = (time.perf_counter() - started) * 1000
    sha = await _write_blob(gz) if usable else None
    if sha is None:                      # store or index unreachable: serve, keep nothing
        logger.info("bp_cache: built %s in %.0fms, NOT stored (index/store unavailable)", key, ms)
        return Cached("", gz, count, build_ms=ms, stored=False, media_type=media_type)
    await _remember(BlueprintCacheEntry(
        key=key, blob_sha=sha, byte_len=len(gz),
        voxel_count=count, size=payload.get("size"),
    ))
    logger.info("bp_cache: built %s in %.0fms (%d voxels, %d KB stored)",
                key, ms, count, len(gz) // 1024)
    return Cached(sha, gz, count, build_ms=ms, media_type=media_type)


async def build_uncached(build: Callable[[], Awaitable[dict]], fmt: str = "json") -> Cached:
    """Package a payload for the same serving path *without* storing it anywhere.

    For sources we've promised not to keep - a partner's uploaded ``.tmod`` lives
    in Redis under its own TTL and never touches disk, and its decoded voxels are
    just as much their file as the bytes are. They pay the decode every view;
    that's the deal.
    """
    encode, media_type = FORMATS[fmt]
    started = time.perf_counter()
    payload = await build()
    gz = await asyncio.to_thread(encode, payload)
    return Cached("", gz, int(payload.get("count") or 0),
                  build_ms=(time.perf_counter() - started) * 1000,
                  stored=False, media_type=media_type)


async def _lookup(key: str) -> tuple[BlueprintCacheEntry | None, bool]:
    """``(entry, index_usable)``. A cache is an optimisation, never a dependency:
    if the index can't be read the preview is decoded and served as before, and
    nothing is written."""
    try:
        return await BlueprintCacheEntry.find_one(BlueprintCacheEntry.key == key), True
    except Exception:  # noqa: BLE001
        logger.warning("bp_cache: index unavailable, decoding uncached", exc_info=True)
        return None, False


async def _read_blob(sha: str) -> bytes | None:
    try:
        return await asyncio.to_thread(_store.get, sha)
    except OSError:
        logger.warning("bp_cache: could not read blob %s", sha, exc_info=True)
        return None


async def _write_blob(gz: bytes) -> str | None:
    """The blob's sha once stored, or None if the store wouldn't take it - in which
    case the payload is still served, just not remembered."""
    try:
        sha, _created = await asyncio.to_thread(_store.put, gz)
    except OSError:
        logger.warning("bp_cache: could not write a %d-byte payload", len(gz), exc_info=True)
        return None
    return sha


async def _forget(entry: BlueprintCacheEntry) -> None:
    try:
        await entry.delete()
    except Exception:  # noqa: BLE001
        logger.warning("bp_cache: could not drop stale row %s", entry.key, exc_info=True)


async def _remember(entry: BlueprintCacheEntry) -> None:
    """Insert an index row, tolerating the race where a concurrent first-open of
    the same model already wrote it (both built the identical payload)."""
    try:
        await entry.insert()
    except DuplicateKeyError:
        pass
    except Exception:  # noqa: BLE001 - the cache is best-effort, never fatal
        logger.warning("bp_cache: could not record %s", entry.key, exc_info=True)


# ── serving ────────────────────────────────────────────────────────────────

def respond(request: Request, cached: Cached, *, max_age: int = 300) -> Response:
    """Serve a cached payload: the stored bytes verbatim when the client takes
    gzip (every browser does), decompressed otherwise, and a bodiless 304 when
    the client already holds this exact model."""
    headers = {
        "Cache-Control": f"public, max-age={max_age}",
        "Vary": "Accept-Encoding",
        # Whether THIS response came out of the cache, readable in DevTools' Network
        # panel (and Server-Timing draws the build cost in the waterfall). Without
        # these you cannot tell a warm open from a cold one from the outside.
        "X-Kiwi-Cache": cached.state,
        "Server-Timing": (f'bpcache;desc="{cached.state}"' if cached.hit else
                          f'bpcache;desc="{cached.state}";dur={cached.build_ms:.0f}'),
    }
    if cached.etag:                      # empty only when the payload wasn't cached
        etag = f'"{cached.etag}"'
        headers["ETag"] = etag
        if _matches(request.headers.get("if-none-match"), etag):
            return Response(status_code=304, headers=headers)

    if "gzip" in request.headers.get("accept-encoding", "").lower():
        headers["Content-Encoding"] = "gzip"
        return Response(content=cached.gz, media_type=cached.media_type, headers=headers)
    return Response(content=gzip.decompress(cached.gz),
                    media_type=cached.media_type, headers=headers)


def _matches(header: str | None, etag: str) -> bool:
    if not header:
        return False
    return any(t.strip().removeprefix("W/") == etag for t in header.split(","))


__all__ = ["ASSEMBLY_VERSION", "PACK_VERSION", "Cached", "NoPayload", "build_uncached",
           "get_or_build", "key_for_assembly", "key_for_file", "key_for_tmod", "respond"]
