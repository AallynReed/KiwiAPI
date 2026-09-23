"""badges.json - every badge group, its ranks, what each rank asks and what it pays.

`prefabs/meta/badges.binfab` is one object: field 0 is the reward table
(`{0 id, 1 $name, 2 claim id}`), field 1 the badge groups (`{0 group id, 1 ranks}`).
A rank is `{0 BadgeRank ordinal, 2 reward ids, 3 requirement}`; the requirement is
`{0 completion kind, 1 payload}` with the payload's own fields on its leaf section.
A reward's claim id keys `prefabs/claim/badge.binfab`, whose claims say what is
granted: a collectible, `count` x an item prefab, points (cubits), or a compound.

Each badge collectible (`prefabs/collections/badge/*`) names its group and rank in
component 299 (`{0 group id, 1 rank, 2 stat bonuses for owning it}`); that, not
the file name, links a rank to its collectible. Group ids differ in case between
files (`AllGemStats_badge` / `allgemstats_badge`), so joins ignore case. The
category is the `collection_badge.binfab` entry that lists the collectible.

Payloads by kind, each label tied to game text or the exe:
- metric `{0 PlayerMetric ordinal, 1 amount}`. The ordinal's name is compiled into
  Trove_x64.exe only (read by `codexes.badges.parse_metric_names`); its label is
  `$Metrics_<name>`. No exe in the tree -> the ordinal and amount, unnamed.
- dragonsouls `{0 dragon id, 1 amount}`: the id is field 0 of
  `prefabs/dragonsoul/<dragon>.binfab` and of a soul item's consume component 301.
- STBossKilled `{0 boss prefab, 1 kills, 2 KTowerDifficulty}`.
- UpgradeGems `{0 scope, 1 amount}`: 0 = one stat ($GemStatsBadge*, the
  SingleGemStat group), 1 = every stat ($AllGemStatsBadge*, the AllGemStats group).
- SubClassEquipped `{0 level, 1 power rank}` ($SubClassEquippedType[NoLevel]).
- minprestigelevelsacrossclasses `{0 paragon level, 1 classes}`,
  tinyquesttotalpetsoflevel `{0 ally level, 1 allies}`, and the single-amount kinds
  (friends, loyalty, referafriend, tinyquestconcurrentbuffedpets) - each matches
  the rank collectibles' own names ("Reach level 20 with 5 Allies").
- entitlementUnlocked `{0 entitlement, 1 $status text}`; none = no tracked counter.

RANKS and DIFFICULTIES copy the exe's KBadgeRank / KTowerDifficulty tables (the
rank names also end all 271 ranked collectible file names). A group's display
name is the title every rank's name shares after "Badge: ", else its category when
the category holds only this group, else its metric label, else the category.
"""
from __future__ import annotations

from typing import Any

from app.trove.codexes.badges import EXE_PATH, parse_metric_names
from app.trove.decode.ability import Prefabs, identity, modifiers, walk_values
from app.trove.decode.common import locale, stem
from app.trove.decode.tree import GameTree
from app.trove.decode.wire import Obj, WireError, parse

TITLE = "Badges"
OUTPUT = "badges.json"
# Rewards reach into any prefab folder (items, placeables, equipment), so all of prefabs/.
PREFIXES = ("prefabs/", "languages/en/", EXE_PATH)
INDENT, FINAL_NEWLINE = 4, False

BADGES, CLAIMS = "meta/badges", "claim/badge"
CATEGORIES, BADGE_DIR = "collections/collection_badge", "prefabs/collections/badge/"
RANKS = ("bronze", "silver", "gold", "platinum", "diamond", "obsidian", "trovium")
DIFFICULTIES = ("Normal", "Hard", "Ultra")
BADGE_LINK, MODEL, DRAGON_SOUL = 299, 37, 301
STATUS = {"friends": "$FriendsBadageStatusFormat", "loyalty": "$LoyaltyBadgeStatusFormat",
          "referafriend": "$RAFBadgeStatusFormat"}
GEM_STATUS = ("$GemStatsBadgeFormat", "$AllGemStatsBadgeFormat")


def _leaf(v: Any) -> dict:
    return v.leaf if isinstance(v, Obj) else {}


