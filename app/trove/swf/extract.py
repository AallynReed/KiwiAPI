"""Read the tag stream of a SWF and pull out every embedded bitmap as a PNG.

A SWF is a header followed by a flat list of tags. The ones that matter here are
the five bitmap tags - each defines one image against a *character id*, the
numeric handle the rest of the movie refers to it by:

======  =========================  ====================================
tag     name                       payload
======  =========================  ====================================
6       DefineBits                 JPEG, minus its tables (see tag 8)
21      DefineBitsJPEG2            self-contained JPEG / PNG / GIF89a
35      DefineBitsJPEG3            as above, plus a zlib alpha channel
90      DefineBitsJPEG4            as JPEG3, plus a deblocking parameter
20/36   DefineBitsLossless(2)      zlib-deflated raw pixels, 4 layouts
======  =========================  ====================================

Everything is normalised to RGBA PNG on the way out, because a browser cannot
display half of these as-is: the lossless tags are not an image format at all,
and a JPEG3's transparency lives in a separate deflate stream that has to be
composited back on by hand.

Character ids are meaningless to a human, so :func:`extract_images` also tries to
recover a *name* for each bitmap. Only exported symbols carry one, and a bitmap
is almost never exported directly - it is filled into a shape, which is placed
in a sprite, which is what gets the name. So names are resolved by walking that
reference graph backwards. It is best-effort by nature: a bitmap that only ever
gets attached from ActionScript has no static reference to follow, and stays
unnamed. Dimensions, format and the thumbnail itself are always available, which
is what actually makes the set browsable.
"""

from __future__ import annotations

import io
import logging
import lzma
import struct
import zlib
from collections import deque
from dataclasses import dataclass, field

from PIL import Image

logger = logging.getLogger("kiwi.swf")

# A malformed or hostile file must not be able to exhaust the box. These bound
# the work before any of it is done.
MAX_INFLATED = 256 * 1024 * 1024   # refuse absurd decompression bombs
MAX_IMAGES = 4000                  # per movie
MAX_PIXELS = 8192 * 8192           # per image

BITMAP_TAGS = frozenset({6, 20, 21, 35, 36, 90})
SHAPE_TAGS = {2: 1, 22: 2, 32: 3, 83: 4}       # tag -> DefineShape version
PLACE_TAGS = frozenset({4, 26, 70})

# Non-bitmap tags worth counting for the "what else is in here" summary.
_INVENTORY = {
    "shapes": frozenset(SHAPE_TAGS),
    "sprites": frozenset({39}),
    "fonts": frozenset({10, 48, 75, 91}),
    "texts": frozenset({11, 33, 37}),
    "sounds": frozenset({14, 18, 45}),
    "morphs": frozenset({46, 84}),
    "buttons": frozenset({7, 34}),
    "videos": frozenset({60}),
    "binary": frozenset({87}),
    "scripts": frozenset({12, 59, 82}),
}


class SwfError(Exception):
    """The bytes are not a SWF we can read."""


@dataclass(slots=True)
class SwfHeader:
    version: int
    compression: str            # "none" | "zlib" | "lzma"
    width: int                  # pixels (the file stores twips)
    height: int
    frame_rate: float
    frame_count: int


@dataclass(slots=True)
class SwfImage:
    char_id: int
    name: str | None
    source: str                 # tag name it came from
    codec: str                  # how it was stored in the movie
    width: int
    height: int
    data: bytes = field(repr=False)     # full size, ready for an <img>
    mime: str = "image/png"
    thumb: bytes | None = field(default=None, repr=False)   # PNG, None when data is small enough


# ─── container ────────────────────────────────────────────────────────────

