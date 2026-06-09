import base64

import pytest

from app.trove import tmod

# --- Hash + LEB128 ---------------------------------------------------------


def test_calculate_hash_matches_trove_dll():
    # Golden values captured from BetterTroveTools' native trove.dll (the real
    # Trove checksum). The high-byte tails (>=0x80) are the ones that exposed the
    # signed-char sign-extension quirk in the C - keep them here as a regression
    # guard since the DLL isn't available in CI.
    golden = {
        b"": 2166136261,                       # offset basis
        b"A": 3289118412,                      # 1-byte tail
        b"AB": 3345950853,                     # 2-byte tail
        b"ABC": 751230962,                     # 3-byte tail
        b"ABCD": 971824332,                    # one full word
        b"Hello, Trove!": 613010621,
        b"\xff\xfe\xfd\xfc\xfb": 2513481199,   # word + high 1-byte tail
        b"\xde\xad\xbe\xef\xca\xfe": 4065814253,  # word + high 2-byte tail
        b"\xaa\xbb\xcc": 3371389995,           # high 3-byte tail
    }
    for data, expected in golden.items():
        assert tmod.calculate_hash(data) == expected, data
    assert tmod.calculate_hash(b"hello, trove!") != golden[b"Hello, Trove!"]  # byte-sensitive


def test_leb128_round_trip():
    for value in (0, 1, 127, 128, 300, 16384, 2166136261, 0xFFFFFFFF):
        encoded = tmod.write_leb128(value)
        decoded, pos = tmod.read_leb128(encoded, 0)
        assert decoded == value and pos == len(encoded)
    # The classic 300 → AC 02 example.
    assert tmod.write_leb128(300) == bytes([0xAC, 0x02])


# --- Build → read round-trip ----------------------------------------------


def test_build_then_read_round_trips():
    files = [("ui/icon.png", b"\x89PNG fake bytes"), ("Config/Default.cfg", b"key=value\n")]
    data = tmod.build_tmod(
        version=1,
        properties={"title": "My Mod", "author": "Aallyn", "modVersion": "1.0"},
        files=files,
    )
    assert isinstance(data, bytes) and len(data) > 12

    out = tmod.read_tmod(data)
    # modLoader is always stamped KiwiAPI (not BTT), even though we didn't send it.
    assert out["properties"]["modLoader"] == "KiwiAPI"
    assert out["properties"]["title"] == "My Mod" and out["properties"]["author"] == "Aallyn"
    assert out["version"] == 1 and out["file_count"] == 2

    by_path = {f["path"]: f for f in out["files"]}
    # Paths are normalized to lowercase posix.
    assert set(by_path) == {"ui/icon.png", "config/default.cfg"}
    icon = by_path["ui/icon.png"]
    assert icon["size"] == len(b"\x89PNG fake bytes")
    assert icon["checksum"] == tmod.calculate_hash(b"\x89PNG fake bytes")
    assert base64.b64decode(icon["content_base64"]) == b"\x89PNG fake bytes"


def test_read_metadata_only_omits_content():
    data = tmod.build_tmod(1, {"title": "X"}, [("a/b.txt", b"some bytes here")])
    out = tmod.read_tmod(data, metadata_only=True)
    assert out["metadata_only"] is True
    f = out["files"][0]
    assert f["path"] == "a/b.txt" and f["size"] == 15 and f["checksum"] >= 0
    assert f.get("content_base64") is None  # content not loaded


def test_build_overrides_client_mod_loader():
    data = tmod.build_tmod(1, {"title": "X", "modLoader": "BTT"}, [("a.txt", b"x")])
    assert tmod.read_tmod(data, metadata_only=True)["properties"]["modLoader"] == "KiwiAPI"


def test_build_requires_files():
    with pytest.raises(tmod.TmodError):
        tmod.build_tmod(1, {"title": "X"}, [])


def test_read_rejects_garbage():
    with pytest.raises(tmod.TmodError):
        tmod.read_tmod(b"not a tmod")
    with pytest.raises(tmod.TmodError):
        tmod.read_tmod(b"\xff" * 8 + b"\x01\x00\x00\x00")  # header size past EOF
