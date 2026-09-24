"""quests.json - adventures, quest lines, Tiny Quests and the Golden Thread.

Every adventure-like thing is a PersonalObjective (the exe's reflected class). The
files that list them:
- `meta/activities/<set>.binfab`: root field 1 the objectives, field 2 the set's
  KActivityType (the exe's enum: Event 0, Expertise 1 ... TinyQuests 11). Expertise
  is the tracker's "Quests" tab (`$ActivityExpertiseTitle`).
- `meta/goldenthread/goldenthread.binfab`: root field 0, the Golden Thread - the
  new-player questline plus every event's story chain.
- `meta/activity.binfab` (ActivityInfoData): field 3 caps how many adventures of a
  type can be active at once and field 5 how many rewards a type pays per day, both
  `{0 amount, 1 KActivityType}` (the "$TooMany..." and "$DailyActivitiesLimit"
  checks read them); field 4 maps a thread id to `{1 ordered objective ids}`.

PersonalObjective fields, each from the tracker and accept code: 0 id; 1 name key;
2 description key, 15 the short text shown when 2 is empty; 3 objective; 4 reward
claim id; 12 seconds the adventure lasts once taken (-1 none; the tracker counts it
down, and the texts agree: 86400 "available for 24 hours", 259200 "3 days"); 13 icon,
with 20 saying what it is (the exe's Icon/Blueprint/Prefab table; 3 = a plain .dds);
14 resets daily (the client refuses a retake completed after the day's 11:00 boundary,
"$DailyActivityCompleted"). The other fields (Tiny Quest scoring 19/22/29-32 among
them) are unproven, and a Tiny Quest's objective is a placeholder.

An objective is `{0 kind, 1 params}`; kinds register lowercased, so `Mastery` and
`mastery` are one class. Params keep their fields on the last section. Per kind,
the field the client compares progress against (its goal getter) is `goal`; the
other labels come from each kind's event handler: a target list empty on both it
and its tag list means "any"; `requirement` must hold when the event fires;
`take_items` / `removes` are items the client takes away. Metric ordinals are
named from Trove_x64.exe like badges; `activity_types` values match the texts
("Complete Club Adventures" = [2, 3]).

A claim id resolves in any `prefabs/claim/*` file (one id space; an id no file has,
or two files define differently, gets no rewards); what it grants is read as
badges.py reads badge claims. Which adventures an NPC offers on a given day, which
event chain is live, and Tiny Quest outcomes are server-side.

Groups: one per activities file, in thread order when a thread shares its name.
Golden Thread entries go by thread, else by id stem (`event_july2025_03` ->
`event_july2025`) in file order; a lone id joins the chain its "Title n/m:"
description prefix names, else "Other". A chain is named by that prefix. The world
quest/trigger prefabs (`prefabs/quest*`) carry no text and are not read.
"""
from __future__ import annotations

import re
from collections import Counter
from typing import Any

from app.trove.codexes.badges import EXE_PATH, parse_metric_names
from app.trove.decode.ability import Prefabs, identity
from app.trove.decode.badges import Text, _grants
from app.trove.decode.fields import IDENTITY, stat_key
from app.trove.decode.recipes import _components, _requirement
from app.trove.decode.tree import GameTree
from app.trove.decode.wire import Obj, Prefab, WireError, parse

TITLE = "Quests and Adventures"
OUTPUT = "quests.json"
PREFIXES = ("prefabs/", "languages/en/", EXE_PATH)
INDENT, FINAL_NEWLINE = 4, False

SETS_DIR, GOLDEN, META = "prefabs/meta/activities/", "meta/goldenthread/goldenthread", "meta/activity"
ID, NAME, DESC, OBJECTIVE, CLAIM, TIME_LIMIT, ICON, DAILY, SUMMARY, ICON_KIND = 0, 1, 2, 3, 4, 12, 13, 14, 15, 20
ACTIVITY_TYPES = {0: "Event", 1: "Expertise", 2: "ClubMemberAdventure", 3: "ClubNonMemberAdventure",
                  4: "WorldObjective", 5: "NPCAdventure", 6: "GeodeNPCAdventure",
                  7: "CrystallogyAdventure", 8: "RepeatableEvent", 10: "AutoUseTome", 11: "TinyQuests"}
ICON_KINDS = {1: "Blueprint", 2: "Prefab"}

