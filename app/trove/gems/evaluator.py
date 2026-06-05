"""Gem evaluator — score a typed-in gem and cost out perfecting it.

Ported from BetterTroveTools `backend/gems_and_builds/gem_evaluator.py`. Pure
calculation: given a tier/type/level and 3 stats (type + entered value + proc
count), it returns quality %, estimated Power Rank, per-stat progress, and the
focus-material plan to reach 100%.
"""

import math
from itertools import product

from .bases import (
    get_empowered_gem_pr_threshold,
    get_increment_power_rank_empowered,
    get_increment_power_rank_lesser,
    get_lesser_gem_pr_threshold,
    get_stat_base_empowered,
    get_stat_base_lesser,
    get_stat_threshold_empowered,
    get_stat_threshold_lesser,
)
from .constants import GemElement, GemRestriction, GemStatType, GemTier, GemType

# Rounded game values can sit a few hundredths outside the theoretical [0,1].
PROGRESS_TOLERANCE = 0.05

FOCUS_OPTIONS = {
    "optimized_all": {"label": "Superior + Precise + Rough", "values": [12.5, 5.0, 2.5],
                      "keys": ["superior", "precise", "rough"]},
    "optimized_precise_rough": {"label": "Precise + Rough", "values": [5.0, 2.5],
                                "keys": ["precise", "rough"]},
    "rough_only": {"label": "Rough Only", "values": [2.5], "keys": ["rough"]},
}

FOCUS_RECIPES = {
    "rough": {"label": "Rough Focus",
              "materials": {"bound_brilliance": 1, "heart_of_darkness": 4, "flux": 1200}},
    "precise": {"label": "Precise Focus",
                "materials": {"bound_brilliance": 1, "water_gem_dust": 3000, "air_gem_dust": 3000,
                              "fire_gem_dust": 3000, "flux": 2000}},
    "superior": {"label": "Superior Focus",
                 "materials": {"bound_brilliance": 1, "diamond_dragonite": 30, "titan_soul": 3,
                               "flux": 50000}},
}


def _gem_pr_increment_total(gem_tier: GemTier, gem_type: GemType, level: int) -> int:
    func = get_increment_power_rank_lesser if gem_type == GemType.LESSER else get_increment_power_rank_empowered
    return sum(func(gem_tier, lvl) for lvl in range(1, level + 1))


def _infer_element(stat_types: list[GemStatType]) -> GemElement:
    return GemElement.COSMIC if GemStatType.LIGHT in stat_types else GemElement.WATER


def _infer_restriction(stat_types: list[GemStatType]) -> GemRestriction | None:
    if GemStatType.PHYSICAL_DAMAGE in stat_types:
        return GemRestriction.FIERCE
    if GemStatType.MAGIC_DAMAGE in stat_types:
        return GemRestriction.ARCANE
    return None


def _calculate_focus_counts(remaining_percent: float, option_key: str) -> dict:
    option = FOCUS_OPTIONS[option_key]
    counts: dict[str, int] = dict.fromkeys(("superior", "precise", "rough"), 0)
    remaining_units = max(0, math.ceil((remaining_percent - 1e-9) / 2.5))
    unit_values = [int(round(value / 2.5)) for value in option["values"]]
    for focus_key, unit_value in zip(option["keys"], unit_values, strict=False):
        counts[focus_key] = remaining_units // unit_value
        remaining_units %= unit_value
    if remaining_units > 0:
        counts[option["keys"][-1]] += math.ceil(remaining_units / unit_values[-1])
    counts["total"] = counts["superior"] + counts["precise"] + counts["rough"]
    return counts


def _build_focus_plan(display_progress: float, containers: int) -> dict:
    current_percent = round(display_progress * 100, 2)
    remaining_percent = max(0.0, 100.0 - current_percent)
    per_container = {k: _calculate_focus_counts(remaining_percent, k) for k in FOCUS_OPTIONS}
    totals = {}
    for option_key, option in FOCUS_OPTIONS.items():
        cc = per_container[option_key]
        total_counts = {
            "superior": cc["superior"] * containers, "precise": cc["precise"] * containers,
            "rough": cc["rough"] * containers, "total": cc["total"] * containers,
        }
        recipe_totals: dict[str, int] = {}
        for focus_key in ("superior", "precise", "rough"):
            count = total_counts[focus_key]
            if count <= 0:
                continue
            for material_key, amount in FOCUS_RECIPES[focus_key]["materials"].items():
                recipe_totals[material_key] = recipe_totals.get(material_key, 0) + amount * count
        totals[option_key] = {"label": option["label"], **total_counts, "recipe_totals": recipe_totals}
    return {"current_percent": current_percent, "remaining_percent": round(remaining_percent, 2),
            "per_container": per_container, "totals": totals}


