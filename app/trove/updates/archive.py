"""Pure-Python Trove `.tfa` / `.tfi` archive reader.

A directory like `ui/` holds one `index.tfi` plus `archive0.tfa`, `archive1.tfa`, …
The `.tfi` is the index: a flat list of logical-file entries, each pointing at the
archive it lives in, its byte range within that archive's *decompressed* content,
and a per-file Trove FNV-1a hash. A `.tfa` is just a zlib stream of the
concatenated (decompressed) file bytes.

Two properties make the version archiver cheap, both confirmed against
BetterTroveTools' reader:
  - The TFI carries a per-file hash, so two TFI versions diff at the index level -
    we learn exactly which logical files changed without extracting anything.
  - Every entry names its `archive_index`, so a single changed `archiveN.tfa` can
    be extracted alone (download it + the `.tfi`; siblings aren't needed).

Format per TFI entry (all ints LEB128, little-endian):
  leb128 name_len · name_len bytes (UTF-8, may be null-padded) · leb128 archive_index
  · leb128 offset · leb128 size · leb128 fnv_hash
"""

from __future__ import annotations

import zlib
from collections.abc import Iterable, Iterator
from dataclasses import dataclass

from app.trove.troveio import calculate_hash, read_leb128


@dataclass(frozen=True, slots=True)
class TfiEntry:
    name: str            # logical path (posix), relative to the .tfi's directory
    archive_index: int   # which archiveN.tfa it lives in
    offset: int          # byte offset within that archive's DECOMPRESSED content
    size: int            # logical file size in bytes
    fnv_hash: int        # Trove FNV-1a of the logical file content (the change key)


def parse_tfi(data: bytes) -> list[TfiEntry]:
    """Parse `.tfi` index bytes into entries. Pure / no I/O."""
    entries: list[TfiEntry] = []
    pos = 0
    n = len(data)
    try:
        while pos < n:
            name_len, pos = read_leb128(data, pos)
            raw_name = data[pos:pos + name_len]
            pos += name_len
            # Names are UTF-8 and may be null-padded; keep the prefix and posix it.
            name = raw_name.split(b"\x00", 1)[0].decode("utf-8").replace("\\", "/")
            archive_index, pos = read_leb128(data, pos)
            offset, pos = read_leb128(data, pos)
            size, pos = read_leb128(data, pos)
            fnv_hash, pos = read_leb128(data, pos)
            entries.append(TfiEntry(name, archive_index, offset, size, fnv_hash))
    except (IndexError, UnicodeDecodeError, ValueError) as e:
        raise ArchiveError(f"malformed .tfi: {e}") from e
    return entries


def decompress_tfa(raw: bytes) -> bytes:
    """Inflate a `.tfa`'s zlib stream into the concatenated file content."""
    try:
        return zlib.decompressobj(wbits=zlib.MAX_WBITS).decompress(raw)
    except zlib.error as e:
        raise ArchiveError(f"could not inflate .tfa: {e}") from e


def entries_for_archive(entries: Iterable[TfiEntry], archive_index: int) -> list[TfiEntry]:
    """The TFI entries that live in one specific archiveN.tfa."""
    return [e for e in entries if e.archive_index == archive_index]


def slice_entries(content: bytes, entries: Iterable[TfiEntry]) -> Iterator[tuple[TfiEntry, bytes]]:
    """Yield (entry, bytes) by slicing decompressed archive content by offset/size."""
    for e in entries:
        yield e, content[e.offset:e.offset + e.size]


def extract_archive(tfa_raw: bytes, entries: Iterable[TfiEntry], archive_index: int) -> dict[str, bytes]:
    """Decompress one archiveN.tfa and return {logical_name: bytes} for its entries.

    Needs only this archive + the parsed TFI - sibling archives aren't required.
    """
    content = decompress_tfa(tfa_raw)
    members = entries_for_archive(entries, archive_index)
    return {e.name: data for e, data in slice_entries(content, members)}


def verify_entry(entry: TfiEntry, data: bytes) -> bool:
    """True if extracted bytes match the entry's size and Trove FNV-1a hash."""
    return len(data) == entry.size and calculate_hash(data) == entry.fnv_hash


class ArchiveError(ValueError):
    """Raised on a malformed .tfi/.tfa (mapped to 400/skip by callers)."""
