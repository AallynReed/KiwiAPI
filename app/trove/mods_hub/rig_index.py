"""Live rig resolution: which baked creature rig a mod's blueprint parts belong to.

AUTHORITATIVE, not heuristic. The game's prefab binfabs bind each creature's
blueprint meshes to a skeleton (``<name>.skeleton.gr2``) and a per-part ``AP_*``
attach point. The indexer's ``reindex_rigs`` extracts that from EVERY skeleton-binding
prefab (mounts, allies' ``_npc``, skins/costumes, npc/mobs) into the ``rig_binding``
table on every game sync, so this resolver just reads the live map and matches the
mod's blueprint basenames against it. It is cached in-process and refreshes
automatically when the branch is reindexed (the cache key is the codex's
``(parser_version, updated_at)`` signature, which ``reindex_rigs`` bumps).

Two questions, two indexes, one table:

  ``resolve``       "which creature do THESE parts belong to" - a mod hands us its
                    blueprint basenames and gets back a skeleton + attach points.
  ``creature_for``  "which creature owns THIS part, and what else is in it" - the
                    embed has one game file path and wants the whole model.

The second is why a binding carries its source **prefab**. One prefab is one creature,
but a *skeleton* is shared by every creature that uses it (``mount_raptor`` covers every
raptor mount in the game) and every blueprint lives in the same flat ``blueprints/``
folder - so neither the skeleton nor the path can name one creature. Only the prefab can.

When the map has no match (a brand-new creature not yet archived, an NPC the codex
doesn't classify, or Postgres disabled in dev) it returns nothing and the caller renders
no assembled model - there is NO name-overlap heuristic fallback, so a mod is never
rendered onto a guessed/wrong skeleton. We either know the rig from the game's own
prefab data or we don't render it.
"""
from __future__ import annotations

import asyncio
import re
from collections import Counter
from dataclasses import dataclass, field

from app.core.config import settings
from app.trove.codexes import pg_store

# branch -> (codex signature, RigMap)
_cache: dict[str, tuple] = {}
_lock = asyncio.Lock()


@dataclass(frozen=True)
class RigMap:
    """Both views of a branch's bindings, built in one pass over the ordered rows."""

    # blueprint basename -> (skeleton, AP key). The mod-side lookup. A basename shared
    # by several prefabs collapses here: they are the same mesh at the same bone, so
    # every row agrees on the answer this index gives.
    by_blueprint: dict[str, tuple[str, str]] = field(default_factory=dict)
    # prefab path -> (skeleton, {blueprint basename: AP key}) - ONE creature's part set.
    creatures: dict[str, tuple[str, dict[str, str]]] = field(default_factory=dict)
    # blueprint basename -> prefab that owns it (first in the store's stable ordering).
    owner: dict[str, str] = field(default_factory=dict)

    def __bool__(self) -> bool:
        return bool(self.by_blueprint)


def _build(rows: list[tuple[str, str, str, str]]) -> RigMap:
    """One pass over the store's (prefab, blueprint)-ordered rows. ``setdefault`` makes
    "first row wins" the rule for the two basename-keyed indexes, which that ordering
    then makes deterministic."""
    by_blueprint: dict[str, tuple[str, str]] = {}
    creatures: dict[str, tuple[str, dict[str, str]]] = {}
    owner: dict[str, str] = {}
    for prefab, blueprint, skeleton, ap in rows:
        by_blueprint.setdefault(blueprint, (skeleton, ap))
        owner.setdefault(blueprint, prefab)
        creatures.setdefault(prefab, (skeleton, {}))[1][blueprint] = ap
    return RigMap(by_blueprint=by_blueprint, creatures=creatures, owner=owner)


_STYLE_RE = re.compile(r"\[[^\]]*\]")          # a Trove style suffix: name[stylename]
_TIER_RE = re.compile(r"_lvl\d+")


def _match_variant(basename: str, rig_map: RigMap) -> tuple[str, str] | None:
    """A styled or tiered spelling of a part the map does know.

    Trove writes a cosmetic variant as ``<base>[style]`` and a tier as ``<base>_lvlN``,
    so a mod shipping ``equipment_helm_crimefighter_lvl2[technoshyft]`` never matched the
    map's ``equipment_helm_crimefighter`` and its head silently went missing. Only these
    two documented suffixes are stripped, and only down to a name the map already binds -
    an unknown name still resolves to nothing rather than being guessed onto a rig."""
    seen = {basename}
    for cand in (_STYLE_RE.sub("", basename),
                 _TIER_RE.sub("", basename),
                 _TIER_RE.sub("", _STYLE_RE.sub("", basename))):
        cand = cand.strip().strip("_")
        if not cand or cand in seen:
            continue
        seen.add(cand)
        found = rig_map.by_blueprint.get(cand)
        if found is not None:
            return found
    return None


