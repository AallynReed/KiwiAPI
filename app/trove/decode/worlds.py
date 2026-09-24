"""worlds.json - the worlds players travel to, the biomes the game names, Delve
gateways and the Shadow Tower.

The client holds no world generator and no dungeon definitions: which biomes a
world is made of, which dungeons spawn where, their bosses, blueprints and loot all
come from the server (`server_data/...`; the files only ever name its ids). What
the client does carry, read structurally:

- Portals (`placeable/portal/*_interactive`, component 154; leaf fields): 0 the
  portal type, 1 the world's difficulty rank, 3 its scenario ids, 22/23 name and
  description keys. Type labels are the `$WorldPortalType_*` keys Trove_x64.exe
  picks per value (FUN_1407d0420: 1 and 15 Adventure, 12 Arena = Shadow Tower,
  14 Adventure2, 22 Discovery; every file value lines up with its portal's name).
  Rank = the "(10)" in "Moonless Dark Difficulty (10)" on every ranked portal, and
  the rank its ZoneRankPowerRank requirement (component 239) gates on:
  `{0 rank, 1 use the current world's rank}` ("Min{0}" / "MinByWorld" in the exe).
  Scenario ids are what `scenario_channels` and the Shadow Tower floor sets name.
  The placeable item that puts a portal down names it in component 50.
- Difficulty names: rank 0-3 `$ZoneLevel_Novice/Adept/Elite/Master`, rank n >= 4
  `$ZoneLevel_Uber_<n-3>` (ui.binfab; the Switch table in platform_nx says
  "Uber-2" where every PC portal text says "Shade (5)").
- Biome challenges (`challenge/challenge`, root 0): {0 id, 1 levels {0 name key,
  1 description key}, 2 objective {0 kind, 1 params}}. The "Complete dungeons in
  the <biome>" challenges are the `quest` objectives whose params list `biome/*`
  ids - the world-gen biomes the game counts as that biome (its sub-zones).
- Zone tags: adventures (`meta/activities`, `meta/goldenthread`) whose objective
  only counts inside zones carrying a tag (`zonetag` requirement) - the tag a
  biome's zones carry, with the adventures that use it.
- Tiny Quest biome synergies (`meta/tinyquests` root 11): tag -> name key, icon.
- Delve gateways (`placeable/matchmaker/**`, component 493): 0 the depth the
  delve starts at ("Stable Gateway: Depth 20" ... "Depth 136", "Min Depth: 23";
  -1 = none), 1 rows `{0 id, 1 kind}` the server builds the delve from. Kind 1 is
  a Delve biome (`amperium` = "...in the AMPERIA biome"), kind 7 a boss
  (`delve_boss_*` = "...leads to the Boss: Spike Walker!"); every gateway's text
  agrees. Other kinds (3, 6, 13) and fields are unproven and left out.
- Shadow Tower (`tower/floor_sets`, TowerFloorSetData): root 0 maps floor index
  -> {0 scenario ids, 1 boss {0 quest id, 1 name key}, 2 one row per
  KTowerDifficulty (Normal, Hard, Ultra)}. Row fields from the exe getters that
  feed shadowtower.swf `setFloor`: 0 the Flux cost ("It costs {0} Flux"), 4 the
  rank whose power rank gates loot ("You must be Power Rank {0} or higher"),
  5 leaderboard icon, 6 portrait. Field 2 (a weekly reward list) is empty on
  every floor, and 1 and 3 are unproven.

Not joined, because no file joins them: challenge biomes, zone tags, synergy
biomes and portal worlds use different ids and slightly different names
("Candoria" / "Candorian", "Fae Wilds" / "Fae Forest"); a Delve boss or Shadow
Tower boss id to an NPC prefab; lighting/cloud/fx sets to a biome (only Sky
Projector items name them).
"""
from __future__ import annotations

from typing import Any

from app.trove.codexes.localize import resolve_stat_name
from app.trove.decode.common import locale, stem
from app.trove.decode.fields import stat_key
from app.trove.decode.tree import GameTree
from app.trove.decode.wire import Obj, WireError, parse, strings

TITLE = "Worlds, biomes and dungeons"
OUTPUT = "worlds.json"
PREFIXES = ("prefabs/placeable/portal/", "prefabs/placeable/matchmaker/", "prefabs/tower/",
            "prefabs/challenge/", "prefabs/meta/", "blueprints/", "languages/en/")
INDENT, FINAL_NEWLINE = None, True

