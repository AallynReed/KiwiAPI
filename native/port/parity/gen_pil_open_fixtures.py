"""Golden fixtures for Image.open over the formats Image Studio and the Mods Hub
take, and for the hub's WebP thumbnails, from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_pil_open_fixtures.py
Writes native/kiwi-server/testdata/images/pil.json: each input (base64), what
`Image.open(...).convert("RGBA")` gives, `n_frames`, and the real
app/trove/mods_hub/store.render_thumbnail at every hub width.
"""
import base64
import hashlib
import io
import json
import os
import sys
import warnings
from pathlib import Path

warnings.filterwarnings("ignore")
ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "images" / "pil.json"
os.chdir(ROOT)
sys.path.insert(0, str(ROOT))
os.environ.setdefault("SECRET_KEY", "pil-fixture-secret")

from PIL import Image, ImageDraw  # noqa: E402

from app.trove.mods_hub import store  # noqa: E402


def scene(w, h, alpha=False, seed=1):
    im = Image.new("RGB", (w, h))
    px = im.load()
    s = seed
    for y in range(h):
        for x in range(w):
            s = (s * 1103515245 + 12345) & 0x7FFFFFFF
            n = (s >> 16) % 7
            px[x, y] = ((x * 255 // w + n) % 256, (y * 255 // h + 2 * n) % 256, ((x + y) * 3 + n) % 256)
    d = ImageDraw.Draw(im)
    d.ellipse([w // 5, h // 5, w * 3 // 5, h * 4 // 5], fill=(240, 30, 60))
    d.rectangle([w // 2, h // 6, w * 5 // 6, h // 2], fill=(20, 200, 90))
    d.line([0, h - 1, w - 1, 0], fill=(255, 255, 255), width=3)
    if not alpha:
        return im
    a = Image.new("L", (w, h))
    ap = a.load()
    for y in range(h):
        for x in range(w):
            ap[x, y] = 0 if x < w // 6 else min(255, (x * 300 // w + y * 100 // h))
    im.putalpha(a)
    return im


def save(im, fmt, **kw):
    b = io.BytesIO()
    im.save(b, fmt, **kw)
    return b.getvalue()


def cases():
    big, big_a = scene(420, 280), scene(420, 280, alpha=True)
    small, small_a = scene(150, 100, seed=7), scene(150, 100, alpha=True, seed=9)
    tall = scene(60, 420, seed=3)
    out = {
        "jpeg_420": save(big, "JPEG", quality=85),
        "jpeg_444": save(big, "JPEG", quality=92, subsampling=0),
        "jpeg_422": save(big, "JPEG", quality=80, subsampling=1),
        "jpeg_prog": save(big, "JPEG", quality=80, progressive=True),
        "jpeg_gray": save(big.convert("L"), "JPEG", quality=88),
        "jpeg_cmyk": save(big.convert("CMYK"), "JPEG", quality=90),
        "jpeg_small": save(small, "JPEG", quality=95),
        "jpeg_tall": save(tall, "JPEG", quality=90),
        "mpo": save(small, "MPO", save_all=True, append_images=[small_a.convert("RGB")]),
        "webp_lossy": save(big, "WEBP", quality=75),
        "webp_lossy_alpha": save(big_a, "WEBP", quality=70),
        "webp_lossless": save(big_a, "WEBP", lossless=True),
        "webp_small": save(small, "WEBP", quality=90),
        "webp_anim": save(small, "WEBP", save_all=True, append_images=[small_a.convert("RGB")], duration=100),
        "gif_p": save(big.quantize(200), "GIF"),
        "gif_p_trns": save(big.quantize(64), "GIF", transparency=5),
        "gif_l": save(big.convert("L"), "GIF"),
        "gif_small": save(small.quantize(32), "GIF"),
        "gif_anim": save(small.quantize(64), "GIF", save_all=True, append_images=[small_a.convert("RGB").quantize(64)]),
        "png_rgb": save(big, "PNG"),
        "png_rgba": save(big_a, "PNG"),
        "png_la": save(big_a.convert("LA"), "PNG"),
        "png_p_trns": save(big.quantize(100), "PNG", transparency=3),
        "png_l_trns": save(big.convert("L"), "PNG", transparency=128),
        "png_rgb_small": save(small, "PNG"),
        "png_rgba_small": save(small_a, "PNG"),
        "png_apng": save(small, "PNG", save_all=True, append_images=[small_a.convert("RGB")]),
    }
    i16 = Image.new("I;16", (420, 280))
    p = i16.load()
    for y in range(280):
        for x in range(420):
            p[x, y] = (x * 140 + y * 7) % 600
    out["png_i16"] = save(i16, "PNG")
    out["png_i16_trns"] = save(i16, "PNG", transparency=100)
    return out


def main():
    rows = []
    for name, data in cases().items():
        row = {"name": name, "data": base64.b64encode(data).decode()}
        try:
            im = Image.open(io.BytesIO(data))
            im.load()
            rgba = im.convert("RGBA")
            row["open"] = {
                "mode": im.mode,
                "size": list(im.size),
                "frames": getattr(im, "n_frames", 1),
                "rgba": hashlib.sha256(rgba.tobytes()).hexdigest(),
            }
        except Exception as e:  # noqa: BLE001
            row["open"] = {"error": type(e).__name__}
        thumbs = {}
        for w in store.THUMB_WIDTHS:
            t = store.render_thumbnail(data, w)
            thumbs[str(w)] = None if t is None else {"len": len(t), "sha": hashlib.sha256(t).hexdigest()}
        row["thumbs"] = thumbs
        rows.append(row)
    OUT.write_text(json.dumps(rows, indent=0) + "\n", encoding="utf-8", newline="\n")
    print(OUT, len(rows), sum(len(r["data"]) for r in rows))


if __name__ == "__main__":
    main()
