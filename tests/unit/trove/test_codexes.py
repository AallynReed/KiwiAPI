"""Pure tests for the codex foundation: the binfab reader, path→type mapping,
and identity extraction. Real prefabs only exist in the server's archive, so we
hand-build synthetic `.binfab` wire data here (the format is self-describing).
"""

import re

from app.trove.codexes import binfab, mastery
from app.trove.codexes import read as codexes_read
from app.trove.codexes.extract import extract_entry, refine_mount
from app.trove.codexes.types import classify

# --- wire builders ----------------------------------------------------------

def _uleb(n: int) -> bytes:
    out = bytearray()
    while True:
        b = n & 0x7F
        n >>= 7
        if n:
            out.append(b | 0x80)
        else:
            out.append(b)
            break
    return bytes(out)


def _marker(field: int = 0) -> bytes:
    return _uleb((field << 4) | 0xE)  # wt 0xE = composite marker


def _str_field(field: int, text: str) -> bytes:
    raw = text.encode("ascii")
    return _uleb((field << 4) | 8) + _uleb(len(raw)) + raw  # wt 8 = length-prefixed


def _varint_field(field: int, value: int) -> bytes:
    return _uleb((field << 4) | 0) + _uleb(value)  # wt 0 = varint


def _entity_prefab(name_key: str, category: str, desc_key: str, tradable: bool) -> bytes:
    """A minimal entity prefab: an identity component between two markers."""
    stream = (
        _marker()
        + _str_field(1, name_key)
        + _str_field(2, category)
        + _str_field(5, desc_key)
        + _varint_field(14, 2 if tradable else 1)
        + _marker()
    )
    return bytes([0x05, 0x00]) + _uleb(len(stream)) + stream  # <fmt> 00 <len> <stream>


def _locale_entry(key: str, value: str) -> bytes:
    return key.encode("ascii") + bytes([0x18]) + _uleb(len(value.encode())) + value.encode()


# --- binfab reader ----------------------------------------------------------

def test_uleb_roundtrip():
    for n in (0, 1, 127, 128, 255, 16384, 2**32):
        assert binfab.read_uleb(_uleb(n), 0) == (n, len(_uleb(n)))


def test_content_start_finds_header():
    data = _entity_prefab("$n", "Cat", "$d", True)
    assert binfab.content_start(data) == 3  # past [fmt, 00, uleb-len]


def test_decode_identity():
    data = _entity_prefab("$prefabs_pet_wolf_name", "Pets", "$prefabs_pet_wolf_desc", True)
    ident = binfab.decode_identity(data)
    assert ident is not None
    assert ident["name_key"] == "$prefabs_pet_wolf_name"
    assert ident["category"] == "Pets"
    assert ident["desc_key"] == "$prefabs_pet_wolf_desc"
    assert ident["tradable"] is True


def test_decode_identity_not_tradable():
    data = _entity_prefab("$n", "Items", "$d", False)
    assert binfab.decode_identity(data)["tradable"] is False


def test_decode_identity_none_without_run():
    # No identity component — just bare bytes, no markers.
    assert binfab.decode_identity(b"\x00\x01\x02\x03") is None


def test_harvest_strings():
    data = _entity_prefab("$n", "Mounts", "$d", True)
    found = {s for _off, _field, s in binfab.harvest_strings(data)}
    assert {"$n", "Mounts", "$d"} <= found


def test_extract_localization_map():
    blob = _locale_entry("$wolf_name", "Dire Wolf") + _locale_entry("$wolf_desc", "A loyal pet.")
    loc = binfab.extract_localization_map(blob)
    assert loc["$wolf_name"] == "Dire Wolf"
    assert loc["$wolf_desc"] == "A loyal pet."


# --- path → type mapping ----------------------------------------------------

def test_classify_each_type():
    cases = {
        "prefabs/item/fish/cod.binfab": "fish",
        "prefabs/item/unlocker/memento_glow.binfab": "memento",
        "prefabs/collections/pet/wolf.binfab": "ally",
        "prefabs/collections/mount/horse.binfab": "mount",  # dragons split off later, by category
        "prefabs/collections/badge/founder.binfab": "badge",
        "prefabs/recipes/forge.binfab": "recipe",
        "prefabs/item/sword.binfab": "item",
    }
    for path, expected in cases.items():
        assert classify(path) == expected, path


def test_classify_excludes_npc_pets():
    assert classify("prefabs/collections/pet/shopkeeper_npc.binfab") is None


def test_classify_rejects_non_codex_paths():
    assert classify("blueprints/foo.blueprint") is None
    assert classify("languages/en/strings.binfab") is None
    assert classify("prefabs/collections/other/thing.binfab") is None
    assert classify("prefabs/item/sword.txt") is None


# --- extraction -------------------------------------------------------------

def test_extract_entry_resolves_names():
    data = _entity_prefab("$wolf_name", "Pets", "$wolf_desc", True)
    loc = {"$wolf_name": "Dire Wolf", "$wolf_desc": "A loyal pet."}
    entry = extract_entry("ally", "prefabs/collections/pet/wolf.binfab", data, loc)
    assert entry["codex_type"] == "ally"
    assert entry["name"] == "Dire Wolf"
    assert entry["category"] == "Pets"
    assert entry["description"] == "A loyal pet."
    assert entry["tradable"] is True
    assert entry["name_key"] == "$wolf_name"


