"""mastery.json - the Trove, Geode and PvP Mastery ladders, and what each
collectible is worth.

`prefabs/meta/{meta,geodemeta,pvpmeta}.binfab` (root):
  0 points per mastery source `{0 source, 1 points}`
  1 levels `{level: {0 points to reach it from the level before, 1 reward ids}}`
    (levels 0 and 1 cost nothing: you start at 1)
  2 what every level grants up to the cap, 5 the cap, 6 what every level past it
    grants: rows `{3 kind, 5 stat modifier records}` (kind 1 = stats)
  3 rewards `{0 id, 1 text key, 2 icon, 3 kind, 4 claim id, 5 stat modifier records}`
These objects have one section each, which the generic reader can't know (it
folds levels 11-19 into level 10), so they are read by `_single` below.

A reward's claim (`prefabs/claim/<ladder>.binfab`: `{id: {0, 1 {0 type, 1 [base
{3 name key}, ..., own]}}}`) says what it hands out: `ClaimCollectable` own 0 rows
`{0 collection type, 1 ref}`, `ClaimPrefab` own 0 rows `{0 item, 1 count}`,
`ClaimPoints` own `{0 amount}`, `ClaimCompound` own 0 nested claims.

The source ids are the exe's mastery-message switch (FUN_1402601a0, 0922 client):
0/1 EquipmentAppearance, 2 Recipe, 3 Mount, 4 Pet, 5 Skin, 7 Cart, 8 ClassLevel,
9 ClassLevel20Milestone, 10 ClassLevelMax, 11 ProfessionTier, 12 Flask, 13 Wings,
14 Tome, 15 Boat, 16 Sail, 17 FishingPole, 18 Fish, 19 FlaskEffect, 21 Memento,
22 Aura, 29 Pet's message again (the Geode companion: 10 points, 50 Geode Mastery,
as the codex notes found). Badges have no message; the two unnamed sources worth
20 (6, 20) are both 20, so a badge is 20 either way. Secondary skins have no
source here and get no value.

A collectible's value is its source's points times the multiplier of the
`meta/multipliers` (Geode: `meta/geode_multipliers`) group listing it
(`{0 multiplier, 1 rows {0 collection type, 1 ref}}`), else the root field 1
default group; a collectible listed only in collection groups whose field 2 is 0
(the Hidden groups) counts for nothing (mementos.py).
"""
from __future__ import annotations

from typing import Any

from app.trove.decode.ability import Prefabs, identity
from app.trove.decode.common import locale, stat_rows
from app.trove.decode.recipes import COLLECTION_TYPES
from app.trove.decode.tree import GameTree
from app.trove.decode.wire import ARR, BIGMAP, END, MAP, OBJ, STRMAP, Obj, WireError, _Reader, parse

TITLE = "Mastery"
OUTPUT = "mastery.json"
PREFIXES = ("prefabs/meta/", "prefabs/claim/", "prefabs/collections/", "prefabs/item/", "languages/en/")
INDENT, FINAL_NEWLINE = None, True

LADDERS = (("trove", "Trove Mastery", "meta", "meta/multipliers"),
           ("geode", "Geode Mastery", "geodemeta", "meta/geode_multipliers"),
           ("pvp", "PvP Mastery", "pvpmeta", None))
SOURCE = {"EquipmentAppearance": 0, "Recipe": 2, "Mount": 3, "Pet": 4, "Skin": 5, "Cart": 7, "Flask": 12,
          "Wings": 13, "Tome": 14, "Boat": 15, "Sail": 16, "FishingPole": 17, "Fish": 18, "FlaskEffect": 19,
          "Badge": 20, "Aura": 22, "GeodeCompanion": 29, "Memento": 21}
SOURCE_NAMES = {0: "Style", 1: "Hat style", 2: "Recipe", 3: "Mount", 4: "Ally", 5: "Costume", 7: "Mag Rider",
                8: "Class level", 9: "Every 20th class level", 10: "Class at max level", 11: "Profession tier",
                12: "Flask", 13: "Wings", 14: "Tome", 15: "Boat", 16: "Sail", 17: "Fishing pole", 18: "Fish",
                19: "Emblem", 20: "Badge rank", 21: "Memento", 22: "Aura", 29: "Geode companion"}
SKIN, SECONDARY_SKIN, STYLE = (COLLECTION_TYPES.index(x) for x in ("Skin", "SecondarySkin", "EquipmentAppearance"))


