"""Character-stat OCR PARSER tests - the engine-agnostic accuracy core.

No OCR engine / image needed: we feed the text lines an OCR engine would emit and
assert the label/number pairing, multilingual matching, and validation flags. This
is where correctness lives; the engine just turns pixels into these lines.
"""
from app.trove.ocr import parse
from app.trove.ocr import vocabulary as vocab


def _stats(lines):
    return parse.extract_character_stats(lines)["stats"]


# --- vocabulary --------------------------------------------------------------

def test_normalize_strips_accents_and_punctuation():
    assert vocab.normalize("Dégâts physiques") == "degats physiques"
    assert vocab.normalize("Critical Hit:") == "critical hit"


def test_match_label_exact_fuzzy_and_miss():
    assert vocab.match_label("Physical Damage")[0] == "physical_damage"
    assert vocab.match_label("Coup critique")[0] == "critical_hit"        # French
    assert vocab.match_label("Physjcal Damaqe")[0] == "physical_damage"   # OCR typos
    assert vocab.match_label("totally unrelated text") is None


# --- single-column English ---------------------------------------------------

def test_single_column_value_before_label():
    lines = [
        "14,690 Physical Damage",
        "860,987 Magic Damage",
        "2,135,403 Maximum Health",
        "140.7% Critical Hit",
        "3,533.9% Critical Damage",
    ]
    s = _stats(lines)
    assert s["physical_damage"]["value"] == 14690
    assert s["physical_damage"]["unit"] == "count"
    assert s["magic_damage"]["value"] == 860987
    assert s["maximum_health"]["value"] == 2135403
    assert s["critical_hit"]["value"] == 140.7
    assert s["critical_hit"]["unit"] == "percent"
    assert s["critical_damage"]["value"] == 3533.9
    assert all(v["in_range"] and v["type_match"] for v in s.values())


def test_thousands_separator_and_ocr_comma_period_misread():
    # OCR routinely misreads ',' as '.'; a thousands group is always 3 digits, so a
    # trailing 1-2 digit group marks the decimal and everything else is grouping.
    assert _stats(["Physical Damage 636.088"])["physical_damage"]["value"] == 636088
    assert _stats(["Maximum Health 2.716,754"])["maximum_health"]["value"] == 2716754
    assert _stats(["Critical Damage 3.073.0%"])["critical_damage"]["value"] == 3073.0
    # well-formed US-style values still parse correctly
    assert _stats(["Physical Damage 799,894"])["physical_damage"]["value"] == 799894
    assert _stats(["Critical Hit 114.8%"])["critical_hit"]["value"] == 114.8


def test_value_after_label_and_power_rank_both_orders():
    assert _stats(["Power Rank 50111"])["power_rank"]["value"] == 50111
    assert _stats(["50111 Power Rank"])["power_rank"]["value"] == 50111
    assert _stats(["Coefficient 28,302,649"])["coefficient"]["value"] == 28302649


# --- two columns merged onto one OCR line ------------------------------------

def test_two_column_line_pairs_each_stat_to_its_own_value():
    # The right-hand column lands on the same visual line as the left.
    s = _stats(["Physical Damage 799,894 Critical Hit 114.8%"])
    assert s["physical_damage"]["value"] == 799894
    assert s["critical_hit"]["value"] == 114.8 and s["critical_hit"]["unit"] == "percent"


def test_label_between_two_numbers_uses_type_to_disambiguate():
    # int stat must take the integer, percent stat must take the percent, even
    # though each label sits between both numbers.
    s = _stats(["1,155,050 Physical Damage 4,351.8% Critical Damage"])
    assert s["physical_damage"]["value"] == 1155050
    assert s["critical_damage"]["value"] == 4351.8


# --- Coefficient: a DERIVED stat (floor(max(phys,magic) * (1 + critDmg/100))) ----

