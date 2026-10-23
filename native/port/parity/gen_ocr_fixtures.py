"""Golden fixtures for the OCR building blocks (kiwi_core::ocr), from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_ocr_fixtures.py
Writes native/kiwi-server/testdata/ocr/blocks.json.

Inputs are generated from a shared LCG so only outputs are stored. Every call
is the one RapidOCR 3.8.4 makes (OpenCV 5.0.0, NumPy 2.4.6, pyclipper 1.4.0,
shapely 2.2.0, Pillow 13), on this machine's CPU dispatch (AVX2, no AVX-512,
the same class as the production host).
"""
import hashlib
import io
import json
import os
import struct
import sys
from pathlib import Path

sys.path.insert(0, os.getcwd())

import cv2  # noqa: E402
import numpy as np  # noqa: E402
import pyclipper  # noqa: E402
from PIL import Image  # noqa: E402
from rapidocr.ch_ppocr_det.utils import DBPostProcess  # noqa: E402
from rapidocr.utils.process_img import get_rotate_crop_image  # noqa: E402

from app.trove.ocr import parse, vocabulary  # noqa: E402

OUT = Path(__file__).resolve().parents[2] / "kiwi-server" / "testdata" / "ocr" / "blocks.json"


def lcg(seed, n):
    x = seed & 0xFFFFFFFF
    out = bytearray(n)
    for i in range(n):
        x = (x * 1103515245 + 12345) & 0x7FFFFFFF
        out[i] = (x >> 16) & 0xFF
    return bytes(out)


def sha(b):
    return hashlib.sha256(bytes(b)).hexdigest()


def f32bits(v):
    return struct.unpack("<I", struct.pack("<f", float(v)))[0]


def f64bits(v):
    return struct.unpack("<Q", struct.pack("<d", float(v)))[0]


def img(seed, w, h, c=3):
    return np.frombuffer(lcg(seed, w * h * c), dtype=np.uint8).reshape(h, w, c).copy()


