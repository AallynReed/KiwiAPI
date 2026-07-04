"""Recipe prefab parsing, ported from BTT (`models/trove/prefab_recipe.py`).

Recipe prefabs carry no identity component, so a filename is all the generic
extractor gets. The real content is a list of quantified item paths
(`08 <len> <path> 10 <amount>`): the crafted **output** plus its **ingredients**,
with non-path tokens spelling out **requirements** (class / power rank / collection
/ zone). The output's display name + description come from the output item's own
prefab, resolved through the locale tables - so a `resolve_meta(path)` callback
that reads referenced item prefabs is passed in by the indexer.

Pure + stdlib-only; names degrade to a prettified path stem when a referenced
prefab isn't resolvable.
"""

from __future__ import annotations

import re
from pathlib import PurePosixPath

from app.trove.codexes.binfab import clean_localized_text, read_varint, unzig

_ASCII_RE = re.compile(rb"[ -~]{3,}")
# Path-ish prefixes that mark an item/placeable/etc. reference (vs. a token/label).
_PATH_PREFIXES = ("item/", "placeable/", "block/", "collections/", "effects/")

# Non-path requirement tokens -> their display label.
_REQUIREMENT_LABELS = {
    "activeclass": "Active Class",
    "powerrank": "Power Rank",
    "hascollection": "Requires Collection",
    "hastitle": "Requires Title",
}

# Output path prefix -> a coarse recipe category.
_CATEGORY_BY_OUTPUT: tuple[tuple[str, str], ...] = (
    ("item/mount/", "Mount"),
    ("item/pet/", "Ally"),
    ("collections/pet/", "Ally"),
    ("item/unlocker/", "Memento"),
    ("item/plantseed/", "Seed"),
    ("item/consumable/titles/", "Title"),
    ("placeable/", "Placeable"),
    ("block/", "Block"),
    ("item/lootbox/newrings/", "Ring"),
    ("item/consumable/gearcrafting/", "Gear"),
    ("item/costume/", "Costume"),
)


def _normalize(text: str) -> str:
    return clean_localized_text(str(text or "").strip()).rstrip("(").strip('"').strip("'").strip()


def _pretty_name(path: str) -> str:
    stem = PurePosixPath(str(path or "").replace("\\", "/")).stem
    for suffix in ("_notrade", "_trade"):
        if stem.endswith(suffix):
            stem = stem[: -len(suffix)]
            break
    return stem.replace("_", " ").strip().title()


def _looks_like_path(text: str) -> bool:
    return str(text or "").strip().lower().startswith(_PATH_PREFIXES)


def _is_material(path: str) -> bool:
    p = str(path or "").lower()
    return p.startswith(("item/crafting/", "item/currency/", "item/dragon/"))


def _is_output_candidate(path: str) -> bool:
    p = str(path or "").lower()
    if not p or p.startswith(("effects/", "collections/")):
        return False
    return p.startswith(("item/", "placeable/", "block/"))


def _scan_strings(content: bytes) -> list[str]:
    out: list[str] = []
    for match in _ASCII_RE.finditer(content):
        text = _normalize(match.group().decode("ascii", errors="ignore"))
        if text:
            out.append(text)
    return out


def decode_quantified_paths(content: bytes) -> list[dict]:
    """`08 <uleb len> <ascii path> 10 <uleb amount>` records -> [{path, amount}]."""
    rows: list[dict] = []
    pos = 0
    n = len(content)
    while pos < n - 3:
        if content[pos] != 0x08:
            pos += 1
            continue
        length, cursor = read_varint(content, pos + 1)
        if length is None or length <= 0 or cursor + length > n:
            pos += 1
            continue
        raw = content[cursor:cursor + length]
        if any(b < 32 or b > 126 for b in raw):
            pos += 1
            continue
        text = _normalize(raw.decode("ascii", errors="ignore"))
        if not _looks_like_path(text):
            pos += 1
            continue
        nxt = cursor + length
        if nxt >= n or content[nxt] != 0x10:
            pos = nxt
            continue
        amount_raw, end = read_varint(content, nxt + 1)
        if amount_raw is None:
            pos += 1
            continue
        rows.append({"path": text, "amount": int(unzig(int(amount_raw))), "end": end})
        pos = end
    return rows


def _split_sections(strings: list[str]) -> tuple[list[str], list[str]]:
    nulls = [i for i, s in enumerate(strings) if s.lower() == "null"]
    if len(nulls) >= 2:
        return strings[:nulls[0]], strings[nulls[1] + 1:]
    if len(nulls) == 1:
        return strings[:nulls[0]], strings[nulls[0] + 1:]
    return strings, []


def _choose_output(strings: list[str]) -> str:
    head, tail = _split_sections(strings)
    for section in (tail, head):
        for text in section:
            if _is_output_candidate(text) and not _is_material(text):
                return text
    for text in tail + head:
        if text.startswith("collections/"):
            return text
    return ""


def _output_amount(output_path: str, quantified: list[dict]) -> int:
    for row in reversed(quantified):
        if _normalize(row["path"]) == _normalize(output_path):
            return row["amount"] if row["amount"] > 0 else 1
    return 1


def _requirements(strings: list[str], ingredient_paths: set[str], output_path: str) -> list[str]:
    out: list[str] = []
    norm = [_normalize(s) for s in strings]
    output_path = _normalize(output_path)
    for i, text in enumerate(norm):
        if not text or text.lower() == "null":
            continue
        low = text.lower()
        if text in ingredient_paths or text == output_path or _looks_like_path(text) or low.startswith("$prefabs_"):
            continue
        if low == "zonetag" and i + 1 < len(norm) and norm[i + 1] and norm[i + 1].lower() != "zonetag":
            out.append(f"Zone: {norm[i + 1]}")
        elif low in _REQUIREMENT_LABELS:
            out.append(_REQUIREMENT_LABELS[low])
    return list(dict.fromkeys(out))


