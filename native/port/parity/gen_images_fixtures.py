"""Golden fixtures for kiwi-server's Image Studio (src/images), from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/linux_png.py run native/port/parity/gen_images_fixtures.py
Writes native/kiwi-server/testdata/images/golden.json.

Renders run the real app/images/render.py with og_image's font lists pointed at
the DejaVu files in kiwi-pil/testdata/fonts. The model cases build the stored
document and the DTO the create and update paths produce (Beanie's encoder,
pydantic's dump; PUT answers reloaded_dto, since save() merges the stored
document back), the binding list is service.bindings(), and the sniff cases
call the hub store's sniff_image. The clock is frozen.
"""
import asyncio
import base64
import datetime as dt
import io
import json
import os
import sys
import warnings
from pathlib import Path

warnings.filterwarnings("ignore")
ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "images" / "golden.json"
FONTS = ROOT / "native" / "kiwi-pil" / "testdata" / "fonts"
os.chdir(ROOT)
sys.path.insert(0, str(ROOT))
os.environ.setdefault("SECRET_KEY", "images-fixture-secret")

import pydantic_core  # noqa: E402
from bson import ObjectId, json_util  # noqa: E402
from bson.json_util import CANONICAL_JSON_OPTIONS  # noqa: E402
from PIL import Image  # noqa: E402
from pymongo.asynchronous.database import AsyncDatabase  # noqa: E402

import app.core.utils as cu  # noqa: E402

NOW = dt.datetime(2026, 10, 22, 9, 30, 15, 123456, tzinfo=dt.timezone.utc)
CREATED = dt.datetime(2026, 10, 1, 8, 0, 0, 250000, tzinfo=dt.timezone.utc)


class _Frozen(dt.datetime):
    @classmethod
    def now(cls, tz=None):
        return NOW


cu.datetime = _Frozen

from app.images import render as R  # noqa: E402
from app.images import service as S  # noqa: E402
from app.images.models import ImageDesign  # noqa: E402
from app.images.schemas import DesignBody  # noqa: E402
from app.site import og_image as og  # noqa: E402
from app.trove.mods_hub import store as hub_store  # noqa: E402

og._REG[:] = [str(FONTS / "DejaVuSans.ttf")]
og._BOLD[:] = [str(FONTS / "DejaVuSans-Bold.ttf")]


async def _cmd(self, *a, **k):
    return {"version": "7.0.0"}


async def _lcn(self, *a, **k):
    return []


AsyncDatabase.command = _cmd
AsyncDatabase.list_collection_names = _lcn


async def _init():
    from beanie import init_beanie
    from pymongo import AsyncMongoClient
    client = AsyncMongoClient("mongodb://127.0.0.1:1", tz_aware=True, connect=False, serverSelectionTimeoutMS=200)
    await init_beanie(database=client["kiwi"], skip_indexes=True, document_models=[ImageDesign])


asyncio.new_event_loop().run_until_complete(_init())

OWNER = ObjectId("65a1b2c3d4e5f60718293a4b")
DESIGN = ObjectId("65c000000000000000000001")
RNOW = 1761125415


def b64(b: bytes) -> str:
    return base64.b64encode(b).decode()


def png_of(im: Image.Image) -> bytes:
    buf = io.BytesIO()
    im.save(buf, "PNG")
    return buf.getvalue()


def blobs() -> dict[str, bytes]:
    rgba = Image.new("RGBA", (40, 30))
    rgba.putdata([(x * 6, y * 8, 200 - x * 3, (x * 7 + y * 5) % 256) for y in range(30) for x in range(40)])
    rgb = Image.new("RGB", (23, 47))
    rgb.putdata([((x * 11) % 256, (y * 5) % 256, (x * y) % 256) for y in range(47) for x in range(23)])
    pal = rgb.convert("P", palette=Image.ADAPTIVE, colors=16)
    pal.info["transparency"] = 3
    gray = Image.linear_gradient("L").resize((17, 9))
    out = {"a" * 64: png_of(rgba), "b" * 64: png_of(rgb), "d" * 64: png_of(gray)}
    buf = io.BytesIO()
    pal.save(buf, "PNG", transparency=3)
    out["c" * 64] = buf.getvalue()
    return out


BLOBS = blobs()
CTX = {"name": "Racing Challenge", "type": "racing", "ends_at": RNOW + 1100, "starts_at": RNOW - 3700,
       "count": 42, "nothing": None, "far": RNOW - 3 * 86400 - 5000, "soon": RNOW + 45,
       "line": "**Bold** and [a link](https://x.y/z) `code` *em*", "bad_ts": "soon"}