PORTALS, MATCHMAKERS = "prefabs/placeable/portal/", "prefabs/placeable/matchmaker/"
IDENTITY, BLUEPRINT, PLACER, PORTAL, REQUIREMENTS, DELVE = 49, 37, 50, 154, 239, 493
ID_NAME, ID_DESCRIPTION = 1, 5
P_TYPE, P_RANK, P_SCENARIOS, P_NAME, P_DESCRIPTION = 0, 1, 3, 22, 23
PORTAL_TYPES = {1: "$WorldPortalType_Adventure", 15: "$WorldPortalType_Adventure",
                12: "$WorldPortalType_Arena", 14: "$WorldPortalType_Adventure2",
                22: "$WorldPortalType_Discovery"}
TYPE_ORDER = (1, 15, 14, 22, 12)
DELVE_DEPTH, DELVE_ROWS = 0, 1
DELVE_KINDS = {1: "delve_biomes", 7: "delve_bosses"}
ZONE_LEVELS = ("$ZoneLevel_Novice", "$ZoneLevel_Adept", "$ZoneLevel_Elite", "$ZoneLevel_Master")
TOWER_DIFFICULTIES = ("$ShadowTower_DifficultyNormal", "$ShadowTower_DifficultyHard",
                      "$ShadowTower_DifficultyUltra")
FS_SCENARIOS, FS_BOSS, FS_LEVELS = 0, 1, 2
LV_FLUX, LV_RANK, LV_ICON, LV_PORTRAIT = 0, 4, 5, 6
SYNERGIES, SYNERGY_PREFIX = 11, "TQ_Tag_Biome_"


def _leaf(v: Any) -> dict:
    return v.leaf if isinstance(v, Obj) else {}


def _list(v: Any) -> list:
    if isinstance(v, dict):
        return [v[k] for k in sorted(v, key=str)]
    return list(v) if isinstance(v, (list, tuple)) else []


def _strs(v: Any) -> list[str]:
    return [s for s in _list(v) if isinstance(s, str) and s]


def _rel(path: str) -> str:
    return path.removeprefix("prefabs/").removesuffix(".binfab")


def _parse(tree: GameTree, path: str):
    try:
        return parse(tree.read(path) or b"")
    except WireError:
        return None


class _Text:
    """`$key` -> English; the Switch overrides (platform_nx) lose to the PC tables."""

    def __init__(self, tree: GameTree):
        self.table: dict[str, str] = {}
        paths = tree.files("languages/en/", ".binfab")
        for path in sorted(paths, key=lambda p: (stem(p).startswith("platform_"), p)):
            for k, v in locale(tree.read(path)).items():
                self.table.setdefault(k, v)

    def __call__(self, value: Any) -> str:
        if not isinstance(value, str) or not value:
            return ""
        return self.table.get(value, "") if value.startswith("$") else ""


class _Blueprints:
    def __init__(self, tree: GameTree):
        self.paths = {p.removeprefix("blueprints/").lower(): p.removeprefix("blueprints/")
                      for p in tree.files("blueprints/", ".blueprint")}

    def __call__(self, raw: Any) -> str:
        if not isinstance(raw, str) or not raw:
            return ""
        name = raw.replace("\\", "/").lower()
        return self.paths.get(name) or self.paths.get(name + ".blueprint") or ""


def _difficulty_names(text: _Text) -> list[str]:
    names = [text(k) for k in ZONE_LEVELS]
    while text(f"$ZoneLevel_Uber_{len(names) - 3}"):
        names.append(text(f"$ZoneLevel_Uber_{len(names) - 3}"))
    return names


def _requirements(pf, text: _Text) -> list[dict]:
    out: list[dict] = []
    for row in _list(_leaf(pf.component(REQUIREMENTS)).get(0)):
        kind, params = _leaf(row).get(0), _leaf(row).get(1)
        own = params[-1] if isinstance(params, Obj) and len(params) > 1 else {}
        k = kind.lower() if isinstance(kind, str) else ""
        if k == "zonerankpowerrank":
            out.append({"kind": "power_rank_of_world"} if own.get(1) else
                       {"kind": "power_rank_of_rank", "rank": own.get(0, 0)})
        elif k == "tags":
            out.append({"kind": "tags", "tags": _strs(own.get(0))})
        elif k == "stat" and isinstance(own.get(0), int):
            key = stat_key(own[0])
            out.append({"kind": "stat", "stat": resolve_stat_name(text.table, key),
                        "at_least": round(float(own.get(1, 0)), 4)})
        elif k:
            out.append({"kind": k})
    return out