def _rows(v: Any) -> list[dict]:
    return [r.leaf for r in v or [] if isinstance(r, Obj)]


def _goal(fmt: str, amount: int, *rest: str) -> str:
    """A `{0}/{1} ...` progress line turned into its goal: `{0}/{1}` -> the amount."""
    if "{0}/{1}" not in fmt:
        return ""
    out = fmt.replace("{0}/{1}", f"{amount:,}")
    for i, text in enumerate(rest, start=2):
        out = out.replace(f"{{{i}}}", text)
    return out


class Text:
    """`$key -> English text`, parsing only the tables whose bytes hold a wanted key:
    parsing every table was most of the run, and badges need about ten."""

    def __init__(self, tree: GameTree):
        self._raw = {p: tree.read(p) or b"" for p in tree.files("languages/en/", ".binfab")}
        self._map: dict[str, str] = {}

    def __call__(self, key: Any) -> str:
        if not isinstance(key, str) or not key.startswith("$"):
            return ""
        if key not in self._map:
            needle = key.encode()
            for path in [p for p, data in self._raw.items() if needle in data]:
                for k, v in locale(self._raw.pop(path)).items():
                    self._map.setdefault(k, v)
            self._map.setdefault(key, "")
        return self._map[key]


def _metric_names(tree: GameTree) -> list[str]:
    exe = tree.read(EXE_PATH)
    return parse_metric_names(exe) if exe else []


def _dragons(tree: GameTree, prefabs: Prefabs, text: Text) -> dict[int, dict]:
    """Dragon id -> its `dragonsoul/` stem and the soul items that feed it."""
    out: dict[int, dict] = {}
    for path in tree.files("prefabs/dragonsoul/", ".binfab"):
        pf = prefabs.get(path[len("prefabs/"):])
        did = _leaf(pf.root).get(0) if pf else None
        if isinstance(did, int):
            out[did] = {"dragon": stem(path), "souls": []}
    for path in tree.files("prefabs/item/dragon/", ".binfab"):
        if path.count("/") != 3:
            continue
        rel = path[len("prefabs/"):-len(".binfab")]
        pf = prefabs.get(rel)
        did = _leaf(pf.component(DRAGON_SOUL)).get(0) if pf else None
        if did in out:
            name = text(identity(pf).get("name_key"))
            out[did]["souls"].append((rel, name))
    return out


def _requirement(req: Any, text: Text, metrics: list[str], dragons: dict[int, dict]) -> dict:
    kind = _leaf(req).get(0) or ""
    p = _leaf(_leaf(req).get(1))
    out: dict[str, Any] = {"kind": kind}
    if kind == "metric":
        mid, amount = p.get(0, 0), p.get(1, 0)
        out.update(metric_id=mid, amount=amount)
        name = metrics[mid] if 0 < mid < len(metrics) else ""
        if name:
            out["metric"] = name
            label = text(f"$Metrics_{name}")
            if label:
                out.update(label=label, text=f"{amount:,} {label}")
    elif kind == "dragonsouls":
        did, amount = p.get(0, 0), p.get(1, 0)
        out.update(dragon_id=did, amount=amount)
        dragon = dragons.get(did)
        if dragon:
            out["dragon"] = dragon["dragon"]
            names = {n for _, n in dragon["souls"] if n}
            if dragon["souls"]:
                out["soul_items"] = [r for r, _ in dragon["souls"]]
            if len(names) == 1:
                soul = names.pop()
                out["soul_name"] = soul
                if soul.endswith(" Dragon Soul"):
                    out["text"] = _goal(text("$DragonSoulsBadgeStatusFormat"), amount,
                                        soul.removesuffix(" Dragon Soul"))
    elif kind == "STBossKilled":
        diff = p.get(2, 0)
        out.update(boss=p.get(0, ""), amount=p.get(1, 0), difficulty_id=diff)
        if 0 <= diff < len(DIFFICULTIES):
            out["difficulty"] = DIFFICULTIES[diff]
    elif kind == "UpgradeGems":
        scope, amount = p.get(0, 0), p.get(1, 0)
        out.update(all_stats=scope == 1, amount=amount)
        if scope in (0, 1):
            out["text"] = _goal(text(GEM_STATUS[scope]), amount)
    elif kind == "SubClassEquipped":
        level, power = p.get(0, 0), p.get(1, 0)
        out.update(level=level, power_rank=power)
        fmt = text("$SubClassEquippedType" if level else "$SubClassEquippedTypeNoLevel")
        out["text"] = (fmt.replace("{0}", f"{level:,}").replace("{1}", f"{power:,}") if level
                       else fmt.replace("{0}", f"{power:,}"))
    elif kind == "minprestigelevelsacrossclasses":
        level, classes = p.get(0, 0), p.get(1, 0)
        out.update(paragon_level=level, classes=classes)
        fmt = text("$MinPrestigeLevelsAcrossClassesBadgeStatusType")
        if fmt:
            out["text"] = f"{classes:,} " + fmt.replace("{0}", f"{level:,}")
    elif kind == "tinyquesttotalpetsoflevel":
        out.update(ally_level=p.get(0, 0), allies=p.get(1, 0))
    elif kind == "tinyquestconcurrentbuffedpets":
        out["allies"] = p.get(0, 0)
    elif kind == "entitlementUnlocked":
        out["entitlement"] = p.get(0, "")
        if text(p.get(1)):
            out["text"] = text(p.get(1))
    elif kind in STATUS:
        out["amount"] = p.get(0, 0)
        out["text"] = _goal(text(STATUS[kind]), out["amount"])
    elif kind != "none":
        out["payload"] = {k: v for k, v in p.items() if isinstance(v, (int, float, str))}
    return {k: v for k, v in out.items() if v != ""}


