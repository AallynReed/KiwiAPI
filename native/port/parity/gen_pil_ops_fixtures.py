"""Golden fixtures for kiwi-pil's whole-image operations (src/ops.rs, src/decode.rs
and png::encode_rgba): what Image Studio's renderer calls besides drawing.

Writes native/kiwi-pil/testdata/ops.json. Run from the repo root with the
production-pinned venv:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/linux_png.py run native/port/parity/gen_pil_ops_fixtures.py
"""

from __future__ import annotations

import base64
import hashlib
import io
import json
import random
import struct
import zlib
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "native" / "kiwi-pil" / "testdata" / "ops.json"
RNG = random.Random(1337)


def b64(b: bytes) -> str:
    return base64.b64encode(b).decode()


def pix(im: Image.Image, full: bool = False) -> dict:
    raw = im.tobytes()
    out = {"w": im.width, "h": im.height, "sha": hashlib.sha256(raw).hexdigest()}
    if full or len(raw) <= 6000:
        out["raw"] = b64(raw)
    return out


def rand_l(w: int, h: int) -> Image.Image:
    return Image.frombytes("L", (w, h), bytes(RNG.randrange(256) for _ in range(w * h)))


def rand_rgba(w: int, h: int) -> Image.Image:
    data = bytearray()
    for _ in range(w * h):
        a = RNG.choice([0, 255, RNG.randrange(256), RNG.randrange(256)])
        data += bytes([RNG.randrange(256), RNG.randrange(256), RNG.randrange(256), a])
    return Image.frombytes("RGBA", (w, h), bytes(data))


def resize_cases() -> list:
    out = []
    for (w, h), (nw, nh) in [((7, 5), (13, 9)), ((40, 30), (11, 17)), ((5, 5), (5, 12)), ((9, 4), (31, 4)),
                             ((64, 48), (3, 2)), ((1, 1), (6, 3)), ((33, 21), (32, 22)), ((23, 47), (40, 30)),
                             ((30, 60), (10, 5)), ((50, 40), (45, 10)), ((12, 50), (30, 7))]:
        src = rand_l(w, h)
        out.append({"mode": "L", "src": pix(src, True), "size": [nw, nh], "out": pix(src.resize((nw, nh)))})
        src = rand_rgba(w, h)
        out.append({"mode": "RGBA", "src": pix(src, True), "size": [nw, nh], "out": pix(src.resize((nw, nh)))})
    grad = Image.linear_gradient("L")
    for d in (36, 67, 300):
        out.append({"mode": "gradient", "size": [d, d], "out": pix(grad.resize((d, d)))})
    return out


def rotate_cases() -> list:
    out = []
    grad = Image.linear_gradient("L").resize((60, 60))
    for angle in (0, 15, -30, 45, 90, 180, 270, -90, 123, 359, 721, -405):
        out.append({"src": pix(grad), "angle": angle, "out": pix(grad.rotate(angle, resample=Image.BILINEAR))})
    rect = rand_l(23, 14)
    for angle in (90, 33, 270, 180):
        out.append({"src": pix(rect), "angle": angle, "out": pix(rect.rotate(angle, resample=Image.BILINEAR))})
    return out


def composite_cases() -> list:
    out = []
    for w, h in ((9, 7), (16, 3)):
        a, b, m = rand_rgba(w, h), rand_rgba(w, h), rand_l(w, h)
        out.append({"op": "composite", "a": pix(a), "b": pix(b), "mask": pix(m), "out": pix(Image.composite(a, b, m))})
    for (dw, dh), (sw, sh), (dx, dy) in [((12, 9), (5, 4), (3, 2)), ((12, 9), (8, 8), (-3, 4)), ((6, 6), (9, 9), (2, -5)),
                                         ((6, 6), (3, 3), (6, 0)), ((10, 10), (10, 10), (0, 0))]:
        dst, src, m = rand_rgba(dw, dh), rand_rgba(sw, sh), rand_l(sw, sh)
        ac = dst.copy()
        ac.alpha_composite(src, (dx, dy))
        out.append({"op": "alpha_composite", "dst": pix(dst), "src": pix(src), "at": [dx, dy], "out": pix(ac)})
        pm = dst.copy()
        pm.paste(src, (dx, dy), m)
        out.append({"op": "paste_masked", "dst": pix(dst), "src": pix(src), "mask": pix(m), "at": [dx, dy],
                    "out": pix(pm)})
    for op in (0.5, 1.5, -0.25, 0.999):
        src = rand_rgba(5, 4)
        alpha = src.getchannel("A").point(lambda a, o=op: int(a * o))
        out.append({"op": "point", "src": pix(src), "opacity": op, "out": pix(alpha)})
    return out


