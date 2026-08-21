"""Numeric stat bonuses + visible ability bonuses from a collection prefab.

Two extractors, both grounded in the handoff's byte evidence:

- ``extract_stat_bonuses`` scans the window between the ``_description`` string and
  the ``.blueprint`` string for stat records shaped
  ``[header] [4-byte LE float] 38 [len] [label] 46``. The stat id and operation
  byte sit at fixed offsets behind the ``0x38`` label marker
  (``stat = unzig(data[i-8])``, ``op = data[i-6]``, ``float = data[i-4:i]``); the
  amount is normalized by the operation byte. A record is only kept when its
  decoded stat id is a known stat (the strongest false-positive filter, and what
  the handoff means by "numeric stat ID is more authoritative"). The operation byte
  then splits each id in two - see ``bonus_key`` - so `stat_id` alone no longer
  determines `stat`.

- ``extract_abilities`` harvests literal ``abilities/…`` refs and classifies each
  as a displayed bonus or a hidden/mechanical ref (kept as evidence, not emitted
  as a visible row). Flask ``use_flask_health_*`` rows paired with a
  ``use_flask_dummyhealth_*`` twin are demoted to evidence to avoid a duplicate.

Pure + stdlib-only.
"""

from __future__ import annotations

import math
import re
import struct

from app.trove.codexes.binfab import unzig

# Stat id -> `$Stat_…` key. BTT's full table, with the two `_controller` regen
# names from the handoff. The handoff's smaller table is a subset of this.
STAT_KEYS: dict[int, str] = {
    0x00: "$Stat_PhysicalDamage",
    0x01: "$Stat_SpellDamage",
    0x02: "$Stat_MaxHealth",
    0x03: "$Stat_MaxEnergy",
    0x04: "$Stat_HealthRegen_controller",
    0x05: "$Stat_EnergyRegen_controller",
    0x06: "$Stat_Stability",
    0x07: "$Stat_CriticalHitChance",
    0x08: "$Stat_MovementSpeed",
    0x09: "$Stat_Jump",
    0x0A: "$Stat_Superstition",
    0x0B: "$Stat_IncomingDamageMod",
    0x0C: "$Stat_OutgoingDamageMod",
    0x0D: "$Stat_MagicFind",
    0x0E: "$Stat_Mining",
    0x0F: "$Stat_AttackSpeed",
    0x10: "$Stat_MaxFlasks",
    0x11: "$Stat_CraftingSpeed",
    0x12: "$Stat_CooldownSpeed",
    0x13: "$Stat_Acceleration",
    0x14: "$Stat_TurningRate",
    0x15: "$Stat_ExperienceBoost",
    0x16: "$Stat_CriticalHitDamage",
    0x17: "$Stat_BattleFactor",
    0x18: "$Stat_ActionTimeMod",
    0x19: "$Stat_PowerRank",
    0x1A: "$Stat_Glide",
    0x1B: "$Stat_ShootProjectileSpeedMultiplier",
    0x1C: "$Stat_RedGemStatBoost",
    0x1D: "$Stat_BlueGemStatBoost",
    0x1E: "$Stat_YellowGemStatBoost",
    0x1F: "$Stat_DoubleHitChance",
    0x20: "$Stat_JackpotExperience",
    0x21: "$Stat_GemEfficiency",
    0x22: "$Stat_AdventurineGainBoost",
    0x23: "$Stat_ClubExperienceBoost",
    0x24: "$Stat_MaxExploration",
    0x25: "$Stat_JumpSpeedMultiplier",
    0x26: "$Stat_GreenGemStatBoost",
    0x27: "$Stat_Light",
    0x28: "$Stat_Dark",
    0x29: "$Stat_OpalGemStatBoost",
    0x2A: "$Stat_MaxNCharge",
    0x2B: "$Stat_NChargeRegen",
    0x2C: "$Stat_MaxArmor",
}

