"""subclass_abilities.json - what each class grants when it is equipped as a subclass,
tier by tier.

A class prefab names its subclass ability (`abilities/subclass/<folder>/<folder>`;
its root also maps subclass levels 1/10/15/20/25/30 to the flat stat bonuses that
classes.json already carries). That prefab's identity holds the name, description
and icon; its chain fans out through effect groups (component 46) into tier
prefabs, each gated by a requirement (component 239, `{0 type, 1 params}`):

- `subclasspowerrank` / `subclasslevel`, params `{0 min, 1 max, 2 mode}`. These are
  the same requirement template as `classlevel` / `powerrank` (identical ctors in
  Trove_x64.exe). The evaluators (FUN_14085a250 / FUN_14085a1a0) read the Power
  Rank the client computes for the subclass's class (FUN_140815530 with the
  subclass id) and that class's level, then test mode 0 `min <= v`, 1 `v <= max`,
  2 `min <= v <= max`. The text builder names stat 0x19, "Power Rank".
  So a tier's threshold is its min (0 for mode 1) and `until` its max (none for
  mode 0). Every tier ladder is Power Rank 0/5000/.../40000 except Fae
  Trickster's wing-speed ladder, which is subclass level 0/10/15/20/25/30.

Only prefabs the root actually reaches count: several classes ship tier files
nothing points at (Bard Peaceful Song 8-9, Vanguardian shockwave 8-9, Boomeranger
heal 8-9, Chloromancer spawn 8), and Dracolyte's chooser names tier 8-9 files the
client does not ship (`missing`). Each emitted tier lists every part whose own
window contains the threshold, so a ladder that stops at 34,999 shows as absent
from 35,000 on. Two siblings with different but intersecting windows (Fae
Trickster tier 7 is open-ended from 30,000 under tiers 8-9; Lunar Lancer tier 9
starts at 30,000) are listed under `overlapping`: both requirements pass, but
whether the effect group applies both is not established.

Per part, ``ability.describe`` gives damage (`multiplier` = share of the damage
stat), healing, effect durations and stat changes. Labelled on top of that:

- combat-event spawner (150), section 2: 4 cooldown, 5 chance (the client fires
  when a uniform [0,1] roll is <= it, FUN_1406561d0, so >= 1 is every time), 7
  health fraction for the health events, 16 required damage tag (Solarion
  `basicattack`), 26 damage-type filter with 27 = magic (Dracolyte "Physical
  damage", Shadow Hunter "Magic attacks"), 30 damage tags that never trigger it
  (each class's own proc damage tag is in it). Event flags, each tied to game
  text: 0 damage taken, 1 damage dealt, 3 own health below, 8 damage received,
  9 nearby enemy death, 13 critical hit, 17 enemy defeated, 28 target health
  below, 35 health drops below. Unlabelled flags are left out.
- life leech (281) field 3: "Damaging enemies grants 10% Life Leech" is 0.1.
- damage over time (156) section 1 field 5: seconds between ticks.
- reflect (336) field 2 multiplier (50 = "5000% reflect") and 5 excluded tags.
- shield (131) section 2 field 3: hits above this share of max health are
  absorbed (Glacial Ward "greater than 50% of your HP", 0.5).
- weighted pick (275) rows `{0 effect, 1 weight}`; NPCs a part spawns by path.
"""
from __future__ import annotations

from typing import Any

from app.trove.decode import fields as F
from app.trove.decode.ability import (
    Prefabs,
    describe,
    identity,
    is_modifier,
    refs,
    walk,
    walk_values,
)
from app.trove.decode.common import locale, stem
from app.trove.decode.tree import GameTree
from app.trove.decode.wire import Obj, strings

TITLE = "Subclass abilities"
OUTPUT = "subclass_abilities.json"
PREFIXES = ("prefabs/class/", "prefabs/abilities/subclass/", "languages/en/")
INDENT, FINAL_NEWLINE = 4, False

ROOT = "abilities/subclass/"
REQUIREMENTS = 239
KINDS = {"subclasspowerrank": "subclass_power_rank", "subclasslevel": "subclass_level"}
REQ_MIN, REQ_MAX, REQ_MODE = 0, 1, 2
AT_LEAST, AT_MOST, BETWEEN = 0, 1, 2

