"""Generated wiki pages for game-data kinds (allies, mounts, companions, …): lookups
and render models over the decoded game data.

URLs use the game's own prefab name with dashes (``/ally/delve-dragonfly-jadedrake``)
because display names repeat across variants; the write-up is stored at
``<kind>/<slug>``. Each kind's page adds its own sections through
``wiki/kinds/<kind>.html``.
"""
from __future__ import annotations

import re
from typing import Any

from app.trove import stats as trove_stats
from app.trove.codexes.localize import resolve_stat_name
from app.trove.decode import store as gamedata
from app.wiki.ability_view import card, num, stat_text, vfx_links

# file: decoder output; key: where the list sits when the file is a dict;
# lead: the index page's intro; facets: entry fields the index filters by.
KINDS: dict[str, dict[str, Any]] = {
    "ally": {"prompt": "where to get it, how it plays, what it pairs with",
             "file": "ally_abilities.json", "plural": "allies", "title": "Allies", "icon": "fa-paw",
             "lead": "Every ally in the game files: the stats it grants and, for the {n} that have one, "
                     "what its abilities do."},
    "mount": {"prompt": "where to get it and what it is like to ride",
              "file": "mount_abilities.json", "plural": "mounts", "title": "Mounts", "icon": "fa-horse",
              "lead": "Every mount in the game files: the stats it grants in each slot and, for the {n} "
                      "that have one, what its abilities do."},
    "wings": {"prompt": "where to get them and how they feel to fly",
              "file": "collectibles.json", "key": "wings", "plural": "wings", "title": "Wings", "icon": "fa-feather",
              "facets": {"group": "Collection"},
              "lead": "Every pair of wings: the speed and glide they give and the effects they wear."},
    "boat": {"prompt": "where to get it and how it handles",
             "file": "collectibles.json", "key": "boats", "plural": "boats", "title": "Boats", "icon": "fa-sailboat",
             "facets": {"group": "Collection"},
             "lead": "Every boat: its speed, turning and acceleration, and what its cannon hits for."},
    "sail": {"prompt": "where to get it",
             "file": "collectibles.json", "key": "sails", "plural": "sails", "title": "Sails", "icon": "fa-flag",
             "facets": {"group": "Collection"},
             "lead": "Every sail in the game and where the collection screen files it."},
    "aura": {"prompt": "where to get it and what it looks like in play",
             "file": "collectibles.json", "key": "auras", "plural": "auras", "title": "Auras", "icon": "fa-wand-sparkles",
             "facets": {"group": "Collection"},
             "lead": "Every weapon aura in the game and where the collection screen files it."},
    "magrider": {"prompt": "where to get it and how it rides the rails",
                 "file": "collectibles.json", "key": "magriders", "plural": "magriders", "title": "Mag Riders",
                 "icon": "fa-person-snowboarding", "facets": {"group": "Collection"},
                 "lead": "Every Mag Rider and where the collection screen files it."},
    "flask": {"prompt": "where to get it and when it is worth drinking",
              "file": "collectibles.json", "key": "flasks", "plural": "flasks", "title": "Flasks", "icon": "fa-flask",
              "group_by": "group", "group_order": ["Flasks", "Emblems"], "facets": {"group": "Collection"},
              "lead": "Every flask and emblem: how many charges a flask holds and what drinking it does."},
    "tome": {"prompt": "where to get it and what it is worth charging",
             "file": "collectibles.json", "key": "tomes", "plural": "tomes", "title": "Tomes", "icon": "fa-book",
             "group_by": "group", "group_order": ["Tomes", "Legendary Tomes", "Mastery"], "facets": {"group": "Collection"},
             "lead": "Every tome and what it produces when fully charged."},
    "fishing-pole": {"prompt": "where to get it and where it fishes best",
                     "file": "collectibles.json", "key": "fishing_poles", "plural": "fishing-poles",
                     "title": "Fishing Poles", "icon": "fa-water", "facets": {"group": "Collection", "liquids": "Liquid"},
                     "lead": "Every fishing pole and the liquids it can fish in."},
    "costume": {"prompt": "where to get it and what it goes well with",
                "file": "cosmetics.json", "key": "costumes", "plural": "costumes", "title": "Costumes",
                "icon": "fa-shirt", "group_by": "class", "facets": {"class": "Class"},
                "lead": "Every costume, by class. Each page links to the Dressing Room to try it on."},
    "bomb-skin": {"prompt": "where to get it",
                  "file": "cosmetics.json", "key": "bomb_skins", "plural": "bomb-skins", "title": "Bomb Skins",
                  "icon": "fa-bomb", "group_by": "group", "facets": {"group": "Bomb type"},
                  "lead": "Every Bomber Royale bomb skin: what it looks like and the effects it swaps in."},
    "style": {"prompt": "which of these styles are worth hunting",
              "file": "cosmetics.json", "key": "style_slots", "plural": "styles", "title": "Styles",
              "icon": "fa-hat-wizard", "noun": "style slots",
              "lead": "Every hat, face, weapon and banner style, one page per slot, filed the way the "
                      "collection screen files them."},
    "companion": {"prompt": "where to find it and which of its perks matter",
                  "file": "companions.json", "plural": "companions", "title": "Companions",
                  "icon": "fa-dove", "facets": {"rarity": "Rarity"},
                  "lead": "Every Geode companion: what each of its levels costs and what it grants."},
    "fish": {"prompt": "where it bites and what it is worth",
             "file": "fish.json", "key": "fish", "plural": "fish", "title": "Fish", "icon": "fa-fish",
             "group_by": "collection",
             "group_order": ["Water", "Lava", "Chocolate", "Plasma", "The Depths", "Enchanted", "Turtles"],
             "facets": {"rarity": "Rarity", "liquid": "Liquid", "collection": "Collection"},
             "lead": "Every fish: its weight range, the trophy each size earns and what it breaks down into, "
                     "plus how lures change your odds."},
    "memento": {"prompt": "which delve drops it and what it is good for",
                "file": "mementos.json", "key": "mementos", "plural": "mementos", "title": "Mementos",
                "icon": "fa-box-archive", "group_by": "category",
                "group_order": ["Creature Mementos", "Boss Mementos", "Biome Mementos", "Hidden",
                                "Not in the collection"],
                "facets": {"rarity": "Rarity", "category": "Category"},
                "lead": "Every Delve memento: the mastery it gives, what Loot Collecting it yields and "
                        "what it teaches or crafts."},
    "station": {"prompt": "where to find it and what is worth crafting first",
                "file": "recipes.json", "key": "stations", "plural": "recipes", "title": "Recipes",
                "icon": "fa-hammer", "noun": "crafting stations",
                "lead": "Every crafting station and the recipes it offers, tab by tab. Filter by a station "
                        "or by anything it makes."},
    "badge": {"prompt": "the fastest way to earn each rank",
              "file": "badges.json", "plural": "badges", "title": "Badges", "icon": "fa-medal",
              "group_by": "tag", "group_order": ["Gameplay", "Social", "Dragon"], "facets": {"tag": "Type"},
              "lead": "Every badge: what each rank asks of you and what it pays out."},
}