# kind -> params field -> label; lists of prefabs get names, "requirement" is decoded.
OBJECTIVES: dict[str, dict[int, str]] = {
    "adventurecompleted": {0: "adventures", 1: "goal", 3: "activity_types"},
    "itemacquired": {0: "items", 1: "goal", 4: "take_items", 5: "requirement", 6: "item_tags"},
    "itemconsumed": {0: "items", 1: "goal", 4: "requirement"},
    "itemfished": {0: "items", 1: "goal", 3: "tags", 4: "requirement"},
    "interacted": {0: "targets", 2: "requirement", 3: "goal", 5: "tags", 6: "removes", 7: "remove_one"},
    "npckilled": {0: "npcs", 1: "goal", 3: "npc_tags", 4: "require_both", 5: "requirement"},
    "craftingcomplete": {0: "stations", 1: "recipes", 3: "goal", 4: "removes", 5: "remove_one"},
    "quest": {0: "quests", 1: "requirement", 2: "goal"},
    "trigger": {0: "triggers", 2: "goal", 3: "requirement"},
    "metric": {0: "metric", 1: "goal"},
    "blockplaced": {2: "goal"},
    "collectionunlock": {4: "goal"},
    "itemlooted": {1: "goal"},
    "itemupgrade": {1: "goal"},
    "upgradematerial": {1: "goal"},
    "tomelevel": {1: "goal"},
    "jump": {0: "goal"},
    "walk": {0: "goal"},
    "tinyquestinstacompleted": {0: "goal"},
    "classlevel": {0: "level"},
    "mastery": {0: "level"},
    "professionrank": {0: "rank", 1: "professions"},
    "stattotal": {0: "amount", 1: "stats"},
    "requestworld": {0: "world_type", 2: "world"},
    "activeui": {0: "ui"},
}
PREFAB_LISTS = frozenset({"items", "targets", "npcs", "stations", "removes"})
SET_KINDS = frozenset({"objectivesetall", "objectivesetany"})
CHAIN_TITLE = re.compile(r"^(?:(.{3,60}?) \d+\s*/\s*\d+\s*[:.]|\d+\s*/\s*\d+ (.{3,60}?):)")
ID_STEM = re.compile(r"_\d+(?:_\w+)?$")


def _leaf(v: Any) -> dict:
    return v.leaf if isinstance(v, Obj) else {}


def _rows(v: Any) -> list[dict]:
    return [r.leaf for r in v or () if isinstance(r, Obj)]


def _strs(v: Any) -> list[str]:
    return [s for s in v or () if isinstance(s, str) and s]


def _humanize(stem: str) -> str:
    words = stem.replace("_", " ").split()
    return " ".join(w.upper() if w in ("npc", "pvp", "cm") else w.capitalize() for w in words)


class _Identities(Prefabs):
    """Prefabs reduced to their identity component, the only part the names need."""

    def get(self, rel: str) -> Prefab | None:
        rel = rel.removesuffix(".binfab")
        if rel not in self._cache:
            data = self.tree.read(f"prefabs/{rel}.binfab")
            comps = _components(data, {IDENTITY}) if data else {}
            self._cache[rel] = Prefab("entity", components=list(comps.items())) if data else None
        return self._cache[rel]


class _Game:
    def __init__(self, tree: GameTree):
        self.prefabs = _Identities(tree)
        self.text = Text(tree)
        exe = tree.read(EXE_PATH)
        self.metrics = parse_metric_names(exe) if exe else []
        self.tree = tree
        self.claims: dict[str, Any] = {}
        self.names: dict[str, str] = {}         # objective id -> name, filled before decoding

    def load_claims(self, wanted: set[str]) -> None:
        """The claim records of ``wanted`` ids, parsing only the claim files naming one."""
        clash: set[str] = set()
        needles = [w.encode() for w in wanted]
        for path in self.tree.files("prefabs/claim/", ".binfab"):
            data = self.tree.read(path) or b""
            if not any(n in data for n in needles):
                continue
            try:
                table = _leaf(parse(data).root).get(0)
            except WireError:
                continue
            for cid, rec in (table or {}).items():
                if cid in self.claims and self.claims[cid] != rec:
                    clash.add(cid)
                self.claims.setdefault(cid, rec)
        for cid in clash:                       # the game's winner between files is unknown
            del self.claims[cid]

    def say(self, key: Any) -> str:
        if isinstance(key, str) and key.startswith("@"):
            return key[1:]
        return self.text(key)

    def prefab_name(self, rel: str) -> str:
        return self.say(identity(self.prefabs.get(rel)).get("name_key"))


