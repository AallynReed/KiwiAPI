"""Rebuild site/static/assets/data/stats/pvp.json - the PvE -> PvP stat curves.

PvP converts each PvE stat through a `PVPStatRange` record in
`prefabs/pvp/data/stats/<mode>_statranges.binfab`. A record is
`<wt4 key> 00 <zig stat>` followed by these fields until `1e`:

  1 vec2   (not read by the curve)       5 f32  sqrt coefficient
  2 vec2   output range [lo, hi]         6 bool curve on
  3 vec2   (not read by the curve)       7 bool raise values below the knee to it
  4 f32    knee                          8 f32  soft-cap ceiling

Trove_x64.exe applies them in FUN_14081dff0 (see site/static/calculators.js
`pvpValue`). The files ship inside the client's .tfa archives, so this reads the
Live install directly rather than an extracted tree.

Run after a game patch:  python scripts/decode_pvp_stat_ranges.py
Point TROVE_LIVE_DIR at the client if it is not in the default Glyph folder.
"""

from __future__ import annotations

import json
import os
import struct
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from app.trove.codexes.binfab import read_uleb, unzig  # noqa: E402
from app.trove.codexes.bonuses import STAT_KEYS  # noqa: E402
from app.trove.updates.archive import extract_archive, parse_tfi, verify_entry  # noqa: E402

LIVE = Path(os.environ.get("TROVE_LIVE_DIR", r"C:/Program Files (x86)/Glyph/Games/Trove/Live"))
STATS_DIR = LIVE / "prefabs" / "pvp" / "data" / "stats"
OUT = Path(__file__).resolve().parents[1] / "site" / "static" / "assets" / "data" / "stats" / "pvp.json"

# Mode key (calculators.js PVP_MODES names it) -> file.
MODES = {
    "pvp": "pvp_statranges.binfab",
    "battleroyale": "pvp_battleroyale_statranges.binfab",
    "bloodstone": "pvp_bloodstone_statranges.binfab",
}

# `$Stat_` key -> wire-to-sheet scale, the same as scripts/decode_class_levels.py.
# calculators.js PVP_SHEET names and orders them; stats absent here are not on the sheet.
STATS = {
    "PhysicalDamage": 1, "SpellDamage": 1, "MaxHealth": 1, "MaxEnergy": 1,
    "HealthRegen_controller": 1, "EnergyRegen_controller": 1, "MovementSpeed": 1,
    "AttackSpeed": 1, "Jump": 1, "CriticalHitChance": 0.1, "CriticalHitDamage": 1,
}
REQUIRED = (2, 4, 5, 6, 7, 8)


def parse(data: bytes) -> list[dict]:
    if data[:2] != b"\x3e\xae":
        raise ValueError("not a statranges list")
    count, pos = read_uleb(data, 2)
    pos += 1
    rows = []
    for _ in range(count):
        key, pos = read_uleb(data, pos)
        if key & 0xF != 4 or data[pos] != 0:
            raise ValueError(f"bad record header at {pos}")
        stat, pos = read_uleb(data, pos + 1)
        row: dict = {"stat": unzig(stat)}
        while data[pos] != 0x1E:
            key, pos = read_uleb(data, pos)
            field, wire = key >> 4, key & 0xF
            if wire == 0xC:
                if data[pos] != 0x24:
                    raise ValueError(f"field {field}: not a vec2")
                row[field] = list(struct.unpack_from("<2f", data, pos + 1))
                pos += 9
            elif wire == 4:
                row[field] = struct.unpack_from("<f", data, pos)[0]
                pos += 4
            elif wire == 0:
                value, pos = read_uleb(data, pos)
                row[field] = unzig(value)
            else:
                raise ValueError(f"field {field}: unknown wire type {wire}")
        rows.append(row)
        pos += 1
    return rows


def main() -> None:
    entries = parse_tfi((STATS_DIR / "index.tfi").read_bytes())
    files = extract_archive((STATS_DIR / "archive0.tfa").read_bytes(), entries, 0)
    for e in entries:
        if not verify_entry(e, files[e.name]):
            raise SystemExit(f"{e.name}: hash mismatch")

    modes = {}
    for key, name in MODES.items():
        stats = {}
        for row in parse(files[name]):
            stat = STAT_KEYS.get(row["stat"], "").removeprefix("$Stat_")
            if stat not in STATS:
                continue
            missing = [f for f in REQUIRED if f not in row]
            if missing:
                raise SystemExit(f"{name} {stat}: missing fields {missing}")
            stats[stat] = {
                "scale": STATS[stat], "out": [round(v, 4) for v in row[2]],
                "knee": round(row[4], 4), "sqrt": round(row[5], 4),
                "curve": bool(row[6]), "floor": bool(row[7]), "cap": round(row[8], 4),
            }
        modes[key] = stats

    OUT.write_text(json.dumps({"modes": modes}, indent=1) + "\n", encoding="utf-8")
    print(f"wrote {OUT} ({', '.join(f'{k} {len(v)}' for k, v in modes.items())})")


if __name__ == "__main__":
    main()
