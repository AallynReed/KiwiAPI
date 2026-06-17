"""Numeric stat bonuses + visible ability bonuses from a collection prefab.

Two extractors, both grounded in the handoff's byte evidence:

- ``extract_stat_bonuses`` scans the window between the ``_description`` string and
  the ``.blueprint`` string for stat records shaped
  ``[header] [4-byte LE float] 38 [len] [label] 46``. The stat id and operation
  byte sit at fixed offsets behind the ``0x38`` label marker
  (``stat = unzig(data[i-8])``, ``op = data[i-6]``, ``float = data[i-4:i]``); the
  amount is normalized by the operation byte. A record is only kept when its
  decoded stat id is a known stat (the strongest false-positive filter, and what
  the handoff means by "numeric stat ID is more authoritative").

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

# Operation byte -> name (handoff's normalization table).
OPERATIONS: dict[int, str] = {0: "MultiplySum", 2: "Add", 4: "Set", 8: "Multiply"}

_LABEL_RE = re.compile(r"^[A-Za-z0-9_/.$\-]+$")
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
    if operation == 4:                       # Set
        return amount, False
    # operation == 2 (Add) and anything else: additive family.
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


def _stat_window(data: bytes) -> tuple[int, int]:
    """Scan window: end of the `_description` string -> the `.blueprint` string,
    with a conservative fallback. (0, len) means "scan everything"."""
    desc = data.find(b"_description")
    if desc < 0:
        return 0, len(data)
    start = desc + len(b"_description")
    blueprint = data.find(b".blueprint", start)
    end = blueprint if blueprint > start else min(len(data), start + 4096)
    return start, end


def _detect(data: bytes, start: int, end: int) -> list[dict]:
    records: list[dict] = []
    n = len(data)
    end = min(end, n)
    component: int | None = None
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

        stat_id = unzig(data[i - 8])
        stat_key = STAT_KEYS.get(stat_id)
        if stat_key is None:                     # unknown stat => treat as noise
            i += 1
            continue

        operation = data[i - 6]
        display, is_percent = _normalize(stat_key, operation, value)
        records.append({
            "stat": stat_key,
            "stat_id": stat_id,
            "operation": OPERATIONS.get(operation, operation),
            "amount": value,
            "value": display,
            "is_percent": is_percent,
            "label": label,
            "slot": _slot_for_label(label),
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
