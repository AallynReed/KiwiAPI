"""The two prefab structures a dressing room needs, read straight off the wire.

**The class socket table.** Every ``prefabs/class/<name>.binfab`` carries a list of
equipment sockets, each one a bone name, a VFX bone, an equipment-slot number and a
flag::

    08 <len> AP_bow_up   18 <len> VFX_weapon_l   20 0a   30 02   1e
    08 <len> prop_r_JNT  18 <len> prop_r_JNT     20 1c   30 00   1e
    08 <len> AP_hat      18 <len> AP_hat         20 18   30 00   1e
       ^ bone the model attaches to   ^ VFX bone   ^ slot   ^ flag

That table is the whole compatibility model, and it is the game's own: it says the
Knight holds one melee style at ``AP_r_prop``, the Candy Barbarian holds one at each of
``prop_r_JNT`` and ``prop_l_JNT`` (which is what "dual wielding" is - one selected style
drawn twice), the Gunslinger the same with pistols, and the Boomeranger accepts a bow
*or* a melee style because it declares sockets for both. Nothing here is inferred from a
name.

**The equipment slot.** A style prefab (``prefabs/equipment/…``) names its model
blueprint and carries the same slot number in field 10 of its identity block, so a style
and a socket are matched on one number rather than on "does the filename contain sword".
That matters: ``spear_pinata_starbar`` is modelled as a spear but the game files it as a
melee style, and the number is right where the filename is wrong.

Pure + stdlib-only; the callers supply the bytes.
"""

from __future__ import annotations

import re

# Slot number -> display family. Verified across every equipment/ prefab in the live
# archive; the numbers not listed here are non-appearance equipment (rings) and are
# dropped rather than guessed into a family.
SLOTS: dict[int, str] = {
    8: "Gun",
    10: "Bow",
    12: "Staff",
    16: "Spear",
    18: "Fist",
    24: "Hat",
    26: "Face",
    28: "Melee",
    40: "Banner",
}

HAT_SLOT = 24
FACE_SLOT = 26
# The families a class can hold in a weapon socket - everything that isn't worn.
WEAPON_SLOTS = frozenset({8, 10, 12, 16, 18, 28})

# 08 <len> <name> 18 <len> <target> 20 <slot> 30 <flag> 1e
_SOCKET_RE = re.compile(
    rb"\x08(.)([\x20-\x7e]{2,40})\x18(.)([\x20-\x7e]{0,40})\x20([\x00-\x7f])\x30([\x00-\x7f])\x1e",
    re.S,
)

_SKELETON_RE = re.compile(rb"([A-Za-z0-9_]+)\.skeleton\.gr2")
_DISPLAY_RE = re.compile(rb"\$DisplayName_[A-Za-z0-9_]+")
_BONE_RE = re.compile(r"^[A-Za-z0-9_]+$")


def _ap_key(bone: str) -> str:
    """The rig key a socket bone is baked under: ``AP_hat`` -> ``hat``, and a plain
    joint (``prop_r_JNT``) under its own lowercased name. Mirrors the offline baker."""
    return bone[3:].lower() if bone[:3].lower() == "ap_" else bone.lower()


def parse_class(data: bytes) -> dict | None:
    """``{skeleton, name_key, sockets: [{bone, ap, slot, family, flag}]}`` for a class
    prefab.

    ``name_key`` is the prefab's own ``$DisplayName_…`` key, read rather than rebuilt
    from the filename - the two differ in case (``$DisplayName_Knight`` for
    ``knight.binfab``), and a rebuilt key misses the locale table and leaves the class
    labelled with its stem ("Candybarbarian" for Candy Barbarian).

    ``None`` when the prefab names no skeleton or declares no socket - a class form that
    isn't wearable (``dinotamer_ultimate``) rather than something to guess at."""
    skel = _SKELETON_RE.search(data)
    if not skel:
        return None
    sockets = []
    seen: set[tuple[str, int]] = set()
    for m in _SOCKET_RE.finditer(data):
        len_name, raw_name, len_target, raw_target, slot, flag = m.groups()
        if len_name[0] != len(raw_name) or len_target[0] != len(raw_target):
            continue                       # a coincidental byte run, not a real field pair
        bone = raw_name.decode("ascii", "ignore")
        if not _BONE_RE.match(bone) or slot[0] not in SLOTS:
            continue
        key = (bone, slot[0])
        if key in seen:
            continue
        seen.add(key)
        sockets.append({
            "bone": bone,
            "ap": _ap_key(bone),
            "slot": slot[0],
            "family": SLOTS[slot[0]],
            "flag": flag[0],
        })
    if not sockets:
        return None
    display = _DISPLAY_RE.search(data)
    return {
        "skeleton": skel.group(1).decode("ascii", "ignore").lower(),
        "name_key": display.group(0).decode("ascii", "ignore") if display else "",
        "sockets": sockets,
    }


def style_slot(ident_flags: dict[int, int] | None) -> int | None:
    """The equipment slot number from a style prefab's decoded identity flags, or None
    when the prefab carries no slot (then it isn't offered - we don't infer one)."""
    if not ident_flags:
        return None
    slot = ident_flags.get(10)
    return slot if slot in SLOTS else None


def weapon_families(sockets: list[dict]) -> list[str]:
    """The distinct weapon families a class's sockets accept, in declared order."""
    out: list[str] = []
    for s in sockets:
        if s["slot"] in WEAPON_SLOTS and s["family"] not in out:
            out.append(s["family"])
    return out


def sockets_for_slot(sockets: list[dict], slot: int) -> list[dict]:
    """Every socket that takes this slot number. More than one means the class draws
    the selected style once per socket (dual wielding, or a bow split across its two
    limb attach points) - the count is the game's, not a rule of ours."""
    return [s for s in sockets if s["slot"] == slot]
