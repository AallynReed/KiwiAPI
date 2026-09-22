"""Helpers the ability decoders share."""
from __future__ import annotations

import re
import struct

from app.trove.codexes.binfab import harvest_strings
from app.trove.codexes.bonuses import extract_stat_bonuses
from app.trove.codexes.localize import resolve_stat_name

# f11 varint = b0 01 | f12 float (multiplier) = c4 01 | f13 varint = d0 01 | f14 float (base) = e4 01
_BLOCK = re.compile(rb"\xb0\x01[\x00-\xff]\xc4\x01(....)\xd0\x01[\x00-\xff]\xe4\x01(....)", re.S)


def locale(data: bytes | None) -> dict[str, str]:
    """`$key -> text`, paired by byte adjacency so a missing value cannot shift the map."""
    if not data:
        return {}
    raw = harvest_strings(data)
    out = {}
    for i, (off, field, text) in enumerate(raw):
        if field != 0 or not text.startswith("$") or i + 1 >= len(raw):
            continue
        noff, nfield, ntext = raw[i + 1]
        if nfield == 1 and 0 <= noff - (off + len(text)) <= 4:
            out[text] = ntext
    return out


def damage_blocks(data: bytes) -> list[tuple[float, float]]:
    """`[(multiplier, base)]` for each damage record in a prefab."""
    out = []
    for match in _BLOCK.finditer(data):
        mult = struct.unpack("<f", match.group(1))[0]
        base = struct.unpack("<f", match.group(2))[0]
        if 0 <= mult < 1000 and -10 <= base <= 10:
            out.append((round(mult, 4), round(base * 100, 4)))
    return out


def refs(data: bytes, prefix: str) -> list[str]:
    """Paths starting with ``prefix`` this prefab names, in order, deduped.

    Prefab refs are length-prefixed, so the byte in front of the match gives the
    exact end - scanning for a printable run instead runs past it into the next
    record and invents paths like `blast_jump_explosionX`.
    """
    seen: set[str] = set()
    out: list[str] = []
    for match in re.finditer(re.escape(prefix.encode()), data):
        start = match.start()
        if start == 0:
            continue
        length = data[start - 1]
        if not 0 < length <= 200 or start + length > len(data):
            continue
        chunk = data[start:start + length]
        if not all(32 <= b < 127 for b in chunk):
            continue
        rel = chunk.decode()
        if rel not in seen:
            seen.add(rel)
            out.append(rel)
    return out


def stat_rows(data: bytes, prefab: str | None = None) -> list[dict]:
    """Stat records in the units the site's data files use, carrying their op.

    `amount` keeps the raw wire figure because `value` is normalised for display
    and a `Multiply` loses information doing it: incoming-damage 0.85 reads as a
    15% reduction, but shot-speed 1.5 is a 50% increase - same op, opposite sense.
    """
    rows = []
    for bonus in extract_stat_bonuses(data):
        sid, op, raw = bonus["stat_id"], bonus["operation"], bonus["amount"]
        value = abs(bonus["value"])
        if sid == 0x0F and op == "MultiplySum":      # AttackSpeed stores percent on buffs
            value = raw * 100 if abs(raw) < 1 else raw
        elif sid == 0x0C and op == "Add":            # OutgoingDamageMod stores a fraction
            value = raw * 100
        row = {"name": resolve_stat_name({}, bonus["stat"]), "stat": bonus["stat"],
               "op": op, "value": round(value, 4), "amount": round(raw, 4)}
        if prefab is not None:
            row["prefab"] = prefab
        if row not in rows:
            rows.append(row)
    return rows


def stem(path: str) -> str:
    """`prefabs/item/gem/large/blue_x_t3.binfab` -> `blue_x_t3`."""
    return path.rsplit("/", 1)[-1].rsplit(".", 1)[0]
