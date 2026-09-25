"""class_rewards.json - what each class hands out as it levels, and per Paragon level.

A class prefab (`prefabs/class/<class>.binfab`, root) has:
  7  levels `{level: {1 abilities it unlocks, 2 stat changes (class_levels.py),
     3 the "You have unlocked ..." text key, 4 the costume it grants (a `skins/` id:
     `knight_lvl2` at level 10), 5 a claim id, 6 style blueprints}}`. The level
     claims (`LVL05_ANY_CLASS` ...) are in no claim file: the server hands those
     out, so they are not listed.
  24 Paragon levels `{level: {1 claim ids}}`, 25 the reward for a level not listed,
  28 the class's "prime" Paragon reward. Which levels are prime is not in the files.
Claims read as in mastery.py (`claim/levelup`).
"""
from __future__ import annotations

from typing import Any

from app.trove.decode.ability import Prefabs, identity
from app.trove.decode.common import locale
from app.trove.decode.mastery import _claim
from app.trove.decode.tree import GameTree
from app.trove.decode.wire import Obj, WireError, parse

TITLE = "Class rewards"
OUTPUT = "class_rewards.json"
PREFIXES = ("prefabs/class/", "prefabs/claim/", "prefabs/abilities/", "prefabs/item/",
            "languages/en/")
INDENT, FINAL_NEWLINE = 1, True

LEVELS, PARAGON, PARAGON_DEFAULT, PARAGON_PRIME = 7, 24, 25, 28
L_ABILITIES, L_TEXT, L_COSTUME, L_STYLES = 1, 3, 4, 6


def _leaf(v: Any) -> dict:
    return v.leaf if isinstance(v, Obj) else v if isinstance(v, dict) else {}


def _bp(name: str) -> str:
    return "bp:" + name.rsplit("/", 1)[-1].lower().removesuffix(".blueprint")


def build(tree: GameTree) -> dict:
    prefabs = Prefabs(tree)
    names: dict[str, str] = {}
    for path in tree.files("languages/en/", ".binfab"):
        for key, text in locale(tree.read(path)).items():
            names.setdefault(key, text)
    claim_pf = prefabs.get("claim/levelup")
    table = _leaf(claim_pf.root if claim_pf else None).get(0) or {}

    def claim(cid: Any) -> dict:
        node = table.get(cid) if isinstance(cid, str) and isinstance(table, dict) else None
        return _claim(node[0].get(1), prefabs, names) if isinstance(node, Obj) else {}

    classes = {}
    for path in tree.files("prefabs/class/", ".binfab"):
        tech = path.rsplit("/", 1)[-1].removesuffix(".binfab")
        if "_" in tech:
            continue
        try:
            root = _leaf(parse(tree.read(path) or b"").root)
        except WireError:
            continue
        levels = []
        for lv, row in sorted((root.get(LEVELS) or {}).items()):
            r = _leaf(row)
            if not isinstance(lv, int) or not r:
                continue
            entry: dict[str, Any] = {"level": lv}
            text = names.get(r.get(L_TEXT) or "", "")
            if text:
                entry["text"] = text
            abilities = []
            for ref in r.get(L_ABILITIES) or []:
                if isinstance(ref, str):
                    ref = ref.removesuffix(".binfab")
                    abilities.append({"ref": ref, "name": names.get(identity(prefabs.get(ref)).get("name_key", ""), "")})
            if abilities:
                entry["abilities"] = abilities
            costume = r.get(L_COSTUME)
            if isinstance(costume, str) and costume:
                entry["costume"] = f"skins/{costume}"
            granted = [_bp(b) for b in r.get(L_STYLES) or [] if isinstance(b, str)]
            if granted:
                entry["styles"] = list(dict.fromkeys(granted))
            if len(entry) > 1:
                levels.append(entry)
        paragon: dict[str, Any] = {"levels": len([k for k in root.get(PARAGON) or {} if isinstance(k, int)])}
        every = claim((_leaf(root.get(PARAGON_DEFAULT)).get(1) or [None])[0])
        prime = claim((_leaf(root.get(PARAGON_PRIME)).get(1) or [None])[0])
        if every.get("grants"):
            paragon["every"] = every["grants"]
        if prime.get("grants"):
            paragon["prime"] = prime["grants"]
        classes[tech] = {"levels": levels, "paragon": paragon}
    return {"classes": dict(sorted(classes.items()))}


def count(data: dict) -> int:
    return sum(len(c["levels"]) for c in data["classes"].values())
