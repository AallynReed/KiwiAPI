"""Rebuild app/trove/gamedata/gem_abilities.json from the extracted game tree.

Empowered gems are the ones that carry an ability, and the game names them by
file: `prefabs/item/gem/large/<element>_<slug>_t<tier>.binfab`. The item prefab
itself is only metadata (name key, description key, blueprint), so the numbers
come from `prefabs/abilities/gems/<slug>*` - the same damage record the class
abilities use (field 12 = weapon-damage multiplier, field 14 = base/100) plus
ordinary stat records.

Small gems carry no ability - they are `blue_t1`, `blue_t10_concat1` and so on,
with no slug - which is why only `large/` is walked.

Gem STAT ROLL values (gamedata/mystic.json) are deliberately not touched: they
are not in the prefab tree. `gems/meta/gem_upgradedata.binfab` holds upgrade
costs (flux, gemrepair, warpseed) and the item prefabs hold no stat records, so
the per-tier roll ranges are computed by the game and cannot be read from here.

Run after a game patch:  python scripts/decode_gem_abilities.py
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
OUT = Path(__file__).resolve().parents[1] / "app" / "trove" / "gamedata" / "gem_abilities.json"

# The file prefix is the gem's colour; these are the names the game's own
# descriptions use ("Empowered Water Gem for the Ice Sage.").
ELEMENTS = {"blue": "Water", "red": "Fire", "yellow": "Air",
            "opal": "Cosmic", "prismatic": "Prismatic"}

_ITEM = re.compile(r"^([a-z]+)_(.+)_t(\d+)$")
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


def _stat_rows(data: bytes, prefab: str) -> list[dict]:
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
        row = {"name": STAT_LABELS.get(sid, bonus["stat"]), "stat": bonus["stat"],
               "op": op, "value": round(value, 4), "amount": round(raw, 4),
               "prefab": prefab}
        if row not in rows:
            rows.append(row)
    return rows


def build() -> list[dict]:
    aloc = locale(os.path.join(GAME, "languages", "en", "prefabs_item_gem_large.binfab"))
    ability_files = sorted(glob.glob(os.path.join(PREFABS, "abilities", "gems", "*.binfab")))
    by_stem = {os.path.basename(p)[: -len(".binfab")]: p for p in ability_files}

    gems: dict[tuple[str, str], dict] = {}
    for path in sorted(glob.glob(os.path.join(PREFABS, "item", "gem", "large", "*.binfab"))):
        stem = os.path.basename(path)[: -len(".binfab")]
        match = _ITEM.match(stem)
        if not match:
            continue
        colour, slug, tier = match.groups()
        key = (colour, slug)
        entry = gems.setdefault(key, {
            "element": ELEMENTS.get(colour, colour), "slug": slug,
            "name": "", "description": "", "tiers": [], "prefabs": [],
            "stats": [], "stages": [],
        })
        entry["tiers"].append(int(tier))
        # Three ways the text is reached, in order of authority. The prefab names
        # its own key and that key is not always the filename (`empyrean_barrier`
        # is keyed `..._empyrean_barrier_spawner_...`). Failing that, the filename.
        # Failing that, the same slug under another colour: an ability keeps its
        # name across elements and only one colour's entry is always written -
        # Air's Bawk-Bomb has no key of its own, Water's does.
        keys = [t[: -len("_name")] for t in
                (x[2] for x in harvest_strings(open(path, "rb").read()))
                if t.startswith("$prefabs_item_gem_large") and t.endswith("_name")]
        keys.append(f"$prefabs_item_gem_large_{stem}")
        keys += [f"$prefabs_item_gem_large_{c}_{slug}_t{tier}" for c in ELEMENTS if c != colour]
        for base in keys:
            entry["name"] = entry["name"] or aloc.get(base + "_name", "")
            entry["description"] = entry["description"] or aloc.get(base + "_description", "")

    for (_colour, slug), entry in gems.items():
        entry["tiers"] = sorted(set(entry["tiers"]))
        # The slug names the ability; its prefabs are that slug plus its spawners,
        # effects and bullets. Longest-first so a stage keeps its most specific name.
        for stem in sorted(by_stem, key=len, reverse=True):
            if stem != slug and not stem.startswith(slug + "_"):
                continue
            data = open(by_stem[stem], "rb").read()
            rows = _stat_rows(data, f"abilities/gems/{stem}")
            stages = [{"name": stem.replace("_", " ").title(), "prefab": f"abilities/gems/{stem}",
                       "base": base, "multiplier": mult}
                      for mult, base in damage_blocks(data) if mult or base]
            if rows or stages:
                entry["prefabs"].append(f"abilities/gems/{stem}")
            for row in rows:
                if row not in entry["stats"]:
                    entry["stats"].append(row)
            for stage in stages:
                if stage not in entry["stages"]:
                    entry["stages"].append(stage)
        entry["prefabs"].sort()

    return sorted(gems.values(), key=lambda g: (g["element"], g["name"] or g["slug"]))


if __name__ == "__main__":
    data = build()
    named = sum(1 for g in data if g["name"])
    with_num = sum(1 for g in data if g["stats"] or g["stages"])
    print(f"gems: {len(data)} | named {named} | with decoded numbers {with_num}")
    for gem in data:
        if gem["stats"] or gem["stages"]:
            bits = [f"{s['name']} {s['op']} {s['value']}" for s in gem["stats"]]
            bits += [f"{s['multiplier'] * 100:g}%" for s in gem["stages"]]
            print(f"  {gem['element']:10} {gem['name'] or gem['slug']:24} {bits}")
    with open(OUT, "w", encoding="utf-8", newline="") as fh:
        fh.write(json.dumps(data, indent=4, ensure_ascii=False).replace("\n", "\r\n"))
    print(f"wrote {OUT}")
