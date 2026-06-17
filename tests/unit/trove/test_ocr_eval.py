"""Tests for the OCR eval harness (scripts/ocr_eval.py).

The harness's SCORING is pure, so we verify it here without needing the OCR
engine. We also validate the committed ground-truth fixtures so a typo'd stat
key or a percent stored as a string can't silently poison the accuracy oracle.
"""
import json

from app.trove.ocr import vocabulary as vocab
from scripts.ocr_eval import _DEFAULT_TRUTH, aggregate, score_image, values_match


def _ex(value, unit="count"):
    """Shape one entry the way parse.extract_character_stats() emits it."""
    return {"value": value, "unit": unit, "raw": str(value),
            "confidence": 1.0, "in_range": True, "type_match": True}


def test_values_match_int_exact_and_float_tolerant():
    assert values_match(799894, 799894)
    assert not values_match(799894, 799895)
    assert values_match(114.8, 114.80)          # representation diff absorbed
    assert not values_match(114.8, 120.0)        # real misread caught


def test_score_image_correct_wrong_missed():
    extracted = {
        "physical_damage": _ex(799894),
        "critical_hit": _ex(120.0, "percent"),   # wrong (truth 114.8)
        # maximum_health missing -> counted as missed
    }
    truth = {"physical_damage": 799894, "critical_hit": 114.8, "maximum_health": 3239060}
    score = score_image(extracted, truth, complete=False)
    assert score["correct"] == ["physical_damage"]
    assert [w["stat"] for w in score["wrong"]] == ["critical_hit"]
    assert score["missed"] == ["maximum_health"]
    assert score["spurious"] == []
    assert score["total"] == 3


def test_complete_flag_controls_spurious():
    extracted = {"physical_damage": _ex(799894), "jump": _ex(74)}
    truth = {"physical_damage": 799894}
    # complete=True: the extra 'jump' is a false positive
    assert score_image(extracted, truth, complete=True)["spurious"] == ["jump"]
    # complete=False (partial label): extras are not penalised
    assert score_image(extracted, truth, complete=False)["spurious"] == []


def test_aggregate_rolls_up_accuracy():
    a = score_image({"jump": _ex(74)}, {"jump": 74, "light": 13185}, complete=False)  # 1/2
    b = score_image({"jump": _ex(74)}, {"jump": 74}, complete=False)                  # 1/1
    agg = aggregate([a, b])
    assert agg["images"] == 2
    assert agg["total_stats"] == 3
    assert agg["correct"] == 2
    assert agg["missed"] == 1
    assert abs(agg["stat_accuracy"] - 2 / 3) < 1e-9


def test_ground_truth_fixture_is_valid():
    """Every key in the committed ground truth must be a real stat key, and
    every value a number - else the oracle is lying."""
    data = json.loads(_DEFAULT_TRUTH.read_text(encoding="utf-8"))
    known = set(vocab.all_keys())
    entries = {k: v for k, v in data.items() if not k.startswith("_")}
    assert entries, "ground truth has no labeled images"
    for fname, entry in entries.items():
        assert isinstance(entry.get("complete"), bool), f"{fname}: missing 'complete'"
        for stat_key, value in entry["stats"].items():
            assert stat_key in known, f"{fname}: unknown stat key '{stat_key}'"
            assert isinstance(value, (int, float)), f"{fname}: {stat_key} not numeric"