def chunk(kind: bytes, data: bytes) -> bytes:
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)


def raw_png(w: int, h: int, depth: int, color: int, rows: list[bytes], plte: bytes | None = None,
            trns: bytes | None = None) -> bytes:
    body = b"".join(b"\0" + r for r in rows)
    out = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, depth, color, 0, 0, 0))
    if plte is not None:
        out += chunk(b"PLTE", plte)
    if trns is not None:
        out += chunk(b"tRNS", trns)
    return out + chunk(b"IDAT", zlib.compress(body)) + chunk(b"IEND", b"")


def rand_rows(w: int, h: int, depth: int, channels: int) -> list[bytes]:
    nbytes = (w * depth * channels + 7) // 8
    return [bytes(RNG.randrange(256) for _ in range(nbytes)) for _ in range(h)]


def decode_cases() -> list:
    out = []
    w, h = 7, 5
    specs = [
        (1, 0, None, None), (2, 0, None, None), (4, 0, None, None), (8, 0, None, None),
        (8, 0, None, struct.pack(">H", 77)), (2, 0, None, struct.pack(">H", 3)), (1, 0, None, struct.pack(">H", 0)),
        (8, 2, None, None), (16, 2, None, None), (8, 2, None, struct.pack(">HHH", 10, 20, 30)),
        (8, 4, None, None), (16, 4, None, None), (8, 6, None, None), (16, 6, None, None),
    ]
    channels = {0: 1, 2: 3, 4: 2, 6: 4, 3: 1}
    for depth, color, plte, trns in specs:
        rows = rand_rows(w, h, depth, channels[color])
        if color == 0 and depth == 8 and trns:
            rows[0] = bytes([77]) + rows[0][1:]
        if color == 2 and depth == 8 and trns:
            rows[1] = bytes([10, 20, 30]) + rows[1][3:]
        out.append(raw_png(w, h, depth, color, rows, plte, trns))
    for depth in (1, 2, 4, 8):
        n = 1 << depth
        plte = bytes(RNG.randrange(256) for _ in range(3 * max(1, n - 1)))
        rows = rand_rows(w, h, depth, 1)
        out.append(raw_png(w, h, depth, 3, rows, plte, None))
        out.append(raw_png(w, h, depth, 3, rows, plte, bytes([255, 0, 255])))
        out.append(raw_png(w, h, depth, 3, rows, plte, bytes(RNG.randrange(256) for _ in range(min(n, 5)))))
    buf = io.BytesIO()
    rand_rgba(9, 6).save(buf, "PNG")
    out.append(buf.getvalue())
    good = raw_png(w, h, 8, 2, rand_rows(w, h, 8, 3))
    idat = good.index(b"IDAT")
    crc_at = idat + 4 + struct.unpack(">I", good[idat - 4:idat])[0]
    out.append(good[:crc_at] + b"\0\0\0\0" + good[crc_at + 4:])
    out.append(good[:29] + b"\0\0\0\0" + good[33:])
    cases = []
    for data in out:
        try:
            got = pix(Image.open(io.BytesIO(data)).convert("RGBA"))
        except Exception as e:  # noqa: BLE001
            got = {"error": type(e).__name__}
        cases.append({"png": b64(data), "out": got})
    return cases


def encode_cases() -> list:
    out = []
    for w, h in ((5, 3), (40, 31), (150, 80)):
        im = rand_rgba(w, h) if w < 100 else Image.linear_gradient("L").resize((w, h)).convert("RGBA")
        buf = io.BytesIO()
        im.save(buf, "PNG")
        out.append({"src": pix(im, True), "png": b64(buf.getvalue())})
    return out


def main() -> None:
    data = {
        "resize": resize_cases(),
        "rotate": rotate_cases(),
        "composite": composite_cases(),
        "decode": decode_cases(),
        "encode": encode_cases(),
    }
    OUT.write_text(json.dumps(data, separators=(",", ":")), encoding="utf-8")
    print(OUT, OUT.stat().st_size)


if __name__ == "__main__":
    main()