PROC_CHANCE, PROC_HEALTH, PROC_TAG, PROC_TYPED, PROC_MAGIC, PROC_IGNORES = 5, 7, 16, 26, 27, 30
EVENTS = {0: "damage_taken", 1: "damage_dealt", 3: "health_below", 8: "damage_received",
          9: "nearby_enemy_death", 13: "critical_hit", 17: "enemy_defeated", 26: "damage_dealt",
          28: "target_health_below", 35: "health_drops_below"}
HEALTH_EVENTS = {3, 28, 35}
LEECH, DOT, REFLECT, SHIELD, WEIGHTED = 281, 156, 336, 131, 275
NULLIFY = F.MOD_OPS.index("Nullify")


def _num(v: Any) -> float | int | None:
    if isinstance(v, bool) or not isinstance(v, (int, float)):
        return None
    return round(v, 4) if isinstance(v, float) else v


def _leaf(v: Any) -> dict:
    return v.leaf if isinstance(v, Obj) else {}


def _window(pf) -> tuple[str, int, int | None] | None:
    """(kind, threshold, until) of a tier prefab's subclass requirement."""
    obj = pf.component(REQUIREMENTS) if pf else None
    for row in _leaf(obj).get(0) or []:
        kind = KINDS.get(str(_leaf(row).get(0)))
        params = _leaf(row).get(1)
        if kind is None or not isinstance(params, Obj):
            continue
        own = params.leaf
        mode, low, high = own.get(REQ_MODE, 0), own.get(REQ_MIN, 0), own.get(REQ_MAX, 0)
        return kind, (0 if mode == AT_MOST else low), (high if mode in (AT_MOST, BETWEEN) else None)
    return None


def _trigger(rel: str, pf) -> dict | None:
    obj = pf.component(F.PROC_SPAWNER)
    if obj is None or len(obj) <= F.PROC_SECTION:
        return None
    s = obj[F.PROC_SECTION]
    out: dict[str, Any] = {"prefab": rel, "on": list(dict.fromkeys(n for i, n in EVENTS.items() if s.get(i)))}
    for key, i in (("chance", PROC_CHANCE), ("cooldown", F.PROC_COOLDOWN)):
        if _num(s.get(i)) is not None:
            out[key] = _num(s[i])
    if HEALTH_EVENTS & {i for i in EVENTS if s.get(i)} and _num(s.get(PROC_HEALTH)):
        out["health"] = _num(s[PROC_HEALTH])
    if s.get(PROC_TYPED):
        out["damage_type"] = "magic" if s.get(PROC_MAGIC) else "physical"
    if isinstance(s.get(PROC_TAG), str) and s[PROC_TAG]:
        out["requires_tag"] = s[PROC_TAG]
    ignores = [t for t in s.get(PROC_IGNORES) or [] if isinstance(t, str)]
    if ignores:
        out["ignores_tags"] = ignores
    return out


def _extras(prefabs: Prefabs, row: dict) -> None:
    """Labelled fields of the few effect components whose meaning is established."""
    pf = prefabs.get(row["prefab"])
    obj = pf.component(row["component"]) if pf else None
    if obj is None:
        return
    cid, leaf = row["component"], obj.leaf
    if cid == LEECH and _num(leaf.get(3)):
        row["life_leech"] = _num(leaf[3])
    elif cid == DOT and len(obj) > 1 and _num(obj[1].get(5)):
        row["tick"] = _num(obj[1][5])
    elif cid == REFLECT and _num(leaf.get(2)):
        row["reflect"] = _num(leaf[2])
        tags = [t for t in leaf.get(5) or [] if isinstance(t, str)]
        if tags:
            row["reflect_ignores_tags"] = tags
    elif cid == SHIELD and len(obj) > 2 and (_num(obj[2].get(3)) or 0) > 0:
        row["absorbs_over_max_health"] = _num(obj[2][3])
    vetoed = [v[F.MOD_LABEL] for v in walk_values(obj) if is_modifier(v) and v.get(F.MOD_OP) == NULLIFY]
    if vetoed:
        row["nullifies"] = vetoed


