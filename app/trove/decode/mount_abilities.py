"""mount_abilities.json - every mount, its stats and the abilities it adds.

Mounts are `prefabs/collections/mount/*.binfab`. Their mount component (80) holds
the stat groups a rider gets per slot (field 6; component 369 adds what owning it
grants at all), the passive abilities that ride along
(field 3 - almost always just the block harvester every mount has) and the
triggered ones (field 4: input, attach point, ability prefab). Each ability is
read structurally by ``ability.describe``.
"""
from __future__ import annotations

from app.trove.decode.ability import Prefabs, identity, modifiers, walk_values
from app.trove.decode.ally_abilities import _detail
from app.trove.decode.common import locale, stem
from app.trove.decode.tree import GameTree
from app.trove.decode.wire import Obj, strings

TITLE = "Mount abilities"
OUTPUT = "mount_abilities.json"
PREFIXES = ("prefabs/collections/mount/", "prefabs/abilities/", "prefabs/sfx/", "languages/en/")
INDENT, FINAL_NEWLINE = 4, False

MOUNT = 80
MOUNT_PASSIVES, MOUNT_TRIGGERED, MOUNT_STATS = 3, 4, 6
TRIGGER_ABILITY = 2
# A stat group's field 0 is the slot it applies in - a flying mount grants ground
# speed as a mount and glide speed as wings. Same ids as bonuses._SLOT_BY_CONTEXT.
SLOTS = {0: "$EquipmentSlot_Mount", 1: "$EquipmentSlot_Cart", 2: "$EquipmentSlot_Wings",
         3: "$EquipmentSlot_Boat"}
# Collection-unlock component: bonuses that apply for owning the mount at all
# (a dragon's permanent +1000 Max Health), equipped or not.
UNLOCK = 369
# Every mount carries it; it is what lets a mount break blocks, not a feature of one.
SHARED = frozenset({"abilities/equipment/mount_block_harvester"})


def _ability_refs(mount: Obj) -> list[str]:
    out: list[str] = []
    for ref in mount.get(MOUNT_PASSIVES) or []:
        if isinstance(ref, str):
            out.append(ref)
    for row in mount.get(MOUNT_TRIGGERED) or []:
        leaf = row.leaf if isinstance(row, Obj) else {}
        if isinstance(leaf.get(TRIGGER_ABILITY), str):
            out.append(leaf[TRIGGER_ABILITY])
    out += [s for s in strings(mount) if s.startswith("abilities/")]
    seen: list[str] = []
    for ref in out:
        ref = ref.removesuffix(".binfab")
        if ref not in seen and ref not in SHARED:
            seen.append(ref)
    return seen


def _stat_groups(pf, mount: Obj) -> list[dict]:
    groups = []
    unlock = pf.component(UNLOCK)
    stats = modifiers(walk_values(unlock)) if unlock is not None else []
    if stats:
        groups.append({"slot": "unlock", "stats": stats})
    for group in mount.get(MOUNT_STATS) or []:
        leaf = group.leaf if isinstance(group, Obj) else {}
        stats = modifiers(walk_values(leaf.get(1) or []))
        if stats:
            groups.append({"slot": SLOTS.get(leaf.get(0, 0), f"slot_{leaf.get(0)}"), "stats": stats})
    return groups


def build(tree: GameTree) -> list[dict]:
    prefabs = Prefabs(tree)
    names = locale(tree.read("languages/en/prefabs_collections_mount.binfab"))
    abilities_text: dict[str, str] = {}
    for path in tree.files("languages/en/prefabs_abilities", ".binfab"):
        abilities_text.update(locale(tree.read(path)))

    mounts = []
    for path in tree.files("prefabs/collections/mount/", ".binfab"):
        if path.count("/") != 3:
            continue
        slug = stem(path)
        pf = prefabs.get(f"collections/mount/{slug}")
        mount = pf.component(MOUNT) if pf else None
        if pf is None or mount is None:
            continue
        ident = identity(pf)
        name = names.get(ident.get("name_key", ""), "")
        if not name:
            continue                        # no display name: not a mount players see
        powers = []
        for ref in _ability_refs(mount):
            if prefabs.get(ref) is None:
                continue
            aid = identity(prefabs.get(ref))
            row = {"ref": ref}
            title = abilities_text.get(aid.get("name_key", ""), "")
            text = abilities_text.get(aid.get("description_key", ""), "")
            if title:
                row["name"] = title
            if text:
                row["text"] = text
            detail = _detail(prefabs, ref, abilities_text)
            if detail or title or text:
                powers.append({**row, **detail})
        mounts.append({
            "slug": slug, "name": name,
            "description": names.get(ident.get("description_key", ""), ""),
            "prefab": f"collections/mount/{slug}",
            "stats": _stat_groups(pf, mount),
            "abilities": powers,
        })
    return sorted(mounts, key=lambda m: m["name"].lower())


def count(data: list) -> int:
    return len(data)

