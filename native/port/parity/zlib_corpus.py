"""A deflate corpus for proving the Rust side's classic zlib equals CPython's.

Production Python (Linux) compresses with classic zlib 1.3.1, and so does
CPython 3.13 on Windows (`zlib.ZLIB_VERSION`). This writes inputs (random,
text, binary game files, PNG scanlines, runs, boundary sizes) and, for every
level 0-9, strategy, memLevel, wbits and feeding pattern, the sha256 of what
CPython's `zlib.compressobj` (or one-shot `zlib.compress`) produces. The
ignored tests `zlib::tests::cpython_corpus` (kiwi-core) and
`png::tests::zipencode_corpus` (kiwi-pil) replay it:

    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/zlib_corpus.py OUTDIR
    $env:KIWI_ZLIB_CORPUS = "OUTDIR"; cargo test -p kiwi-core -p kiwi-pil -- --ignored corpus
"""

from __future__ import annotations

import hashlib
import json
import random
import struct
import sys
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
assert zlib.ZLIB_RUNTIME_VERSION == "1.3.1", zlib.ZLIB_RUNTIME_VERSION


def png_rows(path: Path) -> tuple[bytes, int] | None:
    data = path.read_bytes()
    pos, idat, row = 8, b"", 0
    while pos < len(data):
        n, kind = struct.unpack(">I4s", data[pos:pos + 8])
        body = data[pos + 8:pos + 8 + n]
        if kind == b"IHDR":
            w, _h, depth, color = struct.unpack(">IIBB", body[:10])
            if body[12]:
                return None
            channels = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}[color]
            row = 1 + (w * channels * depth + 7) // 8
        elif kind == b"IDAT":
            idat += body
        pos += 12 + n
    return zlib.decompress(idat), row


def inputs() -> list[tuple[str, bytes, int]]:
    rnd = random.Random(1234)
    out: list[tuple[str, bytes, int]] = []
    add = lambda name, data, row=0: out.append((name, data, row))  # noqa: E731
    add("empty", b"")
    add("one", b"x")
    add("hello", b"hello hello hello")
    for n in (100, 5000, 70000, 300000, 2_000_000):
        add(f"random{n}", rnd.randbytes(n))
    for n in (1000, 100000, 1_500_000):
        add(f"zeros{n}", bytes(n))
    add("abab", b"ab" * 200000)
    for n in (5000, 400000):
        add(f"low_entropy{n}", bytes(rnd.choice(b"\x00\x01\x02\xff") for _ in range(n)))
        add(f"skewed{n}", bytes(min(255, int(rnd.expovariate(0.05))) for _ in range(n)))
    text = b"".join(p.read_bytes() for p in sorted((ROOT / "app").rglob("*.py")))
    add("python4m", text[:4_000_000])
    for n in (257, 258, 259, 16383, 16384, 32767, 32768, 32769, 65535, 65536, 65537, 131072, 262145):
        add(f"text{n}", text[1_000_000:1_000_000 + n])
    gd = ROOT / "app" / "trove" / "gamedata"
    jsons = sorted(p for p in gd.rglob("*.json") if p.stat().st_size > 20000)
    for p in jsons[:6]:
        add(f"json:{p.name}", p.read_bytes()[:3_000_000])
    binaries = sorted((ROOT / "native" / "kiwi-pil" / "testdata" / "fonts").glob("*.ttf"))
    binaries += sorted(p for p in (ROOT / "native" / "kiwi-server" / "testdata").rglob("*")
                       if p.suffix in (".tfa", ".binfab", ".swf", ".bnk", ".jpg", ".dds", ".tmod", ".gz"))[:8]
    for p in binaries:
        add(f"bin:{p.name}", p.read_bytes()[:2_000_000])
    pngs = sorted((ROOT / "native" / "kiwi-server" / "testdata").rglob("*.png"))[::3]
    pngs += sorted((ROOT / "native" / "kiwi-pil" / "testdata").rglob("*.png"))
    for p in pngs:
        r = png_rows(p)
        if r:
            add(f"png:{p.parent.name}/{p.name}", r[0], r[1])
    mixed = bytearray()
    while len(mixed) < 1_200_000:
        k = rnd.random()
        mixed += rnd.randbytes(rnd.randint(1, 3000)) if k < 0.3 else text[rnd.randint(0, 3_000_000):][:rnd.randint(1, 9000)]
    add("mixed", bytes(mixed))
    return out


def feed(z, data: bytes, plan: str, row: int) -> bytes:
    if plan == "whole":
        parts = [data]
    elif plan == "rows":
        parts = [data[i:i + row] for i in range(0, len(data), row)]
    elif plan.startswith("chunk"):
        n = int(plan[5:])
        parts = [data[i:i + n] for i in range(0, len(data), n)]
    else:
        sizes, parts, i, seed = (1, 7, 300, 4096, 40000, 100000), [], 0, len(data)
        while i < len(data):
            seed = (seed * 6364136223846793005 + 1442695040888963407) % (1 << 64)
            n = sizes[(seed >> 33) % len(sizes)]
            parts.append(data[i:i + n])
            i += n
    return b"".join(z.compress(p) for p in parts)


def main() -> None:
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    blob = bytearray()
    index = []
    cases = []
    for name, data, row in inputs():
        index.append({"name": name, "offset": len(blob), "len": len(data), "row": row})
        blob += data
    for i, item in enumerate(index):
        data = bytes(blob[item["offset"]:item["offset"] + item["len"]])
        big = len(data) > 500_000
        plans = ["whole"] + (["rows"] if item["row"] else []) + ([] if big else ["chunk32768", "random"])
        for level in range(-1, 10):
            for strategy in (0, 1) if not big or level in (0, 1, 5, 6, 9) else (1,):
                for mem in (8, 9):
                    for wbits in (15, -15, 31) if not big else (15,):
                        for plan in plans:
                            for flush in (zlib.Z_FINISH, zlib.Z_SYNC_FLUSH) if plan != "rows" else (zlib.Z_FINISH,):
                                z = zlib.compressobj(level, zlib.DEFLATED, wbits, mem, strategy)
                                c = feed(z, data, plan, item["row"]) + z.flush(flush)
                                cases.append([i, level, strategy, mem, wbits, plan, flush, hashlib.sha256(c).hexdigest()])
        for level in range(-1, 10):
            for wbits in (15, -15, 31):
                c = zlib.compress(data, level, wbits)
                cases.append([i, level, 0, 8, wbits, "oneshot", zlib.Z_FINISH, hashlib.sha256(c).hexdigest()])
        for strategy in (2, 3, 4):
            z = zlib.compressobj(6, zlib.DEFLATED, 15, 8, strategy)
            c = feed(z, data, "whole", 0) + z.flush()
            cases.append([i, 6, strategy, 8, 15, "whole", zlib.Z_FINISH, hashlib.sha256(c).hexdigest()])
        print(f"{i + 1}/{len(index)} {item['name']} ({len(cases)} cases)", flush=True)
    (out / "inputs.bin").write_bytes(blob)
    (out / "cases.json").write_text(json.dumps({"zlib": zlib.ZLIB_RUNTIME_VERSION, "inputs": index, "cases": cases}))
    print(f"{len(index)} inputs, {len(cases)} cases, {len(blob)} input bytes")


if __name__ == "__main__":
    main()
