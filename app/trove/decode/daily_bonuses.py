"""daily_bonuses.json - the Daily Bonus for each day of the week (wiki /daily-bonuses).

Three files make the welcome screen's Daily Bonus window:

- `prefabs/loot/dailybonus.binfab` (root 0) holds one row per day: {0 day index,
  Monday first; 1 the weekday text key `$Welcome_DailyBonus_Monday`; 8 effects
  `{0 kind, 1 params}`}. Kinds seen: `stats` (params 0 = stat modifier records),
  `pvpexperience` (0 = multiplier) and `claim` (0 = multiplier, 1 claim id, the
  reward table it multiplies). Fields 3, 4 and 7 name server loot tables and are
  left out.
- `ui/welcomescreen.swf`: each day's panel sprite holds its weekday text and its
  title text (`$Welcome_DailyBonus_ShadowDay` = "Delve Day"), which joins a row to
  its title. `WelcomeDailyBonusWindow.BONUSES` (Monday first) names the day's text
  set: `$Welcome_daily_bonus_details_<set>_<1..4>`, plus `_patron` for Patrons.
  `DAYS_L` gives the weekday names.
- `languages/en/`: the text.

The listed bonuses are the game's own text; most of them (drop rates, harvest and
ore chances) are applied by the server and appear in no file. The effects are
what the file itself applies.
"""
from __future__ import annotations

import re
from typing import Any

from app.trove.decode.ability import modifiers, walk_values
from app.trove.decode.badges import Text
from app.trove.decode.tree import GameTree
from app.trove.decode.wire import Obj, parse
from app.trove.swf.abc import static_arrays
from app.trove.swf.extract import _decompress, _iter_tags, _sprite_child_refs

TITLE = "Daily bonuses"
OUTPUT = "daily_bonuses.json"
PREFIXES = ("prefabs/loot/", "ui/", "languages/en/")
INDENT, FINAL_NEWLINE = 1, True

TABLE = "prefabs/loot/dailybonus.binfab"
SWF = "ui/welcomescreen.swf"
WINDOW = "WelcomeDailyBonusWindow"
D_DAY, D_WEEKDAY, D_EFFECTS = 0, 1, 8
E_KIND, E_PARAMS = 0, 1
DEFINE_EDIT_TEXT, DEFINE_SPRITE = 37, 39
_KEY = re.compile(rb"\$Welcome_DailyBonus_[A-Za-z0-9_]+")
DETAIL_LINES = 4


def _leaf(v: Any) -> dict:
    return v.leaf if isinstance(v, Obj) else v if isinstance(v, dict) else {}


def _titles(swf: bytes) -> dict[str, str]:
    """Weekday text key -> the title key its panel sprite shows beside it."""
    texts: dict[int, str] = {}
    sprites: list[list[int]] = []
    for code, body in _iter_tags(_decompress(swf)):
        if code == DEFINE_EDIT_TEXT and (m := _KEY.search(body)):
            texts[int.from_bytes(body[:2], "little")] = m.group().decode()
        elif code == DEFINE_SPRITE:
            sprites.append(_sprite_child_refs(body))
    out: dict[str, str] = {}
    for kids in sprites:
        keys = [texts[k] for k in kids if k in texts]
        titles = [k for k in keys if k.endswith("Day")]
        for weekday in (k for k in keys if not k.endswith("Day")):
            if len(titles) == 1:
                out[weekday] = titles[0]
    return out


def _effects(raw: Any) -> tuple[list[dict], list[dict]]:
    """(stat modifiers, multipliers) from a day's effect rows."""
    stats: list[dict] = []
    multipliers: list[dict] = []
    for row in raw if isinstance(raw, list) else []:
        e = _leaf(row)
        kind, params = e.get(E_KIND), _leaf(e.get(E_PARAMS))
        if kind == "stats":
            stats += [m for m in modifiers(walk_values(params.get(0))) if m not in stats]
        elif kind == "pvpexperience" and isinstance(params.get(0), (int, float)):
            multipliers.append({"kind": "pvpexperience", "multiplier": params[0]})
        elif kind == "claim" and isinstance(params.get(1), str):
            multipliers.append({"kind": "claim", "claim": params[1], "multiplier": params.get(0)})
    return stats, multipliers


def build(tree: GameTree) -> dict:
    swf = tree.read(SWF)
    if not swf:
        raise ValueError(f"{SWF} missing")
    arrays = static_arrays(swf).get(WINDOW, {})
    sets, weekdays = arrays.get("BONUSES") or [], arrays.get("DAYS_L") or []
    if len(sets) != 7 or len(weekdays) != 7:
        raise ValueError(f"{WINDOW}.BONUSES/DAYS_L not found")
    titles = _titles(swf)
    text = Text(tree)
    days = []
    for row in (parse(tree.read(TABLE) or b"").root or Obj()).get(0) or []:
        r = _leaf(row)
        day, key = r.get(D_DAY), r.get(D_WEEKDAY)
        if not isinstance(day, int) or not 0 <= day < 7:
            continue
        lines = [f"$Welcome_daily_bonus_details_{sets[day]}_{i}" for i in range(1, DETAIL_LINES + 1)]
        stats, multipliers = _effects(r.get(D_EFFECTS))
        days.append({
            "day": day,
            "weekday": weekdays[day],
            "name": text(titles.get(key, "") if isinstance(key, str) else ""),
            "set": sets[day],
            "bonuses": [t for t in (text(k) for k in lines) if t],
            "patron_bonuses": [t for t in (text(f"{k}_patron") for k in lines) if t],
            "stats": stats,
            "multipliers": multipliers,
        })
    return {"days": sorted(days, key=lambda d: d["day"])}


def count(data: dict) -> int:
    return len(data["days"])