# `$EquipmentSlot_*` -> what the stat group is labelled on a mount page.
SLOT_NAMES = {"unlock": "When unlocked", "$EquipmentSlot_Mount": "As a mount", "$EquipmentSlot_Wings": "As wings",
              "$EquipmentSlot_Boat": "As a boat", "$EquipmentSlot_Cart": "As a cart"}
# Where a wing or boat effect is attached, as the model names it.
ATTACH_NAMES = {"VFX_l_wing": "Left wing", "VFX_r_wing": "Right wing", "VFX_ground": "Ground", "VFX_body": "Body"}
RARITY_TONES = {"common": "common", "uncommon": "uncommon", "rare": "rare", "epic": "epic", "legendary": "legendary"}


def url_slug(prefab_slug: str) -> str:
    return prefab_slug.replace("_", "-")


def _rows(spec: dict) -> list[dict]:
    data = gamedata.load(spec["file"], []) or []
    if isinstance(data, dict):
        data = data.get(spec.get("key", ""), [])
    return data


@gamedata.cached(*(s["file"] for s in KINDS.values()))
def _index() -> dict[str, dict[str, dict]]:
    # An entry whose name is its prefab name has no display name in the game - not
    # something a player can own - so it gets no page.
    return {kind: {url_slug(e["slug"]): e for e in _rows(spec)
                   if e.get("name") and e["name"] != e["slug"]}
            for kind, spec in KINDS.items()}


def find(kind: str, slug: str) -> dict | None:
    """An entry by URL slug or by prefab name."""
    table = _index().get(kind, {})
    return table.get(slug) or table.get(url_slug(slug))


def entries(kind: str) -> list[dict]:
    return list(_index().get(kind, {}).values())


def storage_slug(kind: str, entry: dict) -> str:
    return f"{kind}/{entry['slug']}"


def url(kind: str, entry: dict) -> str:
    return f"/{kind}/{url_slug(entry['slug'])}"


@gamedata.cached(*(s["file"] for s in KINDS.values()))
def _by_prefab() -> dict[str, str]:
    """``collections/pet/x`` -> its wiki URL, for linking one kind's page to another's."""
    return {e["prefab"]: url(kind, e) for kind in KINDS for e in entries(kind) if e.get("prefab")}


def link(prefab: str | None) -> str:
    return _by_prefab().get((prefab or "").removesuffix(".binfab"), "")


def blueprint_name(name: str) -> str:
    """Some prefabs name a blueprint without its extension; the renderer needs it."""
    return name if not name or name.lower().endswith(".blueprint") else f"{name}.blueprint"