def _grants(claim: Any, prefabs: Prefabs, text: Text) -> list[dict]:
    """What one claim hands out."""
    top = _leaf(claim)
    kind, params = top.get(0), top.get(1)
    if not isinstance(params, Obj):
        return []
    secs = list(params) + [{}, {}, {}]

    def named(rel: str) -> str:
        return text(identity(prefabs.get(rel)).get("name_key"))

    out: list[dict] = []
    if kind == "ClaimCollectable":
        for row in _rows(secs[1].get(0)):
            rel = row.get(1, "")
            if rel.endswith(".blueprint"):             # a style: no prefab of its own
                out.append({"type": "collectable", "blueprint": rel})
            elif prefabs.exists(rel):
                out.append({"type": "collectable", "prefab": rel, "name": named(rel)})
            else:                                       # a recipe id, not a prefab path
                out.append({"type": "collectable", "id": rel})
    elif kind == "ClaimPrefab":
        for row in _rows(secs[1].get(0)):
            rel = row.get(0, "")
            out.append({"type": "item", "prefab": rel, "name": named(rel), "count": row.get(1, 1)})
    elif kind == "ClaimPoints":
        out.append({"type": "points", "amount": secs[2].get(0, 0), "name": text(secs[0].get(3)),
                    "icon": secs[0].get(6, "")})
    elif kind == "ClaimCompound":
        for sub in secs[1].get(0) or []:
            out += _grants(sub, prefabs, text)
    else:
        out.append({"type": kind or ""})
    return [{k: v for k, v in g.items() if v != ""} for g in out]


def _collectibles(tree: GameTree, prefabs: Prefabs, text: Text) -> dict[tuple[str, int], dict]:
    """(group id casefolded, rank) -> the badge collectible that component 299 ties to it."""
    out: dict[tuple[str, int], dict] = {}
    for path in tree.files(BADGE_DIR, ".binfab"):
        if path.count("/") != 3:
            continue
        rel = path[len("prefabs/"):-len(".binfab")]
        pf = prefabs.get(rel)
        link = _leaf(pf.component(BADGE_LINK)) if pf else {}
        if not isinstance(link.get(0), str):
            continue
        ident = identity(pf)
        model = _leaf(pf.component(MODEL)).get(0) if pf else None
        row: dict[str, Any] = {"prefab": rel, "name": text(ident.get("name_key")),
                               "description": text(ident.get("description_key"))}
        if isinstance(model, str) and model.endswith(".blueprint"):
            row["blueprint"] = model
        stats = modifiers(walk_values(link.get(2)))
        if stats:
            row["stats"] = stats
        out[(link[0].casefold(), link.get(1, 0))] = row
    return out


