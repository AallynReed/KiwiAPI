"""Golden fixtures for kiwi-pil: Pillow 13 drawing scenes and text metrics.

Each scene is a list of ImageDraw calls on an RGBA canvas (Draw(img, "RGBA"),
the way app/site/og_image.py draws). The ops go to scenes.json, Pillow's
result to scenes/<name>.png (RGBA, lossless), and textbbox() results for many
strings and sizes to metrics.json. src/tests.rs replays every op in Rust and
compares pixels exactly.

Fonts are the Debian trixie DejaVu files in kiwi-pil/testdata/fonts (the ones
production loads). Pillow's Windows wheel bundles the same FreeType 2.14.3 and,
like the production container, has no libraqm, so text uses the basic layout.

Run from the repo root with the production-pinned venv:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_pil_fixtures.py
"""

from __future__ import annotations

import json
import random
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont, features

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "native" / "kiwi-pil" / "testdata"
FONTS = {"regular": OUT / "fonts" / "DejaVuSans.ttf", "bold": OUT / "fonts" / "DejaVuSans-Bold.ttf"}

assert not features.check("raqm"), "fixtures need Pillow's basic layout (no libraqm)"
assert features.version("freetype2") == "2.14.3"

TEXTS = [
    "PLAYER ACTIVITY", "~5,517", "active players", "in the last 24 hours", "ACTIVE · 7D",
    "~13,617", "trove.aallyn.net/activity", "Last 7 days · Trove server time (UTC−11)",
    "Wed", "Oct 15", "4,712", "Activity data warming up…", "TROVE PC SERVER STATUS",
    "All systems online", "Partial outage", "99.87% · 30d uptime", "uptime n/a · 30d",
    "Checked 12s ago", "AVAWAVAYA To Tj LT", "fi fl ffi", "Äöü ÉÈ ç ß", "Привет мир",
    "—", "…", "", " ", "a  b", "Ångström (x²)", "1234567890", "{}[]()<>",
    "Hourly Challenge", "Chaos Chest", "Here now · away", "Leaves in 2h 14m",
]


def ink(rng: random.Random, alpha=None) -> list[int]:
    return [rng.randrange(256) for _ in range(3)] + [alpha if alpha is not None else rng.randrange(256)]


def run(img: Image.Image, ops: list[dict]) -> None:
    d = ImageDraw.Draw(img, "RGBA")
    fonts: dict = {}

    def font(o):
        key = (o["font"], o["size"])
        if key not in fonts:
            fonts[key] = ImageFont.truetype(str(FONTS[o["font"]]), o["size"])
        return fonts[key]

    def t(v):
        return tuple(v) if v is not None else None

    for o in ops:
        k = o["op"]
        if k == "rectangle":
            d.rectangle(o["xy"], fill=t(o.get("fill")), outline=t(o.get("outline")), width=o.get("width", 1))
        elif k == "ellipse":
            d.ellipse(o["xy"], fill=t(o.get("fill")), outline=t(o.get("outline")), width=o.get("width", 1))
        elif k == "polygon":
            d.polygon([tuple(p) for p in o["xy"]], fill=t(o["fill"]))
        elif k == "line":
            d.line([tuple(p) for p in o["xy"]], fill=t(o["fill"]), width=o["width"],
                   joint="curve" if o.get("curve") else None)
        elif k == "pieslice":
            d.pieslice(o["xy"], o["start"], o["end"], fill=t(o["fill"]))
        elif k == "rounded_rectangle":
            d.rounded_rectangle(o["xy"], radius=o["radius"], fill=t(o.get("fill")),
                                outline=t(o.get("outline")), width=o.get("width", 1))
        elif k == "text":
            d.text(tuple(o["xy"]), o["text"], font=font(o), fill=t(o["fill"]))
        else:
            raise ValueError(k)