def _single(data: bytes) -> dict:
    """A bare object read on the rule that every object has one section."""
    r = _Reader(data, len(data))

    def value(p: int, wt: int) -> tuple[Any, int]:
        if wt == OBJ:
            return section(p)
        if wt == ARR:
            _, ewt, q = r.key(p)
            slots: dict[int, Any] = {}
            while True:
                k, q = r.zz(q, 70)
                op, slot = k & 7, k >> 3
                if op == 4:
                    return [slots.get(i) for i in range(max(slots, default=-1) + 1)], q
                if op == 0:
                    slots[slot] = None
                    continue
                if op != 2:
                    raise WireError("array op")
                slots[slot], q = value(q, ewt)
        if wt in (MAP, BIGMAP, STRMAP):
            _, vwt, q = r.key(p)
            out: dict[Any, Any] = {}
            while True:
                if wt == MAP:
                    k, q = r.zz(q, 70)
                    op, mk = k & 7, k >> 3
                else:
                    op, q = data[q], q + 1
                    if wt == BIGMAP:
                        mk, q = r.zz(q, 70)
                    else:
                        n, q = r.uvar(q)
                        mk, q = data[q:q + n].decode("utf-8", "replace"), q + n
                if op == 4:
                    return out, q
                if op in (0, 3):
                    out[mk] = None
                    continue
                out[mk], q = value(q, vwt)
        return r.scalar(p, wt)

    def section(p: int) -> tuple[dict, int]:
        out: dict[int, Any] = {}
        while True:
            idx, wt, q = r.key(p)
            if wt == END:
                return out, q
            out[idx], p = value(q, wt)

    root, end = section(0)
    if end != len(data):
        raise WireError("trailing bytes")
    return root


def _leaf(v: Any) -> dict:
    return v.leaf if isinstance(v, Obj) else {}


def _claims(prefabs: Prefabs, names: dict[str, str], stem: str) -> dict[str, dict]:
    pf = prefabs.get(f"claim/{stem}")
    table = _leaf(pf.root if pf else None).get(0)
    out = {}
    for cid, node in (table or {}).items() if isinstance(table, dict) else ():
        if isinstance(node, Obj):
            out[cid] = _claim(node[0].get(1), prefabs, names)
    return out


def _claim(claim: Any, prefabs: Prefabs, names: dict[str, str]) -> dict:
    if not isinstance(claim, Obj) or not claim:
        return {}
    kind, body = claim[0].get(0), claim[0].get(1)
    if not isinstance(body, Obj) or not body:
        return {}
    own = body[-1] if len(body) > 1 else {}
    out: dict[str, Any] = {}
    title = names.get(body[0].get(3) or "", "")
    if title:
        out["name"] = title
    grants = []
    if kind == "ClaimCollectable":
        for row in own.get(0) or []:
            r = _leaf(row)
            if isinstance(r.get(0), int) and isinstance(r.get(1), str):
                grants.append({"ref": _ref(r[0], r[1])})
    elif kind == "ClaimPrefab":
        for row in own.get(0) or []:
            r = _leaf(row)
            if isinstance(r.get(0), str):
                item = r[0].removesuffix(".binfab")
                grants.append({"ref": item, "count": r.get(1) or 1,
                               "name": names.get(identity(prefabs.get(item)).get("name_key", ""), "")})
    elif kind == "ClaimCompound":
        for sub in own.get(0) or []:
            grants += _claim(sub, prefabs, names).get("grants") or []
    if grants:
        out["grants"] = grants
    return out


def _ref(ctype: int, ref: str) -> str:
    ref = ref.removesuffix(".binfab")
    if ctype == SKIN and "/" not in ref:
        return f"skins/{ref}"
    if ctype == SECONDARY_SKIN and "/" not in ref:
        return f"skins/secondary/{ref}"
    return ref


