"""What an ability does, read structurally from its prefab chain.

An ability prefab names the prefabs it spawns (projectile -> explosion -> effect ...),
and the numbers sit along that chain in components the client documents: energy
and cooldown on the action component, DamageParameters and HealingParameters on
whatever deals or restores, stat modifiers and a duration on each effect. The
field meanings live in ``fields``; this module only walks and collects.
"""
from __future__ import annotations

from collections.abc import Iterable, Iterator
from typing import Any

from app.trove.codexes.bonuses import _normalize, bonus_key
from app.trove.codexes.localize import resolve_stat_name
from app.trove.decode import fields as F
from app.trove.decode.tree import GameTree
from app.trove.decode.wire import Obj, Prefab, WireError, parse

WALK_LIMIT = 80


class Prefabs:
    """Parsed prefabs of a game tree, by logical path (``abilities/x/y``), cached."""

    def __init__(self, tree: GameTree):
        self.tree = tree
        self._cache: dict[str, Prefab | None] = {}
        self._vfx: dict[str, str] | None = None

    def get(self, rel: str) -> Prefab | None:
        rel = rel.removesuffix(".binfab")
        if rel not in self._cache:
            data = self.tree.read(f"prefabs/{rel}.binfab")
            try:
                self._cache[rel] = parse(data) if data else None
            except WireError:
                self._cache[rel] = None
        return self._cache[rel]

    def exists(self, rel: str) -> bool:
        return self.tree.exists(f"prefabs/{rel.removesuffix('.binfab')}.binfab")

    def vfx_table(self) -> dict[str, str]:
        """``$VFX_KEY -> Particles/....pkfx`` from ``prefabs/sfx/sfx.binfab``."""
        if self._vfx is None:
            self._vfx = {}
            pf = self.get("sfx/sfx")
            for rows in (pf.root.leaf.values() if pf and pf.root else ()):
                for row in rows if isinstance(rows, list) else ():
                    leaf = row.leaf if isinstance(row, Obj) else {}
                    key, path = leaf.get(0), leaf.get(1)
                    if isinstance(key, str) and isinstance(path, str) and path.lower().endswith(".pkfx"):
                        self._vfx.setdefault(key, path)
        return self._vfx


def walk_values(value: Any) -> Iterator[Any]:
    """Every node under a parsed value (dicts, lists, scalars), depth first."""
    yield value
    if isinstance(value, dict):
        for v in value.values():
            yield from walk_values(v)
    elif isinstance(value, (list, tuple)):
        for v in value:
            yield from walk_values(v)


def component_values(pf: Prefab) -> Iterator[Any]:
    for _, obj in pf.components:
        yield from walk_values(obj)
    if pf.root is not None:
        yield from walk_values(pf.root)


def refs(pf: Prefab, prefix: str = "abilities/") -> list[str]:
    """Prefab paths under ``prefix`` this prefab names, in order, deduped."""
    out: list[str] = []
    for v in component_values(pf):
        if isinstance(v, str) and v.startswith(prefix):
            rel = v.removesuffix(".binfab")
            if rel not in out:
                out.append(rel)
    return out


def identity(pf: Prefab | None) -> dict[str, str]:
    """``{name_key, description_key, icon}`` from the identity component."""
    obj = pf.component(F.IDENTITY) if pf else None
    if obj is None:
        return {}
    leaf = obj.leaf
    return {k: leaf[i] for k, i in (("name_key", F.ID_NAME), ("description_key", F.ID_DESCRIPTION),
                                    ("icon", F.ID_ICON))
            if isinstance(leaf.get(i), str) and leaf[i]}


def action(pf: Prefab | None) -> dict | None:
    """Energy cost and cooldown from the prefab's action component, when it has one."""
    for cid, obj in pf.components if pf else ():
        spec = F.ACTIONS.get(cid)
        if spec is None:
            continue
        energy_i, cooldown_i = spec
        out: dict[str, Any] = {"component": cid}
        if energy_i is not None:
            out["energy"] = _num(obj.get(energy_i, 0))
        if cooldown_i is not None:
            out["cooldown"] = _num(obj.get(cooldown_i, 0))
        return out
    return None


def _num(v: Any) -> float | int:
    if isinstance(v, bool) or not isinstance(v, (int, float)):
        return 0
    return round(v, 4) if isinstance(v, float) else v


