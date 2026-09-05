"""Rebuild app/trove/gamedata/ring_abilities.json from the extracted game tree.

Class rings live in `prefabs/abilities/mods_01/<class>/ring_<slug>.binfab`. Unlike
class abilities and gems, a ring names itself with LITERAL text rather than a
locale key - "Candy Barbarian: Spin-To-Win" and its description sit in the prefab
as plain strings - so that is the bridge here.

The ring itself carries no numbers; it points at the prefab that implements it
(`ring_spin_to_win` -> `spin_to_win_swap`), and the damage and stat records sit
down that chain, same shape as the class abilities: field 12 is the weapon-damage
multiplier, field 14 the base as a fraction of 100.

Run after a game patch:  python scripts/decode_ring_abilities.py
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
from app.trove.codexes.bonuses import STAT_LABELS, extract_stat_bonuses  # noqa: E402

GAME = os.environ.get("TROVE_GAME_DIR", r"E:\Trove")
PREFABS = os.path.join(GAME, "prefabs")
MODS = os.path.join(PREFABS, "abilities", "mods_01")
OUT = Path(__file__).resolve().parents[1] / "app" / "trove" / "gamedata" / "ring_abilities.json"

_BLOCK = re.compile(rb"\xb0\x01[\x00-\xff]\xc4\x01(....)\xd0\x01[\x00-\xff]\xe4\x01(....)", re.S)
# "Candy Barbarian: Spin-To-Win" - a display title, not a path or a locale key.
_TITLE = re.compile(r"^[A-Z][A-Za-z' ]+:\s*\S.*$")


def damage_blocks(data: bytes) -> list[tuple[float, float]]:
    out = []
    for match in _BLOCK.finditer(data):
        mult = struct.unpack("<f", match.group(1))[0]
        base = struct.unpack("<f", match.group(2))[0]
        if 0 <= mult < 1000 and -10 <= base <= 10:
            out.append((round(mult, 4), round(base * 100, 4)))
    return out


def prefab_bytes(rel: str) -> bytes | None:
    path = os.path.join(PREFABS, rel.replace("/", os.sep) + ".binfab")
    return open(path, "rb").read() if os.path.exists(path) else None


def refs(data: bytes, folder: str) -> list[str]:
    """`abilities/mods_01/<class>/…` paths this prefab names, length-prefixed."""
    seen: set[str] = set()
    out: list[str] = []
    needle = f"abilities/mods_01/{folder}/".encode()
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


def _title_and_description(data: bytes) -> tuple[str, str, str]:
    """`(class, name, description)` from the prefab's own literal strings."""
    strings = [s[2] for s in harvest_strings(data)]
    title = next((s for s in strings
                  if _TITLE.match(s) and "/" not in s and not s.endswith(".blueprint")), "")
    if not title:
        return "", "", ""
    owner, _, name = title.partition(":")
    # The description is the longest remaining sentence; paths and one-word tags
    # (Equipment, quantitydecay) are never it.
    body = ""
    for s in strings:
        if s == title or "/" in s or s.endswith(".blueprint") or " " not in s:
            continue
        if len(s) > len(body):
            body = s
    return owner.strip(), name.strip(), body.strip()


def _stat_rows(data: bytes, prefab: str) -> list[dict]:
    rows = []
    for bonus in extract_stat_bonuses(data):
        sid, op, raw = bonus["stat_id"], bonus["operation"], bonus["amount"]
        value = abs(bonus["value"])
        if sid == 0x0F and op == "MultiplySum":
            value = raw * 100 if abs(raw) < 1 else raw
        elif sid == 0x0C and op == "Add":
            value = raw * 100
        row = {"name": STAT_LABELS.get(sid, bonus["stat"]), "stat": bonus["stat"],
               "op": op, "value": round(value, 4), "amount": round(raw, 4), "prefab": prefab}
        if row not in rows:
            rows.append(row)
    return rows


