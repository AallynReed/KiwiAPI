"""Gem Builds - recommended gem proc layouts ranked by damage coefficient.

Ported from BetterTroveTools `utils/gem_engine.py` (`StarChartParser` +
`GemOptimizerEngine`) and `models/trove/builds.py`. Reads the build data files
under `gamedata/` and works on plain class dicts (from `classes.json`) rather
than re-creating the original enum/Pydantic class model.
"""

from __future__ import annotations

import base64
import itertools
import json
import re
from functools import cache, lru_cache
from pathlib import Path
from typing import Any

_DATA_DIR = Path(__file__).parent.parent / "gamedata"

BUILD_TYPES = ("Light", "Farm", "Health")
# damage_type display name -> the builds/ subfolder name for dragons_damage.
_DMG_FOLDER = {"Physical Damage": "physical_damage", "Magic Damage": "magic_damage"}

# Blessing of the Lilypad - the ally buff. Ally stat values in builds/ally.json
# are already the level-30 numbers (Scorpius 700 Light / 36.25% PD = its base
# 400/25% scaled by the L30 multipliers), so the buff multiplies those directly.
# Measured per stat class, not per ally: two allies gave identical ratios.
# Damage and Critical Damage share one 31% class; Light is its own 15.5%.
# Power Rank takes nothing from the buff. Stability and Movement Speed have no
# measured multiplier yet, so an ally granting those stays unbuffed, not guessed.
LILYPAD_MULTIPLIERS = {
    "Light": 1.155,
    "Physical Damage": 1.31,
    "Magic Damage": 1.31,
    "Critical Damage": 1.31,
}


def _lilypad(name: str, value: float, active: bool) -> float:
    """An ally's L30 stat value, with the Lilypad buff applied when active."""
    return value * LILYPAD_MULTIPLIERS.get(name, 1.0) if active else value


@cache
def _load(relative: str) -> Any:
    path = _DATA_DIR / relative
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (FileNotFoundError, json.JSONDecodeError):
        return {}


def _stat_value(stats: list[dict], name: str) -> float:
    for stat in stats:
        if stat.get("name") == name:
            return stat.get("value") or 0.0
    return 0.0


class StarChartParser:
    """Decodes a star-chart build code into aggregated passive stats."""

    COMPACT_CODE_PREFIX = "SC:"
    ROOT_TO_ABBREV = {"combat": "c", "gathering": "g", "pve": "p"}
    ABBREV_TO_ROOT = {v: k for k, v in ROOT_TO_ABBREV.items()}

    def __init__(self, star_chart_raw_data: dict):
        self.node_map: dict[str, dict] = {}
        self.parent_map: dict[str, str | None] = {}
        if star_chart_raw_data:
            for constell in star_chart_raw_data.values():
                self._flatten_tree(constell)
        self.selectable_paths = sorted(
            path for path, node in self.node_map.items() if node.get("Type") != "Root"
        )

    def _flatten_tree(self, node: dict, parent_path: str | None = None) -> None:
        if "Path" in node:
            self.node_map[node["Path"]] = node
            self.parent_map[node["Path"]] = parent_path
        for child in node.get("Stars", []):
            self._flatten_tree(child, node.get("Path"))

    def _expand_terminal_path(self, path: str) -> set[str]:
        expanded: set[str] = set()
        current = path
        while current and current in self.node_map:
            node = self.node_map[current]
            if node.get("Type") == "Root" or current in expanded:
                break
            expanded.add(current)
            parent = self.parent_map.get(current)
            if not parent or self.node_map.get(parent, {}).get("Type") == "Root":
                break
            current = parent
        return expanded

    def _decode_compact_path(self, token: str) -> str | None:
        compact = str(token or "").strip().lower()
        if not compact:
            return None
        root_name = self.ABBREV_TO_ROOT.get(compact[0])
        if not root_name:
            return None
        segments = re.findall(r"[a-z]+|\d+", compact[1:])
        path = ".".join([root_name, *segments])
        node = self.node_map.get(path)
        if not node or node.get("Type") == "Root":
            return None
        return path

    def _decode_build_code(self, build_code: str) -> set[str]:
        compact = str(build_code or "").strip()
        if not compact:
            return set()
        if compact.startswith((self.COMPACT_CODE_PREFIX, "v2:")):
            selected: set[str] = set()
            payload = compact.split(":", 1)[1]
            if "|" in payload:
                for token in payload.split("|"):
                    path = self._decode_compact_path(token)
                    if path:
                        selected.update(self._expand_terminal_path(path))
                return selected
            padded = payload + ("=" * ((4 - len(payload) % 4) % 4))
            for node_id in base64.urlsafe_b64decode(padded.encode("utf-8")):
                if 0 <= node_id < len(self.selectable_paths):
                    selected.update(self._expand_terminal_path(self.selectable_paths[node_id]))
            return selected
        decoded = base64.b64decode(compact).decode("utf-8")
        return {p for p in decoded.split("$") if p in self.node_map}

    def parse_build_code(self, build_code: str) -> dict:
        result = {"stats": {}, "abilities": [], "paths_count": 0}
        if not build_code or not self.node_map:
            return result
        try:
            selected_paths = self._decode_build_code(build_code)
        except Exception:
            return result
        result["paths_count"] = len(selected_paths)

        overwrites: set[str] = set()
        for path in selected_paths:
            node = self.node_map.get(path)
            if node and "Overwrites" in node:
                overwrites.update(node["Overwrites"])
        active_paths = selected_paths - overwrites

        abilities: set[str] = set()
        for path in active_paths:
            node = self.node_map.get(path)
            if not node:
                continue
            for stat in node.get("Stats", []):
                name = stat.get("name")
                if not name:
                    continue
                try:
                    val = float(stat.get("value", 0) or 0)
                except (TypeError, ValueError):
                    val = 0.0
                bucket = result["stats"].setdefault(name, {"flat": 0.0, "pct": 0.0})
                bucket["pct" if stat.get("percentage", False) else "flat"] += val
            if "Abilities" in node:
                abilities.update(node["Abilities"])
        result["abilities"] = list(abilities)
        return result


