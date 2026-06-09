"""Collectible mastery, ported from BTT (`models/trove/prefab_ally.py`).

Mastery = a per-type **base** (mount 50, badge 20, pet 10, fish 5, …) times a
per-item **multiplier** (0/2/3/5) read from `prefabs/meta/multipliers.binfab`.
The multipliers file groups identifiers under four markers; the Nth group maps to
the Nth multiplier in (0, 2, 3, 5). Pure + stdlib-only; the binary parse is tuned
to the real layout, so validate against the live archive after indexing.
"""

from __future__ import annotations

from app.trove.codexes.binfab import read_uleb

# (path fragment, base mastery), checked in order - first match wins.
_BASE_RULES: tuple[tuple[str, int], ...] = (
    ("collections/mount", 50),
    ("collections/boat", 50),
    ("collections/badge", 20),
    ("collections/tome", 20),
    ("collections/pet", 10),
    ("item/companion", 10),
    ("collections/sail", 10),
    ("item/fish", 5),
    ("item/unlocker", 5),
    ("recipe_", 2),
    ("equipment_", 0),
)

_MARKER = b"\xBE\x01\xAE"
_GROUP_MULTIPLIERS = (0, 2, 3, 5)


def infer_mastery_base(identifier: str) -> tuple[str, int]:
    """(normalized identifier, base mastery) for a collection path."""
    normalized = identifier.replace("\\", "/")
    for fragment, base in _BASE_RULES:
        if fragment in normalized:
            return normalized, base
    if "/" not in normalized:          # bare token => a skin
        return f"collections/skin/{normalized}", 35
    return normalized, 0


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


def parse_multipliers(content: bytes) -> dict[str, dict]:
    """`prefabs/meta/multipliers.binfab` bytes -> {identifier: {multiplier, base,
    predicted}}. Defensive: returns whatever parsed cleanly on a malformed tail."""
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
                position = next_pos + len(pattern) + 2
                length, position = read_uleb(content, position)
                raw = content[position:position + length].decode("ascii", errors="ignore")
                position += length
                identifier, base = infer_mastery_base(raw)
                rows[identifier] = {
                    "multiplier": multiplier, "base": base, "predicted": base * multiplier,
                }
    except (IndexError, ValueError):
        pass
    return rows


def mastery_for(identifier: str, multipliers: dict[str, dict]) -> int | None:
    """Mastery for a collection identifier: the multipliers-file value if present,
    else the type base (None when the type carries no mastery - items/recipes)."""
    normalized, base = infer_mastery_base(identifier)
    row = multipliers.get(normalized)
    if row is not None:
        return row["predicted"]
    return base if base > 0 else None