def _worlds(tree: GameTree, text: _Text, blueprints: _Blueprints, levels: list[str]) -> list[dict]:
    items: dict[str, tuple[str, Any]] = {}
    for path in tree.files(PORTALS, ".binfab"):
        if path.endswith("_interactive.binfab"):
            continue
        pf = _parse(tree, path)
        placer = pf.component(PLACER) if pf is not None else None
        for ref in strings(placer) if placer is not None else []:
            if ref.endswith("_interactive"):
                items.setdefault(ref.lower(), (_rel(path), pf))
    worlds = []
    for path in tree.files(PORTALS, "_interactive.binfab"):
        pf = _parse(tree, path)
        if pf is None:
            continue
        portal = _leaf(pf.component(PORTAL))
        kind = portal.get(P_TYPE)
        name = text(portal.get(P_NAME))
        if kind not in PORTAL_TYPES or not name:
            continue
        rel = _rel(path)
        item_rel, item = items.get(rel.lower(), ("", None))
        entry: dict[str, Any] = {"slug": stem(rel).removesuffix("_interactive"), "name": name}
        if desc := text(portal.get(P_DESCRIPTION)):
            entry["description"] = desc
        entry["type"] = text(PORTAL_TYPES[kind])
        reqs = _requirements(pf, text)
        rank = portal.get(P_RANK, 0)
        if isinstance(rank, int) and (rank > 0 or any(r.get("rank") == rank for r in reqs)):
            entry["rank"] = rank
            if 0 <= rank < len(levels):
                entry["difficulty"] = levels[rank]
        if scenarios := _strs(portal.get(P_SCENARIOS)):
            entry["scenarios"] = scenarios
        if reqs:
            entry["requires"] = reqs
        entry["prefab"] = item_rel or rel
        if item is not None:
            ident = _leaf(item.component(IDENTITY))
            if (label := text(ident.get(ID_NAME))) and label != name:
                entry["item_name"] = label
        bp = blueprints(_leaf(item.component(BLUEPRINT)).get(0)) if item is not None else ""
        if bp := bp or blueprints(_leaf(pf.component(BLUEPRINT)).get(0)):
            entry["blueprint"] = bp
        worlds.append((TYPE_ORDER.index(kind), entry))
    worlds.sort(key=lambda w: (w[0], w[1].get("rank", -1), w[1]["slug"]))
    return [w for _, w in worlds]


def _objective_rows(v: Any, out: list[dict]) -> None:
    """Every PersonalObjective (`{0 id, 1 name key, 3 objective}`) under ``v``."""
    if isinstance(v, Obj):
        leaf = v.leaf
        if (isinstance(leaf.get(0), str) and isinstance(leaf.get(1), str) and leaf[1].startswith("$")
                and isinstance(leaf.get(3), Obj)):
            out.append(leaf)
        for sec in v:
            for x in sec.values():
                _objective_rows(x, out)
    elif isinstance(v, (list, tuple, dict)):
        for x in _list(v):
            _objective_rows(x, out)


def _zone_tags(node: Any, found: list[str]) -> None:
    if isinstance(node, Obj):
        leaf = node.leaf
        kind, params = leaf.get(0), leaf.get(1)
        if isinstance(kind, str) and kind.lower() == "zonetag" and isinstance(params, Obj) and len(params) > 1:
            found.extend(t for t in _strs(params[-1].get(0)) if t not in found)
        for sec in node:
            for x in sec.values():
                _zone_tags(x, found)
    elif isinstance(node, (list, tuple, dict)):
        for x in _list(node):
            _zone_tags(x, found)


def _tagged_adventures(tree: GameTree, text: _Text) -> list[dict]:
    tags: dict[str, dict[str, dict]] = {}
    paths = tree.files("prefabs/meta/activities/", ".binfab") + tree.files("prefabs/meta/goldenthread/", ".binfab")
    for path in paths:
        pf = _parse(tree, path)
        if pf is None or pf.root is None:
            continue
        rows: list[dict] = []
        _objective_rows(pf.root, rows)
        for row in rows:
            found: list[str] = []
            _zone_tags(row[3], found)
            for tag in found:
                adventure = {"id": row[0], "name": text(row[1])} if text(row[1]) else {"id": row[0]}
                tags.setdefault(tag, {}).setdefault(row[0], adventure)
    return [{"tag": t, "adventures": list(a.values())} for t, a in sorted(tags.items(), key=lambda x: x[0].lower())]


def _biomes(tree: GameTree, text: _Text) -> list[dict]:
    pf = _parse(tree, "prefabs/challenge/challenge.binfab")
    out = []
    for row in _list(_leaf(pf.root if pf is not None else None).get(0)):
        leaf = _leaf(row)
        objective = _leaf(leaf.get(2))
        params = objective.get(1)
        own = params[-1] if isinstance(params, Obj) and len(params) > 1 else {}
        zones = [z for z in _strs(own.get(0)) if z.startswith("biome/")]
        if objective.get(0) != "quest" or not zones:
            continue
        level = next((_leaf(v) for v in _list(leaf.get(1)) if isinstance(v, Obj)), {})
        entry: dict[str, Any] = {"slug": str(leaf.get(0, "")).removesuffix("_quest"), "name": text(level.get(0))}
        if desc := text(level.get(1)):
            entry["description"] = desc
        entry["challenge"] = leaf.get(0)
        entry["zones"] = [z.removeprefix("biome/") for z in zones]
        out.append(entry)
    return out


