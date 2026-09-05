"""Server-side render model for /abilities.

Five decoded datasets, one page: empowered gems, class rings, class abilities,
the star chart's and allies'. They are reference data, so the whole thing is rendered rather
than fetched, and abilities.js only switches tabs and filters what is there.

The first three are regenerated from the game tree by scripts/decode_*_abilities.py;
the star chart's ride along in star_chart.json. The shapes differ, so each is folded
into one common card here: a name, some chips, the game's own description, and
whatever numbers the files back.
"""
import json
import re
from functools import cache
from pathlib import Path
from typing import Any

from app.trove.gems.model import gem_lookups

_DATA = Path(__file__).resolve().parents[1] / "trove" / "gamedata"

# Filter order for the gems tab; matches how the game groups them.
ELEMENT_ORDER = ("Water", "Fire", "Air", "Cosmic", "Prismatic")

# The Masterful Prismatic Gem is a wildcard that fits any empowered slot; it grants
# no ability of its own, so it has no place on a page about what abilities do. It
# stays in gem_abilities.json - it is a real gem - and is dropped only here.
SKIP_GEMS = {("Prismatic", "mastery")}

# "Empowered Air Gem for the Revenant. …" and, for the Prismatic gem which
# belongs to no class, "Empowered Prismatic Gem; can be socketed …".
_LEAD = re.compile(
    r"^Empowered\s+\w+\s+Gem\b(?:\s+for\s+(?:the\s+)?([^.;]+?))?\s*[.;]\s*(.*)$",
    re.S | re.I)