# Operation byte -> name. The game's own `KModType` reflection table (Trove_x64.exe,
# listed right after `KStatType`) names all seven in order - MultiplySum, Add, Set,
# Nullify, Multiply, Minimum, Maximum - and the wire byte is the zig-zag of the index,
# so each is 2x its position. Every one of them occurs: a sweep of all 42,112 prefabs
# finds Add 3,025, Set 2,619, MultiplySum 1,020, Multiply 685, Nullify 54, Maximum 7,
# Minimum 2. The last three are movement restrictions and debuffs (the two Minimum
# records sit in a file literally named `test_shout_minimum`), never collectible bonuses.
OPERATIONS: dict[int, str] = {
    0: "MultiplySum",     # scales the stat by the amount (0.30 = +30% of it)
    2: "Add",             # flat amount on the stat, whether or not it displays as a percent
    4: "Set",             # replaces the stat
    6: "Nullify",         # zeroes it
    8: "Multiply",        # multiplies the output (patron buffs and the like)
    10: "Minimum",        # floors it
    12: "Maximum",        # caps it
}


def bonus_key(stat_key: str) -> str:
    """The `…Bonus` twin of a stat key. `MultiplySum` states a different thing from
    `Add` on the SAME stat id - it scales the stat rather than adding to it - so the two
    get separate keys rather than one name covering both. The gap is invisible wherever
    both render as a percentage: `+30% Critical Damage` is 30 points under `Add` and 900
    points under `MultiplySum` on a 3,000% sheet.

    `_controller` stays on the end, where the game puts it.
    """
    base, sep, tail = stat_key.partition("_controller")
    return f"{base}Bonus{sep}{tail}"


_LABEL_RE = re.compile(r"^[A-Za-z0-9_/.$\-]+$")

# Designer-label suffix -> stat key, used ONLY when the numeric id is unknown. The id
# stays authoritative wherever it decodes; this recovers records that would otherwise
# be discarded as noise.
_LABEL_STATS: tuple[tuple[str, str], ...] = (
    ("physicaldamage", "$Stat_PhysicalDamage"),
    ("spelldamage", "$Stat_SpellDamage"),
    ("maxhealth", "$Stat_MaxHealth"),
    ("maxenergy", "$Stat_MaxEnergy"),
    ("healthregen", "$Stat_HealthRegen_controller"),
    ("energyregen", "$Stat_EnergyRegen_controller"),
    ("criticalhitchance", "$Stat_CriticalHitChance"),
    ("criticalhitdamage", "$Stat_CriticalHitDamage"),
    ("critdamage", "$Stat_CriticalHitDamage"),
    ("attackspeed", "$Stat_AttackSpeed"),
    ("magicfind", "$Stat_MagicFind"),
    ("jump", "$Stat_Jump"),
    ("mining", "$Stat_Mining"),
)


def _stat_from_label(label: str) -> str | None:
    """The stat a designer label names, or None. Matched on a trailing token so
    `ground_movespeed` doesn't collide with `movespeed`-suffixed neighbours."""
    lowered = label.lower()
    if lowered.endswith(("ground_movespeed", "wing_movespeed")):
        return "$Stat_MovementSpeed"
    if lowered.endswith("glide_movespeed"):
        return "$Stat_Glide"
    for suffix, key in _LABEL_STATS:
        if lowered == suffix or lowered.endswith("_" + suffix):
            return key
    return None
_ABILITY_RE = re.compile(rb"abilities/[A-Za-z0-9_/.\\]+")

# Ability refs that are hidden/mechanical, never displayed as a collection bonus.
_HIDDEN_ABILITY_SUBSTRINGS: tuple[str, ...] = (
    "flask_auto_use",
    "_reset_death_save",
    "equip_flask_cooldown",
    "mount_block_harvester",
)


def decode_le_float(raw: bytes) -> float | None:
    """A finite little-endian 32-bit float, or None (wrong length / nan / inf)."""
    if len(raw) != 4:
        return None
    value = struct.unpack("<f", raw)[0]
    if math.isnan(value) or math.isinf(value):
        return None
    return value


def _normalize(stat_key: str, operation: int, amount: float) -> tuple[float, bool]:
    """(display value, is_percent) for a raw amount under an operation byte."""
    if operation == 0:                       # MultiplySum
        return amount * 100, True
    if operation == 8:                       # Multiply
        return (amount - 1) * 100, True
    if operation != 2:                       # Set / Nullify / Minimum / Maximum
        return amount, False                 # all state the stat itself, never a delta
    # Add: a flat amount, which a few stats nonetheless display as a percent.
    if stat_key == "$Stat_CriticalHitChance":
        return amount / 10, False
    if stat_key in ("$Stat_CriticalHitDamage", "$Stat_AttackSpeed"):
        return amount, True
    return amount, False


