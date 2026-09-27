"""insignia.json - the sigil beside a player's name: a Power Rank shield on a set
of Mastery wings.

`prefabs/meta/insignia.binfab` (root) holds two ascending tables of rows `{0 from}`:
0 the Power Rank each shield starts at, 1 the mastery level each set of wings
starts at. Wings top out at 1100, Trove Mastery's 1000 levels plus Geode's 100, so
they follow total mastery.

The art is `ui/nameplate.swf` (the HUD and character sheet carry the same 88 px
set): the sprite placing children named `wings` and `shield`, one frame per
tier (`Nameplate.setText` jumps each to its frame). Every frame is a shape filled
with one bitmap. Frames are stored as base64 PNGs on one shared canvas, so a sigil
is its wings with its shield composited on top.
"""
from __future__ import annotations

import base64
import io
import struct
from typing import Any

from PIL import Image

from app.trove.decode.mastery import _single
from app.trove.decode.tree import GameTree
from app.trove.swf.extract import (
    BITMAP_TAGS,
    SHAPE_TAGS,
    _decode_jpeg,
    _decode_lossless,
    _decompress,
    _iter_tags,
    _shape_bitmap_fills,
    _sprite_frames,
)

TITLE = "Sigils"
OUTPUT = "insignia.json"
PREFIXES = ("prefabs/meta/insignia.binfab", "ui/nameplate.swf")
INDENT, FINAL_NEWLINE = 1, True

TABLE = "prefabs/meta/insignia.binfab"
SWF = "ui/nameplate.swf"
LAYERS = (("wings", "mastery", 1), ("shield", "power_rank", 0))


def _thresholds(table: dict) -> list[int]:
    out = [table[k][0] for k in sorted(table)]
    if out != sorted(out):
        raise ValueError(f"insignia thresholds not ascending: {out}")
    return out


def _layers(swf: bytes) -> dict[str, list[list[tuple[Image.Image, int, int]]]]:
    """Each layer's frames as (bitmap, x, y) pieces, in pixels from the sigil origin."""
    tables: bytes | None = None
    bitmaps: dict[int, tuple[int, bytes]] = {}
    shapes: dict[int, list] = {}
    sprites: dict[int, list] = {}
    for code, body in _iter_tags(_decompress(swf)):
        if code == 8:
            tables = body
        elif code in BITMAP_TAGS and len(body) >= 2:
            bitmaps[struct.unpack("<H", body[:2])[0]] = (code, body)
        elif code in SHAPE_TAGS and len(body) >= 2:
            shapes[struct.unpack("<H", body[:2])[0]] = _shape_bitmap_fills(body, SHAPE_TAGS[code])
        elif code == 39 and len(body) >= 2:
            sprites[struct.unpack("<H", body[:2])[0]] = _sprite_frames(body)

    names = {name for name, _, _ in LAYERS}
    for frames in sprites.values():
        children = {p.name: p for p in frames[0].values()} if frames else {}
        if names <= children.keys():
            break
    else:
        raise ValueError(f"{SWF}: no sprite places {sorted(names)}")

    def bitmap(char_id: int) -> Image.Image:
        code, body = bitmaps[char_id]
        return (_decode_lossless(body, code) if code in (20, 36) else _decode_jpeg(body, code, tables))[1]

    out: dict[str, list[list[tuple[Image.Image, int, int]]]] = {}
    for name in names:
        child = children[name]
        layer = []
        for frame in sprites[child.char_id]:
            pieces = []
            for depth in sorted(frame):
                fills = shapes.get(frame[depth].char_id)
                if not fills or len(fills) != 1:
                    raise ValueError(f"{SWF} {name}: frame piece is not one bitmap fill")
                char_id, m = fills[0]
                if abs(m.sx - 20) > 0.01 or abs(m.sy - 20) > 0.01 or m.r0 or m.r1:
                    raise ValueError(f"{SWF} {name}: bitmap fill is scaled or rotated")
                tx = child.matrix.tx + frame[depth].matrix.tx + m.tx
                ty = child.matrix.ty + frame[depth].matrix.ty + m.ty
                pieces.append((bitmap(char_id), round(tx / 20), round(ty / 20)))
            layer.append(pieces)
        out[name] = layer
    return out


def _png(img: Image.Image) -> str:
    buf = io.BytesIO()
    img.save(buf, format="PNG", optimize=True)
    return base64.b64encode(buf.getvalue()).decode("ascii")


def build(tree: GameTree) -> dict[str, Any]:
    table, swf = tree.read(TABLE), tree.read(SWF)
    if table is None or swf is None:
        raise ValueError(f"missing {TABLE if table is None else SWF}")
    root = _single(table)
    layers = _layers(swf)

    pieces = [p for frames in layers.values() for frame in frames for p in frame]
    left, top = min(x for _, x, _ in pieces), min(y for _, _, y in pieces)
    width = max(x + im.width for im, x, _ in pieces) - left
    height = max(y + im.height for im, _, y in pieces) - top

    out: dict[str, Any] = {"size": [width, height]}
    for name, key, field in LAYERS:
        froms, frames = _thresholds(root[field]), layers[name]
        if len(froms) != len(frames):
            raise ValueError(f"{name}: {len(froms)} tiers but {len(frames)} frames")
        tiers = []
        for start, frame in zip(froms, frames, strict=True):
            canvas = Image.new("RGBA", (width, height))
            for im, x, y in frame:
                canvas.alpha_composite(im.convert("RGBA"), (x - left, y - top))
            tiers.append({"from": start, "image": _png(canvas)})
        out[key] = tiers
    return out


def count(data: dict) -> int:
    return len(data.get("power_rank", [])) + len(data.get("mastery", []))