class GemOptimizerEngine:
    def __init__(self):
        self.classes_data = _load("classes.json")
        self.foods = _load("builds/food.json")
        self.allies = _load("builds/ally.json")
        face = _load("builds/face_damage.json")
        self.face_damage = face.get("Face", 0) if isinstance(face, dict) else 0
        self.gem_stats = _load("mystic.json")
        self.star_parser = StarChartParser(_load("star_chart.json"))
        self.classes = {c["name"]: c for c in self.classes_data} if self.classes_data else {}

    def sum_file_values(self, relative: str) -> float:
        data = _load(f"builds/{relative}.json")
        return sum(data.values()) if isinstance(data, dict) and data else 0.0

    def generate_combinations(self, farm: bool = False):
        first_set = [[i, 9 - i] for i in range(10)]
        second_set = [[i, 18 - i] for i in range(19)]
        third_set = [[x, y, z] for x in range(4) for y in range(4) for z in range(4)
                     if x + y + z == 3 and (z == 3 if not farm else True)]
        fourth_set = [[x, y, z] for x in range(7) for y in range(7) for z in range(7)
                      if x + y + z == 6 and (z == 6 if not farm else True)]
        return itertools.product(first_set, second_set, third_set, fourth_set)

    def calculate_gem_stats(self, build_type: str, build) -> tuple[float, float, float]:
        if not self.gem_stats:
            return 0, 0, 0
        if build_type == "Health":
            first_l, first_e = self.gem_stats["Lesser"]["Maximum Health"], self.gem_stats["Empowered"]["Maximum Health"]
            second_l, second_e = self.gem_stats["Lesser"]["Maximum Health %"], self.gem_stats["Empowered"]["Maximum Health %"]
        else:
            first_l, first_e = self.gem_stats["Lesser"]["Damage"], self.gem_stats["Empowered"]["Damage"]
            second_l, second_e = self.gem_stats["Lesser"]["Critical Damage"], self.gem_stats["Empowered"]["Critical Damage"]
        third_l, third_e = self.gem_stats["Lesser"]["Light"], self.gem_stats["Empowered"]["Light"]

        first = 3 * first_e[0] + 6 * first_l[0]
        second = 3 * second_e[0] + 6 * second_l[0]
        third = 1 * third_e[0] + 2 * third_l[0]
        cosmic_first = 1 * first_e[0] + 2 * first_l[0]
        cosmic_second = 1 * second_e[0] + 2 * second_l[0]

        first += first_e[1] * build[0][0]
        second += second_e[1] * build[0][1]
        first += first_l[1] * build[1][0]
        second += second_l[1] * build[1][1]
        cosmic_first += first_e[1] * build[2][0]
        cosmic_second += second_e[1] * build[2][1]
        third += third_e[1] * build[2][2]
        cosmic_first += first_l[1] * build[3][0]
        cosmic_second += second_l[1] * build[3][1]
        third += third_l[1] * build[3][2]

        return (first + cosmic_first) * 1.1, (second + cosmic_second) * 1.1, third * 1.1

    def calculate_builds(self, config: dict) -> list[dict]:
        if not self.classes:
            raise BuildError("Class data not loaded.")
        character = config["character"]
        subclass = config["subclass"]
        selected_class = self.classes.get(character)
        if selected_class is None:
            raise BuildError(f"Unknown class '{character}'.")
        if subclass not in self.classes:
            raise BuildError(f"Unknown subclass '{subclass}'.")

        build_type = config.get("build_type", "Light")
        ally = config.get("ally", "boot_clown")
        food = config.get("food", "")
        critical_damage_count = int(config.get("critical_damage_count", 3))
        light_target = int(config.get("light", 0))

        damage_type = "Magic Damage" if selected_class.get("damage_type") == "Magic" else "Physical Damage"

        if build_type == "Health":
            first = self.sum_file_values("health") + _stat_value(selected_class["stats"], "Maximum Health")
            second = self.sum_file_values("health_per") + _stat_value(selected_class["stats"], "Maximum Health %")
            third, fourth, fifth, sixth = 0, 0, 100, 100
            damage_type = "Maximum Health"
        else:
            first = self.sum_file_values("damage")
            second = self.sum_file_values("critical_damage")
            third = self.sum_file_values("light")
            fourth = self.sum_file_values("bonus_damage")
            fifth, sixth = 100, 100

            first += _stat_value(selected_class["stats"], damage_type)
            second += _stat_value(selected_class["stats"], "Critical Damage")
            if not config.get("no_face"):
                first += self.face_damage
            first += self.sum_file_values(f"{_DMG_FOLDER[damage_type]}/dragons_damage") + self.sum_file_values("dragons_damage")
            second += self.sum_file_values("dragons_critical_damage")

            if food and food in self.foods:
                for stat in self.foods[food].get("stats", []):
                    if stat["name"] == damage_type:
                        fourth += stat["value"] if stat["percentage"] else 0
                        first += 0 if stat["percentage"] else stat["value"]
                    if stat["name"] == "Critical Damage":
                        second += stat["value"]
                    if stat["name"] == "Light":
                        third += stat["value"]

            if ally and ally in self.allies:
                lilypad = bool(config.get("ally_buff", True))
                for stat in self.allies[ally].get("stats", []):
                    value = _lilypad(stat["name"], stat["value"], lilypad)
                    if stat["name"] == damage_type:
                        fourth += value if stat["percentage"] else 0
                        first += 0 if stat["percentage"] else value
                    if stat["name"] == "Critical Damage":
                        fifth += value if stat["percentage"] else 0
                        second += 0 if stat["percentage"] else value
                    if stat["name"] == "Light":
                        third += value

            second -= 48.1 * (3 - critical_damage_count)
            if "Solarion" in (character, subclass):
                third += 140
            if damage_type == "Physical Damage" and subclass == "Lunar Lancer":
                first += 750
            if damage_type == "Magic Damage" and subclass in ("Ice Sage", "Shadow Hunter"):
                first += 750
            if subclass in ("Bard", "Boomeranger"):
                second += 20
            if config.get("subclass_active"):
                if subclass == "Bard":
                    fourth += 45
                    second += 45
                if subclass == "Gunslinger":
                    fourth += 5.5
                if subclass in ("Lunar Lancer", "Candy Barbarian"):
                    fourth += 30
            if config.get("berserker_battler"):
                third += 750
            if config.get("litany"):
                sixth += 1

            if config.get("star_chart"):
                chart_stats = self.star_parser.parse_build_code(config["star_chart"])["stats"]
                dmg = chart_stats.get(damage_type, {})
                crit = chart_stats.get("Critical Damage", {})
                light = chart_stats.get("Light", {})
                first += dmg.get("flat", 0)
                second += crit.get("flat", 0)
                third += light.get("flat", 0)
                fourth += dmg.get("pct", 0)
                fifth += crit.get("pct", 0)
                sixth += light.get("pct", 0)

        class_bonus = next((b["value"] for b in selected_class["bonuses"] if b["name"] == damage_type), None)

        # Rankings are decided several decimals below the display rounding, so
        # high precision widens every field to 8 places rather than 1-2.
        precise = bool(config.get("high_precision"))

        def rd(value: float, digits: int) -> float:
            return round(value, 8 if precise else digits)

        raw_builds = []
        for build_tuple in self.generate_combinations(farm=build_type == "Farm"):
            build = list(build_tuple)
            gem_first, gem_second, gem_third = self.calculate_gem_stats(build_type, build)
            cfirst = first + gem_first
            csecond = second + gem_second
            cthird = third + gem_third
            final = cfirst * (1 + fourth / 100)
            if class_bonus is not None:
                final *= 1 + (class_bonus / 100)
            coefficient = rd(final * (1 + (csecond * (fifth / 100)) / 100), 2)
            raw_builds.append([build, cfirst, csecond, rd(cthird * (sixth / 100), 2), fourth, fifth, final, class_bonus, coefficient])

        raw_builds.sort(key=lambda x: ([abs(x[3] - light_target), -x[-1]] if light_target else -x[-1]))

        results = []
        for i, build_data in enumerate(raw_builds[:200]):
            boosts: list[int] = []
            for arr in build_data[0]:
                boosts.extend(arr)
            if not light_target or (light_target and build_type == "Health"):
                del boosts[9]
                del boosts[6]
            if not light_target and build_type != "Health":
                boosts = boosts[:4]
            build_text = "/".join(str(x) for x in boosts[:4])
            if len(boosts) > 4:
                build_text += " + " + "/".join(str(x) for x in boosts[4:])
            results.append({
                "rank": i + 1, "layout": build_text, "base_dmg": rd(build_data[1], 2),
                "crit_dmg": rd(build_data[2], 1), "light": build_data[3], "bonus_dmg": rd(build_data[4], 8),
                "total_dmg": rd(build_data[6], 2), "class_bonus": build_data[7], "coefficient": build_data[8],
            })
        return results


