"""Filesystem blob storage for the Mods Hub.

Reuses the same content-addressed store as the update archive
(``app/trove/updates/cas.py``): SHA-256 keyed, sharded, atomic, idempotent.
One store under ``settings.mods_store_dir`` holds everything - versioned file
blobs, compiled ``.tmod`` builds, and banner/preview images - all deduped by
content hash. The store does sync I/O, so every call is wrapped in
``asyncio.to_thread`` to keep the event loop free.
"""

from __future__ import annotations

import asyncio
import hashlib
import struct

from app.core.config import settings
from app.trove.updates.cas import ContentStore

_store = ContentStore(settings.mods_store_dir)


async def put_blob(data: bytes) -> tuple[str, bool]:
    """Store bytes; return ``(sha256_hex, created)``."""
    return await asyncio.to_thread(_store.put, data)


def blob_sha(data: bytes) -> str:
    """The store key bytes WOULD get, without storing them - for hashing an
    artifact we only need to recognise later (an upload we repack before storing)."""
    return hashlib.sha256(data).hexdigest()


async def get_blob(sha: str) -> bytes | None:
    return await asyncio.to_thread(_store.get, sha)


async def has_blob(sha: str) -> bool:
    return await asyncio.to_thread(_store.has, sha)


# --- image sniffing --------------------------------------------------------
# We accept a small whitelist of web image formats. Detection is by magic
# bytes (never trust the client's declared content-type), and dimensions are
# parsed from the header where cheap - purely informational, so a parse miss
# just leaves width/height None rather than rejecting the upload.

_ALLOWED_IMAGE_TYPES = {"image/png", "image/jpeg", "image/webp", "image/gif"}


def sniff_image(data: bytes) -> tuple[str, int | None, int | None] | None:
    """Return ``(content_type, width, height)`` for a recognised image, else None."""
    if len(data) < 24:
        return None
    # PNG: 8-byte signature, then IHDR with width/height as big-endian u32.
    if data[:8] == b"\x89PNG\r\n\x1a\n":
        try:
            w, h = struct.unpack(">II", data[16:24])
            return "image/png", w, h
        except struct.error:
            return "image/png", None, None
    # GIF87a / GIF89a: width/height little-endian u16 at offset 6.
    if data[:6] in (b"GIF87a", b"GIF89a"):
        try:
            w, h = struct.unpack("<HH", data[6:10])
            return "image/gif", w, h
        except struct.error:
            return "image/gif", None, None
    # WEBP: RIFF....WEBP
    if data[:4] == b"RIFF" and data[8:12] == b"WEBP":
        return "image/webp", _webp_dims(data)[0], _webp_dims(data)[1]
    # JPEG: starts with FFD8; scan segments for a SOF marker to read dimensions.
    if data[:2] == b"\xff\xd8":
        return "image/jpeg", *_jpeg_dims(data)
    return None


def _jpeg_dims(data: bytes) -> tuple[int | None, int | None]:
    i, n = 2, len(data)
    while i + 9 < n:
        if data[i] != 0xFF:
            i += 1
            continue
        marker = data[i + 1]
        # SOF0..SOF15 (excluding DHT/JPG/DAC at C4/C8/CC) carry frame dims.
        if 0xC0 <= marker <= 0xCF and marker not in (0xC4, 0xC8, 0xCC):
            try:
                h, w = struct.unpack(">HH", data[i + 5:i + 9])
                return w, h
            except struct.error:
                return None, None
        if marker in (0xD8, 0xD9) or 0xD0 <= marker <= 0xD7:
            i += 2
            continue
        try:
            seg_len = struct.unpack(">H", data[i + 2:i + 4])[0]
        except struct.error:
            return None, None
        i += 2 + seg_len
    return None, None


def _webp_dims(data: bytes) -> tuple[int | None, int | None]:
    fmt = data[12:16]
    try:
        if fmt == b"VP8 ":
            w = struct.unpack("<H", data[26:28])[0] & 0x3FFF
            h = struct.unpack("<H", data[28:30])[0] & 0x3FFF
            return w, h
        if fmt == b"VP8L":
            b = data[21:25]
            bits = b[0] | (b[1] << 8) | (b[2] << 16) | (b[3] << 24)
            return (bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1
        if fmt == b"VP8X":
            w = (data[24] | (data[25] << 8) | (data[26] << 16)) + 1
            h = (data[27] | (data[28] << 8) | (data[29] << 16)) + 1
            return w, h
    except (struct.error, IndexError):
        return None, None
    return None, None