def argsorts():
    out = []
    rng = np.random.default_rng(7)
    for n in [2, 3, 4, 5, 6, 7, 8, 9, 12, 16, 17, 20, 31, 33, 45, 64, 65, 100, 128, 129, 200, 256, 257, 300, 517, 1000]:
        for trial in range(4):
            k = rng.integers(0, max(2, n // 3), size=n)
            if trial == 3:
                k = np.sort(k)
            a = k.astype(np.float64) / 3.0
            out.append({"kind": "f64", "thirds": k.tolist(), "order": np.argsort(a).tolist()})
    for trial in range(60):
        a = rng.integers(0, 4, size=4).astype(np.int32)
        out.append({"kind": "i32", "keys": a.tolist(), "order": np.argsort(a).tolist()})
    for trial in range(10):
        a = rng.integers(0, 3, size=2).astype(np.int32)
        out.append({"kind": "i32", "keys": a.tolist(), "order": np.argsort(a).tolist()})
    return out


RESIZES = [(5, 7, 13, 11), (40, 30, 736, 544), (100, 48, 37, 48), (64, 96, 32, 48), (3, 3, 32, 32),
           (2100, 40, 2016, 32), (17, 48, 192, 48), (33, 21, 66, 42), (200, 60, 100, 30), (1, 9, 7, 48),
           (640, 360, 736, 416), (91, 23, 160, 48)]


def resizes():
    out = []
    for i, (w, h, dw, dh) in enumerate(RESIZES):
        r = cv2.resize(img(100 + i, w, h), (dw, dh))
        out.append({"seed": 100 + i, "w": w, "h": h, "dw": dw, "dh": dh, "sha": sha(r.tobytes())})
    return out


def mask_of(seed, w, h, density):
    raw = np.frombuffer(lcg(seed, w * h), dtype=np.uint8).reshape(h, w)
    m = (raw < density).astype(np.uint8)
    m = cv2.dilate(m, np.ones((3, 3), np.uint8)) if seed % 2 else m
    return m


def contour_cases():
    post = DBPostProcess(thresh=0.3, box_thresh=0.5, max_candidates=1000, unclip_ratio=1.6, use_dilation=True)
    out = []
    for i, (w, h, density) in enumerate([(40, 30, 40), (64, 48, 25), (33, 17, 90), (80, 60, 12), (25, 25, 128)]):
        seed = 300 + i
        m = mask_of(seed, w, h, density)
        dil = cv2.dilate(m, np.array([[1, 1], [1, 1]]))
        contours, _ = cv2.findContours((dil * 255).astype(np.uint8), cv2.RETR_LIST, cv2.CHAIN_APPROX_SIMPLE)
        rects = []
        for c in contours:
            box, sside = post.get_mini_boxes(c)
            rect = cv2.minAreaRect(c)
            rects.append({"rect": [f32bits(rect[0][0]), f32bits(rect[0][1]), f32bits(rect[1][0]),
                                   f32bits(rect[1][1]), f32bits(rect[2])],
                          "box": [f32bits(v) for v in box.reshape(-1)], "sside": f32bits(sside)})
        out.append({"seed": seed, "w": w, "h": h, "density": density, "dilated_sha": sha(dil.tobytes()),
                    "contours": [c.reshape(-1).tolist() for c in contours], "rects": rects})
    return out


def scores_and_unclip():
    post = DBPostProcess(thresh=0.3, box_thresh=0.5, max_candidates=1000, unclip_ratio=1.6, use_dilation=True)
    out = []
    rng = np.random.default_rng(11)
    pred = (np.frombuffer(lcg(500, 4 * 60 * 50), dtype=np.uint8).astype(np.float32) / 255.0).reshape(-1)[:60 * 50]
    pred = pred.reshape(50, 60).astype(np.float32)
    for k in range(40):
        cx, cy = rng.uniform(-5, 65), rng.uniform(-5, 55)
        w, h, a = rng.uniform(3, 40), rng.uniform(3, 15), rng.uniform(-90, 0)
        box = cv2.boxPoints(((cx, cy), (w, h), a)).astype(np.float32)
        score = post.box_score_fast(pred, box.reshape(-1, 2))
        expanded = post.unclip(box)
        mini, sside = post.get_mini_boxes(expanded)
        out.append({"box": [f32bits(v) for v in box.reshape(-1)], "score": f64bits(score),
                    "expanded": expanded.reshape(-1).tolist(),
                    "mini": [f32bits(v) for v in mini.reshape(-1)], "sside": f32bits(sside)})
    return {"pred_seed": 500, "w": 60, "h": 50, "cases": out}


def crops():
    out = []
    rng = np.random.default_rng(13)
    src = img(600, 120, 80)
    for k in range(30):
        cx, cy = rng.uniform(10, 110), rng.uniform(10, 70)
        w, h, a = rng.uniform(5, 60), rng.uniform(5, 30), rng.uniform(-90, 0)
        if k % 3 == 0:
            a = -90.0
        box = cv2.boxPoints(((cx, cy), (w, h), a)).astype(np.float32)
        box = np.round(box).astype(np.float32)
        c = get_rotate_crop_image(src, box.copy())
        out.append({"box": [f32bits(v) for v in box.reshape(-1)], "shape": list(c.shape), "sha": sha(c.tobytes())})
    return {"seed": 600, "w": 120, "h": 80, "cases": out}


def lanczos():
    out = []
    for i, (w, h, s) in enumerate([(30, 9, 4), (7, 3, 4), (100, 20, 4), (13, 13, 2), (64, 5, 3)]):
        a = img(700 + i, w, h)
        up = np.asarray(Image.fromarray(a).resize((w * s, h * s), Image.LANCZOS))
        out.append({"seed": 700 + i, "w": w, "h": h, "dw": w * s, "dh": h * s, "sha": sha(up.tobytes())})
    return out


def decodes():
    out = []
    base = img(800, 37, 23)
    variants = []
    im = Image.fromarray(base)
    for name, kw in [("png_rgb", ("PNG", {})), ("png_rgba", ("PNG", {})), ("png_p", ("PNG", {})),
                     ("png_l", ("PNG", {})), ("png_la", ("PNG", {})), ("png_i16", ("PNG", {})),
                     ("png_1", ("PNG", {})), ("bmp_rgb", ("BMP", {})), ("bmp_p", ("BMP", {})),
                     ("gif", ("GIF", {})), ("jpeg_q90", ("JPEG", {"quality": 90})),
                     ("jpeg_444", ("JPEG", {"quality": 95, "subsampling": 0})),
                     ("jpeg_prog", ("JPEG", {"quality": 80, "progressive": True})),
                     ("jpeg_l", ("JPEG", {"quality": 85})), ("webp_lossless", ("WEBP", {"lossless": True})),
                     ("webp_lossy", ("WEBP", {"quality": 80}))]:
        src = im
        if name == "png_rgba":
            src = im.convert("RGBA")
            src.putalpha(Image.fromarray(img(801, 37, 23, 1)[:, :, 0]))
        elif name in ("png_p", "bmp_p", "gif"):
            src = im.convert("P", palette=Image.ADAPTIVE, colors=200)
        elif name in ("png_l", "jpeg_l"):
            src = im.convert("L")
        elif name == "png_la":
            src = im.convert("LA")
        elif name == "png_i16":
            src = Image.fromarray((np.frombuffer(lcg(802, 37 * 23 * 2), dtype=np.uint16).reshape(23, 37) // 97).astype(np.uint16))
        elif name == "png_1":
            src = im.convert("1")
        fmt, opts = kw
        buf = io.BytesIO()
        src.save(buf, fmt, **opts)
        data = buf.getvalue()
        rgb = np.asarray(Image.open(io.BytesIO(data)).convert("RGB"))
        variants.append({"name": name, "file": data.hex(), "w": rgb.shape[1], "h": rgb.shape[0],
                         "sha": sha(rgb.tobytes())})
    out.extend(variants)
    return out


LINES = [
    ["14,690 Physical Damage", "Critical Hit 114.8%", "Physical Damage 799,894 Critical Hit 114.8%"],
    ["Maximum Health", "3,239,060", "Critical Damage 3438.3 %", "Coefficient", "28,302,108"],
    ["Dégâts physiques 799.894", "Coup critique 114,8%", "Degats critiques 3438.3%", "Rg de pouvoir", "x", "38,000"],
    ["Physjcal Damaqe 12,345", "Magic Damage 1.234.567", "Jump 74 Light 13185", "Stability 145%", "Attack Speed 204"],
    ["Max Health 2.716,754", "Energy Regen 177", "Coefficient 99,999,999", "Physical Damage 1,000", "Critical Damage 100%"],
    ["Movement Speed 132 Lasermancy 450", "Battle Factor 6", "Flask Capacity 28", "Experience Gain 221", "Magic Find 14253"],
    ["", "   ", "Superstition 0", "Chaos Factor 12 Light", "1 2 3", "Power Rank 35,201"],
    ["Ｐｈｙｓｉｃａｌ Ｄａｍａｇｅ １２３", "Critical Hit\t5\t%", "Health Regen 4237", "Health Regen 4,237.5"],
]


def texts():
    out = []
    for lines in LINES:
        out.append({"lines": lines, "result": parse.extract_character_stats(lines)})
    norms = ["Dégâts physiques", "  Physical--Damage!! ", "İstanbul ǅ ﬁ", "ＡＢＣ１２３", "", "Ⅻ ½"]
    labels = ["Physjcal Damaqe", "max heal", "Critical", "Crit Hit", "coef", "Rang de puissance x", "jumps", "Energie"]
    return {"cases": out, "normalize": [[n, vocabulary.normalize(n)] for n in norms],
            "labels": [[t, vocabulary.match_label(t)] for t in labels],
            "numbers": [[t, parse.parse_number(t)] for t in
                        ["1,234", "636.088", "2.716,754", "114.8", "3,438.3", "12 %", "1.2.3", "7", "12.5\t%", "１,２３４"]]}


def main():
    golden = {
        "argsort": argsorts(),
        "resize": resizes(),
        "contours": contour_cases(),
        "scores": scores_and_unclip(),
        "crops": crops(),
        "lanczos": lanczos(),
        "decode": decodes(),
        "text": texts(),
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(golden, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
