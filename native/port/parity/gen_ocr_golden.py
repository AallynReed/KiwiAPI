"""End-to-end OCR goldens for /v1/ocr/character, from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_ocr_golden.py [FONT_DIR]
Writes native/kiwi-server/testdata/ocr/{golden.json,*.png,*.jpg}.

Draws stat-sheet screenshots in the game's own font (Open Sans, FONT_DIR
defaults to E:/Trove/fonts) and records what the real pipeline answers:
RapidOCR 3.8.4 on onnxruntime 1.26.0, app.trove.ocr's parser and recovery
pass, and pydantic's serialization of CharacterStatsOcr. Each label's
`ocr_row` re-read is recorded too, so the recovery path is checked even
where it finds nothing.
"""
import io
import json
import os
import sys
from pathlib import Path

sys.path.insert(0, os.getcwd())

from PIL import Image, ImageDraw, ImageFont  # noqa: E402

from app.trove.ocr import engine, service  # noqa: E402
from app.trove.ocr import vocabulary as vocab  # noqa: E402
from app.trove.schemas import CharacterStatsOcr  # noqa: E402

OUT = Path(__file__).resolve().parents[2] / "kiwi-server" / "testdata" / "ocr"
FONT_DIR = Path(sys.argv[1] if len(sys.argv) > 1 else "E:/Trove/fonts")
BG = (22, 26, 34)
FG = (226, 220, 200)
GOLD = (240, 196, 92)


def font(size, bold=False):
    return ImageFont.truetype(str(FONT_DIR / ("opensans-bold.ttf" if bold else "opensans-regular.ttf")), size)


def sheet(rows, size=(420, 560), fsize=15, x_val=24, x_label=130, top=40, gap=24, title="Stats"):
    im = Image.new("RGB", size, BG)
    d = ImageDraw.Draw(im)
    d.text((x_val, 10), title, font=font(fsize + 3, True), fill=GOLD)
    for i, (value, label) in enumerate(rows):
        y = top + i * gap
        if value:
            d.text((x_val, y), value, font=font(fsize, True), fill=FG)
        if label:
            d.text((x_label, y), label, font=font(fsize), fill=FG)
    return im


EN = [("799,894", "Physical Damage"), ("14,300", "Magic Damage"), ("3,239,060", "Maximum Health"),
      ("127", "Maximum Energy"), ("4,237", "Health Regen"), ("177", "Energy Regen"), ("145", "Stability"),
      ("132", "Movement Speed"), ("204%", "Attack Speed"), ("74", "Jump"), ("114.8%", "Critical Hit"),
      ("3,438.3%", "Critical Damage"), ("0", "Superstition"), ("14,253", "Magic Find"), ("450", "Lasermancy"),
      ("28", "Flask Capacity"), ("221", "Experience Gain"), ("6", "Battle Factor"), ("13,185", "Light"),
      ("", "Chaos Factor")]

FR = [("799,894", "Dégâts physiques"), ("14,300", "Dégâts magiques"), ("3,239,060", "PV maximum"),
      ("114.8%", "Coup critique"), ("3,438.3%", "Dégâts critiques"), ("13,185", "Lumière"),
      ("145", "Stabilité"), ("74", "Saut"), ("28,302,108", "Coefficient")]


def two_columns():
    im = Image.new("RGB", (640, 260), BG)
    d = ImageDraw.Draw(im)
    pairs = [("Physical Damage", "799,894", "Critical Hit", "114.8%"),
             ("Magic Damage", "14,300", "Critical Damage", "3,438.3%"),
             ("Maximum Health", "3,239,060", "Attack Speed", "204%"),
             ("Light", "13,185", "Jump", "74"),
             ("Power Rank", "35,201", "Magic Find", "14,253")]
    for i, (l1, v1, l2, v2) in enumerate(pairs):
        y = 30 + i * 40
        d.text((20, y), l1, font=font(17), fill=FG)
        d.text((200, y), v1, font=font(17, True), fill=GOLD)
        d.text((340, y), l2, font=font(17), fill=FG)
        d.text((520, y), v2, font=font(17, True), fill=GOLD)
    return im


def rotated_and_vertical():
    im = sheet(EN[:8], size=(420, 300))
    line = Image.new("RGB", (220, 26), BG)
    ImageDraw.Draw(line).text((4, 2), "13,185 Light", font=font(16, True), fill=FG)
    im.paste(line.rotate(180), (180, 240))
    col = Image.new("RGB", (24, 140), BG)
    dc = ImageDraw.Draw(col)
    for i, ch in enumerate("12345"):
        dc.text((5, 4 + i * 26), ch, font=font(16, True), fill=FG)
    im.paste(col, (380, 60))
    return im


def wide_strip():
    im = Image.new("RGB", (900, 40), BG)
    ImageDraw.Draw(im).text((12, 8), "Physical Damage 799,894    Critical Hit 114.8%    Light 13,185",
                            font=font(16), fill=FG)
    return im


def tiny():
    im = Image.new("RGB", (160, 22), BG)
    ImageDraw.Draw(im).text((4, 3), "Jump 74", font=font(13, True), fill=FG)
    return im


def big():
    im = Image.new("RGB", (2400, 1350), (40, 70, 110))
    im.paste(sheet(EN[:12], size=(560, 420), fsize=20, x_label=170, gap=30), (1700, 120))
    return im


def jpeg(im, quality=88):
    b = io.BytesIO()
    im.save(b, "JPEG", quality=quality)
    return b.getvalue()


def png(im):
    b = io.BytesIO()
    im.save(b, "PNG", optimize=True)
    return b.getvalue()


CASES = [
    ("en_sheet.png", "image/png", lambda: png(sheet(EN))),
    ("fr_sheet.png", "image/png", lambda: png(sheet(FR, size=(440, 280), x_label=140))),
    ("two_columns.png", "image/png", lambda: png(two_columns())),
    ("rotated.png", "image/png", lambda: png(rotated_and_vertical())),
    ("wide_strip.png", "image/png", lambda: png(wide_strip())),
    ("tiny.png", "", lambda: png(tiny())),
    ("big.png", "image/png", lambda: png(big())),
    ("en_sheet.jpg", "image/jpeg", lambda: jpeg(sheet(EN[:10], size=(400, 300)))),
]


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    golden = []
    for name, ctype, make in CASES:
        data = make()
        (OUT / name).write_bytes(data)
        arr, elems = engine.recognize(data)
        result = service._process(data)
        body = CharacterStatsOcr(**result).model_dump_json()
        rows = []
        for el in elems:
            m = vocab.match_label(el.text)
            if m is not None and el.box:
                rows.append({"text": el.text, "box": [list(p) for p in el.box],
                             "texts": engine.ocr_row(arr, el.box)})
        golden.append({"file": name, "content_type": ctype,
                       "elems": [{"text": e.text, "box": [list(p) for p in e.box]} for e in elems],
                       "rows": rows, "body": json.loads(body), "body_raw": body})
        print(name, len(data), "bytes,", len(elems), "lines,", result["matched"], "stats")
    (OUT / "golden.json").write_text(json.dumps(golden, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
