"""Read and build Trove ``.tmod`` mod files (binary format).

Ported from BetterTroveTools (`models/trove/mod.py`, `trove.c`, `utils/functions.py`).
Pure Python — the FNV-1a-variant checksum (BTT ships it as a C extension) is
reimplemented here so the API needs no native lib. Nothing is stored: read parses
an uploaded tmod; build serializes one in memory and returns the bytes.

Format (all multi-byte ints little-endian, strings UTF-8):
  [u64 header_size][u16 version][u16 prop_count]
  prop_count × ( leb128 name_len, name, leb128 val_len, value )
  files × ( u8 path_len, path, leb128 index, leb128 offset, leb128 size, leb128 checksum )
  ...header ends at header_size...
  zlib stream (level 0, 32 KiB chunks, Z_SYNC_FLUSH) of the 4-byte-padded file contents
"""

from __future__ import annotations

import base64
import zlib

from pydantic import BaseModel, Field

# The Trove FNV-1a checksum + LEB128 live in troveio (shared with the archive
# reader). Re-exported here so `tmod.calculate_hash` etc. keep working.
from app.trove.troveio import calculate_hash, read_leb128, write_leb128

__all__ = ["calculate_hash", "read_leb128", "write_leb128"]

_CHUNK = 32768
# The modLoader header BTT stamps as "BTT"; the API stamps its own marker instead.
KIWI_MOD_LOADER = "KiwiAPI"


class TmodError(ValueError):
    """Raised on malformed tmod input or invalid build params (mapped to 400)."""


# --- Pydantic models -------------------------------------------------------


class TmodFileEntry(BaseModel):
    path: str                       # internal trove path (lowercase posix)
    index: int
    offset: int                     # byte offset in the decompressed file stream
    size: int                       # uncompressed content size
    checksum: int                   # FNV-1a-variant, 32-bit unsigned
    content_base64: str | None = None  # omitted in metadata-only mode


class TmodReadResponse(BaseModel):
    version: int
    header_size: int
    properties: dict[str, str]      # tmod header key/values (modLoader, title, author, …)
    file_count: int
    files: list[TmodFileEntry]
    metadata_only: bool


class TmodBuildFile(BaseModel):
    path: str                       # internal trove path
    content_base64: str             # file bytes, base64


class TmodBuildRequest(BaseModel):
    version: int = 1
    properties: dict[str, str] = Field(default_factory=dict)  # title, author, modVersion, notes, tags, …
    files: list[TmodBuildFile]


# --- Reader ----------------------------------------------------------------


def _manual_decompress(data: bytes) -> bytes:
    """Fallback for streams the zlib object rejects (custom 7-byte head / 5-byte sync markers)."""
    buf = data[7:-5]
    out = bytearray()
    pos = 0
    for _ in range(len(buf) // (_CHUNK + 5)):
        out += buf[pos:pos + _CHUNK]
        pos += _CHUNK + 5  # skip the 5-byte sync-flush marker
    out += buf[pos:]
    return bytes(out)


def read_tmod(data: bytes, metadata_only: bool = False) -> dict:
    """Parse a .tmod byte string. With metadata_only, file contents are not loaded/returned."""
    if len(data) < 12:
        raise TmodError("file too small to be a .tmod")
    try:
        header_size = int.from_bytes(data[0:8], "little")
        version = int.from_bytes(data[8:10], "little")
        prop_count = int.from_bytes(data[10:12], "little")
        if not 12 <= header_size <= len(data):
            raise TmodError("declared header size is outside the file")

        pos = 12
        properties: dict[str, str] = {}
        for _ in range(prop_count):
            name_size, pos = read_leb128(data, pos)
            name = data[pos:pos + name_size].decode("utf-8")
            pos += name_size
            value_size, pos = read_leb128(data, pos)
            value = data[pos:pos + value_size].decode("utf-8")
            pos += value_size
            properties[name] = value

        file_stream = b""
        if not metadata_only:
            compressed = data[header_size:]
            try:
                file_stream = zlib.decompressobj(wbits=zlib.MAX_WBITS).decompress(compressed)
            except zlib.error:
                file_stream = _manual_decompress(compressed)

        files: list[dict] = []
        while pos < header_size:
            name_size = data[pos]
            pos += 1
            path = data[pos:pos + name_size].decode("utf-8")
            pos += name_size
            index, pos = read_leb128(data, pos)
            offset, pos = read_leb128(data, pos)
            size, pos = read_leb128(data, pos)
            checksum, pos = read_leb128(data, pos)
            entry = {"path": path, "index": index, "offset": offset, "size": size, "checksum": checksum}
            if not metadata_only:
                entry["content_base64"] = base64.b64encode(file_stream[offset:offset + size]).decode("ascii")
            files.append(entry)
    except (IndexError, UnicodeDecodeError, ValueError) as e:
        if isinstance(e, TmodError):
            raise
        raise TmodError(f"malformed .tmod file: {e}") from e

    return {
        "version": version, "header_size": header_size, "properties": properties,
        "files": files, "file_count": len(files), "metadata_only": metadata_only,
    }


# --- Builder ---------------------------------------------------------------


def build_tmod(version: int, properties: dict[str, str], files: list[tuple[str, bytes]],
               mod_loader: str = KIWI_MOD_LOADER) -> bytes:
    """Serialize a .tmod from (path, bytes) files + header properties. Returns the bytes.

    `modLoader` is always (re)stamped to `mod_loader` (the API uses "KiwiAPI",
    where BTT uses "BTT"). Paths are normalized to lowercase posix.
    """
    if not files:
        raise TmodError("a .tmod needs at least one file")

    props = dict(properties)
    props["modLoader"] = mod_loader  # set or override — never trust a client-sent value

    file_stream = bytearray()
    files_table = bytearray()
    offset = 0
    for raw_path, content in files:
        path = raw_path.replace("\\", "/").lstrip("/").lower()
        path_bytes = path.encode("utf-8")
        if len(path_bytes) > 255:
            raise TmodError(f"internal path too long for the .tmod format (>255 bytes): {path}")
        size = len(content)
        padded = content + b"\x00" * ((4 - size % 4) % 4)
        files_table += bytes([len(path_bytes)]) + path_bytes
        files_table += write_leb128(0)                       # index (always 0)
        files_table += write_leb128(offset if size else 0)
        files_table += write_leb128(size)
        files_table += write_leb128(calculate_hash(content))
        file_stream += padded
        offset += len(padded)

    compressor = zlib.compressobj(level=0, strategy=0, wbits=zlib.MAX_WBITS)
    compressed = bytearray()
    for i in range(0, len(file_stream), _CHUNK):
        compressed += compressor.compress(bytes(file_stream[i:i + _CHUNK]))
    compressed += compressor.flush(zlib.Z_SYNC_FLUSH)

    props_stream = bytearray()
    for name, value in props.items():
        nb, vb = name.encode("utf-8"), value.encode("utf-8")
        props_stream += write_leb128(len(nb)) + nb
        props_stream += write_leb128(len(vb)) + vb

    header = bytearray()
    header += (0).to_bytes(8, "little")          # header_size placeholder
    header += version.to_bytes(2, "little")
    header += len(props).to_bytes(2, "little")
    header += props_stream
    header += files_table
    header[0:8] = len(header).to_bytes(8, "little")  # backfill real header size

    return bytes(header) + bytes(compressed)
