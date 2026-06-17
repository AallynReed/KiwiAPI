from app.trove import stats


def test_stat_table_power_rank():
    table = stats.stat_table("power-rank")
    assert table is not None
    assert table["stat"] == "power-rank" and table["label"] == "Power Rank"
    assert table["count"] == len(table["sources"]) > 0
    src = {s["name"]: s for s in table["sources"]}
    assert src["Gems"]["value"] == 44540 and src["Gems"]["percentage"] is False
    assert src["Dragons"]["type"] == "slider"
    # Every source row carries the cleaned shape.
    assert all({"name", "value", "type", "percentage"} <= s.keys() for s in table["sources"])


def test_stat_table_light_keeps_step_and_permanent():
    table = stats.stat_table("light")
    assert table is not None
    by_name = {s["name"]: s for s in table["sources"]}
    ally = by_name["Ally"]
    assert ally["type"] == "slider" and ally["step"] == 10 and ally["permanent"] is True
    assert by_name["Enlightened Buff"]["permanent"] is False


def test_stat_table_unknown_is_none():
    assert stats.stat_table("nope") is None


def test_all_classes_have_tech_name():
    data = stats.all_classes()
    assert data["count"] == len(data["items"]) == 18
    assert all(c["tech_name"] and c["name"] for c in data["items"])
    techs = {c["tech_name"] for c in data["items"]}
    assert {"knight", "adventurer", "bard"} <= techs  # tech_name != display name for adventurer


def test_class_by_tech_name():
    knight = stats.class_by_tech_name("knight")
    assert knight is not None
    assert knight["name"] == "Knight" and knight["damage_type"] == "Physical"
    phys = next(s for s in knight["stats"] if s["name"] == "Physical Damage")
    assert phys["value"] == 3202 and phys["percentage"] is False
    # Boomeranger's token is "adventurer", not its display name.
    boom = stats.class_by_tech_name("adventurer")
    assert boom is not None and boom["name"] == "Boomeranger"
    assert boom["abilities"]  # this class carries ability data
    # A null stat stays null (not coerced).
    energy = next(s for s in boom["stats"] if s["name"] == "Energy")
    assert energy["value"] is None


def test_class_by_tech_name_unknown_is_none():
    assert stats.class_by_tech_name("not_a_class") is None


# --- Coefficient -----------------------------------------------------------

def test_compute_coefficient_matches_real_sheets():
    # Verified against real in-game stat sheets (the game truncates).
    assert stats.compute_coefficient(799894, 14300, 3438.3) == (28302649, "physical")
    assert stats.compute_coefficient(1285094, 10440, 4582.5) == (60174526, "physical")


def test_compute_coefficient_uses_higher_damage():
    # Mage: magic damage is higher, so it drives the coefficient.
    assert stats.compute_coefficient(1000, 500000, 1000.0) == (5500000, "magic")
    # Only one damage stat present -> that one is used.
    assert stats.compute_coefficient(None, 500000, 1000.0) == (5500000, "magic")
    assert stats.compute_coefficient(200000, None, 0.0) == (200000, "physical")


def test_compute_coefficient_none_without_inputs():
    assert stats.compute_coefficient(None, None, 100.0) is None       # no damage
    assert stats.compute_coefficient(799894, 14300, None) is None     # no crit damage