def scenes() -> list[dict]:
    rng = random.Random(1307)
    out = []

    ops = []
    for _ in range(40):
        x0, y0 = rng.uniform(-20, 380), rng.uniform(-20, 260)
        x1, y1 = x0 + rng.uniform(0, 120), y0 + rng.uniform(0, 90)
        ops.append({"op": rng.choice(["rectangle", "ellipse"]), "xy": [x0, y0, x1, y1],
                    "fill": ink(rng) if rng.random() < 0.8 else None,
                    "outline": ink(rng) if rng.random() < 0.5 else None, "width": rng.choice([1, 2, 3, 5])})
    for a, b in [(0, 0), (1, 1), (2, 2), (16, 16), (14, 14), (8, 3), (3, 9), (21, 22)]:
        ops.append({"op": "ellipse", "xy": [30 + a, 30, 30 + 2 * a, 30 + b], "fill": ink(rng, 255)})
        ops.append({"op": "ellipse", "xy": [200, 200, 200 + a, 200 + b], "fill": ink(rng), "outline": ink(rng), "width": 3})
    out.append({"name": "shapes", "size": [400, 300], "bg": [13, 17, 23, 255], "ops": ops})

    ops = []
    for _ in range(25):
        n = rng.randrange(3, 9)
        ops.append({"op": "polygon", "xy": [[rng.uniform(-10, 410), rng.uniform(-10, 310)] for _ in range(n)],
                    "fill": ink(rng)})
    for _ in range(6):
        x = 20.0
        pts = []
        while x < 380:
            pts.append([x, rng.uniform(40, 260)])
            x += rng.uniform(2, 40)
        pts += [[pts[-1][0], 280.0], [pts[0][0], 280.0]]
        ops.append({"op": "polygon", "xy": pts, "fill": ink(rng)})
    out.append({"name": "polygons", "size": [400, 300], "bg": [13, 17, 23, 255], "ops": ops})

    ops = []
    for w in (1, 2, 3, 5, 9):
        for _ in range(6):
            pts = [[rng.uniform(-10, 410), rng.uniform(-10, 310)] for _ in range(rng.randrange(2, 7))]
            ops.append({"op": "line", "xy": pts, "fill": ink(rng), "width": w, "curve": rng.random() < 0.7})
    x, pts = 64.0, []
    while x < 1136:
        pts.append([x, rng.uniform(322, 540)])
        x += rng.uniform(5, 40)
    ops.append({"op": "line", "xy": pts, "fill": [93, 208, 120, 255], "width": 5, "curve": True})
    ops.append({"op": "line", "xy": [[64, 540], [1136, 540]], "fill": [40, 48, 58, 255], "width": 2})
    y = 322
    while y < 540:
        ops.append({"op": "line", "xy": [[700.37, y], [700.37, min(y + 9, 540)]], "fill": [230, 237, 243, 95], "width": 2})
        y += 15
    out.append({"name": "lines", "size": [1200, 630], "bg": [13, 17, 23, 255], "ops": ops})

    ops = []
    for _ in range(30):
        x0, y0 = rng.randrange(-10, 360), rng.randrange(-10, 260)
        ops.append({"op": "pieslice", "xy": [x0, y0, x0 + rng.randrange(0, 60), y0 + rng.randrange(0, 60)],
                    "start": rng.uniform(-400, 400), "end": rng.uniform(-400, 400), "fill": ink(rng)})
    for start in (0, 90, 180, 270, 45.5, -30):
        ops.append({"op": "pieslice", "xy": [100, 100, 140, 128], "start": start, "end": start + 75, "fill": ink(rng)})
    out.append({"name": "pieslices", "size": [400, 300], "bg": [13, 17, 23, 255], "ops": ops})

    ops = []
    for r, w in [(20, 2), (18, 2), (3, 1), (0, 1), (60, 2), (7, 3), (25, 1)]:
        x0, y0 = rng.randrange(0, 300), rng.randrange(0, 200)
        ops.append({"op": "rounded_rectangle", "xy": [x0, y0, x0 + rng.randrange(6, 330), y0 + rng.randrange(6, 170)],
                    "radius": r, "fill": ink(rng, 255), "outline": ink(rng), "width": w})
    ops.append({"op": "rounded_rectangle", "xy": [64, 330, 410, 500], "radius": 20, "fill": [22, 27, 34, 255],
                "outline": [93, 208, 120, 130], "width": 2})
    ops.append({"op": "rounded_rectangle", "xy": [56, 172, 600, 290], "radius": 18, "fill": [22, 27, 34, 255],
                "outline": [255, 152, 0, 90], "width": 2})
    ops.append({"op": "rounded_rectangle", "xy": [56, 172, 62, 290], "radius": 3, "fill": [255, 152, 0, 255]})
    ops.append({"op": "rounded_rectangle", "xy": [10, 10, 30, 30], "radius": 15, "fill": ink(rng, 255)})
    out.append({"name": "rounded", "size": [700, 520], "bg": [13, 17, 23, 255], "ops": ops})

    ops = [{"op": "rectangle", "xy": [0, 0, 600, 200], "fill": [63, 185, 80, 70]},
           {"op": "rectangle", "xy": [0, 200, 600, 300], "fill": [0, 0, 0, 0]}]
    y = 4.0
    for i, s in enumerate(TEXTS):
        size = rng.choice([11, 13, 19, 22, 24, 26, 30, 33, 40, 64, 86, 104]) if i > 4 else [104, 30, 19, 33, 22][i]
        ops.append({"op": "text", "xy": [rng.uniform(-30, 500) if i % 3 else rng.randrange(0, 400), y],
                    "text": s, "font": rng.choice(["regular", "bold"]), "size": size, "fill": ink(rng)})
        y = (y + rng.uniform(5, 30)) % 560
    out.append({"name": "text", "size": [900, 600], "bg": [13, 17, 23, 255], "ops": ops})
    return out


def metrics() -> list[dict]:
    rows = []
    img = Image.new("RGBA", (10, 10))
    d = ImageDraw.Draw(img, "RGBA")
    for name, path in FONTS.items():
        for size in (6, 11, 13, 17, 19, 20, 22, 23, 24, 26, 30, 33, 40, 46, 60, 64, 86, 104, 300):
            f = ImageFont.truetype(str(path), size)
            for s in TEXTS + ["A", "Ag", "Line one\nline two"]:
                rows.append({"font": name, "size": size, "text": s, "bbox": list(d.textbbox((0, 0), s, font=f))})
    return rows


def main() -> None:
    (OUT / "scenes").mkdir(parents=True, exist_ok=True)
    sc = scenes()
    for s in sc:
        img = Image.new("RGBA", tuple(s["size"]), tuple(s["bg"]))
        run(img, s["ops"])
        img.save(OUT / "scenes" / f"{s['name']}.png")
    (OUT / "scenes.json").write_text(json.dumps(sc, ensure_ascii=False, indent=0) + "\n", encoding="utf-8")
    (OUT / "metrics.json").write_text(json.dumps(metrics(), ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"{len(sc)} scenes, metrics written to {OUT}")


if __name__ == "__main__":
    main()