class BuildError(ValueError):
    """Raised on invalid build config (mapped to 400 at the router)."""


@lru_cache(maxsize=1)
def _engine() -> GemOptimizerEngine:
    return GemOptimizerEngine()


def calculate_builds(config: dict) -> list[dict]:
    """Top-200 gem proc layouts for a build config, ranked by damage coefficient."""
    return _engine().calculate_builds(config)


def parse_star_chart(code: str) -> dict:
    """Decode a star-chart build code into aggregated passive stats (for UI previews).

    Returns ``{"stats": {name: {"flat", "pct"}}, "abilities": [...], "paths_count": N}``;
    ``paths_count == 0`` signals an unparseable / empty code.
    """
    return _engine().star_parser.parse_build_code(code or "")


def build_options() -> dict:
    """Valid field values a client can pick for a build config."""
    eng = _engine()
    classes = [c["name"] for c in eng.classes_data] if eng.classes_data else []
    foods = [{"key": k, "label": v.get("qualified_name", k)} for k, v in eng.foods.items()]
    allies = [{"key": "boot_clown", "label": "No ally"}] + [
        {"key": k, "label": v.get("qualified_name", k)} for k, v in eng.allies.items() if k != "boot_clown"
    ]
    return {
        "build_type": list(BUILD_TYPES),
        "character": classes,                       # same list is valid for character AND subclass
        "ally": allies,
        "food": foods,
        "critical_damage_count": {"min": 0, "max": 3, "default": 3},
        "flags": ["berserker_battler", "no_face", "subclass_active", "litany", "ally_buff", "high_precision"],
        "notes": {
            "subclass": "Same options as character.",
            "ally": "Use 'boot_clown' for no ally. Ally stats are the level-30 values.",
            "ally_buff": (
                "Blessing of the Lilypad, on by default: multiplies the ally's level-30 "
                "Light by 1.155 and its Physical/Magic/Critical Damage by 1.31. Ally stats only."
            ),
            "food": "Use \"\" for none.",
            "high_precision": "Round every result field to 8 decimals instead of 1-2.",
            "light": "Farm builds only - target base light; 0 disables light targeting.",
            "star_chart": "Optional star-chart build code (SC:/v2: compact, or base64).",
        },
    }