T = {"type": "text"}
RENDERS = [
    ("solid_text", {"width": 200, "height": 90, "background": {"color1": "#203040"},
                    "layers": [T | {"text": "Hello {name}!", "x": 8, "y": 6, "font_size": 18},
                               T | {"text": "ends {ends_at:R} · {starts_at:R}", "x": 192, "y": 34,
                                    "font_size": 12, "align": "right", "color": "#ff8"},
                               T | {"text": "{far:R} {soon:R} {bad_ts:R} {missing} {nothing}", "x": 100, "y": 60,
                                    "align": "center", "font_size": 11, "bold": True, "opacity": 0.6}]}),
    ("styles", {"width": 260, "height": 120, "layers": [
        T | {"text": "{ends_at:t}|{ends_at:T}|{ends_at:d}", "x": 4, "y": 2, "font_size": 12},
        T | {"text": "{ends_at:D}|{ends_at:f}", "x": 4, "y": 30, "font_size": 12},
        T | {"text": "{ends_at:F}\n{line}", "x": 4, "y": 58, "font_size": 11, "color": "abc"}]}),
    ("wrap", {"width": 220, "height": 160, "background": {"type": "gradient", "color1": "#102030",
                                                          "color2": "#ffcc00", "angle": 90},
              "layers": [T | {"text": "The quick brown fox jumps over the lazy dog again and again\nnew  para",
                              "x": 6, "y": 4, "font_size": 16, "max_width": 150},
                         T | {"text": "centred wrap of a long caption here", "x": 110, "y": 110,
                              "align": "center", "max_width": 90, "font_size": 13, "color": "#00ff00"}]}),
    ("rects", {"width": 180, "height": 120, "background": {"color1": "#000000"}, "layers": [
        {"type": "rect", "x": 5, "y": 5, "w": 60, "h": 30, "color": "#ff0000"},
        {"type": "rect", "x": 70.5, "y": 5.7, "w": 50.2, "h": 40, "radius": 9, "color": "#00ff00", "opacity": 0.5},
        {"type": "rect", "x": 10, "y": 50, "w": 150, "h": 60, "radius": 200, "color": "#3366ff80"},
        {"type": "rect", "x": 100, "y": 60, "w": -20, "h": 10, "color": "#fff"},
        {"type": "rect", "x": 130, "y": 10, "w": 30, "h": 30, "radius": 4, "color": "zz", "opacity": 1.7},
        {"type": "rect", "x": -10, "y": 100, "w": 40, "h": 40, "radius": 3, "color": "#123"}]}),
    ("images", {"width": 160, "height": 100, "background": {"color1": "#334455"}, "layers": [
        {"type": "image", "image_sha": "a" * 64, "x": 4, "y": 4, "w": 60, "h": 45},
        {"type": "image", "image_sha": "a" * 64, "x": 70, "y": 4, "w": 50, "h": 50, "radius": 10, "opacity": 0.6},
        {"type": "image", "image_sha": "b" * 64, "x": -10, "y": 60, "w": 40, "h": 30, "opacity": 0.3},
        {"type": "image", "image_sha": "c" * 64, "x": 130, "y": 70, "w": 40, "h": 40, "radius": 6},
        {"type": "image", "image_sha": "d" * 64, "x": 120, "y": -5, "w": 30.9, "h": 12.2},
        {"type": "image", "image_sha": "e" * 64, "x": 0, "y": 0}]}),
    ("bg_cover", {"width": 120, "height": 64, "background": {"type": "image", "image_sha": "b" * 64, "fit": "cover"}}),
    ("bg_contain", {"width": 120, "height": 64, "background": {"type": "image", "image_sha": "a" * 64,
                                                               "fit": "contain"}}),
    ("bg_stretch", {"width": 64, "height": 64, "background": {"type": "image", "image_sha": "c" * 64,
                                                              "fit": "stretch"}}),
    ("bg_missing", {"width": 50, "height": 40, "background": {"type": "image", "image_sha": "f" * 64,
                                                              "color1": "#808080"}}),
    ("clamped", {"width": 10, "height": 5000, "layers": [T | {"text": "tiny", "font_size": 2},
                                                         T | {"text": "Big", "font_size": 400, "y": 100}]}),
]
for angle in (0, 45, 180, 270, 123, -30, 370, 89):
    RENDERS.append((f"gradient_{angle}", {"width": 96, "height": 54,
                                          "background": {"type": "gradient", "color1": "#ff0000",
                                                         "color2": "#0000ff", "angle": angle}}))


