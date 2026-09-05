"""Server-side render model for /gem-abilities.

The page is a reference list, so it is rendered whole rather than fetched: every
ability is in the payload and the client only filters what is already there.

The source is ``gamedata/gem_abilities.json``, decoded from the game files by
``scripts/decode_gem_abilities.py``. That file has one row per (element, gem), and
the same ability ships in several elements - Stinging Curse is a Water, Fire and
Air gem - so rows are folded by name here: one card per ability, listing the
elements it comes in.
"""
import re
from typing import Any

from app.trove.gems.model import gem_lookups

# Column order in the filter row; matches how the game groups them.
ELEMENT_ORDER = ("Water", "Fire", "Air", "Cosmic", "Prismatic")

# The Masterful Prismatic Gem is a wildcard that fits any empowered slot; it grants
# no ability of its own, so it has no place on a page about what abilities do. It
# stays in gem_abilities.json - it is a real gem - and is dropped only here.
SKIP = {("Prismatic", "mastery")}


# "Empowered Air Gem for the Revenant. …" and, for the Prismatic gem which
# belongs to no class, "Empowered Prismatic Gem; can be socketed …".
_LEAD = re.compile(
    r"^Empowered\s+\w+\s+Gem\b(?:\s+for\s+(?:the\s+)?([^.;]+?))?\s*[.;]\s*(.*)$",
    re.S | re.I)


def _split_description(text: str) -> tuple[str, str]:
    """`("Revenant", "Bulwark Bash loses…")` from the game's own sentence.

    Every description opens by naming the element, which is already a chip on the
    card - and naming one element is wrong for an ability that ships in three - so
    the lead is lifted off and whoever it is `for` becomes the card's own label.
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


def _effects(gem: dict) -> list[dict]:
    """The decoded numbers as printable rows.

    `multiplier` scales the class's damage stat, so it reads as a percentage - the
    wire's 3.5 is 350%. A stat row prints its operation, because Add and
    MultiplySum on the same stat mean different things.
    """
    rows: list[dict] = []
    for stat in gem.get("stats") or []:
        op, value = stat.get("op", ""), stat.get("value") or 0
        if op == "Add":
            shown = f"+{_num(value)}"
        elif op == "Multiply":
            # `amount` is the raw multiplier; 1.5 is +50%, 0.85 is -15%.
            amount = stat.get("amount")
            shown = f"x{_num(amount)}" if amount else f"{_num(value)}%"
        else:
            shown = f"+{_num(value)}%"
        rows.append({"label": stat.get("name", ""), "value": shown, "kind": "stat"})
    for stage in gem.get("stages") or []:
        mult, base = stage.get("multiplier") or 0, stage.get("base") or 0
        if not mult and not base:
            continue
        shown = f"{_num(mult * 100)}%" if mult else ""
        rows.append({
            "label": stage.get("name", ""),
            "value": shown,
            "base": f"+{_num(base)}" if base else "",
            "kind": "damage",
        })
    return rows


def gem_abilities_view() -> dict[str, Any]:
    """`{abilities, elements, count}` - one card per distinct ability."""
    rows = gem_lookups().get("empowered_gems") or []

    folded: dict[str, dict] = {}
    for gem in rows:
        name = gem.get("name") or gem.get("slug") or ""
        if not name or (gem.get("element"), gem.get("slug")) in SKIP:
            continue
        applies_to, body = _split_description(gem.get("description") or "")
        entry = folded.setdefault(name, {
            "name": name,
            "applies_to": applies_to,
            "description": body,
            "elements": [],
            "tiers": [],
            "effects": [],
            "prefabs": [],
        })
        if gem.get("element") and gem["element"] not in entry["elements"]:
            entry["elements"].append(gem["element"])
        entry["tiers"] += gem.get("tiers") or []
        entry["description"] = entry["description"] or body
        entry["applies_to"] = entry["applies_to"] or applies_to
        for effect in _effects(gem):
            if effect not in entry["effects"]:
                entry["effects"].append(effect)
        for prefab in gem.get("prefabs") or []:
            if prefab not in entry["prefabs"]:
                entry["prefabs"].append(prefab)

    abilities = []
    for entry in folded.values():
        entry["elements"].sort(key=lambda e: ELEMENT_ORDER.index(e) if e in ELEMENT_ORDER else 99)
        tiers = sorted(set(entry["tiers"]))
        entry["tiers"] = tiers
        entry["tier_range"] = (f"T{tiers[0]}" if len(tiers) == 1
                               else f"T{tiers[0]}-T{tiers[-1]}") if tiers else ""
        # The filter matches on this, so keep it lowercase and pre-joined.
        entry["search"] = " ".join([entry["name"], entry["description"],
                                    entry["applies_to"], *entry["elements"]]).lower()
        abilities.append(entry)

    abilities.sort(key=lambda a: a["name"].lower())
    used = [e for e in ELEMENT_ORDER if any(e in a["elements"] for a in abilities)]
    return {"abilities": abilities, "elements": used, "count": len(abilities)}