@cache
def _load(name: str) -> Any:
    try:
        return json.loads((_DATA / name).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return []


def _split_description(text: str) -> tuple[str, str]:
    """`("Revenant", "Bulwark Bash loses…")` from the game's own sentence.

    Every gem description opens by naming the element, which is already a chip on
    the card - and naming one element is wrong for an ability that ships in three -
    so the lead is lifted off and whoever it is `for` becomes a chip of its own.
    """
    match = _LEAD.match((text or "").strip())
    if not match:
        return "", (text or "").strip()
    who = (match.group(1) or "").strip()
    rest = match.group(2).strip()
    if not who:
        return "", rest
    return ("Any class" if who.lower() in {"any class", "all classes"} else who), rest


def _num(value: float) -> str:
    """Trim a trailing `.0` so 350.0 prints as 350."""
    return f"{value:g}"


def _phase_label(prefab: str) -> str:
    """`abilities/gems/battle_frenzy_large` -> `Battle Frenzy Large`."""
    return prefab.rsplit("/", 1)[-1].replace("_", " ").title()


def _effects(entry: dict) -> list[dict]:
    """The decoded numbers as printable rows.

    `multiplier` scales the class's damage stat, so it reads as a percentage - the
    wire's 3.5 is 350%. A stat row shows its operation's sense: an `Add` is a flat
    `+25`, a `MultiplySum` a `+20%`, and a `Multiply` keeps the raw multiplier
    because rendering it as a percentage loses which way it goes.
    """
    rows: list[dict] = []
    for stat in entry.get("stats") or []:
        op, value = stat.get("op", ""), stat.get("value") or 0
        if op == "Add":
            shown = f"+{_num(value)}"
        elif op == "Multiply":
            amount = stat.get("amount")
            shown = f"x{_num(amount)}" if amount else f"{_num(value)}%"
        else:
            shown = f"+{_num(value)}%"
        row = {"label": stat.get("name", ""), "value": shown, "base": "", "kind": "stat",
               "prefab": stat.get("prefab", "")}
        if row not in rows:
            rows.append(row)
    for stage in entry.get("stages") or []:
        mult, base = stage.get("multiplier") or 0, stage.get("base") or 0
        if not mult and not base:
            continue
        row = {"label": stage.get("name", ""),
               "value": f"{_num(mult * 100)}%" if mult else "",
               "base": f"+{_num(base)}" if base else "", "kind": "damage",
               "prefab": stage.get("prefab", "")}
        if row not in rows:
            rows.append(row)
    return rows


def _phases(rows: list[dict]) -> list[dict]:
    """Group effect rows by the prefab they came from.

    An ability can apply in stages that stack - Berserk Battler's frenzied state
    is `battle_frenzy_small` and the berserk state it escalates into is
    `battle_frenzy_large` - and flattening them reads as one ability granting
    Attack Speed twice. The prefab IS the phase, so it is what the grouping keys
    on; a single-prefab ability gets one unlabelled group and looks unchanged.

    Ordered weakest-first, which is the order they escalate in.
    """
    groups: dict[str, list[dict]] = {}
    for row in rows:
        groups.setdefault(row.get("prefab") or "", []).append(row)
    if len(groups) < 2:
        return [{"label": "", "prefab": "", "rows": rows}]

    def weight(item: tuple[str, list[dict]]) -> float:
        return sum(abs(float(re.sub(r"[^0-9.\-]", "", r["value"]) or 0)) for r in item[1])

    return [{"label": _phase_label(prefab), "prefab": prefab, "rows": items}
            for prefab, items in sorted(groups.items(), key=weight)]


def _card(name: str, chips: list[dict], description: str, badge: str,
          effects: list[dict], filters: list[str]) -> dict:
    return {
        "name": name, "chips": chips, "description": description, "badge": badge,
        "effects": effects, "phases": _phases(effects), "filters": filters,
        # The client filter matches on this, so keep it lowercase and pre-joined.
        "search": " ".join([name, description, *(c["label"] for c in chips)]).lower(),
    }


def _gem_cards() -> tuple[list[dict], list[str]]:
    """One card per ability, not per gem - the same ability ships in 3 elements."""
    folded: dict[str, dict] = {}
    for gem in gem_lookups().get("empowered_gems") or []:
        name = gem.get("name") or gem.get("slug") or ""
        if not name or (gem.get("element"), gem.get("slug")) in SKIP_GEMS:
            continue
        applies_to, body = _split_description(gem.get("description") or "")
        entry = folded.setdefault(name, {"elements": [], "tiers": [], "who": applies_to,
                                         "description": body, "stats": [], "stages": []})
        if gem.get("element") and gem["element"] not in entry["elements"]:
            entry["elements"].append(gem["element"])
        entry["tiers"] += gem.get("tiers") or []
        entry["description"] = entry["description"] or body
        entry["who"] = entry["who"] or applies_to
        entry["stats"] += [s for s in (gem.get("stats") or []) if s not in entry["stats"]]
        entry["stages"] += [s for s in (gem.get("stages") or []) if s not in entry["stages"]]

    cards = []
    for name, entry in folded.items():
        entry["elements"].sort(
            key=lambda e: ELEMENT_ORDER.index(e) if e in ELEMENT_ORDER else 99)
        tiers = sorted(set(entry["tiers"]))
        badge = "" if not tiers else (
            f"T{tiers[0]}" if len(tiers) == 1 else f"T{tiers[0]}-T{tiers[-1]}")
        chips = [{"label": e, "tone": e.lower()} for e in entry["elements"]]
        if entry["who"]:
            chips.append({"label": entry["who"], "tone": "who"})
        cards.append(_card(name, chips, entry["description"], badge,
                           _effects(entry), entry["elements"]))
    cards.sort(key=lambda c: c["name"].lower())
    used = [e for e in ELEMENT_ORDER if any(e in c["filters"] for c in cards)]
    return cards, used


def _ring_cards() -> tuple[list[dict], list[str]]:
    cards = []
    for ring in _load("ring_abilities.json"):
        owner = ring.get("class") or ""
        chips = [{"label": owner, "tone": "who"}] if owner else []
        cards.append(_card(ring.get("name") or "", chips, ring.get("description") or "",
                           "", _effects(ring), [owner] if owner else []))
    cards.sort(key=lambda c: (c["filters"][0] if c["filters"] else "", c["name"].lower()))
    return cards, sorted({c["filters"][0] for c in cards if c["filters"]})


def _class_cards() -> tuple[list[dict], list[str]]:
    cards = []
    for entry in _load("class_abilities.json"):
        owner = entry.get("name") or ""
        for ability in entry.get("abilities") or []:
            chips = [{"label": owner, "tone": "who"}]
            if ability.get("type"):
                chips.append({"label": ability["type"], "tone": "kind"})
            # Still in the files but the live class prefab no longer reaches it.
            badge = "" if ability.get("active", True) else "Legacy"
            cards.append(_card(ability.get("name") or "", chips,
                               ability.get("description") or "", badge,
                               _effects(ability), [owner]))
    cards.sort(key=lambda c: (c["filters"][0] if c["filters"] else "", c["name"].lower()))
    return cards, sorted({c["filters"][0] for c in cards if c["filters"]})


def _star_chart_cards() -> tuple[list[dict], list[str]]:
    """Star-chart nodes that grant an ability, keyed by constellation.

    A node's `Abilities` is prose and its `Ability_Values` the decoded numbers
    behind it - the buff is conditional, which is why those numbers sit apart from
    the node's passive `Stats` rather than being handed out permanently.
    """
    chart = _load("star_chart.json")
    nodes: list[dict] = []
    if not isinstance(chart, dict):
        return [], []

    def walk(node: dict) -> None:
        nodes.append(node)
        for child in node.get("Stars") or []:
            walk(child)

    for root in chart.values():
        walk(root)

    cards = []
    for node in nodes:
        text = " ".join(node.get("Abilities") or []).strip()
        if not text:
            continue
        constellation = node.get("Constellation") or ""
        chips = [{"label": constellation, "tone": "who"}]
        if node.get("Type"):
            chips.append({"label": node["Type"], "tone": "kind"})
        cards.append(_card(node.get("Name") or node.get("Path") or "", chips, text, "",
                           _effects({"stats": node.get("Ability_Values") or []}),
                           [constellation] if constellation else []))
    cards.sort(key=lambda c: (c["filters"][0] if c["filters"] else "", c["name"].lower()))
    return cards, sorted({c["filters"][0] for c in cards if c["filters"]})


def _ally_cards() -> tuple[list[dict], list[str]]:
    """Allies that do something beyond granting stats.

    1,200 of the 2,412 pet prefabs carry stats, but only 127 also carry an
    ability - and this is a page about abilities, so the flat-stat-only allies are
    left in ally_abilities.json rather than rendered. Filtering is by the stats an
    ally grants, which is what you would be shopping for.
    """
    cards = []
    stats_seen: set[str] = set()
    for ally in _load("ally_abilities.json"):
        if not ally.get("abilities"):
            continue
        granted = [s["name"] for s in ally.get("stats") or [] if not s["name"].startswith("$")]
        chips = [{"label": n, "tone": "kind"} for n in dict.fromkeys(granted)]
        stats_seen.update(granted)
        text = " ".join(a["text"] for a in ally["abilities"] if a.get("text")).strip()
        cards.append(_card(ally.get("name") or ally.get("slug") or "", chips, text, "",
                           _effects(ally), sorted(set(granted))))
    cards.sort(key=lambda c: c["name"].lower())
    return cards, sorted(stats_seen)


def abilities_view(active: str = "gems") -> dict[str, Any]:
    """`{tabs, active}` - every tab rendered, the client only switches them."""
    builders = (("gems", "Gems", "fa-solid fa-gem", _gem_cards),
                ("rings", "Rings", "fa-solid fa-ring", _ring_cards),
                ("classes", "Classes", "fa-solid fa-user", _class_cards),
                ("star-chart", "Star Chart", "fa-solid fa-star", _star_chart_cards),
                ("allies", "Allies", "fa-solid fa-paw", _ally_cards))
    tabs = []
    for key, label, icon, build in builders:
        cards, filters = build()
        tabs.append({"key": key, "label": label, "icon": icon,
                     "cards": cards, "filters": filters, "count": len(cards)})
    if active not in {t["key"] for t in tabs}:
        active = "gems"
    return {"tabs": tabs, "active": active}
