"""Rebuild app/trove/gamedata/class_abilities.json from the extracted game tree.

Each class prefab names the ability prefabs it actually uses; an ability spawns a
chain (ability -> normal/full charge -> bullet -> explosion) and the damage sits
at the leaves. A damage record is field 12 (float, the weapon-damage multiplier)
next to field 14 (float, the base, stored as a fraction of 100).

Validated against the three classes whose ability damage was already curated by
hand: Gunslinger's Blast Jump decodes to base 10 / multiplier 6, matching what
classes.json shipped. Where the two disagree the game files are newer - the
class prefabs point at the `revamp/` abilities, and several multipliers moved.

Run after a game patch:  python scripts/decode_class_abilities.py
Point TROVE_GAME_DIR at the extracted tree if it is not E:\\Trove.
"""

from __future__ import annotations

import glob
import json
import os
import re
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from app.trove.codexes.binfab import harvest_strings  # noqa: E402

GAME = os.environ.get("TROVE_GAME_DIR", r"E:\Trove")
PREFABS = os.path.join(GAME, "prefabs")
OUT = Path(__file__).resolve().parents[1] / "app" / "trove" / "gamedata" / "class_abilities.json"

# f11 varint = b0 01 | f12 float (multiplier) = c4 01 | f13 varint = d0 01 | f14 float (base) = e4 01
_BLOCK = re.compile(rb"\xb0\x01[\x00-\xff]\xc4\x01(....)\xd0\x01[\x00-\xff]\xe4\x01(....)", re.S)


def damage_blocks(data: bytes) -> list[tuple[float, float]]:
    """`[(multiplier, base)]` for each damage record in a prefab."""
    out = []
    for match in _BLOCK.finditer(data):
        mult = struct.unpack("<f", match.group(1))[0]
        base = struct.unpack("<f", match.group(2))[0]
        if 0 <= mult < 1000 and -10 <= base <= 10:
            out.append((round(mult, 4), round(base * 100, 4)))
    return out


def locale(path: str) -> dict[str, str]:
    """`$key -> text`, paired by byte adjacency so a missing value cannot shift the map."""
    raw = harvest_strings(open(path, "rb").read())
    out = {}
    for i, (off, field, text) in enumerate(raw):
        if field != 0 or not text.startswith("$") or i + 1 >= len(raw):
            continue
        noff, nfield, ntext = raw[i + 1]
        if nfield == 1 and 0 <= noff - (off + len(text)) <= 4:
            out[text] = ntext
    return out


def prefab_bytes(rel: str) -> bytes | None:
    path = os.path.join(PREFABS, rel.replace("/", os.sep) + ".binfab")
    return open(path, "rb").read() if os.path.exists(path) else None


def refs(data: bytes, folder: str) -> list[str]:
    """`abilities/<folder>/...` paths this prefab names, in order, deduped.

    Prefab refs are length-prefixed, so the byte in front of the match gives the
    exact end - scanning for a printable run instead runs past it into the next
    record and invents paths like `blast_jump_explosionX`.
    """
    seen: set[str] = set()
    out: list[str] = []
    needle = f"abilities/{folder}/".encode()
    for match in re.finditer(re.escape(needle), data):
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


def stage_name(rel: str) -> str:
    """A readable stage label from the prefab stem - the game ships no stage names."""
    return rel.rsplit("/", 1)[-1].replace("_", " ").title()


def _label(aloc: dict[str, str], folder: str, stem: str) -> tuple[str | None, str]:
    """Ability name + description. Three key shapes ship side by side."""
    for suffix in ("_item_name", "_name", ""):
        name = aloc.get(f"$prefabs_abilities_{folder}_{stem}{suffix}")
        if name:
            desc_suffix = "_item_description" if suffix == "_item_name" else "_description"
            return name, aloc.get(f"$prefabs_abilities_{folder}_{stem}{desc_suffix}", "")
    return None, ""


def _stages(rel: str, root: bytes, folder: str) -> list[dict]:
    stages: list[dict] = []
    seen = {rel}
    queue = [(rel, root)]
    while queue:
        cur, data = queue.pop(0)
        for mult, base in damage_blocks(data):
            if mult == 0 and base == 0:
                continue
            stages.append({"name": stage_name(cur), "prefab": cur, "base": base, "multiplier": mult})
        for child in refs(data, folder):
            if child in seen or len(seen) > 60:
                continue
            seen.add(child)
            child_data = prefab_bytes(child)
            if child_data is not None:
                queue.append((child, child_data))
    return stages