def _walk(rel: str, folder: str, stop: set[str]) -> tuple[list[dict], list[dict], list[str]]:
    """Damage + stat records down one ring's implementation chain."""
    stats: list[dict] = []
    stages: list[dict] = []
    touched: list[str] = []
    seen = {rel}
    queue = [rel]
    while queue:
        cur = queue.pop(0)
        data = prefab_bytes(cur)
        if data is None:
            continue
        rows = _stat_rows(data, cur)
        found = [{"name": cur.rsplit("/", 1)[-1].replace("_", " ").title(), "prefab": cur,
                  "base": base, "multiplier": mult}
                 for mult, base in damage_blocks(data) if mult or base]
        if rows or found:
            touched.append(cur)
        for row in rows:
            if row not in stats:
                stats.append(row)
        for stage in found:
            if stage not in stages:
                stages.append(stage)
        for child in refs(data, folder):
            if child in seen or child in stop or len(seen) > 60:
                continue
            seen.add(child)
            queue.append(child)
    return stats, stages, touched


def _locale_names() -> dict[str, tuple[str, str]]:
    """`ring slug -> (name, description)` from the mods locale table.

    The key is `<class-ish>_<ring slug>` and the class part does not match the
    prefab folder (`candy_barbarian_` vs `candybarbarian/`), so only an exact
    slug tail is accepted - fuzzy matching pairs Deep Wounds with the wrong ring.
    Where it matches, the localised name is better than the prefab's literal
    title, which hyphenates its word breaks: "Melody-Overload" is "Melody
    Overload", and `ring_tactical_shot` is displayed as "Tactical Seekers".
    """
    path = os.path.join(GAME, "languages", "en", "prefabs_abilities_mods.binfab")
    if not os.path.exists(path):
        return {}
    raw = harvest_strings(open(path, "rb").read())
    table: dict[str, str] = {}
    for i, (off, field, text) in enumerate(raw):
        if field != 0 or not text.startswith("$") or i + 1 >= len(raw):
            continue
        noff, nfield, ntext = raw[i + 1]
        if nfield == 1 and 0 <= noff - (off + len(text)) <= 4:
            table[text] = ntext
    out: dict[str, tuple[str, str]] = {}
    for key, value in table.items():
        if not key.endswith("_name"):
            continue
        slug = key[len("$prefabs_abilities_mods_"):-len("_name")]
        desc = table.get(f"$prefabs_abilities_mods_{slug}_description", "")
        out[slug] = (value, desc)
    return out


def build() -> list[dict]:
    names = _locale_names()
    rings = []
    for folder in sorted(os.listdir(MODS)):
        if not os.path.isdir(os.path.join(MODS, folder)):
            continue
        paths = sorted(glob.glob(os.path.join(MODS, folder, "ring_*.binfab")))
        # Other rings of the same class are boundaries, so one ring cannot claim
        # another's numbers - the same rule the class abilities needed.
        others = {f"abilities/mods_01/{folder}/{os.path.basename(p)[:-len('.binfab')]}"
                  for p in paths}
        for path in paths:
            stem = os.path.basename(path)[: -len(".binfab")]
            rel = f"abilities/mods_01/{folder}/{stem}"
            data = open(path, "rb").read()
            owner, name, description = _title_and_description(data)
            if not name:
                continue
            slug = stem[len("ring_"):]
            for key, (loc_name, loc_desc) in names.items():
                if key == slug or key.endswith("_" + slug):
                    name = loc_name or name
                    description = loc_desc or description
                    break
            stats, stages, touched = _walk(rel, folder, others - {rel})
            rings.append({
                "class": owner, "game_folder": folder, "name": name,
                "description": description, "slug": slug,
                "prefab": rel, "prefabs": touched, "stats": stats, "stages": stages,
            })
    return sorted(rings, key=lambda r: (r["class"], r["name"]))


if __name__ == "__main__":
    data = build()
    with_num = sum(1 for r in data if r["stats"] or r["stages"])
    print(f"rings: {len(data)} | classes {len({r['class'] for r in data})} | with numbers {with_num}")
    for ring in data:
        bits = [f"{s['name']} {s['op']} {s['value']}" for s in ring["stats"]]
        bits += [f"{s['multiplier'] * 100:g}%" for s in ring["stages"]]
        print(f"  {ring['class']:16} {ring['name']:26} {bits if bits else ''}")
    with open(OUT, "w", encoding="utf-8", newline="") as fh:
        fh.write(json.dumps(data, indent=4, ensure_ascii=False).replace("\n", "\r\n"))
    print(f"wrote {OUT}")