def _evaluate_gem_candidate(gem_tier, gem_type, level, stats_payload) -> dict:
    stat_types = [GemStatType(s["type"]) for s in stats_payload]
    element = _infer_element(stat_types)
    restriction = _infer_restriction(stat_types)
    pr_increments_total = _gem_pr_increment_total(gem_tier, gem_type, level)
    estimated_power_rank = (100 if gem_type == GemType.EMPOWERED else 0) + pr_increments_total * 3
    pr_thresholds = (
        get_lesser_gem_pr_threshold(gem_tier, element) if gem_type == GemType.LESSER
        else get_empowered_gem_pr_threshold(gem_tier, element)
    )
    total_container_progress = 0.0
    total_containers = 0
    per_stat = []
    issues: list[str] = []

    for stat_payload in stats_payload:
        stat_type = GemStatType(stat_payload["type"])
        stat_value = float(stat_payload["value"])
        extra_containers = int(stat_payload["extra_containers"])
        containers = 1 + extra_containers

        if gem_type == GemType.LESSER:
            stat_base = get_stat_base_lesser(gem_tier, element, stat_type)
            thresholds = get_stat_threshold_lesser(gem_tier, element, stat_type)
        else:
            stat_base = get_stat_base_empowered(gem_tier, element, stat_type)
            thresholds = get_stat_threshold_empowered(gem_tier, element, stat_type)

        threshold_progress = ((stat_value / stat_base) - pr_increments_total) / containers
        raw_progress = (threshold_progress - thresholds[0]) / (thresholds[1] - thresholds[0])
        display_progress = max(0.0, min(1.0, raw_progress))
        pr_contribution = (pr_thresholds[0] + (pr_thresholds[1] - pr_thresholds[0]) * display_progress) * containers

        estimated_power_rank += pr_contribution
        total_container_progress += display_progress * containers
        total_containers += containers

        per_stat.append({
            "type": stat_type.value, "display_name": stat_type.display_name,
            "entered_value": stat_value, "extra_containers": extra_containers, "containers": containers,
            "progress": round(display_progress, 4), "quality_percent": round(display_progress * 100, 2),
            "estimated_pr_contribution": round(pr_contribution, 2),
            "is_within_range": -PROGRESS_TOLERANCE <= raw_progress <= 1 + PROGRESS_TOLERANCE,
            "raw_progress": round(raw_progress, 4), "threshold_progress": round(threshold_progress, 4),
            "focus_plan": _build_focus_plan(display_progress, containers),
        })
        if raw_progress < -PROGRESS_TOLERANCE:
            issues.append(f"{stat_type.display_name} is below the minimum possible value for this level and proc spread.")
        elif raw_progress > 1 + PROGRESS_TOLERANCE:
            issues.append(f"{stat_type.display_name} is above the maximum possible value for this level and proc spread.")

    overall_quality = (total_container_progress / total_containers) if total_containers else 0.0
    gem_focus_totals = {}
    for option_key, option in FOCUS_OPTIONS.items():
        superior = sum(s["focus_plan"]["totals"][option_key]["superior"] for s in per_stat)
        precise = sum(s["focus_plan"]["totals"][option_key]["precise"] for s in per_stat)
        rough = sum(s["focus_plan"]["totals"][option_key]["rough"] for s in per_stat)
        recipe_totals: dict[str, int] = {}
        for s in per_stat:
            for material_key, amount in s["focus_plan"]["totals"][option_key]["recipe_totals"].items():
                recipe_totals[material_key] = recipe_totals.get(material_key, 0) + amount
        gem_focus_totals[option_key] = {
            "key": option_key, "label": option["label"], "superior": superior, "precise": precise,
            "rough": rough, "total": superior + precise + rough, "recipe_totals": recipe_totals,
        }
    headline = dict(gem_focus_totals["optimized_precise_rough"])
    headline["strategy"] = "optimized_precise_rough"

    return {
        "type": gem_type.value, "type_name": gem_type.display_name,
        "element": element.value, "element_name": element.display_name,
        "restriction": restriction.value if restriction else None,
        "restriction_name": restriction.display_name if restriction else "Any",
        "quality": round(overall_quality, 4), "quality_percent": round(overall_quality * 100, 2),
        "calculated_power_rank": round(estimated_power_rank), "has_issues": bool(issues),
        "issues": issues, "focus_totals": gem_focus_totals, "headline_cost": headline,
        "stats": per_stat, "level": level, "tier": gem_tier.value,
    }