def _labelled(leaf: dict, names: dict[int, str]) -> dict[str, Any]:
    out = {}
    for idx, name in names.items():
        v = leaf.get(idx)
        if isinstance(v, bool) or v in (None, "", 0, 0.0):
            continue
        if isinstance(v, (int, float, str)):
            out[name] = round(v, 4) if isinstance(v, float) else v
    return out


def _is_damage(v: Any) -> bool:
    """DamageParameters: the key set plus its field types (other classes share the keys)."""
    return (isinstance(v, dict) and F.DAMAGE_SIGNATURE <= v.keys()
            and all(type(v[i]) is int for i in F.DAMAGE_INTS if i in v)
            and all(type(v[i]) is float for i in F.DAMAGE_FLOATS if i in v))


def damage_records(pf: Prefab) -> list[dict]:
    """Labelled DamageParameters under a prefab that actually deal damage."""
    out = []
    for v in component_values(pf):
        if _is_damage(v):
            rec = _labelled(v, F.DAMAGE)
            if (rec.get("damage") or rec.get("multiplier") or rec.get("max_health_percent")) and rec not in out:
                out.append(rec)
    return out


def _is_healing(v: Any) -> bool:
    return (isinstance(v, dict) and F.HEALING_SIGNATURE <= v.keys() <= F.HEALING_KEYS
            and isinstance(v.get(5), str) and isinstance(v.get(6), str)
            and isinstance(v.get(1), float) and isinstance(v.get(3), float))


def healing_records(pf: Prefab) -> list[dict]:
    out = []
    for _, obj in pf.components:
        for v in walk_values(obj):
            if _is_healing(v):
                rec = _labelled(v, F.HEALING)
                if any(isinstance(rec.get(k), (int, float)) for k in ("health", "max_health_percent", "energy",
                                                                     "max_energy_percent")) and rec not in out:
                    out.append(rec)
    return out


def _is_modifier(v: Any) -> bool:
    return (isinstance(v, dict) and isinstance(v.get(F.MOD_LABEL), str) and isinstance(v.get(F.MOD_FLAGS), int)
            and isinstance(v.get(F.MOD_STAT, 0), int) and isinstance(v.get(F.MOD_OP, 0), int)
            and isinstance(v.get(F.MOD_VALUE), (int, float)) and not isinstance(v.get(F.MOD_VALUE), bool))


# Damage-taken and damage-dealt modifiers store an Add as a fraction (-0.5 = 50% less).
_FRACTION_STATS = frozenset({"$Stat_IncomingDamageMod", "$Stat_OutgoingDamageMod"})


def modifiers(values: Iterable[Any]) -> list[dict]:
    """Stat modifier records, in the units the site's other stat rows use."""
    out = []
    for v in values:
        if not _is_modifier(v):
            continue
        ordinal, op = v.get(F.MOD_STAT, 0), v.get(F.MOD_OP, 0)
        if not 0 <= op < len(F.MOD_OPS):
            continue
        key = F.stat_key(ordinal)
        amount = float(v[F.MOD_VALUE])
        shown, percent = _normalize(key, op * 2, amount)
        if op == 1 and key in _FRACTION_STATS and abs(amount) <= 1:
            shown, percent = amount * 100, True
        if op == 0:
            key = bonus_key(key)
        row = {"stat": key, "name": resolve_stat_name({}, key), "op": F.MOD_OPS[op],
               "value": round(shown, 4), "percent": percent, "amount": round(amount, 4)}
        flags = v.get(F.MOD_FLAGS, 0)
        modes = [n for bit, n in F.MOD_FLAG_NAMES.items() if flags & bit]
        # Only worth saying when it does NOT apply everywhere a player fights.
        if modes and not {"PVE", "PVP"} <= set(modes):
            row["modes"] = modes
        if row not in out:
            out.append(row)
    return out


def effects(pf: Prefab) -> list[dict]:
    """Each effect component: how long it lasts and the stats it changes."""
    out = []
    for cid, obj in pf.components:
        if cid not in F.EFFECTS or not obj:
            continue
        base = obj[0]
        dur = base.get(F.EFFECT_DURATION)
        row: dict[str, Any] = {"component": cid}
        for key, idx in (("name_key", F.EFFECT_NAME), ("description_key", F.EFFECT_DESCRIPTION)):
            if isinstance(base.get(idx), str) and base[idx].startswith("$"):
                row[key] = base[idx]
        if isinstance(dur, (int, float)) and not isinstance(dur, bool) and 0 < dur < 1e6:
            row["duration"] = round(float(dur), 4)
        stats = modifiers(walk_values(obj))
        if stats:
            row["stats"] = stats
        if row.keys() - {"component", "name_key", "description_key"}:
            out.append(row)
    return out