def _slot_for_label(label: str) -> str | None:
    lowered = label.lower()
    if "ground_movespeed" in lowered:
        return "$EquipmentSlot_Mount"
    if "wing_movespeed" in lowered or "glide_movespeed" in lowered:
        return "$EquipmentSlot_Wings"
    return None


# --- structural slot context -------------------------------------------------
#
# A collectible can grant DIFFERENT stats depending on which slot it is equipped in -
# the same prefab carries a mount block, a wings block and a boat block. The slot is
# stated structurally by a context byte, not by the label, so reading it off the label
# (which is what we did) filed every unlabelled record under no slot at all.

# `1E [14|24] 00 <type> BE 01 AE` opens a slot block; <type> selects the slot.
_CONTEXT_OPEN_RE = re.compile(rb"\x1e[\x14\x24]\x00([\x00\x02\x04\x06])\xbe\x01\xae", re.DOTALL)
# The mount block has its own opening shape.
_CONTEXT_MOUNT_RE = re.compile(rb"\xbe\x01\xae\x01\x00\x04\x00\x10\x10\x04\x24$", re.DOTALL)
# A continuation record inside an already-open block; group 1 is its own stat id.
_CONTEXT_CONT_RE = re.compile(rb"\x1e[\x14\x24]\x00([\x00-\x7f])\x10[\x02\x04]\x24$", re.DOTALL)

_SLOT_BY_CONTEXT: dict[int, str] = {
    0x00: "$EquipmentSlot_Mount",
    0x02: "$EquipmentSlot_Cart",
    0x04: "$EquipmentSlot_Wings",
    0x06: "$EquipmentSlot_Boat",
}


def _slot_context(prefix: bytes, current: str | None) -> tuple[str | None, bool, int | None]:
    """`(slot key, opened a new block, continuation stat id)` for one record.

    `prefix` is the bytes leading up to the record's float. A block opener sets the
    slot for itself and every record after it until the next opener; a continuation
    inherits the open slot and, critically, carries its OWN stat id - the header byte
    a continuation record would otherwise be read from belongs to the block, so
    without this every record after the first in a block reports the first one's stat.
    """
    before_float = prefix[:-4] if len(prefix) >= 4 else prefix
    match = _CONTEXT_OPEN_RE.search(before_float)
    if match:
        return _SLOT_BY_CONTEXT.get(match.group(1)[0]), True, None
    if _CONTEXT_MOUNT_RE.search(before_float):
        return "$EquipmentSlot_Mount", True, None
    cont = _CONTEXT_CONT_RE.search(before_float)
    if cont:
        return current, False, unzig(cont.group(1)[0])
    return None, False, None


def _stat_window(data: bytes) -> tuple[int, int]:
    """Scan window around the stat block. (0, len) means "scan everything".

    Starts slightly BEFORE the `_description` key rather than after it: the first
    record's slot-context bytes sit ahead of the key, and starting after it cut them
    off, so the opening block of every prefab lost its slot. Ends at the LAST model
    reference rather than the first - a prefab that names several blueprints had its
    later stat records clipped off entirely."""
    desc = data.find(b"_description")
    if desc < 0:
        return 0, len(data)
    start = max(0, desc - 96)
    end = data.rfind(b".blueprint")
    if end <= start:
        end = min(len(data), desc + 4096)
    return start, end