def _requirement_text(req: dict, g: _Game) -> dict:
    """recipes' requirement shape with its locale keys turned into text."""
    if isinstance(req.get("message"), str):
        req["message"] = g.say(req["message"])
    if isinstance(req.get("text"), tuple):
        fmt, n = req["text"]
        req["text"] = g.say(fmt).replace("{0}", f"{n:,}")
    for child in req.get("of", []):
        _requirement_text(child, g)
    if isinstance(req.get("condition"), dict):
        _requirement_text(req["condition"], g)
    return {k: v for k, v in req.items() if v not in ("", None)}


def _value(label: str, v: Any, g: _Game) -> Any:
    if label == "requirement":
        req = _requirement(v, set())
        return _requirement_text(req, g) if req else None
    if label in PREFAB_LISTS:
        out = []
        for p in _strs(v):
            name = g.prefab_name(p)
            out.append({"prefab": p, "name": name} if name else {"prefab": p})
        return out or None
    if label == "adventures":
        out = []
        for a in _strs(v):
            name = g.names.get(a, "")
            out.append({"id": a, "name": name} if name else {"id": a})
        return out or None
    if label == "activity_types":
        types = [ACTIVITY_TYPES.get(t, t) for t in v or () if isinstance(t, int)]
        return types or None
    if label == "metric":
        name = g.metrics[v] if isinstance(v, int) and 0 < v < len(g.metrics) else ""
        return {"id": v, "name": name, "label": g.text(f"$Metrics_{name}")} if name else {"id": v}
    if label == "stats":
        out = []
        for s in v or ():
            if isinstance(s, int):
                name = g.text(stat_key(s))
                out.append({"id": s, "name": name} if name else {"id": s})
        return out or None
    if label in ("take_items", "remove_one", "require_both"):
        return bool(v) or None
    if isinstance(v, list):
        return _strs(v) or None
    return v if v not in ("", None) else None


def _objective(node: Any, g: _Game) -> dict:
    leaf = _leaf(node)
    kind = leaf.get(0)
    if not isinstance(kind, str) or not kind:
        return {}
    params = leaf.get(1)
    own = params[-1] if isinstance(params, Obj) and params else {}
    key = kind.lower()
    out: dict[str, Any] = {"kind": key}
    if key in SET_KINDS:
        out["match"] = "all" if key == "objectivesetall" else "any"
        subs = own.get(0)
        out["objectives"] = [o for o in (_objective(s, g) for s in (subs or {}).values()) if o]
        return out
    for idx, label in OBJECTIVES.get(key, {}).items():
        v = _value(label, own.get(idx), g)
        if v is not None:
            out[label] = v
    return out


def _entry(adv: dict, g: _Game) -> dict:
    aid = adv.get(ID, "")
    row: dict[str, Any] = {"slug": aid, "name": g.say(adv.get(NAME))}
    desc, summary = g.say(adv.get(DESC)).strip(), g.say(adv.get(SUMMARY)).strip()
    row["description"] = desc or summary
    if desc and summary and summary != desc:
        row["summary"] = summary
    obj = _objective(adv.get(OBJECTIVE), g)
    if obj:
        row["objective"] = obj
    cid = adv.get(CLAIM)
    if isinstance(cid, str) and cid and cid in g.claims:     # some ids name no claim
        row["claim"] = cid
        grants = _grants(_leaf(g.claims[cid]).get(1), g.prefabs, g.text)
        if grants:
            row["rewards"] = grants
    limit = adv.get(TIME_LIMIT)
    if isinstance(limit, (int, float)) and limit > 0:
        row["time_limit"] = int(limit) if float(limit).is_integer() else limit
    if adv.get(DAILY):
        row["daily_reset"] = True
    icon = adv.get(ICON)
    if isinstance(icon, str) and icon:
        row["icon"] = icon
        if adv.get(ICON_KIND) in ICON_KINDS:
            row["icon_kind"] = ICON_KINDS[adv[ICON_KIND]]
    return row


def _load(tree: GameTree, rel: str) -> Obj | None:
    try:
        return parse(tree.read(f"prefabs/{rel}.binfab") or b"").root
    except WireError:
        return None


