"""Style catalogues: `prefabs/loot/{hat,face,weapon_*,pvpbanner}.binfab`.

These are the files that state what a cosmetic style IS. Each catalogue holds
categories, and each category holds rows of
``(blueprint, $name key, $description key, raw-category byte)``.

Two things come out of this that nothing else in the archive states:

- **The style's slot family.** It is the catalogue the row lives in - hats are in
  `hat.binfab`, weapons in `weapon_<class>.binfab`. Previously we guessed the family
  from stem tokens (`"helm"`, `"bow"`, …) and returned "" whenever a name didn't
  contain one, so anything unconventionally named was silently unfiled.
- **The hat base mastery.** The row's raw-category byte (`0x30 <value>`) separates
  `EquipmentAppearanceHelmet` (`0x04`, base 10) from plain `EquipmentAppearance`
  (`0x01`/`0x02`, base 1). An unrecognised value yields no base rather than a guess.

`pvpbanner.binfab` has a different shape - it references `equipment/…` prefab paths
instead of carrying catalogue rows - so it is read by reference scan.

Pure + stdlib-only.
"""

from __future__ import annotations

import re

from app.trove.codexes.binfab import read_uleb

LOOT_ROOT = "prefabs/loot/"

# catalogue stem -> (family label, geode mastery applies). Weapons are one family
# across six files; the class split is in the filename, not the style's slot.
CATALOGUES: dict[str, str] = {
    "hat": "Hat",
    "face": "Face",
    "weapon_bow": "Weapon",
    "weapon_fist": "Weapon",
    "weapon_melee": "Weapon",
    "weapon_pistol": "Weapon",
    "weapon_spear": "Weapon",
    "weapon_staff": "Weapon",
    "pvpbanner": "Banner",
}

# Raw-category byte -> base mastery, for the hat catalogue. Verified against the
# game's own EquipmentAppearance / EquipmentAppearanceHelmet split.
HAT_BASE_BY_RAW: dict[int, int] = {0x04: 10, 0x01: 1, 0x02: 1}
# Every other catalogue is a plain EquipmentAppearance.
DEFAULT_STYLE_BASE = 1

_EQUIPMENT_REF_RE = re.compile(rb"equipment/[A-Za-z0-9_/.\-\[\]]+")


def catalogue_stem(path: str) -> str:
    return path.replace("\\", "/").rsplit("/", 1)[-1].removesuffix(".binfab").lower()


def is_style_catalogue(path: str) -> bool:
    p = path.replace("\\", "/").lower()
    return p.startswith(LOOT_ROOT) and catalogue_stem(p) in CATALOGUES


def _index_pattern(index: int) -> bytes:
    """Row/category marker for element `index`: varint(16*index - 12) + 0x08."""
    value = index * 16 - 12
    out = bytearray()
    while True:
        byte = value & 0x7F
        value >>= 7
        out.append(byte | 0x80 if value else byte)
        if not value:
            break
    return bytes(out) + b"\x08"


def _read_len_string(data: bytes, pos: int) -> tuple[str, int]:
    """Single-byte-length-prefixed string. ('' , pos) when it doesn't fit."""
    if pos >= len(data):
        return "", pos
    length = data[pos]
    pos += 1
    if length == 0 or pos + length > len(data):
        return "", pos
    return data[pos:pos + length].decode("latin1"), pos + length


def _raw_category(data: bytes, pos: int) -> int | None:
    """The `0x30 <value>` category byte that closes a row, or None if absent."""
    if pos < 0 or pos + 1 >= len(data) or data[pos] != 0x30:
        return None
    return data[pos + 1]


def _base_mastery(stem: str, raw_category: int | None) -> int | None:
    if stem == "hat":
        return HAT_BASE_BY_RAW.get(raw_category) if raw_category is not None else None
    return DEFAULT_STYLE_BASE


def parse_pvpbanner(data: bytes) -> list[dict]:
    """Banner rows - `pvpbanner.binfab` lists `equipment/…` prefab references."""
    rows: list[dict] = []
    seen: set[str] = set()
    for match in _EQUIPMENT_REF_RE.finditer(data):
        ref = match.group(0).decode("latin1").replace("\\", "/").rstrip("./").lower()
        if ref in seen:
            continue
        seen.add(ref)
        rows.append({
            "equipment": ref, "blueprint": "", "category": "", "family": "Banner",
            "name_key": "", "desc_key": "", "raw_category": None,
            "base_mastery": DEFAULT_STYLE_BASE, "offset": match.start(),
        })
    return rows


def parse_style_catalogue(data: bytes, path: str) -> list[dict]:
    """Rows of one loot style catalogue.

    Each row is `{blueprint, equipment, category, family, name_key, desc_key,
    raw_category, base_mastery, offset}`. `equipment` is the blueprint basename with
    the extension dropped - the id the rest of the codex keys styles on - while
    `blueprint` keeps the reference AS WRITTEN, because Trove reuses a basename
    across folders and only the folder tells two same-named assets apart.
    """
    stem = catalogue_stem(path)
    family = CATALOGUES.get(stem, "")
    if stem == "pvpbanner":
        return parse_pvpbanner(data)

    rows: list[dict] = []
    n = len(data)
    pos = 2
    if pos >= n:
        return rows
    category_count = data[pos]
    pos += 2

    for category_index in range(1, category_count + 1):
        pattern = _index_pattern(category_index)
        found = data.find(pattern, pos)
        if found < 0:
            break
        pos = found + len(pattern)
        category, pos = _read_len_string(data, pos)
        pos += 3                                   # row-count framing
        element_count, pos = read_uleb(data, pos)
        pos += 1
        if not 0 <= element_count <= 100_000:
            break

        for row_index in range(1, element_count + 1):
            row_pattern = _index_pattern(row_index)
            row_found = data.find(row_pattern, pos)
            if row_found < 0:
                break
            pos = row_found + len(row_pattern)
            blueprint, pos = _read_len_string(data, pos)
            pos += 1
            name_key, pos = _read_len_string(data, pos)
            pos += 1
            desc_key, pos = _read_len_string(data, pos)
            if not blueprint:
                continue
            raw_category = _raw_category(data, pos)
            ref = blueprint.replace("\\", "/").lower()
            rows.append({
                "blueprint": ref,
                "equipment": ref.rsplit("/", 1)[-1].removesuffix(".blueprint"),
                "category": category,
                "family": family,
                "name_key": name_key if name_key.startswith("$") else "",
                "desc_key": desc_key if desc_key.startswith("$") else "",
                "raw_category": raw_category,
                "base_mastery": _base_mastery(stem, raw_category),
                "offset": row_found,
            })
    return rows


def style_index(catalogues: dict[str, bytes]) -> dict[str, dict]:
    """`equipment id -> row` across every loot catalogue.

    `catalogues` is `{logical path: bytes}`. Later catalogues do not overwrite an id
    an earlier one already claimed - an id belongs to exactly one slot, and the first
    catalogue that lists it is the one that owns it."""
    out: dict[str, dict] = {}
    for path, data in sorted(catalogues.items()):
        for row in parse_style_catalogue(data, path):
            key = row["equipment"]
            if key and key not in out:
                out[key] = {**row, "source": path}
    return out
