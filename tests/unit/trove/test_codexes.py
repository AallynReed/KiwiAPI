"""Pure tests for the codex foundation: the binfab reader, path→type mapping,
and identity extraction. Real prefabs only exist in the server's archive, so we
hand-build synthetic `.binfab` wire data here (the format is self-describing).
"""

from app.trove.codexes import binfab
from app.trove.codexes.extract import extract_entry
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
        "prefabs/collections/mount/horse.binfab": "mount",
        "prefabs/collections/dragon/ember.binfab": "dragon",
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
