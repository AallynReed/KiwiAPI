"""Collectible mastery, ported from BTT (`models/trove/prefab_ally.py`) and
extended per the binfab handoff (geode mastery + the full base-family table).

Mastery = a per-type **base** (wings 100, mount/aura 50, flask 25, badge/tome 20,
pet 10, fish 5, …) times a per-item **multiplier** (0/2/3/5) read from
`prefabs/meta/multipliers.binfab`. The multipliers file groups identifiers under
four `BE 01 AE` markers; the Nth group maps to the Nth multiplier in (0, 2, 3, 5).

Geode mastery is the same shape from `prefabs/meta/geode_multipliers.binfab`, with
two differences the handoff calls out: `item/companion/…` has base **50** in geode
mode (not the normal 10), and a group-0 (×0) row **preserves its base** instead of
zeroing it (equipment/blueprint rows, base 0, still resolve to 0).

Pure + stdlib-only; the binary parse is tuned to the real layout, so validate
against the live archive after indexing.
"""

from __future__ import annotations

from app.trove.codexes.binfab import read_uleb

# (path fragment, base mastery), checked in order - first substring match wins.
# Mirrors the handoff's base-family table. Order matters: more specific / higher
# bases ahead of the bare-token skin fallback below.
_BASE_RULES: tuple[tuple[str, int], ...] = (
    ("collections/wings", 100),
    ("collections/mount", 50),
    ("collections/boat", 50),
    ("collections/magrider", 50),
    ("collections/aura", 50),
    ("collections/flask", 25),
    ("collections/badge", 20),
    ("collections/tome", 20),
    ("collections/fishingpole", 20),
    ("collections/pet", 10),
    ("collections/geodecompanion", 10),
    ("item/companion", 10),          # 50 in geode mode (see infer_mastery_base)
    ("collections/sail", 10),
    ("item/fish", 5),
    ("item/unlocker", 5),
    ("collections/memento", 5),
    ("recipe_", 2),
    # Style/equipment appearance rows. Base is NOT 0 - `EquipmentAppearance => 1`
    # (helmet/hat appearances carry a higher base where evidence is present, but the
    # value isn't pinned down here). When such a row sits in multiplier group 0, the
    # group forces the FINAL value to 0; that's an override, not the family base.
    ("equipment_", 1),
    (".blueprint", 1),
)

# Geode-mode base overrides: only `item/companion/…` differs from normal (50 vs 10).
_GEODE_BASE_OVERRIDES: tuple[tuple[str, int], ...] = (
    ("item/companion", 50),
)

_MARKER = b"\xBE\x01\xAE"
_GROUP_MULTIPLIERS = (0, 2, 3, 5)


def infer_mastery_base(identifier: str, *, geode: bool = False) -> tuple[str, int]:
    """(normalized identifier, base mastery) for a collection path.

    `geode=True` applies the geode-mode base overrides (item/companion -> 50).
    """
    normalized = identifier.replace("\\", "/")
    if geode:
        for fragment, base in _GEODE_BASE_OVERRIDES:
            if fragment in normalized:
                return normalized, base
    for fragment, base in _BASE_RULES:
        if fragment in normalized:
            return normalized, base
    if "/" not in normalized:          # bare token => a skin
        return f"collections/skin/{normalized}", 35
    return normalized, 0


def _is_collectible(identifier: str) -> bool:
    """A real collectible identifier (vs. an equipment/blueprint/recipe appearance
    row), used to gate geode group-0 base preservation."""
    return identifier.startswith(("collections/", "item/"))


def _encode_varint(value: int) -> bytes:
    out = bytearray()
    while True:
        byte = value & 0x7F
        value >>= 7
        if value:
            out.append(byte | 0x80)
        else:
            out.append(byte)
            return bytes(out)


def _parse_multiplier_file(content: bytes, *, geode: bool) -> dict[str, dict]:
    """Shared `BE 01 AE`-grouped multiplier parse for both the normal and geode
    files. Defensive: returns whatever parsed cleanly on a malformed tail."""
    rows: dict[str, dict] = {}
    position = 9  # past the file header
    try:
        for multiplier in _GROUP_MULTIPLIERS:
            marker_pos = content.find(_MARKER, position)
            if marker_pos < 0:
                break
            position = marker_pos + len(_MARKER)
            element_count, position = read_uleb(content, position)
            for index in range(1, element_count + 1):
                pattern = _encode_varint(4 + 16 * (index - 1)) + b"\x00"
                next_pos = content.find(pattern, position)
                if next_pos < 0:
                    break
                position = next_pos + len(pattern) + 2  # +2 = row framing bytes
                length, position = read_uleb(content, position)
                raw = content[position:position + length].decode("ascii", errors="ignore")
                position += length
                identifier, base = infer_mastery_base(raw, geode=geode)
                # Group 0 is ×0. In geode mode a positive base is preserved rather
                # than zeroed (the handoff's "don't blindly multiply to zero"), but
                # only for real collectibles - equipment/blueprint/recipe appearance
                # rows stay 0. Normal-mode group 0 always forces 0 (base × 0).
                if geode and multiplier == 0:
                    predicted = base if _is_collectible(identifier) else 0
                else:
                    predicted = base * multiplier
                rows[identifier] = {
                    "multiplier": multiplier, "base": base, "predicted": predicted,
                }
    except (IndexError, ValueError):
        pass
    return rows


def parse_multipliers(content: bytes) -> dict[str, dict]:
    """`prefabs/meta/multipliers.binfab` bytes -> {identifier: {multiplier, base,
    predicted}} (normal mastery)."""
    return _parse_multiplier_file(content, geode=False)


def parse_geode_multipliers(content: bytes) -> dict[str, dict]:
    """`prefabs/meta/geode_multipliers.binfab` bytes -> {identifier: {multiplier,
    base, predicted}} (geode mastery)."""
    return _parse_multiplier_file(content, geode=True)


def mastery_for(identifier: str, multipliers: dict[str, dict]) -> int | None:
    """Normal mastery for a collection identifier: the multipliers-file value if
    present, else the type base (None when the type carries no mastery)."""
    normalized, base = infer_mastery_base(identifier)
    row = multipliers.get(normalized)
    if row is not None:
        return row["predicted"]
    return base if base > 0 else None


def geode_mastery_for(identifier: str, geode_multipliers: dict[str, dict]) -> int | None:
    """Geode mastery for a collection identifier: the geode-file value, or None.

    Unlike normal mastery there is no type-base fallback - geode mastery is driven
    purely by membership in `geode_multipliers.binfab` (a collectible absent from it
    contributes no geode mastery), matching BTT's behavior."""
    normalized, _base = infer_mastery_base(identifier, geode=True)
    row = geode_multipliers.get(normalized)
    return row["predicted"] if row is not None else None