def picture(e: dict) -> str:
    """The blueprint an entry is drawn with: its own, else its highest rank's."""
    if e.get("blueprint"):
        return blueprint_name(e["blueprint"])
    if e.get("styles"):
        return next((blueprint_name(x["blueprint"]) for x in e["styles"] if x.get("blueprint")), "")
    return next((blueprint_name(r["blueprint"]) for r in reversed(e.get("ranks") or []) if r.get("blueprint")), "")


def _stat_rows(stats: list[dict]) -> list[str]:
    return [stat_text(s) for s in stats or []]


def _tone(rarity: str) -> str:
    return RARITY_TONES.get((rarity or "").lower(), "")


# ── index rows ─────────────────────────────────────────────────────────────

def summary(kind: str, e: dict) -> dict:
    """One card of the index page."""
    facets = {k: str(e.get(k) or "") for k in KINDS[kind].get("facets", {})}
    row = {"name": e["name"], "url": url(kind, e), "abilities": len(e.get("abilities") or []),
           "search": f"{e['name']} {e.get('description', '')}".lower(), "facets": facets,
           "sub": "", "tone": ""}
    if kind == "companion":
        row.update(sub=e.get("rarity", ""), tone=_tone(e.get("rarity", "")),
                   search=f"{row['search']} {_companion_search(e)}")
    elif kind in ("wings", "boat", "sail", "aura", "magrider", "flask", "tome", "bomb-skin"):
        row["sub"] = e.get("group", "")
    elif kind == "costume":
        row["facets"]["class"] = class_name(e.get("class", ""))
        row["sub"] = row["facets"]["class"]
    elif kind == "style":
        row["sub"] = f"{len(e.get('styles') or []):,} styles"
        row["search"] += " " + " ".join(x["name"] for x in e.get("styles") or []).lower()
    elif kind == "fishing-pole":
        row["facets"]["liquids"] = "|".join(x.title() for x in e.get("liquids") or [])
        row["sub"] = ", ".join(x.title() for x in e.get("liquids") or [])
    elif kind == "fish":
        row["facets"]["liquid"] = fish_liquid(e)
        row.update(sub=f"{e.get('rarity', '')} · {num(e['weight']['min'])}–{num(e['weight']['max'])} lb"
                   if e.get("weight") else e.get("rarity", ""), tone=_tone(e.get("rarity", "")))
    elif kind == "memento":
        row["name"] = e.get("title") or e["name"]
        row["facets"]["category"] = e.get("category") or "Not in the collection"
        row.update(sub=e.get("rarity", "") + (" · Event" if e.get("event") else ""), tone=_tone(e.get("rarity", "")))
    elif kind == "station":
        recipes = _recipes()
        ids = [rid for g in e.get("groups") or [] for rid in g.get("recipes") or []]
        row["sub"] = f"{len(ids)} recipe{'s' if len(ids) != 1 else ''}"
        row["search"] += " " + " ".join(r.get("name") or "" for rid in ids
                                        for r in (recipes.get(rid) or {}).get("results") or []).lower()
    elif kind == "badge":
        ranks = e.get("ranks") or []
        row["facets"]["tag"] = (e.get("tags") or [""])[0]
        row["sub"] = f"{len(ranks)} rank{'s' if len(ranks) != 1 else ''}"
        first = next((requirement_text(r["requirement"]) for r in ranks if requirement_text(r["requirement"])), "")
        if first:
            row["sub"] += f" · from {first}"
        row["search"] += " " + " ".join(f"{r.get('name', '')} {requirement_text(r['requirement'])}"
                                        for r in ranks).lower()
    return row


def facets(kind: str, rows: list[dict]) -> list[dict]:
    """The index's facet dropdowns, values in first-seen order; "a|b" is a multi-value."""
    out = []
    for key, label in KINDS[kind].get("facets", {}).items():
        values = list(dict.fromkeys(v for r in rows for v in (r["facets"].get(key) or "").split("|") if v))
        if len(values) > 1:
            out.append({"key": key, "label": label, "options": values})
    return out


def groups(kind: str, rows: list[dict]) -> list[dict]:
    """How the index is sectioned; one untitled group unless a kind asks otherwise."""
    field = KINDS[kind].get("group_by")
    if not field:
        return [{"title": "", "rows": rows}]
    out: dict[str, list[dict]] = {}
    for r in rows:
        out.setdefault(r["facets"].get(field, ""), []).append(r)
    order = KINDS[kind].get("group_order") or []
    ranked = sorted(out.items(), key=lambda kv: order.index(kv[0]) if kv[0] in order else len(order))
    return [{"title": k, "rows": v} for k, v in ranked]


# ── page models ────────────────────────────────────────────────────────────

def _text(s: str | None) -> str:
    # Locale text carries line breaks (and the odd ’) as literal escapes.
    text = (s or "").replace("\\n", "\n")
    return re.sub(r"\\u([0-9a-fA-F]{4})", lambda m: chr(int(m.group(1), 16)), text)