def _categories(pf: Any, text: Text) -> dict[str, dict]:
    """Collectible path (casefolded) -> {category, tags} from collection_badge."""
    out: dict[str, dict] = {}
    for cat in _rows(pf.root.get(0) if pf and pf.root else None):
        name = text(cat.get(1)) or cat.get(0, "")
        for item in _rows(cat.get(3)):
            if isinstance(item.get(0), str):
                tags = [text(t) or t for t in item.get(1) or [] if isinstance(t, str)]
                out[item[0].casefold()] = {"category": name, "tags": tags}
    return out


def _group_name(ranks: list[dict], category: str, sole: bool) -> str:
    titles = {r.get("name", "").partition("Badge: ")[2] for r in ranks}
    if len(titles) == 1 and "" not in titles:
        return titles.pop()
    if category and sole:
        return category
    labels = {r["requirement"].get("label", "") for r in ranks}
    if len(labels) == 1 and "" not in labels:
        return labels.pop()
    return category


def build(tree: GameTree) -> list[dict]:
    prefabs = Prefabs(tree)
    text = Text(tree)
    metrics = _metric_names(tree)
    dragons = _dragons(tree, prefabs, text)
    try:
        claims = _leaf(parse(tree.read(f"prefabs/{CLAIMS}.binfab") or b"").root).get(0) or {}
    except WireError:
        claims = {}
    meta = prefabs.get(BADGES)
    if meta is None or meta.root is None:
        return []
    table = {r.get(0): r for r in _rows(meta.root.get(0))}
    collectibles = _collectibles(tree, prefabs, text)
    listed = _categories(prefabs.get(CATEGORIES), text)
    groups_by_category: dict[str, set[str]] = {}
    for (gid, _), c in collectibles.items():
        cat = listed.get(c["prefab"].casefold())
        if cat:
            groups_by_category.setdefault(cat["category"], set()).add(gid)

    out = []
    for group in _rows(meta.root.get(1)):
        gid = group.get(0, "")
        key = gid.casefold()
        ranks = []
        for rank in _rows(group.get(1)):
            n = rank.get(0, 0)
            row: dict[str, Any] = {"rank": n, "tier": RANKS[n] if 0 <= n < len(RANKS) else ""}
            badge = collectibles.get((key, n))
            if badge:
                row.update(badge)
            row["requirement"] = _requirement(rank.get(3), text, metrics, dragons)
            rewards = []
            for rid in rank.get(2) or []:
                if not isinstance(rid, str):
                    continue
                ref = table.get(rid, {})
                grants = _grants(_leaf(claims.get(ref.get(2, ""))).get(1), prefabs, text)
                if badge and [g.get("prefab", "").casefold() for g in grants] == [badge["prefab"].casefold()]:
                    continue                    # the rank's own badge
                reward: dict[str, Any] = {"id": rid, "name": text(ref.get(1))}
                if grants:
                    reward["grants"] = grants
                rewards.append({k: v for k, v in reward.items() if v != ""})
            row["rewards"] = rewards
            ranks.append({k: v for k, v in row.items() if v != ""})

        mine = [c for (g, _), c in collectibles.items() if g == key]
        cats = [listed[c["prefab"].casefold()] for c in mine if c["prefab"].casefold() in listed]
        category = cats[0]["category"] if cats else ""
        tags = sorted({t for c in cats for t in c["tags"]})
        ranked = {r.get("prefab") for r in ranks}
        unranked = [c for c in mine if c["prefab"] not in ranked and c["prefab"].casefold() in listed]
        descriptions = {r.get("description", "") for r in ranks}
        entry: dict[str, Any] = {
            "slug": key,
            "name": _group_name(ranks, category, groups_by_category.get(category) == {key}) or gid,
            "description": descriptions.pop() if len(descriptions) == 1 else "",
            "category": category,
            "tags": tags,
            "ranks": ranks,
        }
        if unranked:
            entry["unranked"] = unranked
        out.append(entry)
    return sorted(out, key=lambda g: (g["category"].lower(), g["name"].lower()))


def count(data: list) -> int:
    return len(data)
