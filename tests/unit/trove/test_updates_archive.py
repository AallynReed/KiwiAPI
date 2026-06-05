import zlib

import pytest

from app.trove.troveio import calculate_hash, write_leb128
from app.trove.updates import archive


def _build(files: list[tuple[str, int, bytes]]) -> tuple[bytes, dict[int, bytes]]:
    """Build a synthetic (tfi_bytes, {archive_index: tfa_raw}) from logical files.

    Mirrors the real layout: each archive's decompressed content is the concatenated
    file bytes; the TFI entry records the per-archive offset, size and Trove FNV hash.
    """
    contents: dict[int, bytearray] = {}
    tfi = bytearray()
    for name, arc, data in files:
        buf = contents.setdefault(arc, bytearray())
        offset = len(buf)
        buf += data
        name_b = name.encode("utf-8") + b"\x00"  # null-padded, to exercise the split
        tfi += write_leb128(len(name_b)) + name_b
        tfi += write_leb128(arc)
        tfi += write_leb128(offset)
        tfi += write_leb128(len(data))
        tfi += write_leb128(calculate_hash(data))
    tfas = {arc: zlib.compress(bytes(buf)) for arc, buf in contents.items()}
    return bytes(tfi), tfas


def test_parse_tfi_round_trip():
    files = [
        ("ui/a.bin", 0, b"alpha bytes"),
        ("ui/b.bin", 0, b"second file, longer content here"),
        ("ui\\nested\\c.bin", 0, b"\x00\x01\x02 binary \xff\xfe"),
    ]
    tfi, _ = _build(files)
    entries = archive.parse_tfi(tfi)
    assert [e.name for e in entries] == ["ui/a.bin", "ui/b.bin", "ui/nested/c.bin"]  # posix-normalized
    assert all(e.archive_index == 0 for e in entries)
    assert entries[0].size == len(b"alpha bytes")
    assert entries[1].offset == len(b"alpha bytes")  # follows the first file
    assert entries[2].fnv_hash == calculate_hash(b"\x00\x01\x02 binary \xff\xfe")


def test_extract_archive_and_verify():
    files = [("ui/a.bin", 0, b"alpha"), ("ui/b.bin", 0, b"bravo bytes")]
    tfi, tfas = _build(files)
    entries = archive.parse_tfi(tfi)
    out = archive.extract_archive(tfas[0], entries, 0)
    assert out == {"ui/a.bin": b"alpha", "ui/b.bin": b"bravo bytes"}
    # The TFI's hash matches the extracted content (integrity self-check).
    by_name = {e.name: e for e in entries}
    for name, data in out.items():
        assert archive.verify_entry(by_name[name], data)


def test_single_archive_extracts_without_siblings():
    # Two archives in one TFI; extract archive 0 with ONLY archive0.tfa present.
    files = [
        ("ui/a.bin", 0, b"in archive zero"),
        ("ui/b.bin", 1, b"in archive one"),
        ("ui/c.bin", 0, b"also archive zero"),
    ]
    tfi, tfas = _build(files)
    entries = archive.parse_tfi(tfi)
    assert set(tfas) == {0, 1}
    only0 = archive.extract_archive(tfas[0], entries, 0)  # tfas[1] never touched
    assert only0 == {"ui/a.bin": b"in archive zero", "ui/c.bin": b"also archive zero"}
    only1 = archive.extract_archive(tfas[1], entries, 1)
    assert only1 == {"ui/b.bin": b"in archive one"}


def test_verify_entry_detects_corruption():
    files = [("x.bin", 0, b"correct content")]
    tfi, _ = _build(files)
    entry = archive.parse_tfi(tfi)[0]
    assert archive.verify_entry(entry, b"correct content") is True
    assert archive.verify_entry(entry, b"corrupted!!!!!!") is False  # same len, wrong bytes
    assert archive.verify_entry(entry, b"short") is False            # wrong size


def test_parse_tfi_rejects_garbage():
    with pytest.raises(archive.ArchiveError):
        archive.parse_tfi(b"\xff\xff\xff")  # truncated varints / no entries terminate cleanly


def test_decompress_tfa_rejects_garbage():
    with pytest.raises(archive.ArchiveError):
        archive.decompress_tfa(b"not a zlib stream")