def _decompress(raw: bytes) -> bytes:
    """Return the movie with its body inflated, header rewritten to ``FWS``."""
    if len(raw) < 8:
        raise SwfError("truncated header")
    sig, version = raw[:3], raw[3]
    head = b"FWS" + bytes([version]) + raw[4:8]
    if sig == b"FWS":
        return raw
    if sig == b"CWS":
        try:
            obj = zlib.decompressobj()
            body = obj.decompress(raw[8:], MAX_INFLATED)
        except zlib.error as exc:
            raise SwfError(f"bad zlib body: {exc}") from None
        if not obj.eof and obj.unconsumed_tail:
            raise SwfError("decompression bomb")
        return head + body
    if sig == b"ZWS":
        # ZWS puts a 4-byte compressed length before the 5 raw LZMA properties,
        # and omits the end marker - so it needs FORMAT_RAW, not FORMAT_ALONE.
        if len(raw) < 17:
            raise SwfError("truncated lzma header")
        try:
            filt = lzma._decode_filter_properties(lzma.FILTER_LZMA1, raw[12:17])  # noqa: SLF001
            dec = lzma.LZMADecompressor(format=lzma.FORMAT_RAW, filters=[filt])
            body = dec.decompress(raw[17:], MAX_INFLATED)
        except (lzma.LZMAError, ValueError) as exc:
            raise SwfError(f"bad lzma body: {exc}") from None
        return head + body
    raise SwfError(f"not a SWF (signature {sig!r})")


def _iter_tags(data: bytes):
    """Yield ``(code, body)`` for each top-level tag, skipping the header."""
    buf = io.BytesIO(data[8:])
    _skip_rect(buf)
    buf.seek(4, 1)                        # frame rate + frame count
    while True:
        head = buf.read(2)
        if len(head) < 2:
            return
        (packed,) = struct.unpack("<H", head)
        code, length = packed >> 6, packed & 0x3F
        if length == 0x3F:
            raw = buf.read(4)
            if len(raw) < 4:
                return
            (length,) = struct.unpack("<I", raw)
        body = buf.read(length)
        if len(body) < length:
            return                        # truncated tail - take what we parsed
        yield code, body
        if code == 0:
            return


