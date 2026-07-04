"""Recipe providers / benches: where a recipe is craftable.

Verified against the live archive: a recipe's craftable-at binding lives in the
crafting-station and profession prefabs, which LIST the recipes they expose -
`placeable/crafting/workbench_*_interactive.binfab` (the physical workbenches),
other `*_interactive`/`*_interactable` stations (guide UI, forges, vendors), and
`professions/*.binfab` (Gearcrafting, Runecrafting, …). We read those and invert the
relation.

The catch the first cut got wrong: these files DON'T store `recipes/<id>` paths -
they pack **bare `recipe_*` tokens** with binary framing bytes glued on (e.g.
`recipe_gearcrafting_weapon_bow_00D`). So we can't trust a raw token boundary; we
match each candidate against the AUTHORITATIVE set of real recipe ids (the
`recipes/*.binfab` stems) and trim glued framing bytes down to the longest known id.
A token that resolves to no real recipe is dropped - never a guessed bench.

Pure + stdlib-only.
"""

from __future__ import annotations

import re

from app.trove.codexes.binfab import harvest_strings

# A `recipe_*` identifier token (lowercased). May carry glued framing bytes on the
# tail (a following field's length byte is often a printable letter) - trimmed against
# the known-id set below.
_RECIPE_TOKEN_RE = re.compile(r"recipe_[a-z0-9_]+")

# Path fragments that mark a prefab as a crafting source. Narrow + verified: every
# recipe-listing prefab in the archive is an `*_interactive`/`*_interactable` station
# or a `professions/` skill. A cheap PATH prefilter so the indexer reads only these.
PROVIDER_PATH_RE = re.compile(r"(interact|profession)", re.IGNORECASE)


def lane_for(path: str) -> str:
    """Coarse provider lane from the prefab's logical path (provenance, not identity)."""
    low = str(path or "").lower()
    if "profession" in low:
        return "profession"
    if "workbench" in low or "/crafting/" in low or "forge" in low:
        return "bench"
    if "vendor" in low:
        return "vendor"
    if "guideui" in low:
        return "guide"
    if "interact" in low:
        return "interactive"
    return "other"


def provider_id_from_path(path: str) -> str:
    """The provider's stable id: its logical path with the prefabs/ root + .binfab
    stripped (so `prefabs/placeable/crafting/workbench_chaos_interactive.binfab` ->
    `placeable/crafting/workbench_chaos_interactive`)."""
    p = str(path or "").replace("\\", "/")
    if p.startswith("prefabs/"):
        p = p[len("prefabs/"):]
    return p.removesuffix(".binfab")


def extract_provider_refs(content: bytes, known_ids: set[str]) -> list[str]:
    """Every real `recipe_*` id a provider prefab references, in source order.

    Each `recipe_*` token is trimmed from the tail to the longest id that actually
    exists (`known_ids` = the recipes/ stems), so glued framing bytes are stripped and
    a token matching no real recipe is dropped."""
    seen: dict[str, None] = {}
    for _off, _field, s in harvest_strings(content):
        for m in _RECIPE_TOKEN_RE.finditer(s.lower()):
            tok = m.group(0)
            while tok and tok not in known_ids:      # trim glued framing bytes
                tok = tok[:-1]
            if tok:
                seen.setdefault(tok, None)
    return list(seen)


def build_provider_map(prefabs: list[tuple[str, bytes]], known_ids: set[str]) -> dict[str, list[dict]]:
    """`recipe_id -> [provider row, ...]` from `(path, content)` provider prefabs.

    Each provider row is `{provider, provider_path, lane}` - the bench/profession that
    exposes the recipe, with full provenance. De-duped on (recipe, provider); the same
    recipe legitimately appears on several benches -> several rows."""
    out: dict[str, list[dict]] = {}
    for path, content in prefabs:
        refs = extract_provider_refs(content, known_ids)
        if not refs:
            continue
        pid = provider_id_from_path(path)
        lane = lane_for(path)
        for rid in refs:
            rows = out.setdefault(rid, [])
            if not any(r["provider"] == pid for r in rows):
                rows.append({"provider": pid, "provider_path": path, "lane": lane})
    return out