def _icon_token(icon: str, folder: str) -> str:
    """The ability-identifying part of a curated icon name.

    `ico_gunslinger_chargeshot_01` -> `chargeshot`, which is what lets a curated
    entry find its prefab after the ability was renamed in game.
    """
    parts = [p for p in icon.split("_") if p]
    parts = [p for p in parts if p not in ("ico", "icon", folder) and not p.isdigit()]
    return "".join(parts)


def _overlay_curated(entry: dict, curated: list[dict], folder: str) -> list[dict]:
    """Carry the hand-written `icon` and `type` onto the decoded abilities.

    Names drift - Boomeranger's `Boomerang` is `Boomerang of the Wind` in game -
    so an exact name match is tried first, then the curated icon token against the
    prefab stem. The icon match only applies when exactly one unclaimed curated
    entry fits, so an ambiguous one is left blank rather than guessed. Curated
    abilities the decode never reached are kept as-is, without a `prefab`.
    """
    by_name = {a["name"]: a for a in curated}
    claimed = set()
    for ability in entry["abilities"]:
        hit = by_name.get(ability["name"])
        if hit:
            claimed.add(hit["name"])
            ability["icon"], ability["type"] = hit.get("icon", ""), hit.get("type", "")

    for ability in entry["abilities"]:
        if ability.get("type"):
            continue
        stem = re.sub(r"[^a-z0-9]", "", ability["prefab"].rsplit("/", 1)[-1].lower())
        fits = [a for a in curated if a["name"] not in claimed
                and (tok := _icon_token(a.get("icon", ""), folder)) and tok in stem]
        if len(fits) == 1:
            ability["icon"], ability["type"] = fits[0].get("icon", ""), fits[0].get("type", "")
            ability["matched_by"] = "icon"

    for a in curated:
        if a["name"] in claimed or any(x.get("icon") == a.get("icon") and x.get("type") == a.get("type")
                                       for x in entry["abilities"]):
            continue
        entry["abilities"].append({
            "name": a["name"], "description": "", "icon": a.get("icon", ""),
            "type": a.get("type", ""), "stages": a.get("stages", []),
        })
    return entry["abilities"]


def build() -> list[dict]:
    curated_path = Path(__file__).resolve().parents[1] / "app" / "trove" / "gamedata" / "classes.json"
    curated_by_class = {c["name"]: (c.get("abilities") or [])
                        for c in json.loads(curated_path.read_text(encoding="utf-8"))}
    display: dict[str, str] = {}
    for name in ("prefabs_class.binfab", "ui.binfab", "new.binfab"):
        path = os.path.join(GAME, "languages", "en", name)
        if os.path.exists(path):
            display.update(locale(path))

    out = []
    for class_path in sorted(glob.glob(os.path.join(PREFABS, "class", "*.binfab"))):
        folder = os.path.basename(class_path)[: -len(".binfab")]
        if folder.endswith("_ultimate"):
            continue
        data = open(class_path, "rb").read()
        key = next((s[2] for s in harvest_strings(data) if s[2].startswith("$DisplayName")), None)
        if not key:
            continue

        # The ability folder is not always the class folder - Fae Trickster's class
        # prefab is `faetrickster` but its abilities live under `abilities/trickster`.
        match = re.search(rb"abilities/([a-z0-9_]+)/", data)
        afolder = match.group(1).decode() if match else folder
        locale_path = os.path.join(GAME, "languages", "en", f"prefabs_abilities_{afolder}.binfab")
        aloc = locale(locale_path) if os.path.exists(locale_path) else {}

        abilities = []
        seen_names: set[str] = set()
        for rel in refs(data, afolder):
            name, desc = _label(aloc, afolder, rel.rsplit("/", 1)[-1])
            if not name or name in seen_names:
                continue
            root = prefab_bytes(rel)
            if root is None:
                continue
            seen_names.add(name)
            abilities.append({
                "name": name,
                "description": desc,
                "prefab": rel,
                "icon": "",
                "type": "",
                "stages": _stages(rel, root, afolder),
            })
        entry = {
            "name": display.get(key, key.replace("$DisplayName_", "")),
            "game_folder": folder,
            "abilities": abilities,
        }
        entry["abilities"] = _overlay_curated(entry, curated_by_class.get(entry["name"], []), folder)
        out.append(entry)
    return out


if __name__ == "__main__":
    classes = build()
    for entry in classes:
        stages = sum(len(a["stages"]) for a in entry["abilities"])
        print(f"  {entry['name']:18} abilities={len(entry['abilities']):2} stages={stages}")
    with open(OUT, "w", encoding="utf-8", newline="") as fh:
        fh.write(json.dumps(classes, indent=4, ensure_ascii=False).replace("\n", "\r\n"))
    print(f"wrote {OUT}")