def proc_cooldown(pf: Prefab | None) -> float | None:
    """A combat-event proc's own cooldown, in seconds."""
    obj = pf.component(F.PROC_SPAWNER) if pf else None
    if obj is None or len(obj) <= F.PROC_SECTION:
        return None
    v = obj[F.PROC_SECTION].get(F.PROC_COOLDOWN)
    return round(float(v), 4) if isinstance(v, (int, float)) and not isinstance(v, bool) and v > 0 else None


def visuals(pf: Prefab) -> dict[str, list[str]]:
    vfx: list[str] = []
    sfx: list[str] = []
    for v in component_values(pf):
        if isinstance(v, str):
            if v.startswith("$VFX_") and v not in vfx:
                vfx.append(v)
            elif v.startswith("$SFX_") and v not in sfx:
                sfx.append(v)
    return {"vfx": vfx, "sfx": sfx}


def walk(prefabs: Prefabs, root: str, stop: Iterable[str] = (), limit: int = WALK_LIMIT,
         prefix: str = "abilities/") -> list[tuple[str, Prefab, int]]:
    """``root`` and every ability prefab it reaches, breadth first, with depth.

    ``stop`` holds prefabs that belong to something else (another named ability of
    the same class): the walk does not enter them, so one ability cannot report
    another's numbers.
    """
    stop = set(stop) - {root}
    out: list[tuple[str, Prefab, int]] = []
    seen = {root}
    queue = [(root, 0)]
    while queue and len(out) < limit:
        rel, depth = queue.pop(0)
        pf = prefabs.get(rel)
        if pf is None:
            continue
        out.append((rel, pf, depth))
        for child in refs(pf, prefix):
            if child not in seen and child not in stop:
                seen.add(child)
                queue.append((child, depth + 1))
    return out


def stage_name(rel: str) -> str:
    """A readable label from the prefab stem - the game ships no stage names."""
    return rel.rsplit("/", 1)[-1].replace("_", " ").title()


def describe(prefabs: Prefabs, root: str, stop: Iterable[str] = (), prefix: str = "abilities/") -> dict:
    """Everything the chain under ``root`` does. Stages and effects carry the
    prefab they came from and a ``_depth`` for callers that de-duplicate across
    abilities sharing sub-prefabs."""
    nodes = walk(prefabs, root, stop, prefix=prefix)
    out: dict[str, Any] = {"prefab": root}
    act = action(nodes[0][1]) if nodes else None
    if act:
        out.update({k: v for k, v in act.items() if k != "component"})
    proc = proc_cooldown(nodes[0][1]) if nodes else None
    if proc is not None:
        out["proc_cooldown"] = proc
    stages, heals, effs, vfx, sfx, touched, actions = [], [], [], [], [], [], []
    for rel, pf, depth in nodes:
        found = False
        act = action(pf)
        if act and depth:
            actions.append({"name": stage_name(rel), "prefab": rel, "_depth": depth,
                            **{k: v for k, v in act.items() if k != "component"}})
        for rec in damage_records(pf):
            stages.append({"name": stage_name(rel), "prefab": rel, "base": rec.get("damage", 0),
                           "multiplier": rec.get("multiplier", 0), "_depth": depth,
                           **{k: v for k, v in rec.items() if k not in ("damage", "multiplier")}})
            found = True
        for rec in healing_records(pf):
            heals.append({"name": stage_name(rel), "prefab": rel, "_depth": depth, **rec})
            found = True
        for eff in effects(pf):
            effs.append({"name": stage_name(rel), "prefab": rel, "_depth": depth, **eff})
            found = True
        vis = visuals(pf)
        vfx += [v for v in vis["vfx"] if v not in vfx]
        sfx += [v for v in vis["sfx"] if v not in sfx]
        if found:
            touched.append(rel)
    table = prefabs.vfx_table()
    out.update({
        "stages": stages,
        "healing": heals,
        "effects": effs,
        "actions": actions,
        "vfx": [{"key": k, "pkfx": table[k]} for k in vfx if k in table],
        "sfx": sfx,
        "prefabs": touched,
    })
    return out