async def _rig_map(branch: str) -> RigMap:
    """The branch's rig map, rebuilt only when the codex index signature changes."""
    if not settings.postgres_enabled:
        return RigMap()
    sig = await pg_store.meta_signature(branch)
    cached = _cache.get(branch)
    if cached and cached[0] == sig:
        return cached[1]
    async with _lock:
        cached = _cache.get(branch)            # re-check after awaiting the lock
        if cached and cached[0] == sig:
            return cached[1]
        rig_map = _build(await pg_store.load_rig_bindings(branch))
        _cache[branch] = (sig, rig_map)
        return rig_map


async def rig_map(branch: str | None = None) -> RigMap:
    """The whole live map. The dressing room needs the ``skins/`` rows in bulk (every
    costume and its parts), which is the one caller that wants the map itself rather
    than an answer about one creature."""
    return await _rig_map(branch or settings.trove_render_branch)


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
    hits = {}
    for b in part_basenames:
        found = rig_map.by_blueprint.get(b)
        if found is None:
            found = _match_variant(b, rig_map)
        if found is not None:
            hits[b] = found
    if not hits:
        return None, {}
    skeleton = Counter(skel for skel, _ap in hits.values()).most_common(1)[0][0]
    attach = {b: ap for b, (skel, ap) in hits.items() if skel == skeleton}
    return skeleton, attach


async def creature_by_prefab(
    prefab: str, branch: str | None = None
) -> tuple[str | None, dict[str, str]]:
    """``(skeleton stem, {blueprint basename: AP key})`` for ONE creature prefab, or
    ``(None, {})``. The codex knows a mount/dragon by its prefab path, so it asks this
    way round; ``creature_for`` is the same lookup entered from a part instead."""
    branch = branch or settings.trove_render_branch
    rig_map = await _rig_map(branch)
    found = rig_map.creatures.get(prefab.replace("\\", "/"))
    if not found:
        return None, {}
    skeleton, parts = found
    return skeleton, dict(parts)


async def creature_for(
    basename: str, branch: str | None = None
) -> tuple[str | None, str | None, dict[str, str]]:
    """The ONE creature a game blueprint belongs to:
    ``(skeleton stem, prefab path, {blueprint basename: AP key})``, or ``(None, None, {})``.

    ``resolve`` answers "which creature do these parts belong to"; this answers "which
    creature is this part OF, and which other parts complete it" - what the embeddable
    viewer needs to assemble a native game creature from a single blueprint path, where
    the caller has one part and wants the whole model. Same authoritative source, so it
    inherits the no-guess rule: an unbound basename returns nothing.

    A mesh reused by several creature prefabs resolves to the first in the store's
    stable ordering. That is arbitrary but not a *guess*: every candidate is a real
    creature the game itself says contains that part, and the game gives us nothing
    finer to choose by. It never risks the failure this replaces - assembling parts
    that belong to no single creature at all.
    """
    branch = branch or settings.trove_render_branch
    rig_map = await _rig_map(branch)
    prefab = rig_map.owner.get(basename)
    if not prefab:
        return None, None, {}
    skeleton, parts = rig_map.creatures[prefab]
    return skeleton, prefab, dict(parts)


async def index_signature(branch: str | None = None) -> str | None:
    """A token that changes whenever the rig map could have - the same codex
    ``(parser_version, updated_at)`` signature ``_rig_map`` keys its own cache on.

    Anything derived from the map (an assembled creature) must carry this in its
    cache key, or a reindex that moves a part to a different attach point would
    keep serving the old model. ``None`` when there is no live map at all
    (Postgres disabled in dev) - a caller with no signature must not cache."""
    branch = branch or settings.trove_render_branch
    if not settings.postgres_enabled:
        return None
    version, updated = await pg_store.meta_signature(branch)
    if not updated:
        return None
    return f"{branch}:{version}:{int(updated.timestamp())}"