def category_from_output(output_path: str) -> str:
    low = str(output_path or "").lower()
    for prefix, label in _CATEGORY_BY_OUTPUT:
        if low.startswith(prefix):
            return label
    return "Recipe"


def _match_mastery_struct(content: bytes, pos: int) -> int | None:
    """Match the recipe-mastery structure at ``pos`` (must start on the ``0x30``
    tag) and return its mastery value, else None.

    The trusted structure is the protobuf-ish tag run (see
    ``recipe_mastery_from_prefab``)::

        30 <00|02> 50 <mastery> 70 <byte> 80 01 <byte> 98 01

    Reading the whole chain (not a bare ``0x50``) is what excludes ``50 70 6c…``
    bytes that are really part of a ``Pplaceable…`` string payload.
    """
    n = len(content)
    if pos >= n or content[pos] != 0x30:
        return None
    v6, cur = read_varint(content, pos + 1)
    if v6 is None or v6 not in (0, 2):                       # 30 <00|02>
        return None
    if cur >= n or content[cur] != 0x50:                     # 50 <mastery>
        return None
    mastery, cur = read_varint(content, cur + 1)
    if mastery is None:
        return None
    if cur >= n or content[cur] != 0x70:                     # 70 <byte>
        return None
    _v14, cur = read_varint(content, cur + 1)
    if _v14 is None:
        return None
    if cur + 1 >= n or content[cur] != 0x80 or content[cur + 1] != 0x01:  # 80 01 <byte>
        return None
    _v16, cur = read_varint(content, cur + 2)
    if _v16 is None:
        return None
    if cur + 1 >= n or content[cur] != 0x98 or content[cur + 1] != 0x01:  # 98 01
        return None
    return int(mastery)


def recipe_mastery_from_prefab(content: bytes) -> int | None:
    """Trusted recipe-mastery byte read STRUCTURALLY from a recipe prefab, or None.

    The mastery value is field 10 (``0x50``) inside the recipe's row structure::

        30 <00|02> 50 <mastery> 70 <byte> 80 01 <byte> 98 01

    Returns the value only when every structural match in the prefab agrees (the
    common case is a single match). Zero matches, or conflicting matches, return
    None - ``recipe_block_glass_brown_01`` (``50 00`` -> 0) vs. ``brown_02``
    (``50 02`` -> 2) is exactly the distinction category/tag/unlocker evidence
    could not make. A None here is review-only, never a broad-category guess.
    """
    matches: list[int] = []
    for pos in range(len(content)):
        if content[pos] != 0x30:
            continue
        value = _match_mastery_struct(content, pos)
        if value is not None:
            matches.append(value)
    if not matches or len(set(matches)) != 1:
        return None
    return matches[0]


def resolve_recipe_mastery(rel: str, content: bytes, multipliers: dict[str, dict]) -> dict:
    """Recipe mastery via the evidence hierarchy -> ``{value, source, prefab_byte}``.

    1. an exact ``prefabs/meta/multipliers.binfab`` row (keyed by the recipe
       identifier) overrides everything (e.g. ``recipe_item_consumable_prismatic_red``
       carries ``50 02`` but the multiplier produces 10);
    2. otherwise the trusted structural prefab byte (``recipe_mastery_from_prefab``);
    3. otherwise ``value=None`` (``source="review"``) - no trusted byte / conflicting
       matches stay review-only rather than falling back to a category guess.
    """
    prefab_byte = recipe_mastery_from_prefab(content)
    stem = rel.replace("\\", "/").rsplit("/", 1)[-1]
    row = multipliers.get(stem)
    if row is not None:
        return {"value": int(row["predicted"]), "source": "multipliers", "prefab_byte": prefab_byte}
    if prefab_byte is not None:
        return {"value": prefab_byte, "source": "prefab", "prefab_byte": prefab_byte}
    return {"value": None, "source": "review", "prefab_byte": None}


def parse_recipe(content: bytes, *, resolve_meta=None) -> dict:
    """Decode a recipe prefab into ``{name, description, category, output,
    ingredients, requirements}``.

    ``resolve_meta(path) -> {"name", "desc"}`` reads a referenced item prefab to
    resolve real display names/descriptions; when omitted (or it returns nothing),
    names degrade to the prettified path stem.
    """
    strings = _scan_strings(content)
    quantified = decode_quantified_paths(content)
    output_path = _choose_output(strings)
    output_norm = output_path.removesuffix(".binfab") if output_path else ""

    ingredient_paths: set[str] = set()
    ingredients: list[dict] = []
    for row in quantified:
        p = _normalize(row["path"])
        if not p or p == output_norm or p.startswith("collections/"):
            continue
        ingredient_paths.add(p)
        ingredients.append({"path": p, "amount": row["amount"], "name": _pretty_name(p)})

    name = _pretty_name(output_norm) if output_norm else ""
    description = ""
    if resolve_meta and output_norm:
        meta = resolve_meta(output_norm) or {}
        name = meta.get("name") or name
        description = meta.get("desc") or ""
        for ing in ingredients:
            im = resolve_meta(ing["path"]) or {}
            if im.get("name"):
                ing["name"] = im["name"]

    return {
        "name": name,
        "description": description,
        "category": category_from_output(output_norm),
        "output": {"path": output_norm, "name": name, "amount": _output_amount(output_norm, quantified)} if output_norm else None,
        "ingredients": ingredients,
        "requirements": _requirements(strings, ingredient_paths, output_norm),
    }