def test_coefficient_derived_when_absent():
    # Coefficient often isn't on the sheet; compute it from the damage + crit reads.
    s = _stats(["Physical Damage 799,894", "Magic Damage 14,300", "Critical Damage 3,438.3%"])
    assert s["coefficient"]["value"] == 28302649       # verified against a real sheet
    assert s["coefficient"]["derived"] is True


def test_coefficient_uses_higher_of_physical_or_magic():
    # A mage's magic damage exceeds physical, so it drives the coefficient.
    s = _stats(["Physical Damage 1,000", "Magic Damage 500,000", "Critical Damage 1,000%"])
    assert s["coefficient"]["value"] == 5500000        # 500000 * (1 + 10)
    assert s["coefficient"]["derived"] is True


def test_read_coefficient_confirmed_by_derivation():
    # When shown AND derivable and they agree, it's a confirmed read (not derived).
    s = _stats(["Physical Damage 799,894", "Magic Damage 14,300",
                "Critical Damage 3,438.3%", "Coefficient 28,302,649"])
    assert s["coefficient"]["value"] == 28302649
    assert s["coefficient"]["derived"] is False


def test_coefficient_mismatch_is_flagged():
    # A read that contradicts its own inputs is surfaced but flagged low-confidence.
    s = _stats(["Physical Damage 799,894", "Critical Damage 3,438.3%",
                "Coefficient 12,345,678"])
    assert s["coefficient"]["value"] == 12345678
    assert s["coefficient"]["confidence"] < 0.6


def test_coefficient_not_invented_without_inputs():
    # No crit damage -> not derivable -> nothing hallucinated.
    s = _stats(["Physical Damage 799,894", "Magic Damage 14,300"])
    assert "coefficient" not in s


# --- cross-line pairing (label + value on separate OCR lines) ----------------

def test_separated_label_value_paired_across_lines():
    # French equipment view: "Rg de pouvoir" + its value land on separate OCR lines
    # (Power Rank isn't derivable, so cross-line pairing is what recovers it).
    s = _stats(["Rg de pouvoir", "Stats", "50613"])
    assert s["power_rank"]["value"] == 50613
    assert s["power_rank"]["confidence"] < 1.0      # cross-line heuristic penalty


def test_crossline_respects_window_and_type():
    # Beyond the small window the orphan is NOT claimed.
    assert "power_rank" not in _stats(["Power Rank", "a", "b", "c", "d", "e", "50613"])
    # A percent orphan must NOT attach to an integer-typed unpaired label.
    assert "power_rank" not in _stats(["Power Rank", "12.5%"])


# --- French client -----------------------------------------------------------

def test_french_labels_map_to_canonical_keys():
    lines = [
        "1,155,050 Dégâts physiques",
        "4,256,519 PV maximum",
        "108.8% Coup critique",
        "9,804 Trouvaille magique",
        "Rg de pouvoir 53471",
    ]
    s = _stats(lines)
    assert s["physical_damage"]["value"] == 1155050
    assert s["maximum_health"]["value"] == 4256519
    assert s["critical_hit"]["value"] == 108.8
    assert s["magic_find"]["value"] == 9804
    assert s["power_rank"]["value"] == 53471


# --- validation flags --------------------------------------------------------

def test_out_of_range_value_is_flagged_low_confidence():
    s = _stats(["Critical Hit 999999%"])      # crit hit max is 1000
    assert s["critical_hit"]["in_range"] is False
    assert s["critical_hit"]["confidence"] < 0.5


def test_type_mismatch_is_flagged():
    s = _stats(["Jump 50%"])                  # jump is an int stat, not a percent
    assert s["jump"]["value"] == 50
    assert s["jump"]["type_match"] is False
    assert s["jump"]["confidence"] < 0.8      # downgraded from the ~1.0 exact-label match


def test_unlabeled_numbers_are_ignored():
    # "LEVEL 30 Dracolyte" has no known stat label -> the 30 must not become a stat.
    out = parse.extract_character_stats(["LEVEL 30 Dracolyte", "Titles"])
    assert out["stats"] == {}
    assert out["total_known"] == len(vocab.all_keys())