def _detail(prefabs: Prefabs, rel: str, stop: set[str], prefix: str) -> dict:
    info = describe(prefabs, rel, stop, prefix=prefix)
    out: dict[str, Any] = {}
    for key in ("stages", "healing", "effects"):
        rows = [{k: v for k, v in r.items() if k not in ("_depth", "name_key", "description_key")}
                for r in info[key]]
        if key == "effects":
            for row in rows:
                _extras(prefabs, row)
            rows = [r for r in rows if r.keys() - {"name", "prefab", "component"}]
        if rows:
            out[key] = rows
    triggers, choices, summons = [], [], []
    for node, pf, _ in walk(prefabs, rel, stop, limit=400, prefix=prefix):
        trig = _trigger(node, pf)
        if trig:
            triggers.append(trig)
        for row in _leaf(pf.component(WEIGHTED)).get(0) or []:
            ref, weight = _leaf(row).get(0), _num(_leaf(row).get(1))
            if isinstance(ref, str) and weight is not None:
                choices.append({"prefab": ref.removesuffix(".binfab"), "weight": weight})
        for _, obj in pf.components:
            summons += [s.removesuffix(".binfab") for s in strings(obj)
                        if s.startswith("npc/") and s.removesuffix(".binfab") not in summons]
    if triggers:
        out["triggers"] = triggers
    if choices:
        out["choices"] = choices
    if summons:
        out["summons"] = summons
    return out


def _subclass(prefabs: Prefabs, text: dict[str, str], root: str) -> dict:
    prefix = root.rsplit("/", 1)[0] + "/"
    nodes = walk(prefabs, root, limit=400, prefix=prefix)
    parent: dict[str, str] = {}
    missing: list[str] = []
    for rel, pf, _ in nodes:
        for child in refs(pf, prefix):
            parent.setdefault(child, rel)
            if not prefabs.exists(child) and child not in missing:
                missing.append(child)
    windows = {rel: w for rel, pf, _ in nodes if (w := _window(pf))}
    tiered = set(windows)

    ident = identity(prefabs.get(root))
    entry: dict[str, Any] = {
        "name": text.get(ident.get("name_key", ""), ""),
        "description": text.get(ident.get("description_key", ""), ""),
        "icon": ident.get("icon", ""),
        "prefab": root,
    }
    base = _detail(prefabs, root, tiered, prefix)
    if base:
        entry["base"] = base

    parts = {rel: {"name": stem(rel).replace("_", " ").title(), "prefab": rel,
                   **_detail(prefabs, rel, tiered - {rel}, prefix)} for rel in windows}
    points = sorted({(kind, t) for kind, t, _ in windows.values()}
                    | {(kind, u + 1) for kind, _, u in windows.values() if u is not None},
                    key=lambda p: (p[0] != "subclass_power_rank", p[1]))
    tiers = []
    for kind, t in points:
        active = [rel for rel, (k, low, high) in windows.items()
                  if k == kind and low <= t and (high is None or t <= high)]
        tier: dict[str, Any] = {"threshold": t, "threshold_kind": kind, "parts": []}
        for rel in active:
            until = windows[rel][2]
            tier["parts"].append({**parts[rel], **({"until": until} if until is not None else {})})
        # Siblings sharing a window are one tier's pieces (Solarion's aoe + heal).
        overlapping = [[a, b] for i, a in enumerate(active) for b in active[i + 1:]
                       if parent.get(a) == parent.get(b) and windows[a] != windows[b]]
        if overlapping:
            tier["overlapping"] = overlapping
        tiers.append(tier)
    entry["tiers"] = tiers
    if missing:
        entry["missing"] = missing
    return entry


def build(tree: GameTree) -> list[dict]:
    prefabs = Prefabs(tree)
    display: dict[str, str] = {}
    for name in ("prefabs_class.binfab", "ui.binfab", "new.binfab"):
        display.update(locale(tree.read(f"languages/en/{name}")))
    text: dict[str, str] = {}
    for path in tree.files("languages/en/prefabs_abilities", ".binfab"):
        text.update(locale(tree.read(path)))

    out = []
    for class_path in tree.files("prefabs/class/", ".binfab"):
        folder = stem(class_path)
        if class_path.count("/") != 2 or folder.endswith("_ultimate"):
            continue
        cls = prefabs.get(f"class/{folder}")
        texts = strings(cls.root) if cls and cls.root is not None else []
        key = next((s for s in texts if s.startswith("$DisplayName")), None)
        root = next((s.removesuffix(".binfab") for s in texts if s.startswith(ROOT)), None)
        if not key or not root or prefabs.get(root) is None:
            continue
        out.append({"class": display.get(key, key.replace("$DisplayName_", "")), "game_folder": folder,
                    **_subclass(prefabs, text, root)})
    return sorted(out, key=lambda e: e["class"])


def count(data: list) -> int:
    return len(data)
