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
import io
import struct

import numpy as np
from PIL import Image

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


# --- thumbnails ------------------------------------------------------------
# Creator uploads are stored exactly as sent (up to `mods_image_max_bytes`) and
# the hub draws them into 354px cards, so a listing page was shipping ~10 MB of
# full-resolution PNG to paint 18 thumbnails. A width variant is rendered on
# first request and stored in the SAME content-addressed store, so it dedupes
# against everything else and inherits the immutable cache headers.
#
# The widths are an allowlist, not a free parameter: sha+width IS the cache key,
# and an open `?w=` would let anyone mint unbounded derivatives of every image in
# the store. 708 = the 354px card at 2x; 400 the preview tiles; 1416 the hero.
THUMB_WIDTHS = (400, 708, 1416)


def render_thumbnail(data: bytes, width: int) -> bytes | None:
    """Downscale to ``width`` and encode WebP. ``None`` means serve the original.

    Sync and CPU-bound - call it through ``asyncio.to_thread``.

    Returning None rather than raising is deliberate: every reason to decline is
    a case where the original IS the right answer (already small enough, an
    animated GIF a resize would freeze on its first frame, a file Pillow won't
    open), so the caller has nothing to handle.

    Resampling runs on premultiplied alpha. Pillow's LANCZOS averages the colour
    channels without weighting them by opacity, so a logo on transparency picks
    up a dark halo where fully transparent black pixels bleed into the edge.
    """
    try:
        img = Image.open(io.BytesIO(data))
        img.load()
    except Exception:
        return None
    if img.width <= width or getattr(img, "n_frames", 1) > 1:
        return None
    height = max(1, round(img.height * width / img.width))
    has_alpha = img.mode in ("RGBA", "LA") or (img.mode == "P" and "transparency" in img.info)
    try:
        if has_alpha:
            small = _resize_straight_alpha(img, (width, height))
        else:
            small = img.convert("RGB").resize((width, height), Image.LANCZOS)
        buf = io.BytesIO()
        small.save(buf, "WEBP", quality=82, method=4)
    except Exception:
        return None
    return buf.getvalue()


def _resize_straight_alpha(img: Image.Image, size: tuple[int, int]) -> Image.Image:
    arr = np.asarray(img.convert("RGBA"), dtype=np.float32)
    arr[:, :, :3] *= arr[:, :, 3:4] / 255.0
    # No `mode=` on fromarray - deprecated in Pillow 12, gone in 13; a 4-channel
    # uint8 array infers RGBA anyway.
    small = np.asarray(
        Image.fromarray(arr.astype(np.uint8)).resize(size, Image.LANCZOS),
        dtype=np.float32,
    )
    alpha = small[:, :, 3:4]
    rgb = np.divide(small[:, :, :3] * 255.0, alpha,
                    out=np.zeros_like(small[:, :, :3]), where=alpha > 0)
    small[:, :, :3] = np.clip(rgb, 0, 255)
    return Image.fromarray(small.astype(np.uint8))


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