def _synergies(tree: GameTree, text: _Text) -> list[dict]:
    pf = _parse(tree, "prefabs/meta/tinyquests.binfab")
    holder = _leaf(pf.root if pf is not None else None).get(SYNERGIES)
    table = holder[0].get(0) if isinstance(holder, Obj) and holder else None
    out = []
    for tag, row in sorted(table.items()) if isinstance(table, dict) else []:
        leaf = _leaf(row)
        if not (isinstance(tag, str) and tag.startswith(SYNERGY_PREFIX)) or not text(leaf.get(0)):
            continue
        entry = {"tag": tag, "name": text(leaf.get(0))}
        if isinstance(leaf.get(1), str) and leaf[1]:
            entry["icon"] = leaf[1]
        out.append(entry)
    return out


def _delves(tree: GameTree, text: _Text, blueprints: _Blueprints) -> list[dict]:
    out = []
    for path in tree.files(MATCHMAKERS, ".binfab"):
        pf = _parse(tree, path)
        delve = pf.component(DELVE) if pf is not None else None
        if pf is None or delve is None or not (name := text(_leaf(pf.component(IDENTITY)).get(ID_NAME))):
            continue
        rel = _rel(path)
        sub = rel.removeprefix(_rel(MATCHMAKERS))
        entry: dict[str, Any] = {"slug": sub.replace("/", "_"), "name": name}
        if desc := text(_leaf(pf.component(IDENTITY)).get(ID_DESCRIPTION)):
            entry["description"] = desc
        entry["group"] = sub.rsplit("/", 1)[0] if "/" in sub else ""
        depth = delve.get(DELVE_DEPTH)
        if isinstance(depth, int) and depth >= 0:
            entry["depth"] = depth
        for row in (_leaf(r) for r in _list(delve.get(DELVE_ROWS))):
            key = DELVE_KINDS.get(row.get(1, 0))
            if key and isinstance(row.get(0), str) and row[0]:
                entry.setdefault(key, []).append(row[0])
        entry["prefab"] = rel
        if bp := blueprints(_leaf(pf.component(BLUEPRINT)).get(0)):
            entry["blueprint"] = bp
        out.append(entry)
    out.sort(key=lambda e: (e["group"], e.get("depth", -1), e["name"].lower(), e["slug"]))
    return out


def _tower(tree: GameTree, text: _Text, levels: list[str]) -> list[dict]:
    pf = _parse(tree, "prefabs/tower/floor_sets.binfab")
    sets = _leaf(pf.root if pf is not None else None).get(0)
    out = []
    for floor, row in sorted(sets.items()) if isinstance(sets, dict) else []:
        leaf = _leaf(row)
        boss = next((_leaf(b) for b in _list(leaf.get(FS_BOSS)) if isinstance(b, Obj)), {})
        quest = boss.get(0) if isinstance(boss.get(0), str) else ""
        entry: dict[str, Any] = {"slug": quest, "name": text(boss.get(1)) or quest, "floor": floor + 1,
                                 "scenarios": _strs(leaf.get(FS_SCENARIOS)), "difficulties": []}
        for i, lv in enumerate(r for r in _list(leaf.get(FS_LEVELS)) if isinstance(r, Obj)):
            d = lv.leaf
            row_out: dict[str, Any] = {"difficulty": text(TOWER_DIFFICULTIES[i]).title() if i < 3 else str(i)}
            if isinstance(d.get(LV_FLUX), int):
                row_out["flux"] = d[LV_FLUX]
            if isinstance(d.get(LV_RANK), int):
                row_out["loot_rank"] = d[LV_RANK]
                if 0 <= d[LV_RANK] < len(levels):
                    row_out["loot_difficulty"] = levels[d[LV_RANK]]
            for key, field in (("leaderboard_icon", LV_ICON), ("portrait", LV_PORTRAIT)):
                if isinstance(d.get(field), str) and d[field]:
                    row_out[key] = d[field]
            entry["difficulties"].append(row_out)
        if quest:
            out.append(entry)
    return out


def build(tree: GameTree) -> dict:
    text = _Text(tree)
    blueprints = _Blueprints(tree)
    levels = _difficulty_names(text)
    return {
        "difficulties": [{"rank": i, "name": n} for i, n in enumerate(levels)],
        "worlds": _worlds(tree, text, blueprints, levels),
        "biomes": _biomes(tree, text),
        "zone_tags": _tagged_adventures(tree, text),
        "tinyquest_biomes": _synergies(tree, text),
        "delve_gateways": _delves(tree, text, blueprints),
        "shadow_tower": _tower(tree, text, levels),
    }


def count(data: dict) -> int:
    return len(data["worlds"]) + len(data["biomes"]) + len(data["delve_gateways"]) + len(data["shadow_tower"])