def test_extract_entry_falls_back_to_filename():
    # No identity + empty locale map → name derived from the path stem.
    entry = extract_entry("item", "prefabs/item/fancy_sword.binfab", b"\x00\x01", {})
    assert entry["name"] == "Fancy Sword"
    assert entry["description"] == ""


# --- dragons: split out of the mount tree by collection category ------------

def _collection_table(*strings: str) -> bytes:
    # parse_collection_table harvests strings in order: a bare category name, then
    # $CollectionName_…, then collections/… members of that group.
    return b"".join(_str_field(1, s) for s in strings)


def test_collection_category_map_groups_mounts():
    table = _collection_table(
        "Dragons", "$CollectionName_Dragons", "collections/mount/ember",
        "Mounts", "$CollectionName_Mounts", "collections/mount/horse",
    )
    m = binfab.collection_category_map(table)
    assert m["collections/mount/ember"] == "Dragons"
    assert m["collections/mount/horse"] == "Mounts"


def test_refine_mount_reclassifies_dragons_by_table():
    cats = {"collections/mount/ember": "Dragons", "collections/mount/horse": "Mounts"}
    dragon = refine_mount({"codex_type": "mount", "category": ""}, "collections/mount/ember", cats)
    assert dragon["codex_type"] == "dragon" and dragon["category"] == "Dragons"
    mount = refine_mount({"codex_type": "mount", "category": ""}, "collections/mount/horse", cats)
    assert mount["codex_type"] == "mount" and mount["category"] == "Mounts"


def test_refine_mount_falls_back_to_inprefab_category():
    # No table entry -> keep the in-prefab category, still detecting 'dragon' in it.
    d = refine_mount({"codex_type": "mount", "category": "Fire Dragon"}, "collections/mount/x", {})
    assert d["codex_type"] == "dragon"
    m = refine_mount({"codex_type": "mount", "category": "Cool Mount"}, "collections/mount/y", {})
    assert m["codex_type"] == "mount"


# --- search filter query builder --------------------------------------------

def test_filter_ands_every_field():
    q = codexes_read._filter("live-us", codex_type="ally", search="wolf",
                             category="Pets", tradable=True)
    assert q["branch"] == "live-us"
    assert q["codex_type"] == "ally"
    assert q["category"] == "Pets"
    assert q["tradable"] is True
    # search matches name OR description
    assert q["$or"] == [
        {"name": {"$regex": "wolf", "$options": "i"}},
        {"description": {"$regex": "wolf", "$options": "i"}},
    ]


def test_filter_escapes_regex_and_omits_unset():
    q = codexes_read._filter("live-us", codex_type=None, search="a.b*[",
                             category=None, tradable=None)
    assert q == {"branch": "live-us", "$or": [
        {"name": {"$regex": re.escape("a.b*["), "$options": "i"}},
        {"description": {"$regex": re.escape("a.b*["), "$options": "i"}},
    ]}


def test_filter_tradable_false_is_kept():
    q = codexes_read._filter("live-us", codex_type="item", search=None,
                             category=None, tradable=False)
    assert q["tradable"] is False and "$or" not in q


# --- mastery (meta/multipliers.binfab) --------------------------------------

def test_infer_mastery_base_rules():
    assert mastery.infer_mastery_base("collections/mount/ember") == ("collections/mount/ember", 50)
    assert mastery.infer_mastery_base("collections/pet/wolf") == ("collections/pet/wolf", 10)
    assert mastery.infer_mastery_base("collections/badge/x") == ("collections/badge/x", 20)
    assert mastery.infer_mastery_base("item/fish/cod") == ("item/fish/cod", 5)
    assert mastery.infer_mastery_base("collections\\mount\\x") == ("collections/mount/x", 50)
    assert mastery.infer_mastery_base("loneword") == ("collections/skin/loneword", 35)  # bare -> skin
    assert mastery.infer_mastery_base("item/sword") == ("item/sword", 0)


def _mult_group(identifier: str) -> bytes:
    # marker + count(1) + pattern(index1 = varint(4)) + \x00 + 2 skip bytes + len + id
    return (b"\xBE\x01\xAE" + _uleb(1) + _uleb(4) + b"\x00" + b"\x00\x00"
            + _uleb(len(identifier)) + identifier.encode())


def test_parse_multipliers_maps_groups_to_multipliers():
    blob = (b"\x00" * 9
            + _mult_group("collections/pet/wolf")        # 1st group -> ×0
            + _mult_group("collections/badge/founder")   # 2nd group -> ×2
            + _mult_group("collections/mount/horse")     # 3rd group -> ×3
            + _mult_group("collections/mount/ember"))    # 4th group -> ×5
    rows = mastery.parse_multipliers(blob)
    assert rows["collections/pet/wolf"]["predicted"] == 0       # 10 × 0
    assert rows["collections/badge/founder"]["predicted"] == 40  # 20 × 2
    assert rows["collections/mount/horse"]["predicted"] == 150   # 50 × 3
    assert rows["collections/mount/ember"]["predicted"] == 250   # 50 × 5


def test_mastery_for_uses_map_then_base():
    m = {"collections/mount/ember": {"multiplier": 5, "base": 50, "predicted": 250}}
    assert mastery.mastery_for("collections/mount/ember", m) == 250
    assert mastery.mastery_for("collections/mount/horse", m) == 50   # absent -> type base
    assert mastery.mastery_for("item/sword", m) is None              # base 0 -> None