def detail(kind: str, e: dict) -> dict[str, Any]:
    d: dict[str, Any] = {"name": e["name"], "description": _text(e.get("description")), "prefab": e.get("prefab", ""),
                         "facts": [], "stat_groups": [], "abilities": []}
    abilities = [card(a, name=a.get("name", ""), description=a.get("text", "")) for a in e.get("abilities") or []]
    if kind == "bomb-skin":
        d["vfx"] = vfx_links(e.get("vfx") or [])
    if kind in ("mount", "wings", "boat", "magrider"):
        d["stat_groups"] = [{"label": SLOT_NAMES.get(g["slot"], resolve_stat_name({}, g["slot"])),
                             "stats": _stat_rows(g["stats"])} for g in e.get("stats") or []]
        if kind == "magrider":
            for g in d["stat_groups"]:
                g["label"] = "Riding" if g["label"] == SLOT_NAMES["$EquipmentSlot_Cart"] else g["label"]
        d["abilities"] = abilities
        d["vfx"] = [{**v, "label": ATTACH_NAMES.get(row.get("key", ""), v["label"])}
                    for row in e.get("vfx") or [] for v in vfx_links([row])]
    elif kind == "ally":
        if e.get("stats"):
            d["stat_groups"] = [{"label": "Stats granted", "stats": _stat_rows(e["stats"])}]
        d["abilities"] = abilities
    elif kind == "companion":
        _companion(d, e)
    elif kind == "badge":
        _badge(d, e)
    elif kind == "fish":
        _fish(d, e)
    elif kind == "memento":
        _memento(d, e)
    elif kind == "station":
        _station(d, e)
    elif kind == "costume":
        _costume(d, e)
    elif kind == "style":
        _style_slot(d, e)
    elif kind in ("flask", "fishing-pole", "tome"):
        if e.get("equip_stats"):
            d["stat_groups"] = [{"label": "When equipped", "stats": _stat_rows(e["equip_stats"])}]
        d["abilities"] = abilities
        if e.get("liquids"):
            d["facts"].append({"label": "Fishes in", "value": ", ".join(x.title() for x in e["liquids"])})
    if e.get("group") and kind != "costume":
        d["facts"].append({"label": "Collection", "value": e["group"]})
    d["facts"] += _sources(e.get("prefab") or "")
    return d


def index_extra(kind: str) -> dict[str, Any]:
    """Shared tables an index page shows beside its grid (``wiki/kinds/<kind>_index.html``)."""
    if kind == "fish":
        return _lure_table()
    if kind == "memento":
        return _memento_rules()
    if kind == "station":
        return _recipe_totals()
    return {}


# ── companions ─────────────────────────────────────────────────────────────

def _companion_stat(row: dict) -> str:
    # The game labels this stat "Max G.A.S." on every companion level that grants it.
    if row.get("stat") == "$Stat_MaxExploration":
        row = {**row, "name": "Max G.A.S."}
    return stat_text(row)


# Ability values companions change, by the game's internal name.
VALUE_NAMES = {"useCost": "N-Charge cost", "cooldown": "cooldown", "grapple_range": "range",
               "grapple_use_cost": "N-Charge cost", "thrustAccelerationXZ": "acceleration",
               "thrustMaxXZ": "thrust", "thrustEnergyUsePerSecond_tier1": "N-Charge use (tier 1)",
               "thrustEnergyUsePerSecond_tier2": "N-Charge use (tier 2)",
               "thrustEnergyUsePerSecond_tier3": "N-Charge use (tier 3)"}
_NUMBER = re.compile(r"[+-]?\d+(?:\.\d+)?%?")


def _value_change(ability: str, v: dict) -> str:
    """One ability-value row ({value, op, amount}) as a sentence."""
    what = VALUE_NAMES.get(v.get("value", ""), v.get("value", ""))
    amount = v.get("amount") or 0
    if v.get("op") == "Multiply":
        pct = (amount - 1) * 100
        change = f"{'+' if pct >= 0 else '-'}{num(abs(pct))}%"
    elif v.get("op") == "Add":
        change = f"{'+' if amount >= 0 else '-'}{num(abs(amount))}"
    else:
        change = f"{v.get('op', '')} {num(amount)}"
    return f"{change} {ability} {what}".strip()


def _grant_lines(level: dict) -> list[str]:
    """What a level's data does, independent of its label."""
    lines = [_companion_stat(s) for s in level.get("stats") or []]
    for a in level.get("abilities") or []:
        for eff in a.get("effects") or []:
            for v in eff.get("values") or []:
                lines.append(_value_change(eff.get("ability_name") or "", v))
            lines += [_companion_stat(s) for s in eff.get("stats") or []]
            restores = eff.get("restores") or {}
            if restores.get("energy"):
                lines.append(f"Restores {num(restores['energy'])} N-Charge")
    return lines


