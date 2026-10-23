#!/usr/bin/env python3
"""Golden outputs for the game-data decoders (native/kiwi-server/src/decode).

    python gen_decode_fixtures.py full GAME_ROOT OUT_DIR [name ...]
        every decoder (or the named ones) over an extracted client tree, written
        as OUT_DIR/<OUTPUT> exactly as the runner writes it, plus OUT_DIR/_counts.json
    python gen_decode_fixtures.py subset GAME_ROOT [name ...]
        cut those small trees from an extracted client: the first files each
        build reads, within a byte budget
    python gen_decode_fixtures.py view SRC_DIR OUT_DIR
        the Workshop binfab preview of every file under SRC_DIR, one .json each
        (testdata/decode/view holds a small committed set: binfabs/ and expected/)
    python gen_decode_fixtures.py fixtures [case ...]
        the small committed trees under testdata/decode/<name>/game (or the named
        cases): each one's golden output as expected.json next to it (and
        code_versions.json)
    python gen_decode_fixtures.py insignia [GAME_ROOT]
        the insignia cases: synthetic movies and tables (testdata/decode/insignia@*),
        plus the real pair from GAME_ROOT as testdata/decode/insignia

Run with the trove-prod venv from the repo root, with kiwi_native importable
(production parses with it), e.g. PYTHONPATH=<dir holding kiwi_native.pyd>, and
through linux_png.py (`linux_png.py run gen_decode_fixtures.py fixtures`) so the
sigil PNGs are deflated as production's Linux Pillow does.
"""
from __future__ import annotations

import io
import json
import sys
import time
import traceback
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))

from app.trove.decode.registry import DECODERS, code_version, dump  # noqa: E402
from app.trove.decode.tree import DirTree  # noqa: E402

TESTDATA = ROOT / "native" / "kiwi-server" / "testdata" / "decode"


def full(game: Path, out: Path, names: list[str]) -> None:
    out.mkdir(parents=True, exist_ok=True)
    names = names or list(DECODERS)
    prefixes = sorted({p for n in names for p in DECODERS[n].PREFIXES})
    tree = DirTree(game, prefixes)
    counts = {}
    for name in names:
        module = DECODERS[name]
        t0 = time.time()
        try:
            data = module.build(tree.only(module.PREFIXES))
            (out / module.OUTPUT).write_bytes(dump(module, data).encode("utf-8"))
            counts[name] = module.count(data)
        except Exception as exc:  # noqa: BLE001
            counts[name] = f"{type(exc).__name__}: {exc}"
            traceback.print_exc()
        print(f"{name:20} {counts[name]!s:>8}  {time.time() - t0:.1f}s", flush=True)
    (out / "_counts.json").write_text(json.dumps(counts, indent=1), encoding="utf-8")


class _Recording(DirTree):
    """A DirTree noting every file a build reads, in order."""

    def __init__(self, root, prefixes):
        super().__init__(root, prefixes)
        self.seen: list[str] = []

    def _fetch(self, path):
        if path not in self.seen:
            self.seen.append(path)
        return super()._fetch(path)