def render_cases() -> list:
    out = []
    for name, body in RENDERS:
        design = DesignBody.model_validate(body)
        png = R.render(design, CTX, lambda sha: BLOBS.get(sha), now=RNOW)
        out.append({"name": name, "body": body, "png": b64(png)})
    return out


def ext(doc) -> dict:
    return json.loads(json_util.dumps(doc, json_options=CANONICAL_JSON_OPTIONS))


def model_cases() -> list:
    from beanie.odm.utils.encoder import Encoder

    bodies = [
        {},
        {"name": "Banner", "width": 700.0, "height": "300", "bind_type": "challenge_racing",
         "background": {"type": "gradient", "angle": 45},
         "layers": [{"type": "rect", "x": 5, "w": 33.5}, {"text": "hi", "y": 2.5, "max_width": 100}]},
        {"width": 9999, "height": 1, "bind_type": "nope", "layers": [{"opacity": 0} for _ in range(43)]},
        {"bind_type": "", "layers": [{"type": "image", "image_sha": "a" * 64, "radius": 3, "x": 1e16}]},
    ]
    out = []
    for body in bodies:
        b = DesignBody.model_validate(body)
        d = ImageDesign(owner_id=OWNER, **b.model_dump())
        d.id = DESIGN
        S._clamp(d)
        created = {"dto": pydantic_core.to_json(S._dto(d)).decode(),
                   "stored": ext(Encoder(to_db=True, keep_nulls=True).encode(d))}
        loaded = ImageDesign.model_validate({**ext_strip(created["stored"]), "_id": DESIGN,
                                             "created_at": CREATED, "updated_at": CREATED})
        for k, v in b.model_dump().items():
            setattr(loaded, k, v)
        S._clamp(loaded)
        loaded.updated_at = NOW
        updated = {"dto": pydantic_core.to_json(S._dto(loaded)).decode(),
                   "stored": ext(Encoder(to_db=True, keep_nulls=True).encode(loaded))}
        reload = ImageDesign.model_validate({**ext_strip(updated["stored"]), "_id": DESIGN})
        out.append({"body": body, "created": created, "updated": updated,
                    "reloaded_dto": pydantic_core.to_json(S._dto(reload)).decode()})
    return out


def ext_strip(canon: dict) -> dict:
    opts = CANONICAL_JSON_OPTIONS.with_options(tz_aware=True, tzinfo=dt.timezone.utc)
    return json_util.loads(json.dumps(canon), json_options=opts)


def substitute_cases() -> list:
    texts = ["{ends_at:R}", "{starts_at:R}", "{far:R} / {soon:R}", "{count:t}", "{nothing:R}", "{bad_ts:F}",
             "{name:x} {name}", "{missing} {line}", "{ends_at:Q}", "{count}", "no vars", "{a b}"]
    return [{"text": t, "out": R.substitute(t, CTX, RNOW), "stripped": R._strip_md(R.substitute(t, CTX, RNOW))}
            for t in texts]


def sniff_cases() -> list:
    samples = [
        BLOBS["a" * 64][:40], b"GIF89a\x10\x00\x20\x00" + b"\0" * 20, b"\xff\xd8\xff\xe0\x00\x10JFIF" + b"\0" * 20,
        b"\xff\xd8\xff\xc0\x00\x11\x08\x01\x02\x03\x04" + b"\0" * 20,
        b"RIFF\0\0\0\0WEBPVP8 " + b"\0" * 10 + bytes([0x34, 0x12, 0x78, 0x56]) + b"\0" * 4,
        b"RIFF\0\0\0\0WEBPVP8L" + b"\0" * 5 + bytes([0xFF, 0x3F, 0x00, 0x00]) + b"\0" * 4,
        b"RIFF\0\0\0\0WEBPVP8X" + b"\0" * 8 + bytes([9, 0, 0, 4, 0, 0]),
        b"RIFF\0\0\0\0WEBPVP8L" + b"\0" * 6, b"not an image at all, no", b"short",
    ]
    return [{"data": b64(s), "out": list(hub_store.sniff_image(s)) if hub_store.sniff_image(s) else None}
            for s in samples]


def main() -> None:
    data = {
        "blobs": {k: b64(v) for k, v in BLOBS.items()},
        "ctx": CTX,
        "now": RNOW,
        "renders": render_cases(),
        "models": model_cases(),
        "substitute": substitute_cases(),
        "bindings": pydantic_core.to_json({"bindings": S.bindings()}).decode(),
        "sniff": sniff_cases(),
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(data, ensure_ascii=False, indent=0), encoding="utf-8")
    print(OUT, OUT.stat().st_size)


if __name__ == "__main__":
    main()
