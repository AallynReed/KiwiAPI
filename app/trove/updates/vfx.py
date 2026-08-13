"""VFX (PopcornFX ``.pkfx``) browsing + preview over the archived game files.

The web viewer (``site/static/pkfx``) parses, simulates and draws an effect in the
browser; all it needs from us is the ``.pkfx`` text plus the assets that effect
names. This is the game-tree half of what ``app/trove/mods_hub/vfx.py`` does for a
mod release, and it shares that module's reference parser.

Trove ships **one** PopcornFX pack - the folder holding ``popcornproject.xml`` -
and every reference inside an effect is relative to its root, so
``Textures/vfx_ray_01.dds`` is exactly that path under the pack. The match is
case-insensitive (the archive stores the CDN manifest's spelling, and the effects
disagree with it: ``VFX_circle_10.dds`` ships as ``vfx_circle_10.dds``) but it is
still an exact *path* match. A reference that isn't in the archived tree is
reported missing rather than resolved to a same-named file elsewhere in the game.
"""

from __future__ import annotations

import asyncio
from typing import NamedTuple

from app.core.config import settings
from app.trove.mods_hub import vfx as pkfx
from app.trove.updates import read as updates_read
from app.trove.updates.cas import ContentStore
from app.trove.updates.models import UpdateBranch, UpdateState

# Pack root, lowercased (every path in the index is compared lowercased).
PACK_ROOT = "particles/vfx/"

# The PopcornFX editor renders a still of each effect beside the project, named
# after the effect file itself (``foo.pkfx`` -> ``foo.pkfx.png``). Only some
# effects have one; the rest list without a thumbnail rather than with a guess.
_THUMB_DIR = PACK_ROOT + "editor/thumbnails/particles/"

_ASSET_SUFFIXES = tuple("." + e for e in pkfx.ASSET_EXTS)


class _Index(NamedTuple):
    ordinal: int
    effects: list[dict]      # every .pkfx in the branch: [{path, size, thumb}], by path
    paths: dict[str, str]    # lowercased path -> archived path, pack subtree only


_CACHE: dict[str, _Index] = {}
_BUILD_LOCK = asyncio.Lock()


async def _index(branch: str) -> _Index:
    """Effect list + pack path map for one branch, rebuilt when that branch takes a
    new build.

    One projected scan of the tree covers both, so neither browsing ~9k effects nor
    resolving an effect's assets costs a regex scan over 140k paths per request.
    """
    ub = await UpdateBranch.find_one(UpdateBranch.branch == branch)
    ordinal = (ub.current_ordinal if ub else 0) or 0
    hit = _CACHE.get(branch)
    if hit is not None and hit.ordinal == ordinal:
        return hit
    async with _BUILD_LOCK:
        hit = _CACHE.get(branch)          # someone may have built it while we waited
        if hit is not None and hit.ordinal == ordinal:
            return hit
        effects: list[dict] = []
        paths: dict[str, str] = {}
        cursor = UpdateState.get_pymongo_collection().find(
            {"branch": branch}, {"path": 1, "size": 1, "_id": 0},
        )
        async for doc in cursor:
            path = doc.get("path")
            if not path:
                continue
            low = path.lower()
            if low.startswith(PACK_ROOT):
                paths[low] = path
            if low.endswith(".pkfx"):
                effects.append({"path": path, "size": doc.get("size", 0), "thumb": None})
                # Every listed effect must be openable. Trove keeps them all inside
                # the pack today, so this only matters if a build ever moves one -
                # better a playable effect resolving its assets against the pack
                # than a row that 404s when clicked.
                paths.setdefault(low, path)
        for e in effects:
            e["thumb"] = paths.get(_THUMB_DIR + pkfx.basename(e["path"]).lower() + ".png")
        # By filename, not by path: effects are named, not filed - all but a
        # handful share one folder, and the strays that don't (a few sit in the
        # editor's thumbnail folder) shouldn't jump the alphabet because of it.
        effects.sort(key=lambda e: (pkfx.basename(e["path"]).lower(), e["path"].lower()))
        built = _Index(ordinal, effects, paths)
        _CACHE[branch] = built
        return built


def _resolve(idx: _Index, ref: str) -> str | None:
    """Archived path for a reference - a full game path or one relative to the pack
    root - or None when the branch doesn't ship it."""
    r = (ref or "").replace("\\", "/").lstrip("/").lower()
    if not r:
        return None
    return idx.paths.get(r) or idx.paths.get(PACK_ROOT + r)


async def _read(branch: str, path: str) -> bytes | None:
    meta = await updates_read.get_file_meta(branch, path)
    if meta is None:
        return None
    store = ContentStore(settings.trove_update_store_dir)
    return await asyncio.to_thread(store.get, meta["content_sha256"])


async def list_effects(branch: str, q: str = "", limit: int = 300, offset: int = 0) -> dict:
    """A page of the branch's effects, newest tree only, with the true match total.

    ``q`` is a plain case-insensitive substring of the path, so the picker can filter
    all ~9k effects without shipping the whole list to the browser."""
    idx = await _index(branch)
    needle = (q or "").strip().lower()
    items = [e for e in idx.effects if needle in e["path"].lower()] if needle else idx.effects
    page = items[offset:offset + limit]
    return {"branch": branch, "items": page, "count": len(page), "total": len(items)}


async def manifest(branch: str, path: str) -> dict | None:
    """One effect's ``.pkfx`` text plus its references, each classified ``game`` or
    ``missing``. None when the branch has no such effect."""
    idx = await _index(branch)
    real = _resolve(idx, path)
    if real is None or not real.lower().endswith(".pkfx"):
        return None
    raw = await _read(branch, real)
    if raw is None:
        return None
    text = raw.decode("utf-8", "replace")
    deps = [
        {"ref": ref, "basename": pkfx.basename(ref),
         "source": "game" if _resolve(idx, ref) else "missing"}
        for ref in pkfx.extract_refs(text)
    ]
    return {
        "branch": branch, "path": real, "pkfx": text, "deps": deps,
        "missing": [d["basename"] for d in deps if d["source"] == "missing"],
        # The effect itself came out of the archive, so the game tree is by
        # definition reachable - a missing dep means the game doesn't ship it.
        "game_available": True,
    }


async def asset(branch: str, ref: str) -> tuple[bytes, str, str] | None:
    """``(bytes, media type, content sha)`` for one asset an effect references, or
    None when the branch doesn't ship it.

    Limited to render-relevant file types inside the pack - the same files the
    explorer already serves - so this stays a resolver, not a second file API."""
    if not ref.lower().endswith(_ASSET_SUFFIXES):
        return None
    idx = await _index(branch)
    real = _resolve(idx, ref)
    if real is None:
        return None
    meta = await updates_read.get_file_meta(branch, real)
    if meta is None:
        return None
    store = ContentStore(settings.trove_update_store_dir)
    raw = await asyncio.to_thread(store.get, meta["content_sha256"])
    if raw is None:
        return None
    return raw, pkfx.media_type_for(real), meta["content_sha256"]
