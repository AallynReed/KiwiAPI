"""lootbox_odds.json - the game's own drop-rate disclosure for its boxes.

The client ships the odds as one text in `languages/en/store_loot_probability.binfab`
(`$Store_Lootbox_Probabilities_Desc`, titled by `..._Title`): per box, a short name line
next to a blank line, notes without a percentage, then `name - n%` lines. Lines
indented by tabs are items under the shallower tier line above them ("Common - 86%");
a box with no indented line lists its items directly. The dash is "-" or "–".
Boxes are named, not referenced, so nothing here is joined to an item.
"""
from __future__ import annotations

import re
from typing import Any

from app.trove.decode.common import locale
from app.trove.decode.tree import GameTree

TITLE = "Lootbox odds"
OUTPUT = "lootbox_odds.json"
PREFIXES = ("languages/en/",)
INDENT, FINAL_NEWLINE = 1, True

TABLE = "languages/en/store_loot_probability.binfab"
ODDS = re.compile(r"^(?P<name>.+?)\s+[-–—]\s+(?P<pct>[\d.]+)\s*%\s*$")
ITEM_INDENT = 3


def _indent(line: str) -> int:
    return len(line) - len(line.lstrip(" \t"))


def _box(name: str, lines: list[str]) -> dict[str, Any]:
    notes, rows = [], []
    for line in lines:
        m = ODDS.match(line.strip())
        if m:
            rows.append((_indent(line), m["name"].strip(), float(m["pct"])))
        elif line.strip():
            notes.append(line.strip())
    tiered = any(i >= ITEM_INDENT for i, _, _ in rows)
    tiers: list[dict] = []
    for indent, label, pct in rows:
        if tiered and indent < ITEM_INDENT:
            tiers.append({"name": label, "chance": pct, "items": []})
        elif tiered and tiers:
            tiers[-1]["items"].append({"name": label, "chance": pct})
        else:
            if not tiers:
                tiers.append({"name": "", "chance": None, "items": []})
            tiers[-1]["items"].append({"name": label, "chance": pct})
    return {"name": name, "notes": notes, "tiers": tiers}


def build(tree: GameTree) -> dict:
    text = locale(tree.read(TABLE))
    body = (text.get("$Store_Lootbox_Probabilities_Desc") or "").replace("\\n", "\n")
    lines = body.split("\n")
    boxes, name, chunk = [], None, []
    for i, line in enumerate(lines):
        blank_near = i == 0 or not lines[i - 1].strip() or i + 1 < len(lines) and not lines[i + 1].strip()
        if (line.strip() and blank_near and _indent(line) == 0 and not ODDS.match(line.strip())
                and not line.strip().endswith(".") and len(line.strip()) <= 60):
            if name:
                boxes.append(_box(name, chunk))
            name, chunk = line.strip().replace("�", "'"), []
        elif name:
            chunk.append(line)
    if name:
        boxes.append(_box(name, chunk))
    return {"title": text.get("$Store_Lootbox_Probabilities_Title") or "", "boxes": [b for b in boxes if b["tiers"] or b["notes"]]}


def count(data: dict) -> int:
    return len(data["boxes"])
