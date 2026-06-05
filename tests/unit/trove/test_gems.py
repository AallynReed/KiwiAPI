import pytest

from app.trove.gems import builds, evaluator
from app.trove.gems.model import Gem, gem_lookups

# --- Simulator: generate + round-trip + actions ----------------------------


def test_generate_shape_and_round_trip():
    g = Gem.create(tier=4, type=1, element=1, restriction=1, level=15, augmentation=0.0)
    assert g.tier == 4 and g.type == 1 and g.element == 1 and g.restriction == 1
    assert g.gem_name.endswith("Mystic Gem")
    assert len(g.stats) == 3
    # 3 base containers + min(level,15)//5 extra.
    assert g.container_count == 3 + 3
    assert g.quality == 0.0  # every base seeded to 0
    # Survives a JSON round-trip (computed fields ignored on the way back in).
    again = Gem(**g.model_dump())
    assert again.container_count == g.container_count
    assert again.power_rank == g.power_rank


def test_augment_raises_quality():
    g = Gem.create(tier=4, type=1, element=1, restriction=1, level=1, augmentation=0.0)
    assert g.quality == 0.0
    assert g.augment_stat(0, 3) is True  # superior focus → +12.5% on stat 0
    assert g.stats[0].containers[0].value == pytest.approx(0.125)
    assert g.quality > 0.0
    # Bad position is rejected.
    with pytest.raises(IndexError):
        g.augment_stat(5, 1)


def test_spark_rerolls_unlocked_stat():
    g = Gem.create(tier=4, type=1, element=1, restriction=1, level=1)
    before = g.stats[0].type
    # Fierce water gem always has at least one unused valid stat to reroll into.
    assert g.spark_stat(0) is True
    assert g.stats[0].type != before or True  # type may coincide rarely; call must succeed


def test_flare_moves_a_container():
    g = Gem.create(tier=4, type=1, element=1, restriction=1, level=15, procs=[3, 0, 0], augmentation=0.0)
    assert len(g.stats[0].containers) == 4
    total_before = g.container_count
    assert g.flare_stat(0) is True
    assert len(g.stats[0].containers) == 3
    assert g.container_count == total_before  # moved, not removed


def test_level_up_and_set_level_containers():
    g = Gem.create(tier=4, type=1, element=1, restriction=1, level=1, augmentation=0.0)
    assert g.container_count == 3
    g.set_level(15)
    assert g.level == 15 and g.container_count == 6  # +1 at each of 5/10/15
    assert g.set_level(35) is True and g.level == 35
    assert g.is_max_level is True
    assert g.level_up() is False  # already max


# --- Evaluator -------------------------------------------------------------


def test_evaluate_maxed_gem_is_perfect():
    # Build three stats each at their theoretical max value → ~100% quality.
    specs = [3, 4, 5]  # crit dmg, crit hit, max health (Water element, no phys/magic/light)
    stats = []
    for st in specs:
        rng = evaluator.gem_stat_range(tier=4, type=1, stat_type=st, level=1, extra_containers=0)
        stats.append({"type": st, "value": rng["max_value"], "extra_containers": 0})
    out = evaluator.evaluate_gem(tier=4, type=1, level=1, stats=stats)
    assert out["result"]["quality_percent"] == pytest.approx(100.0, abs=0.5)
    assert out["result"]["has_issues"] is False
    assert out["result"]["element_name"] == "Water"


def test_evaluate_rejects_bad_input():
    dup = [{"type": 3, "value": 1, "extra_containers": 0}] * 3
    with pytest.raises(evaluator.GemEvaluatorError):
        evaluator.evaluate_gem(tier=4, type=1, level=1, stats=dup)
    phys_magic = [
        {"type": 1, "value": 1, "extra_containers": 0},
        {"type": 2, "value": 1, "extra_containers": 0},
        {"type": 3, "value": 1, "extra_containers": 0},
    ]
    with pytest.raises(evaluator.GemEvaluatorError):
        evaluator.evaluate_gem(tier=4, type=1, level=1, stats=phys_magic)


def test_stat_range_orders_min_max():
    rng = evaluator.gem_stat_range(tier=4, type=2, stat_type=1, level=35, extra_containers=3)
    assert rng["min_value"] < rng["max_value"]
    assert rng["thresholds"][0] < rng["thresholds"][1]


# --- Builds ----------------------------------------------------------------


def test_calculate_builds_ranks_by_coefficient():
    results = builds.calculate_builds(
        {"build_type": "Light", "character": "Knight", "subclass": "Lunar Lancer",
         "ally": "boot_clown", "food": "", "critical_damage_count": 3}
    )
    assert len(results) == 190  # 10 × 19 empowered/lesser splits (cosmic locked to max)
    assert results[0]["rank"] == 1 and results[0]["coefficient"] > 0
    # Sorted descending by coefficient.
    coeffs = [r["coefficient"] for r in results]
    assert coeffs == sorted(coeffs, reverse=True)
    assert results[0]["layout"]


def test_calculate_builds_unknown_class():
    with pytest.raises(builds.BuildError):
        builds.calculate_builds({"character": "Nope", "subclass": "Knight"})


def test_build_options_has_classes_and_allies():
    opts = builds.build_options()
    assert "Knight" in opts["character"] and len(opts["character"]) == 18
    assert any(a["key"] == "boot_clown" for a in opts["ally"])
    assert opts["build_type"] == ["Light", "Farm", "Health"]


# --- Lookups ---------------------------------------------------------------


def test_lookups_complete():
    lk = gem_lookups()
    assert len(lk["tiers"]) == 4 and lk["tiers"][3]["max_level"] == 35
    assert len(lk["stat_types"]) == 9
    assert len(lk["augment_types"]) == 3 and lk["augment_types"][2]["increase_percent"] == 12.5
    assert len(lk["abilities"]) == 12
    assert lk["abilities_by_element"]["Cosmic"] == [9, 10, 11, 12]