def _companion_search(e: dict) -> str:
    return " ".join(lv.get("name", "") for lv in e.get("levels") or []).lower()


def _companion(d: dict, e: dict) -> None:
    d["facts"] = [{"label": "Rarity", "value": e.get("rarity", ""), "tone": _tone(e.get("rarity", ""))}]
    if e.get("rig"):
        d["facts"].append({"label": "Body", "value": e["rig"].replace("_", " ").title()})
    levels, totals = [], {}
    for lv in e.get("levels") or []:
        for c in lv.get("cost") or []:
            key = c.get("item") or c.get("name")
            t = totals.setdefault(key, {"name": c.get("name") or key, "quantity": 0})
            t["quantity"] += c.get("quantity") or 0
        lines = _grant_lines(lv)
        label = lv.get("name") or ""
        # The label is what the game shows; the decoded numbers only when they disagree.
        said = _NUMBER.findall(label)[:1]
        if not said and lines:
            label, differs = "; ".join(lines), []
        else:
            differs = [x for x in lines if _NUMBER.findall(x)[:1] != said]
        levels.append({"level": lv["level"], "cost": lv.get("cost") or [],
                       "label": label or "; ".join(lines),
                       "data": differs if label else []})
    d["levels"] = levels
    d["total_cost"] = list(totals.values())


# ── badges ─────────────────────────────────────────────────────────────────

def requirement_text(req: dict) -> str:
    """A rank's goal: the client's own status line, else one built from its payload."""
    if req.get("text"):
        return req["text"]
    kind = req.get("kind")
    if kind == "STBossKilled" and req.get("amount"):
        return f"{num(req['amount'])} kill{'s' if req['amount'] != 1 else ''} on {req.get('difficulty', '')}".strip()
    if kind == "tinyquesttotalpetsoflevel" and req.get("allies"):
        return f"{num(req['allies'])} All{'ies' if req['allies'] != 1 else 'y'} at level {num(req.get('ally_level', 0))}"
    if kind == "tinyquestconcurrentbuffedpets" and req.get("allies"):
        return f"{num(req['allies'])} buffed All{'ies' if req['allies'] != 1 else 'y'} at once"
    return ""


def _reward(rw: dict) -> dict:
    grants = rw.get("grants") or []
    target = link(grants[0].get("prefab")) if len(grants) == 1 else ""
    lines = [x.strip() for x in _text(rw.get("name") or rw.get("id") or "").split("\n") if x.strip()]
    return {"lines": lines, "url": target}


def _badge(d: dict, e: dict) -> None:
    d["facts"] = [{"label": "Category", "value": e.get("category", "")}]
    d["facts"] += [{"label": "Type", "value": t} for t in e.get("tags") or []]
    d["ranks"] = [{"tier": (r.get("tier") or "").title(), "name": r.get("name", ""),
                   "blueprint": r.get("blueprint", ""),
                   "description": _text(r.get("description")),
                   "requirement": requirement_text(r.get("requirement") or {}),
                   "stats": _stat_rows(r.get("stats") or []),
                   "rewards": [_reward(rw) for rw in r.get("rewards") or []]}
                  for r in e.get("ranks") or []]


# ── fish ───────────────────────────────────────────────────────────────────

def fish_liquid(e: dict) -> str:
    """The liquid whose pools hold it (enchanted fish sit in the others' pools)."""
    return (e.get("pool") or e.get("liquid") or "").title()


def _pct(v: float) -> str:
    return f"{num(round(v * 100, 3))}%"


@gamedata.cached("fish.json")
def _fish_tables() -> dict:
    data = gamedata.load("fish.json", {}) or {}
    return data if isinstance(data, dict) else {}


def _fish(d: dict, e: dict) -> None:
    d["facts"] = [{"label": "Rarity", "value": e.get("rarity", ""), "tone": _tone(e.get("rarity", ""))},
                  {"label": "Liquid", "value": fish_liquid(e)},
                  {"label": "Collection", "value": e.get("collection", "")}]
    if e.get("weight"):
        d["facts"].append({"label": "Weight", "value": f"{num(e['weight']['min'])}–{num(e['weight']['max'])} lb"})
    odds = (_fish_tables().get("size_odds") or {}).get("Default") or {}
    d["sizes"] = [{"size": z.get("size", ""), "weight": f"{num(z['min'])}–{num(z['max'])} lb",
                   "chance": _pct(odds[z["size"]]) if z.get("size") in odds else "",
                   "trophy": (z.get("trophy") or {}).get("name", ""),
                   "trophy_blueprint": (z.get("trophy") or {}).get("blueprint", ""),
                   "yield": [f"{num(y['count'])} × {y['name']}" for y in z.get("deconstruct") or []]}
                  for z in e.get("sizes") or []]


