"""Rebuild app/trove/gamedata/ally_abilities.json from the extracted game tree.

Allies are the `prefabs/collections/pet/*.binfab` that carry combat stats - 1,200
of the 2,412 pet prefabs; the rest are cosmetic pets with nothing to grant. Both
the stat records and the ability refs come out of app/trove/codexes/bonuses.py,
which was written for exactly these collection prefabs, so there is no new decode
here - only the naming and the filtering.

Names and descriptions come from the prefab's own `$prefabs_collections_pet_…`
keys, resolved against languages/en/prefabs_collections_pet.binfab; an ability's
text is resolved the same way from the key its ref carries.

Run after a game patch:  python scripts/decode_ally_abilities.py
Point TROVE_GAME_DIR at the extracted tree if it is not E:\\Trove.
"""

from __future__ import annotations

import glob
import json
import os
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from app.trove.codexes.binfab import decode_identity, harvest_strings  # noqa: E402
from app.trove.codexes.bonuses import (  # noqa: E402
    extract_abilities,
    extract_stat_bonuses,
)
from app.trove.codexes.localize import resolve_stat_name  # noqa: E402

GAME = os.environ.get("TROVE_GAME_DIR", r"E:\Trove")
PETS = os.path.join(GAME, "prefabs", "collections", "pet")
LOCALES = os.path.join(GAME, "languages", "en")
OUT = Path(__file__).resolve().parents[1] / "app" / "trove" / "gamedata" / "ally_abilities.json"


def locale(path: str) -> dict[str, str]:
    """`$key -> text`, paired by byte adjacency so a missing value cannot shift the map."""
    if not os.path.exists(path):
        return {}
    raw = harvest_strings(open(path, "rb").read())
    out = {}
    for i, (off, field, text) in enumerate(raw):
        if field != 0 or not text.startswith("$") or i + 1 >= len(raw):
            continue
        noff, nfield, ntext = raw[i + 1]
        if nfield == 1 and 0 <= noff - (off + len(text)) <= 4:
            out[text] = ntext
    return out


def _stat_rows(data: bytes) -> list[dict]:
    rows = []
    for bonus in extract_stat_bonuses(data):
        sid, op, raw = bonus["stat_id"], bonus["operation"], bonus["amount"]
        value = abs(bonus["value"])
        if sid == 0x0F and op == "MultiplySum":
            value = raw * 100 if abs(raw) < 1 else raw
        elif sid == 0x0C and op == "Add":
            value = raw * 100
        row = {"name": resolve_stat_name({}, bonus["stat"]), "stat": bonus["stat"],
               "op": op, "value": round(value, 4), "amount": round(raw, 4)}
        if row not in rows:
            rows.append(row)
    return rows


def build() -> list[dict]:
    names = locale(os.path.join(LOCALES, "prefabs_collections_pet.binfab"))
    # An ability's text lives in whichever prefabs_abilities_* table its ref belongs
    # to, so merge them all rather than guessing which file to open per ability.
    abilities_text: dict[str, str] = {}
    for path in glob.glob(os.path.join(LOCALES, "prefabs_abilities*.binfab")):
        abilities_text.update(locale(path))

    allies = []
    for path in sorted(glob.glob(os.path.join(PETS, "*.binfab"))):
        data = open(path, "rb").read()
        stats = _stat_rows(data)
        if not stats:
            continue                       # a cosmetic pet, not an ally
        stem = os.path.basename(path)[: -len(".binfab")]
        identity = decode_identity(data) or {}
        name = names.get(identity.get("name_key") or "", "")
        description = names.get(identity.get("desc_key") or "", "")

        powers = []
        for ability in extract_abilities(data):
            if ability.get("hidden"):
                continue
            text = abilities_text.get(ability.get("key") or "", "")
            row = {"ref": ability.get("ref", ""), "text": text}
            if text and row not in powers:
                powers.append(row)

        allies.append({
            "slug": stem, "name": name or stem, "description": description,
            "prefab": f"collections/pet/{stem}", "stats": stats, "abilities": powers,
        })
    return sorted(allies, key=lambda a: a["name"].lower())


if __name__ == "__main__":
    data = build()
    named = sum(1 for a in data if a["name"] != a["slug"])
    with_ability = sum(1 for a in data if a["abilities"])
    print(f"allies: {len(data)} | named {named} | with an ability {with_ability}")
    for ally in data[:8]:
        bits = [f"{s['name']} {s['op']} {s['value']}" for s in ally["stats"]]
        print(f"  {ally['name'][:34]:34} {bits}")
    with open(OUT, "w", encoding="utf-8", newline="") as fh:
        fh.write(json.dumps(data, indent=4, ensure_ascii=False).replace("\n", "\r\n"))
    print(f"wrote {OUT}")