def _detect(data: bytes, start: int, end: int) -> list[dict]:
    records: list[dict] = []
    n = len(data)
    end = min(end, n)
    component: int | None = None
    current_slot: str | None = None
    i = max(start, 8)
    while i < end:
        # Component / slot separator: `BE 01 AE <group> 00 04`.
        if (data[i] == 0xAE and i + 3 < n and data[i + 2] == 0x00 and data[i + 3] == 0x04):
            group = data[i + 1]
            if 0 < group < 32:
                component = group

        if data[i] != 0x38 or i + 1 >= n:        # 0x38 = field 3, wt 8 (label string)
            i += 1
            continue

        length = data[i + 1]
        label_end = i + 2 + length
        if label_end > n:
            i += 1
            continue
        label = data[i + 2:label_end].decode("ascii", errors="ignore")
        if length > 0 and not _LABEL_RE.fullmatch(label):
            i += 1
            continue
        if label_end >= n or data[label_end] != 0x46:   # 0x46 terminator
            i += 1
            continue

        value = decode_le_float(data[i - 4:i])
        if value is None or abs(value) > 1_000_000:
            i += 1
            continue

        # A "…mods" record packs its header two bytes tighter than a normal one.
        prefix = data[max(0, i - 32):i]
        slot, opened, continuation_id = _slot_context(prefix, current_slot)
        if opened:
            current_slot = slot

        stat_id = continuation_id if continuation_id is not None else unzig(data[i - 8])
        stat_key = STAT_KEYS.get(stat_id)
        if stat_key is None:
            # The numeric id is authoritative, but a few records carry an id we don't
            # know while their designer label names the stat unambiguously. Recovering
            # those is better than dropping a real bonus; a label that names nothing
            # still drops.
            label_stat = _stat_from_label(label)
            if label_stat is None:
                i += 1
                continue
            stat_key, stat_id = label_stat, None

        operation = data[i - 6]
        display, is_percent = _normalize(stat_key, operation, value)
        if operation == 0:                   # MultiplySum: a bonus ON the stat, not the stat
            stat_key = bonus_key(stat_key)
        records.append({
            "stat": stat_key,
            "stat_id": stat_id,
            "operation": OPERATIONS.get(operation, operation),
            "amount": value,
            "value": display,
            "is_percent": is_percent,
            "label": label,
            "slot": _slot_for_label(label) or (slot if opened else current_slot),
            "component": component,
            "level": 0,
            "offset": i,
        })
        i = label_end + 1
    return records


def extract_stat_bonuses(data: bytes) -> list[dict]:
    """Numeric stat records from a collection prefab, in source order. Scans the
    description->blueprint window, falling back to a full-file scan if it's empty."""
    start, end = _stat_window(data)
    records = _detect(data, start, end)
    if not records and (start, end) != (0, len(data)):
        records = _detect(data, 0, len(data))
    return records


# --- visible / hidden abilities --------------------------------------------

def _ability_loc_key(ref: str) -> str:
    """`abilities/equipment/onhit_heal` ->
    `$prefabs_abilities_equipment_onhit_heal_<spawner>_description`. Flask
    `use_flask_*` refs use the inventory spawner; everything else the combat one."""
    flat = ref.replace("\\", "/").strip("/").replace("/", "_").replace(".", "_")
    spawner = "inventoryeventeffectspawner" if "use_flask" in ref else "combateventeffectspawner"
    return f"$prefabs_{flat}_{spawner}_description"


def _is_hidden(ref: str) -> bool:
    return any(token in ref for token in _HIDDEN_ABILITY_SUBSTRINGS)


def extract_abilities(data: bytes) -> list[dict]:
    """Literal `abilities/…` refs in source order, each tagged visible or hidden.

    Visible rows carry a derived localization `key` and `amount` 0; hidden rows are
    kept as evidence (no key). Paired flask `use_flask_health_*` rows are demoted
    when a `use_flask_dummyhealth_*` twin exists, so the dummy-health row is the one
    displayed (per the handoff)."""
    refs: list[str] = []
    seen: set[str] = set()
    for match in _ABILITY_RE.finditer(data):
        # `.`/`/` are allowed inside refs but trailing ones are delimiter noise.
        ref = match.group().decode("ascii", errors="ignore").replace("\\", "/").rstrip("./")
        if ref not in seen:
            seen.add(ref)
            refs.append(ref)

    # Flask dummy-health twins suppress the plain health row.
    dummy_suffixes = {
        ref.rsplit("dummyhealth_", 1)[-1]
        for ref in refs if "use_flask_dummyhealth_" in ref
    }

    out: list[dict] = []
    for ref in refs:
        hidden = _is_hidden(ref)
        if ("use_flask_health_" in ref
                and ref.rsplit("use_flask_health_", 1)[-1] in dummy_suffixes):
            hidden = True
        row: dict = {"ref": ref, "hidden": hidden}
        if not hidden:
            row["key"] = _ability_loc_key(ref)
            row["amount"] = 0
        out.append(row)
    return out
