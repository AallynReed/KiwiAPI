"""collectibles.json - wings, boats, sails and auras: what each grants and does.

Which of them a player can own is the member list of
`prefabs/collections/collection_<wings|boat|sail|aura>.binfab`: groups of {0 id,
1 `$CollectionName_*` key, 3 member rows whose field 0 is the collectible}. A
group's name is where the collection screen files it ("Store", "Chaotic", "Event
Vault"); the working groups (InProgress, ReadyForGame, Hidden) are not player
facing, so their members carry no group. Boat folders also hold cannons and their
projectiles; those are parts of a boat, not collectibles, and are in no group.

Wings and boats use the mount component (80) the mount decoder reads: stat groups
per slot (field 6), passive ability refs (3) and triggered rows (4) - a boat's
cannon is a triggered row naming `collections/boat/*_cannon`, whose chain runs
through `collections/boat/` prefabs (cannon -> projectile -> explosion) to the
damage. Field 2 lists the particle effects attached to the model (`{0 .pkfx, 1
attach point}`), which is most of what sets one pair of wings apart. Lava-safe
boats also list `abilities/equipment/boat_lavaimmunity` in component 149; it has
no text of its own and its chain is the lava damage it cancels, so it is left to
the boat's description. Sails and auras carry no stats: identity text and model.
"""
from __future__ import annotations

import re
from typing import Any

from app.trove.decode.ability import Prefabs, describe, identity
from app.trove.decode.ally_abilities import _detail
from app.trove.decode.common import locale, stem
from app.trove.decode.mount_abilities import MOUNT, _ability_refs, _stat_groups
from app.trove.decode.tree import GameTree
from app.trove.decode.wire import Obj

TITLE = "Wings, boats, sails and auras"
OUTPUT = "collectibles.json"
PREFIXES = ("prefabs/collections/", "prefabs/abilities/", "prefabs/sfx/", "languages/en/")
INDENT, FINAL_NEWLINE = 4, False

KINDS = {"wings": "wings", "boats": "boat", "sails": "sail", "auras": "aura"}
BLUEPRINT = 37
MOUNT_EFFECTS = 2
WORKING_GROUP = re.compile(r"^(InProgress|ReadyForGame|Hidden)")


def _leaf(v: Any) -> dict:
    return v.leaf if isinstance(v, Obj) else {}


def _members(prefabs: Prefabs, folder: str, names: dict[str, str]) -> list[tuple[str, str]]:
    """`(collections/<folder>/<slug>, group name)` in collection order."""
    pf = prefabs.get(f"collections/collection_{folder}")
    out: list[tuple[str, str]] = []
    for group in _leaf(pf.root if pf else None).get(0) or []:
        g = _leaf(group)
        gid = g.get(0) if isinstance(g.get(0), str) else ""
        label = "" if WORKING_GROUP.match(gid) else names.get(g.get(1) or "", "") or gid
        for row in g.get(3) or []:
            ref = _leaf(row).get(0)
            if isinstance(ref, str) and ref.startswith(f"collections/{folder}/"):
                ref = ref.removesuffix(".binfab")
                if ref not in (r for r, _ in out):
                    out.append((ref, label))
    return out


def _effects(mount: Obj) -> list[dict]:
    out = []
    for row in mount.get(MOUNT_EFFECTS) or []:
        r = _leaf(row)
        if isinstance(r.get(0), str) and r[0].lower().endswith(".pkfx"):
            out.append({"key": r.get(1) if isinstance(r.get(1), str) else "", "pkfx": r[0]})
    return out


def _part(prefabs: Prefabs, ref: str) -> dict:
    """A boat part (its cannon): the damage at the end of its own prefab chain."""
    info = describe(prefabs, ref, prefix="collections/boat/")
    stages = [{k: v for k, v in r.items() if k != "_depth"} for r in info["stages"]]
    return {"stages": stages} if stages else {}


def _powers(prefabs: Prefabs, refs: list[str], text: dict[str, str]) -> list[dict]:
    powers = []
    for ref in refs:
        if prefabs.get(ref) is None:
            continue
        ident = identity(prefabs.get(ref))
        row: dict[str, Any] = {"ref": ref}
        title = text.get(ident.get("name_key", ""), "")
        desc = text.get(ident.get("description_key", ""), "")
        if title:
            row["name"] = title
        if desc:
            row["text"] = desc
        detail = _part(prefabs, ref) if ref.startswith("collections/") else _detail(prefabs, ref, text)
        if detail or title or desc:
            powers.append({**row, **detail})
    return powers


def _entry(prefabs: Prefabs, ref: str, group: str, names: dict[str, str], text: dict[str, str]) -> dict | None:
    pf = prefabs.get(ref)
    if pf is None:
        return None
    ident = identity(pf)
    name = names.get(ident.get("name_key", ""), "")
    if not name:
        return None
    entry: dict[str, Any] = {"slug": stem(ref), "name": name,
                             "description": names.get(ident.get("description_key", ""), ""), "prefab": ref}
    blueprint = _leaf(pf.component(BLUEPRINT)).get(0)
    if isinstance(blueprint, str) and blueprint:
        entry["blueprint"] = blueprint
    if group:
        entry["group"] = group
    mount = pf.component(MOUNT)
    if mount is not None:
        stats = _stat_groups(pf, mount)
        if stats:
            entry["stats"] = stats
        powers = _powers(prefabs, _ability_refs(mount), text)
        if powers:
            entry["abilities"] = powers
        effects = _effects(mount)
        if effects:
            entry["vfx"] = effects
    return entry


def build(tree: GameTree) -> dict[str, list[dict]]:
    prefabs = Prefabs(tree)
    names = locale(tree.read("languages/en/prefabs_collections.binfab"))
    text: dict[str, str] = {}
    for path in tree.files("languages/en/prefabs_abilities", ".binfab"):
        text.update(locale(tree.read(path)))
    out: dict[str, list[dict]] = {}
    for key, folder in KINDS.items():
        own = {**names, **locale(tree.read(f"languages/en/prefabs_collections_{folder}.binfab"))}
        rows = []
        for ref, group in _members(prefabs, folder, own):
            entry = _entry(prefabs, ref, group, own, {**own, **text})
            if entry:
                rows.append(entry)
        out[key] = sorted(rows, key=lambda e: e["name"].lower())
    return out


def count(data: dict) -> int:
    return sum(len(v) for v in data.values())