def subset(game: Path, names: list[str], budget: int, max_file: int) -> None:
    """Small committed trees: the first prefabs each build reads, then text tables,
    within a byte budget."""
    import shutil
    for name in names or list(DECODERS):
        module = DECODERS[name]
        tree = _Recording(game, list(module.PREFIXES))
        try:
            module.build(tree.only(module.PREFIXES))
        except Exception:  # noqa: BLE001
            pass
        dest = TESTDATA / name / "game"
        shutil.rmtree(dest, ignore_errors=True)
        used = 0
        text = [p for p in tree.seen if p.startswith("languages/")]
        for limit, paths in ((budget * 7 // 10, [p for p in tree.seen if p not in text]), (budget, text)):
            for path in paths:
                size = (game / path).stat().st_size
                if size > max_file or used + size > limit:
                    continue
                (dest / path).parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(game / path, dest / path)
                used += size
        print(name, used)


def view(src: Path, out: Path) -> None:
    """The Workshop binfab preview body for every file under ``src`` (as the route
    sends it, with ``path``), or ``!message`` where it answers 422."""
    from app.trove.decode import view as v

    out.mkdir(parents=True, exist_ok=True)
    for path in sorted(p for p in src.rglob("*") if p.is_file() and p.suffix != ".json"):
        data = path.read_bytes()
        rel = path.relative_to(src).as_posix()
        try:
            if len(data) > v.MAX_BYTES:
                raise v.BinfabViewError("This file is too large to lay out.")
            payload = v._describe_native(data)
            if payload is None:
                payload = v.describe(data)
            text = json.dumps({"path": rel, **payload}, ensure_ascii=False, separators=(",", ":"))
        except v.BinfabViewError as exc:
            text = f"!{exc}"
        (out / (rel.replace("/", "__") + ".json")).write_bytes(text.encode("utf-8"))


def _server_versions() -> dict[str, str]:
    """``code_version`` of every decoder over LF sources, as the server's checkout has them."""
    import pathlib

    original = pathlib.Path.read_bytes
    pathlib.Path.read_bytes = lambda self: original(self).replace(b"\r\n", b"\n")
    try:
        return {name: code_version(module) for name, module in DECODERS.items()}
    finally:
        pathlib.Path.read_bytes = original


def fixtures(only: list[str]) -> None:
    versions = _server_versions()
    (TESTDATA / "code_versions.json").write_text(json.dumps(versions, indent=1) + "\n", encoding="utf-8")
    for case in sorted(p for p in TESTDATA.iterdir() if (p / "game").is_dir()):
        if only and case.name not in only:
            continue
        name = case.name.split("@", 1)[0]
        module = DECODERS[name]
        tree = DirTree(case / "game", list(module.PREFIXES))
        try:
            text = dump(module, module.build(tree.only(module.PREFIXES)))
        except Exception as exc:  # noqa: BLE001
            text = f"!{type(exc).__name__}: {exc}"
        (case / "expected.json").write_bytes(text.encode("utf-8"))
        print(case.name, len(text))


def _varint(v: int) -> bytes:
    out = bytearray()
    while v >= 0x80:
        out.append((v & 0x7F) | 0x80)
        v >>= 7
    out.append(v)
    return bytes(out)


def _zz(v: int) -> bytes:
    return _varint(v << 1 if v >= 0 else (~v << 1) | 1)


def _insignia_table(power_rank: list[int], mastery: list[int]) -> bytes:
    """``{0: {i: {0: from}}, 1: {...}}`` as `prefabs/meta/insignia.binfab` writes it."""
    out = bytearray()
    for field, froms in ((0, power_rank), (1, mastery)):
        out += _zz(field << 6 | 0x27) + _zz(len(froms) << 6 | 0x17)
        for i, v in enumerate(froms):
            out += _zz(i << 3 | 1) + b"\x00" + _zz(v) + _zz(0x0F)
        out += _zz(4)
    return bytes(out + _zz(0x0F))


def _bits(fields: list[tuple[int, int]]) -> bytes:
    acc = n = 0
    for v, nb in fields:
        acc, n = (acc << nb) | (v & ((1 << nb) - 1)), n + nb
    pad = -n % 8
    return (acc << pad).to_bytes((n + pad) // 8, "big")


def _sbits(*vals: int) -> int:
    return max((v if v >= 0 else ~v).bit_length() for v in vals) + 1


def _matrix(tx: int = 0, ty: int = 0, scale: float | None = None) -> bytes:
    f: list[tuple[int, int]] = []
    if scale is None:
        f.append((0, 1))
    else:
        v = round(scale * 65536)
        nb = _sbits(v)
        f += [(1, 1), (nb, 5), (v, nb), (v, nb)]
    nb = _sbits(tx, ty)
    f += [(0, 1), (nb, 5), (tx, nb), (ty, nb)]
    return _bits(f)


def _tag(code: int, body: bytes) -> bytes:
    import struct
    if len(body) < 0x3F:
        return struct.pack("<H", code << 6 | len(body)) + body
    return struct.pack("<HI", code << 6 | 0x3F, len(body)) + body


def _place2(depth: int, char: int | None = None, matrix: bytes | None = None, name: str | None = None,
            move: bool = False) -> bytes:
    import struct
    flags = (0x01 if move else 0) | (0x02 if char is not None else 0) | (0x04 if matrix else 0) | \
        (0x20 if name else 0)
    body = bytes([flags]) + struct.pack("<H", depth)
    body += struct.pack("<H", char) if char is not None else b""
    body += matrix or b""
    body += name.encode() + b"\x00" if name else b""
    return _tag(26, body)


def _sprite(sid: int, frames: list[list[bytes]]) -> bytes:
    import struct
    body = struct.pack("<HH", sid, len(frames))
    for frame in frames:
        body += b"".join(frame) + _tag(1, b"")
    return _tag(39, body + _tag(0, b""))


def _insignia_movie(variant: str = "") -> bytes:
    """A nameplate movie exercising what the real one doesn't: lossless,
    indexed and JPEG3 bitmaps, pieces off the origin, half-pixel offsets, moves,
    removals and a PlaceObject3 child with a colour transform. ``variant`` breaks
    it one way."""
    import struct
    import zlib

    from PIL import Image

    rect = _bits([(16, 5), (0, 16), (1760, 16), (0, 16), (1760, 16)])
    tags = b""
    argb = bytes([255, 200, 30, 30, 128, 64, 0, 128, 0, 0, 0, 0, 64, 10, 20, 30, 255, 0, 255, 0, 200, 100, 150, 200])
    pixels = {"badzlib": b"\x00\x01garbage", "truncated": zlib.compress(argb)[:-6]}.get(variant, zlib.compress(argb))
    tags += _tag(36, struct.pack("<HBHH", 1, 5, 3, 2) + pixels)
    palette = bytes([10, 20, 30, 250, 240, 230, 0, 128, 255])
    rows = bytes([0, 1, 0, 0, 2, 1, 0, 0])
    tags += _tag(20, struct.pack("<HBHHB", 2, 3, 2, 2, 2) + zlib.compress(palette + rows))
    jpg = io.BytesIO()
    Image.new("RGB", (4, 3), (90, 160, 220)).save(jpg, format="JPEG", quality=90)
    alpha = zlib.compress(bytes([255, 128, 0, 64] * 3))
    tags += _tag(35, struct.pack("<HI", 3, len(jpg.getvalue())) + jpg.getvalue() + alpha)
    for sid, bid, tx in ((10, 1, 0), (11, 2, -40), (12, 3, 0), (13, 1, 10)):
        bid = 99 if variant == "nobitmap" and sid == 10 else bid
        scale = 10.0 if variant == "scaled" and sid == 13 else 20.0
        fills = bytes([1, 0x41]) + struct.pack("<H", bid) + _matrix(tx, 0, scale)
        tags += _tag(2, struct.pack("<H", sid) + rect + fills + b"\x00\x10\x00")
    tags += _sprite(20, [[_place2(1, 10, _matrix())],
                         [_place2(1, matrix=_matrix(30, 10), move=True), _place2(2, 11)]])
    tags += _sprite(21, [[_place2(1, 12)],
                         [_tag(28, struct.pack("<H", 1)), _place2(3, 13, _matrix(-50, -50))]])
    tags += _sprite(25, [[_place2(1, 20, name="wings")]])
    cx = _bits([(0, 1), (1, 1), (8, 4), (256, 8), (256, 8), (256, 8), (256, 8)])
    po3 = _tag(70, bytes([0x02 | 0x04 | 0x08 | 0x20, 0]) + struct.pack("<HH", 2, 21) + _matrix(-20, 40) + cx
               + b"shield\x00")
    if variant != "nosprite":
        tags += _sprite(30, [[_place2(1, 20, _matrix(40, -20), "wings"), po3]])
    tags += _tag(1, b"") + _tag(0, b"")
    body = rect + struct.pack("<HH", 24 << 8, 1) + tags
    return b"FWS" + bytes([10]) + struct.pack("<I", 8 + len(body)) + body


def insignia(game: Path | None) -> None:
    import shutil
    movie = _insignia_movie()
    cases = {
        "synthetic": (_insignia_table([0, 250], [0, 10]), movie),
        "unsorted": (_insignia_table([0, 250], [10, 0]), movie),
        "tiers": (_insignia_table([0, 250], [0, 10, 30]), movie),
        "missing": (_insignia_table([0], [0]), None),
    }
    for variant in ("badzlib", "truncated", "nobitmap", "scaled", "nosprite"):
        cases[variant] = (_insignia_table([0, 250], [0, 10]), _insignia_movie(variant))
    for case, (table, swf) in cases.items():
        dest = TESTDATA / f"insignia@{case}" / "game"
        shutil.rmtree(dest.parent, ignore_errors=True)
        (dest / "prefabs/meta").mkdir(parents=True)
        (dest / "prefabs/meta/insignia.binfab").write_bytes(table)
        if swf is not None:
            (dest / "ui").mkdir()
            (dest / "ui/nameplate.swf").write_bytes(swf)
    if game is not None:
        dest = TESTDATA / "insignia" / "game"
        shutil.rmtree(dest.parent, ignore_errors=True)
        for rel in ("prefabs/meta/insignia.binfab", "ui/nameplate.swf"):
            (dest / rel).parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(game / rel, dest / rel)


if __name__ == "__main__":
    if sys.argv[1] == "full":
        full(Path(sys.argv[2]), Path(sys.argv[3]), sys.argv[4:])
    elif sys.argv[1] == "view":
        view(Path(sys.argv[2]), Path(sys.argv[3]))
    elif sys.argv[1] == "subset":
        subset(Path(sys.argv[2]), sys.argv[3:], budget=100_000, max_file=72_000)
    elif sys.argv[1] == "insignia":
        insignia(Path(sys.argv[2]) if len(sys.argv) > 2 else None)
    else:
        fixtures(sys.argv[2:])