def _ladder(slug: str, name: str, stem: str, tree: GameTree, prefabs: Prefabs, names: dict[str, str]) -> dict | None:
    try:
        root = _single(tree.read(f"prefabs/meta/{stem}.binfab") or b"")
    except WireError:
        return None
    claims = _claims(prefabs, names, stem if stem != "pvpmeta" else "pvp")
    levels = []
    for lv, row in sorted((root.get(1) or {}).items()):
        if not isinstance(lv, int) or lv < 1 or not isinstance(row, dict):
            continue
        entry: dict[str, Any] = {"level": lv, "points": row.get(0) or 0}
        ids = [i for i in row.get(1) or [] if isinstance(i, int)]
        if ids:
            entry["rewards"] = ids
        levels.append(entry)
    rewards = []
    for rw in root.get(3) or []:
        if not isinstance(rw, dict) or not isinstance(rw.get(0), int):
            continue
        claim = claims.get(rw.get(4) or "", {})
        out: dict[str, Any] = {"id": rw[0], "text": names.get(rw.get(1) or "", "") or claim.get("name", "")}
        stats = stat_rows(rw.get(5) or [])
        if stats:
            out["stats"] = stats
        if claim.get("grants"):
            out["grants"] = claim["grants"]
        if isinstance(rw.get(2), str) and rw[2]:
            out["icon"] = rw[2]
        rewards.append(out)

    def every(rows: Any) -> list[dict]:
        return [s for r in rows or [] if isinstance(r, dict) and r.get(3) == 1 for s in stat_rows(r.get(5) or [])]

    return {"slug": slug, "name": name,
            "sources": [{"source": r[0], "points": r[1]} for r in root.get(0) or []
                        if isinstance(r, dict) and isinstance(r.get(0), int) and isinstance(r.get(1), int)],
            "levels": levels, "rewards": rewards, "every_level": every(root.get(2)),
            "cap": root.get(5) or 0, "past_cap": every(root.get(6))}


def _values(prefabs: Prefabs, stem: str | None, bases: dict[int, int],
            members: dict[str, tuple[int, bool]]) -> dict[str, int]:
    pf = prefabs.get(stem) if stem else None
    root = _leaf(pf.root if pf else None)
    groups = [_leaf(g) for g in root.get(0) or []]
    default = root.get(1)
    listed: dict[str, float] = {}
    for g in groups:
        for row in g.get(1) or []:
            r = _leaf(row)
            if isinstance(r.get(0), int) and isinstance(r.get(1), str):
                listed.setdefault(_ref(r[0], r[1]), float(g.get(0) or 0))
    fallback = float(groups[default].get(0) or 0) if isinstance(default, int) and 0 <= default < len(groups) else 0.0
    out = {}
    for ref, (ctype, counted) in members.items():
        source = SOURCE.get(COLLECTION_TYPES[ctype])
        if source is None or source not in bases or not counted:
            continue
        value = round(bases[source] * listed.get(ref, fallback))
        if value:
            out[ref] = value
    return out


def _members(tree: GameTree, prefabs: Prefabs) -> dict[str, tuple[int, bool]]:
    """Every collectible: `ref -> (collection type, counts toward mastery)`."""
    lower = {t.lower(): i for i, t in enumerate(COLLECTION_TYPES)}
    out: dict[str, tuple[int, bool]] = {}
    for path in tree.files("prefabs/collections/", ".binfab"):
        stem = path.rsplit("/", 1)[-1].removesuffix(".binfab")
        ctype = lower.get(stem.removeprefix("collection_"))
        if not stem.startswith("collection_") or ctype is None or ctype == STYLE:
            continue
        try:
            root = parse(tree.read(path) or b"").root
        except WireError:
            continue
        for group in _leaf(root).get(0) or []:
            g = _leaf(group)
            for row in g.get(3) or []:
                ref = _leaf(row).get(0)
                if isinstance(ref, str):
                    ref = _ref(ctype, ref)
                    out[ref] = (ctype, out.get(ref, (ctype, False))[1] or g.get(2) != 0)
    return out


def build(tree: GameTree) -> dict:
    prefabs = Prefabs(tree)
    names: dict[str, str] = {}
    for path in tree.files("languages/en/", ".binfab"):
        for key, text in locale(tree.read(path)).items():
            names.setdefault(key, text)
    ladders = [x for x in (_ladder(slug, name, stem, tree, prefabs, names) for slug, name, stem, _ in LADDERS) if x]
    members = _members(tree, prefabs)
    values: dict[str, dict[str, int]] = {}
    for slug, _, _, mult in LADDERS:
        ladder = next((x for x in ladders if x["slug"] == slug), None)
        if not ladder or not mult:
            continue
        bases = {s["source"]: s["points"] for s in ladder["sources"]}
        for ref, v in _values(prefabs, mult, bases, members).items():
            values.setdefault(ref, {})[slug] = v
    return {"ladders": ladders, "source_names": {str(k): v for k, v in SOURCE_NAMES.items()},
            "collectibles": dict(sorted(values.items()))}


def count(data: dict) -> int:
    return sum(len(x["levels"]) for x in data["ladders"]) + len(data["collectibles"])
