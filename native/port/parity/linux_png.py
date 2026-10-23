"""Pillow PNGs as production (Linux) writes them, from the Windows venv.

Pillow's Windows wheel deflates with zlib-ng; the Linux wheels production runs
link classic zlib 1.3.1, the same library as CPython's `zlib` module. The
filtered scanlines are the same on both, so a Windows PNG becomes the Linux one
by inflating its IDAT data and deflating it again with CPython's zlib exactly
as `ZipEncode.c` feeds it (deflateInit2(level, 8, 15, 9, strategy), one
`compress` per scanline, then Z_FINISH; level 9 for optimize=True, else 6;
Z_FILTERED, or the default strategy for palette images), cut into IDAT chunks
of the encoder buffer (`max(MAXBLOCK, width * 4)`, plus an empty one when the
stream fills the last buffer exactly). Classic zlib's bytes do not depend on
how its input or output is split (proven by zlib_corpus.py), so CPython's
compressobj stands in for Pillow's encoder calls.

    python linux_png.py run GENERATOR.py [ARGS...]    PNGs the app saves are the Linux ones
    python linux_png.py files PATH...                  rewrite .png files in place
    python linux_png.py json FILE KEY_PATH...          rewrite base64 PNGs at KEY_PATH (a/[]/b) in place

Run with S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe from the repo root.
"""

from __future__ import annotations

import base64
import io
import json
import runpy
import struct
import sys
import zlib
from pathlib import Path

MAXBLOCK = 65536
CHANNELS = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}


def _chunks(png: bytes) -> list[tuple[bytes, bytes]]:
    assert png[:8] == b"\x89PNG\r\n\x1a\n", "not a PNG"
    out, pos = [], 8
    while pos < len(png):
        n, kind = struct.unpack(">I4s", png[pos:pos + 8])
        out.append((kind, png[pos + 8:pos + 8 + n]))
        pos += 12 + n
    return out


def _chunk(kind: bytes, data: bytes) -> bytes:
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))


def linux_png(png: bytes) -> bytes:
    chunks = _chunks(png)
    kinds = [k for k, _ in chunks]
    first, last = kinds.index(b"IDAT"), len(kinds) - 1 - kinds[::-1].index(b"IDAT")
    assert all(k == b"IDAT" for k in kinds[first:last + 1]), "IDAT chunks not contiguous"
    width, _height, depth, color, _c, _f, interlace = struct.unpack(">IIBBBBB", chunks[0][1])
    assert chunks[0][0] == b"IHDR" and not interlace
    bufsize = max(MAXBLOCK, width * 4)
    parts = [d for _, d in chunks[first:last + 1]]
    assert all(len(p) == bufsize for p in parts[:-1]), "IDAT layout is not Pillow's"
    stream = b"".join(parts)
    level = {2: 6, 3: 9}[stream[1] >> 6]
    raw = zlib.decompress(stream)
    row = 1 + (width * CHANNELS[color] * depth + 7) // 8
    z = zlib.compressobj(level, zlib.DEFLATED, 15, 9, zlib.Z_DEFAULT_STRATEGY if color == 3 else zlib.Z_FILTERED)
    new = b"".join(z.compress(raw[i:i + row]) for i in range(0, len(raw), row)) + z.flush()
    idat = [new[i:i + bufsize] for i in range(0, len(new), bufsize)]
    if len(new) % bufsize == 0:
        idat.append(b"")
    out = b"\x89PNG\r\n\x1a\n" + b"".join(_chunk(k, d) for k, d in chunks[:first])
    out += b"".join(_chunk(b"IDAT", d) for d in idat)
    out += b"".join(_chunk(k, d) for k, d in chunks[last + 1:])
    return out


def _same_pixels(a: bytes, b: bytes) -> bool:
    from PIL import Image

    ia, ib = Image.open(io.BytesIO(a)), Image.open(io.BytesIO(b))
    return (ia.mode, ia.size, ia.tobytes()) == (ib.mode, ib.size, ib.tobytes())


def install(keep: str | None = None) -> None:
    """Every PNG Pillow saves comes out as the Linux one, except those saved
    directly from the file ``keep`` (a generator's own test inputs)."""
    from PIL import Image, PngImagePlugin

    Image.init()
    original = PngImagePlugin._save
    keep = keep and str(Path(keep).resolve())

    def save(im, fp, filename, chunk=PngImagePlugin.putchunk, save_all=False):
        buf = io.BytesIO()
        original(im, buf, filename, chunk, save_all)
        caller = sys._getframe(2).f_code.co_filename
        own_input = keep is not None and str(Path(caller).resolve()) == keep
        fp.write(buf.getvalue() if own_input else linux_png(buf.getvalue()))

    Image.register_save("PNG", save)


def _rewrite(png: bytes) -> bytes:
    new = linux_png(png)
    assert _same_pixels(png, new)
    return new


def _json(path: Path, key_paths: list[str]) -> int:
    text = path.read_text(encoding="utf-8")
    found: list[str] = []

    def walk(v, keys: list[str]) -> None:
        if not keys:
            if isinstance(v, str):
                found.append(v)
            return
        k, rest = keys[0], keys[1:]
        if k == "[]" and isinstance(v, list):
            for x in v:
                walk(x, rest)
        elif isinstance(v, dict) and k in v:
            walk(v[k], rest)

    doc = json.loads(text)
    for kp in key_paths:
        walk(doc, kp.split("/"))
    changed = 0
    for old in found:
        new = base64.b64encode(_rewrite(base64.b64decode(old))).decode()
        if new != old:
            assert text.count(f'"{old}"') >= 1
            text = text.replace(f'"{old}"', f'"{new}"')
            changed += 1
    path.write_text(text, encoding="utf-8", newline="\n")
    return changed


def main() -> None:
    mode, args = sys.argv[1], sys.argv[2:]
    if mode == "run":
        install(keep=args[0])
        sys.argv = args
        runpy.run_path(args[0], run_name="__main__")
    elif mode == "files":
        n = 0
        for p in map(Path, args):
            old = p.read_bytes()
            new = _rewrite(old)
            if new != old:
                p.write_bytes(new)
                n += 1
        print(f"{n} of {len(args)} files rewritten")
    elif mode == "json":
        print(f"{_json(Path(args[0]), args[1:])} PNGs rewritten in {args[0]}")
    else:
        raise SystemExit(__doc__)


if __name__ == "__main__":
    main()