def _build_candidate_with_distribution(gem_tier, gem_type, level, stats_payload, distribution) -> dict:
    normalized = []
    for stat_payload, extra in zip(stats_payload, distribution, strict=False):
        item = dict(stat_payload)
        item["extra_containers"] = extra
        normalized.append(item)
    return _evaluate_gem_candidate(gem_tier, gem_type, level, normalized)


def _distribution_score(candidate: dict) -> float:
    score = 0.0
    for stat in candidate["stats"]:
        raw = stat["raw_progress"]
        clamped = max(0.0, min(1.0, raw))
        score += (raw - clamped) ** 2
    progresses = [s["progress"] for s in candidate["stats"]]
    score += (max(progresses) - min(progresses)) ** 2 * 0.25
    return round(score, 9)


def _guess_distribution(gem_tier, gem_type, level, stats_payload) -> list[int]:
    available = min(level, 15) // 5
    best_distribution = None
    best_score = None
    for distribution in product(range(4), repeat=3):
        if sum(distribution) != available:
            continue
        candidate = _build_candidate_with_distribution(gem_tier, gem_type, level, stats_payload, list(distribution))
        score = _distribution_score(candidate)
        if best_score is None or score < best_score:
            best_score = score
            best_distribution = list(distribution)
    return best_distribution or [0, 0, 0]


class GemEvaluatorError(ValueError):
    """Raised on invalid evaluator input (mapped to 400 at the router)."""


def evaluate_gem(tier: int, type: int, level: int, stats: list[dict], auto_guess_procs: bool = False) -> dict:
    """Evaluate a gem. `stats` = 3 dicts of {type, value, extra_containers}.

    Returns the candidate evaluation plus the proc distribution used.
    """
    gem_tier = GemTier(int(tier))
    gem_type = GemType(int(type))
    level = int(level)
    if len(stats) != 3:
        raise GemEvaluatorError("Exactly 3 stats are required.")

    available = min(level, 15) // 5
    selected: list[GemStatType] = []
    extra_total = 0
    payload = [dict(s) for s in stats]
    for stat in payload:
        stat_type = GemStatType(int(stat.get("type", 0)))
        if stat_type in selected:
            raise GemEvaluatorError("Gem stats must be unique.")
        selected.append(stat_type)
        extra = int(stat.get("extra_containers", 0))
        if extra < 0:
            raise GemEvaluatorError("Extra containers cannot be negative.")
        extra_total += extra
    if GemStatType.PHYSICAL_DAMAGE in selected and GemStatType.MAGIC_DAMAGE in selected:
        raise GemEvaluatorError("Physical Damage and Magic Damage cannot be on the same gem.")

    if auto_guess_procs or extra_total != available:
        guessed = _guess_distribution(gem_tier, gem_type, level, payload)
        for i, extra in enumerate(guessed):
            payload[i]["extra_containers"] = extra
    else:
        guessed = [int(s.get("extra_containers", 0)) for s in payload]

    candidate = _evaluate_gem_candidate(gem_tier, gem_type, level, payload)
    return {"result": candidate, "available_extra_containers": available, "guessed_distribution": guessed}


def gem_stat_range(tier: int, type: int, stat_type: int, level: int = 1,
                   extra_containers: int = 0, element: int | None = None) -> dict:
    """The plausible (min, max) value a stat can roll at, for inline UI hints."""
    gem_tier = GemTier(int(tier))
    gem_type = GemType(int(type))
    st = GemStatType(int(stat_type))
    level = max(1, int(level))
    containers = 1 + max(0, int(extra_containers))
    elem = GemElement(int(element)) if element is not None else _infer_element([st])
    pr_increments_total = _gem_pr_increment_total(gem_tier, gem_type, level)
    if gem_type == GemType.LESSER:
        stat_base = get_stat_base_lesser(gem_tier, elem, st)
        thresholds = get_stat_threshold_lesser(gem_tier, elem, st)
    else:
        stat_base = get_stat_base_empowered(gem_tier, elem, st)
        thresholds = get_stat_threshold_empowered(gem_tier, elem, st)
    return {
        "stat_type": st.value, "stat_display_name": st.display_name, "element": elem.value,
        "containers": containers,
        "min_value": stat_base * (thresholds[0] * containers + pr_increments_total),
        "max_value": stat_base * (thresholds[1] * containers + pr_increments_total),
        "stat_base": stat_base, "thresholds": list(thresholds),
    }
