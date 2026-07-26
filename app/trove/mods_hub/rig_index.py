"""Live rig resolution: which baked creature rig a mod's blueprint parts belong to.

AUTHORITATIVE, not heuristic. The game's prefab binfabs bind each creature's
blueprint meshes to a skeleton (``<name>.skeleton.gr2``) and a per-part ``AP_*``
attach point. The indexer's ``reindex_rigs`` extracts that from EVERY skeleton-binding
prefab (mounts, allies' ``_npc``, skins/costumes, npc/mobs) into the ``rig_binding``
table on every game sync, so this resolver just reads the live map - a ``blueprint
basename -> (skeleton, AP key)`` lookup - and matches the mod's blueprint basenames
against it. It is cached in-process and refreshes automatically when the branch is
reindexed (the cache key is the codex's ``(parser_version, updated_at)`` signature,
which ``reindex_rigs`` bumps).

When the map has no match (a brand-new creature not yet archived, an NPC the codex
doesn't classify, or Postgres disabled in dev) it returns ``(None, {})`` and the
caller renders no assembled model - there is NO name-overlap heuristic fallback, so a
mod is never rendered onto a guessed/wrong skeleton. We either know the rig from the
game's own prefab data or we don't render it.
"""
from __future__ import annotations

import asyncio
from collections import Counter

from app.core.config import settings
from app.trove.codexes import pg_store

# branch -> (codex signature, {blueprint basename: (skeleton, ap key)})
_cache: dict[str, tuple] = {}
_lock = asyncio.Lock()


async def _rig_map(branch: str) -> dict[str, tuple[str, str]]:
    """The branch's rig map, rebuilt only when the codex index signature changes."""
    if not settings.postgres_enabled:
        return {}
    sig = await pg_store.meta_signature(branch)
    cached = _cache.get(branch)
    if cached and cached[0] == sig:
        return cached[1]
    async with _lock:
        cached = _cache.get(branch)            # re-check after awaiting the lock
        if cached and cached[0] == sig:
            return cached[1]
        rig_map = await pg_store.load_rig_map(branch)
        _cache[branch] = (sig, rig_map)
        return rig_map


async def resolve(
    part_basenames: list[str], branch: str | None = None
) -> tuple[str | None, dict[str, str]]:
    """Resolve a mod's blueprint basenames to ``(skeleton stem, {basename: AP key})``
    from the live binfab-derived map, or ``(None, {})`` if nothing matches.

    The winning skeleton is the one the most matched parts agree on (a mod is almost
    always parts of a single creature); only that skeleton's parts are returned."""
    branch = branch or settings.trove_render_branch
    rig_map = await _rig_map(branch)
    if not rig_map:
        return None, {}
    hits = {b: rig_map[b] for b in part_basenames if b in rig_map}
    if not hits:
        return None, {}
    skeleton = Counter(skel for skel, _ap in hits.values()).most_common(1)[0][0]
    attach = {b: ap for b, (skel, ap) in hits.items() if skel == skeleton}
    return skeleton, attach


async def parts_for(
    skeleton: str, branch: str | None = None
) -> dict[str, str]:
    """The REVERSE lookup: every blueprint basename the binfab map binds to
    ``skeleton``, as ``{basename: AP key}`` (empty if unknown).

    ``resolve`` answers "which creature do these parts belong to"; this answers
    "which parts make up this creature" - what the embeddable viewer needs to
    assemble a native game creature from a single blueprint path, where the caller
    has one part and wants the whole model. Same authoritative source, so it
    inherits the no-guess rule: a skeleton with no bindings returns nothing."""
    branch = branch or settings.trove_render_branch
    rig_map = await _rig_map(branch)
    return {b: ap for b, (skel, ap) in rig_map.items() if skel == skeleton}