def _meta(tree: GameTree) -> tuple[list[dict], dict[str, list[str]]]:
    root = _leaf(_load(tree, META))
    limits: dict[int, dict[str, int]] = {}
    for field, label in ((3, "active"), (5, "daily_rewards")):
        for r in _rows(root.get(field)):
            if isinstance(r.get(1), int) and isinstance(r.get(0), int):
                limits.setdefault(r[1], {})[label] = r[0]
    out = [{"type": ACTIVITY_TYPES.get(t, t), **v} for t, v in sorted(limits.items())]
    threads = {k: _strs(_leaf(v).get(1)) for k, v in (root.get(4) or {}).items() if isinstance(k, str) and k}
    return out, threads


def _ordered(rows: list[dict], order: list[str]) -> list[dict]:
    """``rows`` in thread order, each with its step; the unthreaded ones after, as listed."""
    pos = {aid: i for i, aid in enumerate(order)}
    for r in rows:
        if r["slug"] in pos:
            r["step"] = pos[r["slug"]] + 1
    return sorted(rows, key=lambda r: r.get("step", len(order) + 1))


def _chain_title(rows: list[dict]) -> str:
    """The "Title n/m:" prefix most of a chain's descriptions share."""
    titles = Counter((m.group(1) or m.group(2)).strip() for r in rows
                     if (m := CHAIN_TITLE.match(r["description"])))
    return titles.most_common(1)[0][0] if titles else ""


def _slug(text: str) -> str:
    return re.sub(r"[^a-z0-9]+", "-", text.lower()).strip("-")


def build(tree: GameTree) -> dict:
    g = _Game(tree)
    limits, threads = _meta(tree)

    sources: list[tuple[str, int | None, list[dict]]] = []
    for path in tree.files(SETS_DIR, ".binfab"):
        stem = path[len(SETS_DIR):-len(".binfab")]
        root = _load(tree, path[len("prefabs/"):-len(".binfab")])
        advs = _rows(_leaf(root).get(1))
        if advs:
            sources.append((stem, _leaf(root).get(2), advs))
    golden = _rows(_leaf(_load(tree, GOLDEN)).get(0))
    wanted: set[str] = set()
    for _, _, advs in [*sources, ("", None, golden)]:
        for a in advs:
            if isinstance(a.get(ID), str):
                g.names.setdefault(a[ID], g.say(a.get(NAME)))
            if isinstance(a.get(CLAIM), str) and a[CLAIM]:
                wanted.add(a[CLAIM])
    g.load_claims(wanted)

    groups: list[dict] = []
    for stem, typ, advs in sources:
        rows = [_entry(a, g) for a in advs]
        group: dict[str, Any] = {"slug": stem.replace("_", "-"), "name": _humanize(stem), "source": stem,
                                 "type": ACTIVITY_TYPES.get(typ, typ) if isinstance(typ, int) else None}
        if stem in threads:
            group["thread"] = stem
            rows = _ordered(rows, threads[stem])
        group["entries"] = rows
        groups.append(group)

    threaded = {aid: tid for tid, ids in threads.items() for aid in ids}
    stems: dict[str, list[dict]] = {}
    for a in golden:
        aid = a.get(ID, "")
        stems.setdefault(threaded.get(aid) or ID_STEM.sub("", aid), []).append(_entry(a, g))
    chains: dict[str, tuple[str, list[dict]]] = {}
    for stem, rows in stems.items():
        title = _chain_title(rows)
        if len(rows) == 1 and stem not in threads:      # a lone id: file by its chain title
            key, name = (_slug(title), title) if title else ("golden-thread-other", "Other Golden Thread Quests")
            chains.setdefault(key, (name, []))[1].extend(rows)
        else:
            name = "Golden Thread" if stem == "golden_thread" else title or rows[0]["name"] or _humanize(stem)
            chains[stem.replace("_", "-")] = (name, rows)
    for key, (name, rows) in chains.items():
        stem = key.replace("-", "_")
        group = {"slug": key, "name": name, "source": "goldenthread", "type": None}
        if stem in threads:
            group["thread"] = stem
            rows = _ordered(rows, threads[stem])
        group["entries"] = rows
        groups.append(group)

    return {"limits": limits, "groups": [{k: v for k, v in grp.items() if v is not None} for grp in groups]}


def count(data: dict) -> int:
    return sum(len(grp["entries"]) for grp in data["groups"])