def _odds_row(label: str, catch: str, size: str, bite: str, note: str = "") -> dict:
    t = _fish_tables()
    c = (t.get("catch_odds") or {}).get(catch) or {}
    z = (t.get("size_odds") or {}).get(size) or {}
    b = (t.get("bite_time") or {}).get(bite) or {}
    return {"label": label, "note": note,
            "uncommon": _pct(c["uncommon"]) if "uncommon" in c else "",
            "rare": _pct(c["rare"]) if "rare" in c else "",
            "trophy": _pct(1 - z.get("Average Size", 1)) if z else "",
            "gold": _pct(z["Gold Trophy Size"]) if "Gold Trophy Size" in z else "",
            "bite": f"{num(b['min'])}–{num(b['max'])}" if b else ""}


def _lure_table() -> dict[str, Any]:
    rows = [_odds_row("No lure", "Default", "Default", "Default")]
    for lure in _fish_tables().get("lures") or []:
        for eff in lure.get("effects") or []:
            if eff.get("requires"):
                rows.append(_odds_row(lure["name"], eff["catch"], eff["size"], eff["bite"],
                                      "with " + " + ".join(eff["requires"])))
            else:
                rows.append(_odds_row(lure["name"], eff["catch"], eff["size"], eff["bite"]))
    return {"lures": rows}


# ── mementos ───────────────────────────────────────────────────────────────

