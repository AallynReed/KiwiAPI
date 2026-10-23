"""Golden fixtures for kiwi-pil's CFF, CFF2 and auto-hinted faces.

Draws the og card strings (the snapshot catalogs in testdata/og/locales) with
the subset Noto faces from make_og_test_fonts.py at the sizes the cards shrink
through: a static-CFF and a CFF2 face in one-font TTCs (FreeType's cff driver
with the Adobe hinting engine) and unhinted Noto Sans Thai (the auto-hinter).
Each (face, text) row lists [size, textbbox(), first 16 hex digits of the
SHA-256 of an RGBA canvas (bbox + 24 px) with the text drawn at (10 or 10.5,
11)], compared by src/tests.rs.

Run from the repo root with the production-pinned venv:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_pil_faces_fixtures.py
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont, features

ROOT = Path(__file__).resolve().parents[3]
FONTS = ROOT / "native" / "kiwi-pil" / "testdata" / "fonts"
LOCALES = ROOT / "native" / "kiwi-server" / "testdata" / "og" / "locales"
OUT = ROOT / "native" / "kiwi-pil" / "testdata" / "faces.json"

assert not features.check("raqm")
assert features.version("freetype2") == "2.14.3"

FACES = {
    "NotoSansCJK-subset-Regular.ttc": ("ja", "ko", "zh-CN"),
    "NotoSansCJK-subset-Bold.ttc": ("ja", "ko", "zh-CN"),
    "NotoSansThai-subset-Regular.ttf": ("th",),
    "NotoSansThai-subset-Bold.ttf": ("th",),
}
SIZES = [11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 26, 28, 30, 32, 33, 38, 40, 46, 54, 60, 64, 86]
EXTRA = ["trove.aallyn.net", "EU: Online", "—", "…", "~13,617", "AVAWAVAYA To", "1234567890", "ก่ำ ที่ นี่ ปู่ ญู ฐุ",
         "", " "]


def texts(langs: tuple[str, ...]) -> list[str]:
    out: set[str] = set()
    for lang in langs:
        for sub in ("site", "app"):
            out |= {v for v in json.loads((LOCALES / sub / f"{lang}.json").read_text(encoding="utf-8")).values()
                    if isinstance(v, str)}
    return sorted(out) + EXTRA


def main() -> None:
    rows = []
    for face, langs in FACES.items():
        for i, text in enumerate(texts(langs)):
            draws = []
            for size in SIZES[i % 4::4]:
                font = ImageFont.truetype(str(FONTS / face), size)
                bbox = ImageDraw.Draw(Image.new("RGBA", (1, 1))).textbbox((0, 0), text, font=font)
                img = Image.new("RGBA", (max(1, bbox[2] + 24), max(1, bbox[3] + 24)), (13, 17, 23, 255))
                xy = (10.5 if size % 2 else 10, 11)
                ImageDraw.Draw(img, "RGBA").text(xy, text, font=font, fill=(230, 237, 243, 210))
                draws.append([size, list(bbox), hashlib.sha256(img.tobytes()).hexdigest()[:16]])
            rows.append({"face": face, "text": text, "draws": draws})
    OUT.write_text(json.dumps(rows, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"{sum(len(r['draws']) for r in rows)} draws written to {OUT}")


if __name__ == "__main__":
    main()
