"""Geode companion collection bonuses (the upgrade-tree path).

Geode companions are different from other collectibles: the
`collections/collection_geodecompanion.binfab` table points at `item/companion/…`
item prefabs (not `collections/geodecompanion/…`), and each item prefab references
a `<base>_upgrade_tree`. The level bonuses live in that separate upgrade-tree
binfab, scanned by `_level_XX` chunk.

Within a level chunk:
- a stat row is `10 02 24 <4-byte LE float> 38` -> `$Stat_MaxExploration` (0x24)
- an effect row carries a `$…_name` localized key plus an adjacent
  `abilities/discovery/companion/…` ref (the ref is evidence; the `$…_name` key is
  the displayed bonus).

Levels keep source order; the level number comes from `_level_XX`. Pure + stdlib.
"""

from __future__ import annotations

import re

from app.trove.codexes.binfab import harvest_strings
from app.trove.codexes.bonuses import decode_le_float

_UPGRADE_TREE_RE = re.compile(r"[A-Za-z0-9_/]+_upgrade_tree")
_LEVEL_RE = re.compile(rb"([A-Za-z0-9_/]+)_level_(\d{2})")
_EXPLORATION_RE = re.compile(rb"\x10\x02\x24(....)\x38", re.DOTALL)
_NAME_KEY_RE = re.compile(rb"\$[A-Za-z0-9_]+_name")
_COMPANION_ABILITY_RE = re.compile(rb"abilities/discovery/companion/[A-Za-z0-9_/.]+")


def find_upgrade_tree_ref(content: bytes) -> str | None:
    """The `<base>_upgrade_tree` ref inside a geode companion item prefab, or None."""
    for _off, _field, text in harvest_strings(content):
        match = _UPGRADE_TREE_RE.search(text)
        if match:
            return match.group(0).rsplit("/", 1)[-1]
    return None


def geode_companion_members(content: bytes) -> dict[str, str]:
    """`collection_geodecompanion` membership: `item/companion/<key>` (lowercased,
    no `.binfab`) -> rarity group id (Common/Uncommon/Rare/…).

    Unlike `parse_collection_table`, geode members are `item/companion/…` paths, so
    this groups those directly under each `$CollectionName_*` label. The label rejects
    any harvested string carrying a `$` for the same reason as `parse_collection_table`
    (a phantom field can straddle a label into the loc key that follows it)."""
    members: dict[str, str] = {}
    group = ""
    prev_bare = ""
    for _off, _field, s in harvest_strings(content):
        if s.startswith("$CollectionName"):
            group = prev_bare or s.removeprefix("$CollectionName_")
        elif s.startswith("item/companion/"):
            key = s.replace("\\", "/").removesuffix(".binfab").lower()
            members.setdefault(key, group)
        elif "/" not in s and "$" not in s:
            prev_bare = s
    return members


def _parse_level_chunk(chunk: bytes) -> dict:
    stats: list[dict] = []
    for match in _EXPLORATION_RE.finditer(chunk):
        amount = decode_le_float(match.group(1))
        if amount is not None and abs(amount) <= 1_000_000:
            stats.append({
                "stat": "$Stat_MaxExploration", "stat_id": 0x24,
                "amount": amount, "value": amount, "is_percent": False,
            })

    refs = [m.group(0).decode("ascii", errors="ignore") for m in _COMPANION_ABILITY_RE.finditer(chunk)]
    abilities: list[dict] = []
    if refs:
        name_keys = [m.group(0).decode("ascii", errors="ignore") for m in _NAME_KEY_RE.finditer(chunk)]
        for index, ref in enumerate(refs):
            abilities.append({
                "ref": ref,
                "key": name_keys[index] if index < len(name_keys) else None,
                "amount": 0,
            })
    return {"stats": stats, "abilities": abilities}


def parse_upgrade_tree(content: bytes) -> list[dict]:
    """Level bonuses from an upgrade-tree binfab: `[{level, stats, abilities}, …]`
    in source order. Empty if the file has no `_level_XX` chunks."""
    bounds = [(m.start(), int(m.group(2))) for m in _LEVEL_RE.finditer(content)]
    if not bounds:
        return []
    levels: list[dict] = []
    for index, (start, level) in enumerate(bounds):
        end = bounds[index + 1][0] if index + 1 < len(bounds) else len(content)
        parsed = _parse_level_chunk(content[start:end])
        if parsed["stats"] or parsed["abilities"]:
            levels.append({"level": level, **parsed})
    return levels