def _duration(seconds: int | float) -> str:
    for unit, size in (("day", 86400), ("hour", 3600), ("minute", 60)):
        if seconds >= size and seconds % size == 0:
            n = int(seconds // size)
            return f"{n} {unit}{'s' if n != 1 else ''}"
    return f"{num(seconds)} seconds"


def _decay_text(decay: dict | None) -> str:
    if not decay or not decay.get("seconds"):
        return ""
    when = {"Online": " online", "Offline": " offline"}.get(decay.get("trigger", ""), "")
    what = "the whole stack is destroyed" if decay.get("destroys") == "all" else f"{decay.get('destroys')} are destroyed"
    return f"{what} after {_duration(decay['seconds'])}{when}"


def _thing(name: str, prefab: str = "", count: int | None = None) -> dict:
    return {"text": f"{num(count)} × {name}" if count and count != 1 else name, "url": link(prefab)}


def _memento(d: dict, e: dict) -> None:
    d["facts"] = [{"label": "Rarity", "value": e.get("rarity", ""), "tone": _tone(e.get("rarity", ""))},
                  {"label": "Category", "value": e.get("category") or "Not in the collection"}]
    if e.get("mastery"):
        d["facts"].append({"label": "Mastery", "value": num(e["mastery"])})
    elif e.get("category") in (None, "Hidden"):
        d["facts"].append({"label": "Mastery", "value": "None (not counted in the collection)"})
    if e.get("event"):
        d["facts"].append({"label": "Event", "value": "Yes"})
    if e.get("daily_adventure"):
        d["facts"].append({"label": "Daily adventure", "value": "Counts for Corrupted Memories"})
    lists = []
    if e.get("deconstruct"):
        lists.append({"title": "Loot Collecting gives", "icon": "fa-recycle",
                      "rows": [[_thing(o.get("name") or o["item"], o.get("item", ""), o.get("count"))]
                               for o in e["deconstruct"]]})
    if e.get("teaches"):
        lists.append({"title": "Collecting it also teaches", "icon": "fa-scroll",
                      "rows": [[_thing(t.get("name") or t["recipe"])] for t in e["teaches"]]})
    if e.get("crafted_by"):
        lists.append({"title": "Crafted from", "icon": "fa-hammer",
                      "rows": [[_thing(i.get("name") or i["item"], i["item"], i.get("count")) for i in c["ingredients"]]
                               for c in e["crafted_by"]]})
    if e.get("used_in"):
        lists.append({"title": "Used to craft", "icon": "fa-screwdriver-wrench",
                      "rows": [[_thing(m.get("name") or m.get("item") or m.get("unlock", ""),
                                       m.get("item") or m.get("unlock", ""), m.get("count"))
                                for m in u["makes"]] for u in e["used_in"]]})
    d["lists"] = lists
    d["decay"] = _decay_text(e.get("decay"))


@gamedata.cached("mementos.json")
def _memento_rules() -> dict[str, Any]:
    data = gamedata.load("mementos.json", {}) or {}
    rows = (data.get("mementos") or []) if isinstance(data, dict) else []
    mastery: dict[str, set] = {}
    for m in rows:
        if m.get("category") and m.get("mastery"):
            mastery.setdefault(m["category"], set()).add(m["mastery"])
    decays = {_decay_text(m.get("decay")) for m in rows} - {""}
    return {"mastery": [{"category": k, "points": " / ".join(num(v) for v in sorted(vs))}
                        for k, vs in mastery.items()],
            "decay": decays.pop() if len(decays) == 1 else "",
            "adventure": (data.get("adventure") or {}) if isinstance(data, dict) else {}}


# ── crafting stations and recipes ──────────────────────────────────────────

RESETS = {"DailyReset": "day", "WeeklyReset": "week", "MonthlyReset": "month", "SeasonReset": "season"}


@gamedata.cached("recipes.json")
def _recipe_file() -> dict:
    data = gamedata.load("recipes.json", {}) or {}
    return data if isinstance(data, dict) else {}


def _recipes() -> dict[str, dict]:
    return _recipe_file().get("recipes") or {}


def _recipe_totals() -> dict[str, Any]:
    data = _recipe_file()
    offered = {rid for st in data.get("stations") or [] for g in st.get("groups") or [] for rid in g.get("recipes") or []}
    return {"recipes": len(data.get("recipes") or {}), "offered": len(offered),
            "elsewhere": len({rid for o in data.get("other_offerers") or [] for rid in o.get("recipes") or []} - offered),
            "unoffered": len(data.get("not_offered") or [])}


def _bound(label: str, q: dict) -> str:
    lo, hi = q.get("min"), q.get("max")
    if lo is not None and hi is not None:
        return f"{label} {num(lo)}" if lo == hi else f"{label} {num(lo)}–{num(hi)}"
    if lo is not None:
        return f"{label} {num(lo)} or more"
    if hi is not None:
        return f"{label} {num(hi)} or less"
    return ""


def _spaced(name: str) -> str:
    return re.sub(r"(?<=[a-z])(?=[A-Z])", " ", name)


def _class_name(tech: str) -> str:
    c = trove_stats.class_by_tech_name(tech.lower())
    return c["name"] if c else _spaced(tech)


def requirement_lines(q: dict) -> list[str]:
    """A recipe requirement as short lines: the game's own message where it has one."""
    kind = q.get("kind")
    # "Not already owned" carries the game's failure text, which reads as an instruction.
    if kind in ("hastitle", "hascollection") and q.get("match") == "none":
        return ["Only if you don't own it yet"]
    if q.get("message") or q.get("text"):
        return [re.sub(r"\s*\[HK:[^\]]*\]", "", q.get("message") or q["text"]).strip()]
    if kind == "setall":
        return [x for c in q.get("of") or [] for x in requirement_lines(c)]
    if kind == "setany":
        parts = [" and ".join(requirement_lines(c)) for c in q.get("of") or []]
        parts = [p for p in parts if p]
        return ["One of: " + " / ".join(parts)] if len(parts) > 1 else parts
    if kind == "setnone":
        parts = [x for c in q.get("of") or [] for x in requirement_lines(c)]
        return ["Not: " + "; ".join(parts)] if parts else []
    labels = {"powerrank": "Power Rank", "zonelevel": "Zone level", "metalevel": "Mastery Rank",
              "classlevel": "Class level", "highestpowerlevel": "Highest class Power Rank"}
    if kind in labels:
        return [x for x in [_bound(labels[kind], q)] if x]
    if kind == "profession" and q.get("profession"):
        return [x for x in [_bound(f"{q['profession']} skill", q)] if x]
    if kind == "activeclass" and q.get("classes"):
        return ["Playing " + ", ".join(_class_name(c) for c in q["classes"])]
    if kind == "zonetag" and q.get("tags"):
        return ["Only in " + ", ".join(q["tags"])]
    return []


def _product(x: dict) -> dict:
    ref = x.get("path") or (x.get("collectable") or {}).get("ref") or ""
    name = x.get("name") or ""
    amount = x.get("amount") or 0
    return {"text": (f"{num(amount)} × {name}" if amount > 1 else name) if name else "",
            "raw": "" if name else (ref or x.get("claim") or ""), "url": link(ref)}


def _recipe_row(rid: str, r: dict) -> dict:
    notes = requirement_lines(r["requirement"]) if r.get("requirement") else []
    limit = r.get("limit") or {}
    if limit.get("count") and limit.get("reset") in RESETS:
        notes.append(f"{num(limit['count'])} per {RESETS[limit['reset']]}")
    if r.get("season"):
        notes.append(f"{r['season']} season")
    makes = [_product(x) for x in r.get("results") or []]
    search = " ".join(m["text"] or m["raw"] for m in makes)
    return {"id": rid, "makes": makes, "ingredients": [_product(x) for x in r.get("ingredients") or []],
            "notes": notes, "search": search.lower()}


def _station(d: dict, e: dict) -> None:
    recipes = _recipes()
    d["profession"] = e.get("profession_name") or ""
    if d["profession"]:
        d["facts"].append({"label": "Profession", "value": d["profession"]})
    groups = []
    for g in e.get("groups") or []:
        rows = [_recipe_row(rid, recipes[rid]) for rid in g.get("recipes") or [] if rid in recipes]
        if rows:
            groups.append({"name": g.get("name") or "Recipes", "unlock_skill": g.get("unlock_skill"), "rows": rows})
    d["groups"] = groups
    d["recipe_count"] = sum(len(g["rows"]) for g in groups)
    twins = [x for x in (find("station", t) for t in e.get("same_as") or []) if x]
    d["same_as"] = [{"name": t["name"], "url": url("station", t)} for t in twins]


# ── where an entry comes from (recipes and badge rewards) ──────────────────

@gamedata.cached("recipes.json", "badges.json")
def _origins() -> dict[str, dict[str, list[str]]]:
    """``prefab -> {"stations": [station slug], "badges": [badge slug]}``."""
    out: dict[str, dict[str, list[str]]] = {}

    def add(prefab: str | None, key: str, slug: str) -> None:
        if not prefab:
            return
        rows = out.setdefault(prefab.removesuffix(".binfab"), {}).setdefault(key, [])
        if slug not in rows:
            rows.append(slug)

    recipes = _recipes()
    for st in _recipe_file().get("stations") or []:
        for g in st.get("groups") or []:
            for rid in g.get("recipes") or []:
                for res in (recipes.get(rid) or {}).get("results") or []:
                    add(res.get("path") or (res.get("collectable") or {}).get("ref"), "stations", st["slug"])
    badges = gamedata.load("badges.json", []) or []
    for b in badges if isinstance(badges, list) else []:
        for r in b.get("ranks") or []:
            for rw in r.get("rewards") or []:
                for gr in rw.get("grants") or []:
                    add(gr.get("prefab"), "badges", b["slug"])
    return out


def _sources(prefab: str) -> list[dict]:
    got = _origins().get(prefab) or {}
    facts = []
    for slug in got.get("stations") or []:
        st = find("station", slug)
        if st:
            facts.append({"label": "Crafted at", "value": st["name"], "url": url("station", st)})
    for slug in got.get("badges") or []:
        b = find("badge", slug)
        if b:
            facts.append({"label": "Badge reward", "value": b["name"], "url": url("badge", b)})
    return facts


# ── costumes and styles ────────────────────────────────────────────────────

def class_name(tech: str) -> str:
    """A KClass name (``ShadowHunter``) as players know the class."""
    return _class_name(tech) if tech else ""


def _costume(d: dict, e: dict) -> None:
    tech = (e.get("class") or "").lower()
    c = trove_stats.class_by_tech_name(tech)
    if c:
        slug = "-".join("".join(ch if ch.isalnum() else " " for ch in c["name"].lower()).split())
        d["facts"].append({"label": "Class", "value": c["name"], "url": f"/class/{slug}"})
        d["dressing_room"] = {"class": tech, "costume": e["slug"]}


def _style_slot(d: dict, e: dict) -> None:
    groups: dict[str, list[dict]] = {}
    for x in e.get("styles") or []:
        groups.setdefault(x.get("group") or "Other", []).append(x)
    d["style_groups"] = sorted(({"name": k, "styles": v} for k, v in groups.items()),
                               key=lambda g: g["name"] == "Other")
    d["style_count"] = len(e.get("styles") or [])


# ── titles (one data page) ─────────────────────────────────────────────────

def _title_stations() -> dict[str, list[str]]:
    """Title id -> stations whose recipe grants exactly that title: a recipe with a
    Title result whose "not owned yet" requirement names that one id."""
    out: dict[str, list[str]] = {}
    recipes = _recipes()

    def owned_check(q: dict) -> list[str]:
        if q.get("kind") == "hastitle" and q.get("match") == "none":
            return list(q.get("titles") or [])
        return [t for c in q.get("of") or [] for t in owned_check(c)]

    for st in _recipe_file().get("stations") or []:
        for g in st.get("groups") or []:
            for rid in g.get("recipes") or []:
                r = recipes.get(rid) or {}
                ids = owned_check(r.get("requirement") or {})
                if len(ids) == 1 and any(x.get("kind") == "Title" for x in r.get("results") or []):
                    rows = out.setdefault(ids[0], [])
                    if st["slug"] not in rows:
                        rows.append(st["slug"])
    return out


@gamedata.cached("titles.json", "recipes.json")
def titles_page() -> dict[str, Any]:
    rows = []
    stations = _title_stations()
    for t in gamedata.load("titles.json", []) or []:
        crafted = [x for x in (find("station", sid) for sid in stations.get(t["id"], [])) if x]
        rows.append({"name": t["name"], "male": t.get("male", ""), "female": t.get("female", ""),
                     "description": _text(t.get("description")),
                     "crafted": [{"name": x["name"], "url": url("station", x)} for x in crafted],
                     "search": " ".join([t["name"], t.get("female", ""), t.get("description", "")]).lower()})
    return {"titles": rows, "with_description": sum(1 for r in rows if r["description"]),
            "crafted": sum(1 for r in rows if r["crafted"])}