def _skip_rect(buf: io.BytesIO) -> None:
    """Skip a RECT: 5-bit width field, then four fields of that width."""
    first = buf.read(1)
    if not first:
        raise SwfError("truncated rect")
    nbits = first[0] >> 3
    buf.seek((5 + nbits * 4 + 7) // 8 - 1, 1)


def read_header(raw: bytes) -> SwfHeader:
    data = _decompress(raw)
    if len(data) < 9:
        raise SwfError("truncated header")
    nbits = data[8] >> 3
    span = (5 + nbits * 4 + 7) // 8
    bits = _BitReader(data[8 : 8 + span])
    bits.read(5)
    xmin, xmax, ymin, ymax = (bits.read_signed(nbits) for _ in range(4))
    tail = data[8 + span : 12 + span]
    if len(tail) < 4:
        raise SwfError("truncated header")
    rate_lo, rate_hi, frames = struct.unpack("<BBH", tail)
    return SwfHeader(
        version=data[3],
        compression={b"FWS": "none", b"CWS": "zlib", b"ZWS": "lzma"}[raw[:3]],
        width=max(0, (xmax - xmin)) // 20,
        height=max(0, (ymax - ymin)) // 20,
        frame_rate=rate_hi + rate_lo / 256,
        frame_count=frames,
    )


class _BitReader:
    __slots__ = ("_b", "_p")

    def __init__(self, b: bytes) -> None:
        self._b, self._p = b, 0

    def read(self, n: int) -> int:
        v = 0
        for _ in range(n):
            idx = self._p >> 3
            if idx >= len(self._b):
                raise SwfError("bit overrun")
            v = (v << 1) | ((self._b[idx] >> (7 - (self._p & 7))) & 1)
            self._p += 1
        return v

    def read_signed(self, n: int) -> int:
        v = self.read(n)
        return v - (1 << n) if n and (v >> (n - 1)) else v

    def align(self) -> None:
        self._p = (self._p + 7) & ~7

    @property
    def byte_pos(self) -> int:
        return (self._p + 7) >> 3


# ─── bitmap decoding ──────────────────────────────────────────────────────

def _decode_lossless(body: bytes, tag: int) -> tuple[int, Image.Image, str]:
    """DefineBitsLossless(2): zlib-deflated raw pixels in one of four layouts."""
    if len(body) < 7:
        raise SwfError("short lossless tag")
    char_id, fmt, width, height = struct.unpack("<HBHH", body[:7])
    if not width or not height or width * height > MAX_PIXELS:
        raise SwfError(f"bad lossless size {width}x{height}")
    off = 7
    colors = 0
    if fmt == 3:
        colors = body[off] + 1
        off += 1
    try:
        data = zlib.decompress(body[off:])
    except zlib.error as exc:
        raise SwfError(f"bad lossless zlib: {exc}") from None

    has_alpha = tag == 36
    if fmt == 3:
        # Indexed. Palette first, then one byte per pixel, rows padded to 4.
        stride = (width + 3) & ~3
        psize = 4 if has_alpha else 3
        pal = data[: colors * psize]
        rows = data[colors * psize :]
        if len(pal) < colors * psize:
            raise SwfError("truncated palette")
        table = bytearray(colors * 4)
        for i in range(colors):
            c = pal[i * psize : i * psize + psize]
            table[i * 4 : i * 4 + 4] = c if has_alpha else bytes(c) + b"\xff"
        out = bytearray(width * height * 4)
        for y in range(height):
            row = rows[y * stride : y * stride + width]
            base = y * width * 4
            for x, idx in enumerate(row):
                p = idx * 4
                out[base + x * 4 : base + x * 4 + 4] = (
                    table[p : p + 4] if p + 4 <= len(table) else b"\0\0\0\0"
                )
        codec = f"indexed/{colors}"
    elif fmt == 4:
        # PIX15: 16 bits per pixel, one dead bit then 5:5:5, rows padded to 4.
        stride = (width * 2 + 3) & ~3
        out = bytearray(width * height * 4)
        for y in range(height):
            row = data[y * stride : y * stride + width * 2]
            base = y * width * 4
            for x in range(min(width, len(row) // 2)):
                v = (row[x * 2] << 8) | row[x * 2 + 1]
                o = base + x * 4
                out[o] = ((v >> 10) & 31) * 255 // 31
                out[o + 1] = ((v >> 5) & 31) * 255 // 31
                out[o + 2] = (v & 31) * 255 // 31
                out[o + 3] = 255
        codec = "rgb555"
    elif fmt == 5:
        # PIX24/32: stored ARGB, and *premultiplied* when the tag carries alpha.
        # Un-premultiplying is what keeps soft edges from going muddy.
        need = width * height * 4
        if len(data) < need:
            data = data + b"\0" * (need - len(data))
        out = bytearray(need)
        for i in range(width * height):
            a, r, g, b = data[i * 4 : i * 4 + 4]
            o = i * 4
            if not has_alpha:
                out[o : o + 4] = bytes((r, g, b, 255))
            elif a == 0:
                out[o : o + 4] = b"\0\0\0\0"
            elif a == 255:
                out[o : o + 4] = bytes((r, g, b, 255))
            else:
                out[o : o + 4] = bytes((
                    min(255, r * 255 // a), min(255, g * 255 // a),
                    min(255, b * 255 // a), a,
                ))
        codec = "argb32" if has_alpha else "rgb24"
    else:
        raise SwfError(f"unknown lossless format {fmt}")

    img = Image.frombytes("RGBA", (width, height), bytes(out))
    return char_id, img, codec


def _sniff(data: bytes) -> str:
    if data[:8] == b"\x89PNG\r\n\x1a\n":
        return "png"
    if data[:3] == b"GIF":
        return "gif"
    return "jpeg"


def _decode_jpeg(
    body: bytes, tag: int, tables: bytes | None,
) -> tuple[int, Image.Image, str, bytes | None]:
    """DefineBits / JPEG2 / JPEG3 / JPEG4.

    The fourth element is the original payload when it can be served untouched -
    it is already a format a browser understands and we changed nothing. That is
    the common case for opaque art, and it keeps a 40 KB JPEG from being re-encoded
    into a 400 KB PNG. It is None when the pixels had to be rebuilt (an alpha plane
    composited back on, or JPEG tables spliced in).
    """
    if len(body) < 2:
        raise SwfError("short jpeg tag")
    char_id = struct.unpack("<H", body[:2])[0]
    off = 2
    alpha_blob = b""
    if tag in (35, 90):
        if tag == 90:
            off += 2                                   # DeblockParam
        (alpha_off,) = struct.unpack("<I", body[off : off + 4])
        off += 4
        payload = body[off : off + alpha_off]
        alpha_blob = body[off + alpha_off :]
    else:
        payload = body[off:]

    spliced = False
    if tag == 6 and tables:
        spliced = True
        # DefineBits normally carries only scan data, with the quantisation and
        # Huffman tables held once in the movie's JPEGTables tag. But that tag is
        # allowed to be empty, in which case the payload is already a whole JPEG
        # and splicing anything in front of it would corrupt it.
        payload = (
            tables[:-2] + payload[2:] if tables[-2:] == b"\xff\xd9" else tables + payload
        )

    codec = _sniff(payload)
    # Flash tolerates a stray EOI/SOI pair at the JPEG splice point; Pillow does not.
    if codec == "jpeg":
        cleaned = payload.replace(b"\xff\xd9\xff\xd8", b"", 1)
        spliced = spliced or cleaned != payload
        payload = cleaned
    try:
        img = Image.open(io.BytesIO(payload))
        img.load()
    except Exception as exc:                            # noqa: BLE001 - Pillow raises broadly
        raise SwfError(f"undecodable {codec}: {exc}") from None
    if img.width * img.height > MAX_PIXELS:
        raise SwfError("image too large")

    plane = b""
    if alpha_blob:
        try:
            plane = zlib.decompress(alpha_blob)
        except zlib.error:
            plane = b""
    # An alpha plane that is opaque everywhere carries no information - dropping it
    # lets the original JPEG through instead of forcing a re-encode.
    if plane and len(plane) >= img.width * img.height:
        plane = plane[: img.width * img.height]
        if min(plane) == 255:
            plane = b""
    else:
        plane = b""

    img = img.convert("RGBA")
    if plane:
        img.putalpha(Image.frombytes("L", img.size, plane))
        return char_id, img, codec, None
    if spliced or img.mode not in ("RGB", "RGBA", "P", "L"):
        return char_id, img, codec, None
    return char_id, img, codec, payload


# ─── name recovery ────────────────────────────────────────────────────────

def _skip_matrix(bits: _BitReader) -> None:
    if bits.read(1):                       # HasScale
        n = bits.read(5)
        bits.read(n * 2)
    if bits.read(1):                       # HasRotate
        n = bits.read(5)
        bits.read(n * 2)
    n = bits.read(5)                       # TranslateBits (always present)
    bits.read(n * 2)
    bits.align()


def _shape_bitmap_refs(body: bytes, version: int) -> list[int]:
    """Bitmap character ids referenced by a DefineShape's fill styles."""
    bits = _BitReader(body)
    bits.read(16)                          # ShapeId
    n = bits.read(5)                       # ShapeBounds RECT
    bits.read(n * 4)
    bits.align()
    if version == 4:
        n = bits.read(5)                   # EdgeBounds RECT
        bits.read(n * 4)
        bits.align()
        bits.read(8)                       # UsesFillWindingRule / non-scaling flags

    off = bits.byte_pos
    if off >= len(body):
        return []
    count = body[off]
    off += 1
    if count == 0xFF:
        if off + 2 > len(body):
            return []
        count = struct.unpack("<H", body[off : off + 2])[0]
        off += 2

    rgba = version >= 3
    refs: list[int] = []
    for _ in range(count):
        if off >= len(body):
            break
        style = body[off]
        off += 1
        if style == 0x00:
            off += 4 if rgba else 3
        elif style in (0x10, 0x12, 0x13):
            sub = _BitReader(body[off:])
            _skip_matrix(sub)
            off += sub.byte_pos
            if off >= len(body):
                break
            nrec = body[off] & 0x0F
            off += 1 + nrec * (5 if rgba else 4)
            if style == 0x13:
                off += 2               # FocalPoint FIXED8
        elif style in (0x40, 0x41, 0x42, 0x43):
            if off + 2 > len(body):
                break
            refs.append(struct.unpack("<H", body[off : off + 2])[0])
            off += 2
            sub = _BitReader(body[off:])
            _skip_matrix(sub)
            off += sub.byte_pos
        else:
            break                      # unknown style - the rest is unreadable
    return refs


def _sprite_child_refs(body: bytes) -> list[int]:
    """Character ids placed on a DefineSprite's timeline."""
    refs: list[int] = []
    buf = io.BytesIO(body[4:])             # SpriteId + FrameCount
    while True:
        head = buf.read(2)
        if len(head) < 2:
            break
        (packed,) = struct.unpack("<H", head)
        code, length = packed >> 6, packed & 0x3F
        if length == 0x3F:
            raw = buf.read(4)
            if len(raw) < 4:
                break
            (length,) = struct.unpack("<I", raw)
        nested = buf.read(length)
        if code == 0:
            break
        if code not in PLACE_TAGS or len(nested) < 4:
            continue
        if code == 4:
            refs.append(struct.unpack("<H", nested[:2])[0])
        elif code == 26 and nested[0] & 0x02:          # PlaceFlagHasCharacter
            refs.append(struct.unpack("<H", nested[3:5])[0])
        elif code == 70 and nested[0] & 0x02:
            # PlaceObject3 slots two extra flag bytes plus an optional class name
            # before the character id.
            off = 3
            if nested[1] & 0x08:                        # HasClassName
                end = nested.find(b"\x00", off)
                off = len(nested) if end < 0 else end + 1
            off += 2                                    # Depth
            if off + 2 <= len(nested):
                refs.append(struct.unpack("<H", nested[off : off + 2])[0])
    return refs


def _read_symbol_names(body: bytes) -> dict[int, str]:
    """SymbolClass / ExportAssets - both are ``count`` then ``(id, name)`` pairs."""
    out: dict[int, str] = {}
    if len(body) < 2:
        return out
    (count,) = struct.unpack("<H", body[:2])
    off = 2
    for _ in range(count):
        if off + 2 > len(body):
            break
        char_id = struct.unpack("<H", body[off : off + 2])[0]
        off += 2
        end = body.find(b"\x00", off)
        if end < 0:
            break
        out[char_id] = body[off:end].decode("utf-8", "replace")
        off = end + 1
    return out


def _resolve_names(
    bitmaps: set[int],
    symbols: dict[int, str],
    edges: dict[int, list[int]],
) -> dict[int, str]:
    """Name each bitmap after the nearest named character that reaches it.

    ``edges`` maps a character id to the ids it references (sprite -> placed
    children, shape -> filled bitmaps). Walking outward from every named symbol
    at once, breadth-first, gives each bitmap the closest owner rather than an
    arbitrary one.
    """
    named: dict[int, str] = {b: symbols[b] for b in bitmaps if b in symbols}
    queue = deque((cid, name) for cid, name in symbols.items())
    seen: set[int] = set(symbols)
    while queue:
        cid, name = queue.popleft()
        for child in edges.get(cid, ()):
            if child in bitmaps:
                named.setdefault(child, name)
            if child not in seen:
                seen.add(child)
                queue.append((child, name))
    return named


# ─── public entry points ──────────────────────────────────────────────────

THUMB_BOX = 128          # longest edge of a gallery thumbnail, in pixels

_MIME = {"png": "image/png", "gif": "image/gif", "jpeg": "image/jpeg"}


def _pack_png(img: Image.Image) -> bytes:
    out = io.BytesIO()
    img.save(out, format="PNG", optimize=False, compress_level=6)
    return out.getvalue()


def _make_thumb(img: Image.Image) -> bytes | None:
    """A grid-sized PNG, or None when the full image is already small enough that a
    separate thumbnail would just be a second copy of it."""
    if img.width <= THUMB_BOX and img.height <= THUMB_BOX:
        return None
    small = img.copy()
    small.thumbnail((THUMB_BOX, THUMB_BOX), Image.LANCZOS)
    return _pack_png(small)


def extract_images(raw: bytes) -> tuple[SwfHeader, list[SwfImage], dict[str, int]]:
    """Decode a movie into ``(header, images, inventory)``.

    Undecodable individual tags are logged and skipped - one bad bitmap should
    not cost the caller the other forty. Raises :class:`SwfError` only when the
    container itself cannot be read.
    """
    data = _decompress(raw)
    header = read_header(raw)

    tables: bytes | None = None
    pending: list[tuple[int, bytes]] = []
    symbols: dict[int, str] = {}
    edges: dict[int, list[int]] = {}
    inventory: dict[str, int] = dict.fromkeys(_INVENTORY, 0)

    for code, body in _iter_tags(data):
        if code == 8:
            tables = body
        elif code in BITMAP_TAGS:
            if len(pending) < MAX_IMAGES:
                pending.append((code, body))
        elif code in (56, 76):
            symbols.update(_read_symbol_names(body))
        elif code in SHAPE_TAGS:
            if len(body) >= 2:
                shape_id = struct.unpack("<H", body[:2])[0]
                try:
                    refs = _shape_bitmap_refs(body, SHAPE_TAGS[code])
                except SwfError:
                    refs = []
                if refs:
                    edges.setdefault(shape_id, []).extend(refs)
        elif code == 39 and len(body) >= 2:
            sprite_id = struct.unpack("<H", body[:2])[0]
            edges.setdefault(sprite_id, []).extend(_sprite_child_refs(body))

        for key, codes in _INVENTORY.items():
            if code in codes:
                inventory[key] += 1
                break

    ids = {struct.unpack("<H", b[:2])[0] for _, b in pending if len(b) >= 2}
    names = _resolve_names(ids, symbols, edges)

    images: list[SwfImage] = []
    for code, body in pending:
        try:
            if code in (20, 36):
                char_id, img, codec = _decode_lossless(body, code)
                original = None
                source = "DefineBitsLossless2" if code == 36 else "DefineBitsLossless"
            else:
                char_id, img, codec, original = _decode_jpeg(body, code, tables)
                source = {6: "DefineBits", 21: "DefineBitsJPEG2",
                          35: "DefineBitsJPEG3", 90: "DefineBitsJPEG4"}[code]
        except SwfError as exc:
            logger.info("swf: skipping tag %d: %s", code, exc)
            continue
        except Exception as exc:                        # noqa: BLE001
            logger.info("swf: tag %d failed: %s", code, exc)
            continue
        data = original if original is not None else _pack_png(img)
        mime = _MIME.get(codec, "image/png") if original is not None else "image/png"
        images.append(SwfImage(
            char_id=char_id, name=names.get(char_id), source=source, codec=codec,
            width=img.width, height=img.height, data=data, mime=mime,
            thumb=_make_thumb(img),
        ))

    images.sort(key=lambda i: i.char_id)
    return header, images, inventory


def summarize(raw: bytes) -> dict:
    """Header + inventory only - no pixels decoded."""
    data = _decompress(raw)
    header = read_header(raw)
    inventory = dict.fromkeys(_INVENTORY, 0)
    bitmaps = 0
    for code, _body in _iter_tags(data):
        if code in BITMAP_TAGS:
            bitmaps += 1
            continue
        for key, codes in _INVENTORY.items():
            if code in codes:
                inventory[key] += 1
                break
    return {"header": header, "bitmaps": bitmaps, "inventory": inventory}
