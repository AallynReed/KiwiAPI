"""Golden fixtures for SWF and audio (app/trove/swf, app/trove/audio and their
/site routes), from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/linux_png.py run native/port/parity/gen_media_fixtures.py
Writes native/kiwi-server/testdata/media/: golden.json, synthetic movies and a
synthetic bank, plus a few small real files (E:/Trove; regenerate only with
them present).

The Python reference runs without kiwi_native here, so every result is the
pure-Python path, which is what the Rust port must equal. Routes run through
the real app (raw ASGI) with the archive and both content stores in temp dirs
and the bp_cache index unreachable (so every build is a cold one).
"""
import asyncio
import base64
import hashlib
import io
import json
import logging
import lzma
import os
import shutil
import struct
import sys
import tempfile
import time
import types
import urllib.parse
import warnings
import zipfile
import zlib
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
sys.path.insert(0, os.getcwd())

TMP = Path(tempfile.mkdtemp(prefix="media-fixtures-"))
os.environ.update({
    "SECRET_KEY": "media-fixture-secret",
    "TROVE_UPDATE_STORE_DIR": str(TMP / "updates"),
    "MODS_STORE_DIR": str(TMP / "mods"),
})

from PIL import Image  # noqa: E402
from pymongo.asynchronous.database import AsyncDatabase  # noqa: E402

from app.admin import runtime_config as rc  # noqa: E402

for _key, _spec in rc.REGISTRY.items():
    rc._CACHE[_key] = (_spec.default, float("inf"))

from app.trove.audio import bank as bank_reader  # noqa: E402
from app.trove.audio import encode as audio_encode  # noqa: E402
from app.trove.audio import names as audio_names  # noqa: E402
from app.trove.audio import service as audio_service  # noqa: E402
from app.trove.audio import studio as audio_studio  # noqa: E402
from app.trove.audio import wem as wem_reader  # noqa: E402
from app.trove.swf import extract  # noqa: E402
from app.trove.swf import service as swf_service  # noqa: E402
from app.trove.updates.cas import ContentStore  # noqa: E402

OUT = Path(__file__).resolve().parents[2] / "kiwi-server" / "testdata" / "media"
OUT.mkdir(parents=True, exist_ok=True)
GAME = Path("E:/Trove")


async def _cmd(self, *a, **k):
    return {"version": "7.0.0"}


async def _lcn(self, *a, **k):
    return []


AsyncDatabase.command = _cmd
AsyncDatabase.list_collection_names = _lcn


def sha(b):
    return hashlib.sha256(b).hexdigest()


def b64(b):
    return base64.b64encode(b).decode()


def lcg_bytes(seed, n):
    x = seed & 0x7FFFFFFF
    out = bytearray(n)
    for i in range(n):
        x = (x * 1103515245 + 12345) & 0x7FFFFFFF
        out[i] = (x >> 16) & 0xFF
    return bytes(out)


def pattern(w, h, seed, kind):
    """Deterministic RGBA pixels; the Rust tests build the same."""
    if kind == "noise":
        return lcg_bytes(seed, w * h * 4)
    out = bytearray(w * h * 4)
    noise = lcg_bytes(seed, w * h)
    for y in range(h):
        for x in range(w):
            i = (y * w + x) * 4
            n = noise[y * w + x]
            out[i] = (x * 255 // max(w - 1, 1)) & 0xFF
            out[i + 1] = (y * 255 // max(h - 1, 1)) & 0xFF
            out[i + 2] = (x * y + n // 8) & 0xFF
            a = 255 if kind == "opaque" else ((x + y) * 7 + n // 4) & 0xFF
            out[i + 3] = a
    return bytes(out)


# --- Pillow primitives -----------------------------------------------------------

def pil_tables():
    grid = bytes(v for c in range(256) for a in range(256) for v in (c, c, c, a))
    pre = Image.frombytes("RGBA", (256, 256), grid).convert("RGBa").tobytes()
    un = Image.frombytes("RGBa", (256, 256), grid).convert("RGBA").tobytes()
    cmyk = bytes(v for c in range(256) for k in range(256) for v in (c, 255 - c, c // 2, k))
    cm = Image.frombytes("CMYK", (256, 256), cmyk).convert("RGBA").tobytes()
    return {"premultiply": sha(pre), "unpremultiply": sha(un), "cmyk": sha(cm)}


THUMBS = [(129, 1, "noise"), (1, 300, "noise"), (200, 150, "alpha"), (150, 200, "opaque"), (300, 300, "noise"),
          (257, 129, "alpha"), (1000, 37, "alpha"), (640, 480, "opaque"), (130, 129, "noise"), (2000, 3, "alpha")]


def pil_thumbs():
    out = []
    for i, (w, h, kind) in enumerate(THUMBS):
        px = pattern(w, h, 1000 + i, kind)
        img = Image.frombytes("RGBA", (w, h), px)
        thumb = extract._make_thumb(img)
        small = img.copy()
        small.thumbnail((extract.THUMB_BOX, extract.THUMB_BOX), Image.LANCZOS)
        out.append({"w": w, "h": h, "kind": kind, "seed": 1000 + i, "png": sha(extract._pack_png(img)),
                    "thumb": sha(thumb) if thumb else None, "thumb_size": list(small.size),
                    "thumb_rgba": sha(small.tobytes())})
    return out


def jpeg_cases():
    cases = []
    base = Image.frombytes("RGBA", (61, 45), pattern(61, 45, 7, "opaque")).convert("RGB")
    variants = [
        ("420", base, {"quality": 80}),
        ("422", base, {"quality": 70, "subsampling": 1}),
        ("444", base, {"quality": 90, "subsampling": 0}),
        ("gray", base.convert("L"), {"quality": 75}),
        ("progressive", base, {"quality": 60, "progressive": True}),
        ("optimized", base, {"quality": 85, "optimize": True}),
        ("cmyk", base.convert("CMYK"), {"quality": 80}),
        ("icc", base, {"quality": 80, "icc_profile": b"\x00\x01fake-icc-profile" * 40}),
        ("restart", base, {"quality": 75, "restart_marker_blocks": 2}),
    ]
    for name, im, kw in variants:
        buf = io.BytesIO()
        im.save(buf, format="JPEG", **kw)
        cases.append((name, buf.getvalue()))
    good = cases[0][1]
    cases.append(("truncated", good[: len(good) // 2]))
    cases.append(("no_eoi", good[:-2]))
    cases.append(("junk_before_marker", good[:2] + b"\x00\x00" + good[2:]))
    big = Image.frombytes("RGBA", (300, 170), pattern(300, 170, 8, "opaque")).convert("RGB")
    buf = io.BytesIO()
    big.save(buf, format="JPEG", quality=70)
    cases.append(("big", buf.getvalue()))
    out = []
    for name, data in cases:
        rec = {"name": name, "jpeg": b64(data)}
        try:
            img = Image.open(io.BytesIO(data))
            img.load()
            rgba = img.convert("RGBA")
            rec.update({"size": list(img.size), "mode": img.mode, "rgba": sha(rgba.tobytes()),
                        "png": sha(extract._pack_png(rgba)),
                        "thumb": sha(t) if (t := extract._make_thumb(rgba)) else None})
        except Exception as exc:  # noqa: BLE001
            rec["error"] = type(exc).__name__
        out.append(rec)
    return out


def png_payloads():
    out = []
    pal = Image.frombytes("RGBA", (40, 30), pattern(40, 30, 3, "alpha")).convert("RGB").quantize(64)
    ims = []
    b = io.BytesIO()
    pal.save(b, format="PNG", transparency=5)
    ims.append(("p_trns_index", b.getvalue()))
    b = io.BytesIO()
    pal.save(b, format="PNG", transparency=bytes(range(0, 200, 5)))
    ims.append(("p_trns_bytes", b.getvalue()))
    gray = Image.frombytes("RGBA", (40, 30), pattern(40, 30, 4, "opaque")).convert("L")
    b = io.BytesIO()
    gray.save(b, format="PNG", transparency=77)
    ims.append(("l_trns", b.getvalue()))
    b = io.BytesIO()
    Image.frombytes("RGBA", (40, 30), pattern(40, 30, 5, "alpha")).convert("LA").save(b, format="PNG")
    ims.append(("la", b.getvalue()))
    rgb = Image.frombytes("RGBA", (40, 30), pattern(40, 30, 6, "opaque")).convert("RGB")
    b = io.BytesIO()
    rgb.save(b, format="PNG", transparency=(255, 0, 132))
    ims.append(("rgb_trns", b.getvalue()))
    b = io.BytesIO()
    Image.frombytes("RGBA", (40, 30), pattern(40, 30, 9, "alpha")).save(b, format="PNG")
    ims.append(("rgba", b.getvalue()))
    b = io.BytesIO()
    gray.point(lambda v: 255 if v > 128 else 0).convert("1").save(b, format="PNG")
    ims.append(("one", b.getvalue()))
    b = io.BytesIO()
    Image.frombytes("I;16", (40, 30), lcg_bytes(11, 40 * 30 * 2)).save(b, format="PNG")
    ims.append(("i16", b.getvalue()))
    b = io.BytesIO()
    Image.frombytes("RGBA", (40, 30), pattern(40, 30, 12, "alpha")).convert("RGB").quantize(16).save(
        b, format="PNG", bits=4)
    ims.append(("p_4bit", b.getvalue()))
    b = io.BytesIO()
    pal.save(b, format="GIF", transparency=3)
    ims.append(("gif_trns", b.getvalue()))
    b = io.BytesIO()
    gray.save(b, format="GIF")
    ims.append(("gif_gray", b.getvalue()))
    for name, data in ims:
        rec = {"name": name, "data": b64(data)}
        try:
            img = Image.open(io.BytesIO(data))
            img.load()
            rgba = img.convert("RGBA")
            rec.update({"size": list(img.size), "mode": img.mode, "rgba": sha(rgba.tobytes()),
                        "png": sha(extract._pack_png(rgba))})
        except Exception as exc:  # noqa: BLE001
            rec["error"] = type(exc).__name__
        out.append(rec)
    return out


# --- synthetic SWF movies ---------------------------------------------------------

def tag(code, body):
    if len(body) < 0x3F and code not in (6, 20, 21, 35, 36, 90):
        return struct.pack("<H", (code << 6) | len(body)) + body
    return struct.pack("<HI", (code << 6) | 0x3F, len(body)) + body


class Bits:
    def __init__(self):
        self.v, self.n = 0, 0

    def put(self, value, n):
        self.v = (self.v << n) | (value & ((1 << n) - 1))
        self.n += n

    def bytes(self):
        pad = (-self.n) % 8
        v = self.v << pad
        return v.to_bytes((self.n + pad) // 8, "big") if self.n + pad else b""


def rect(xmax, ymax):
    nb = max(xmax.bit_length(), ymax.bit_length()) + 1
    b = Bits()
    b.put(nb, 5)
    for v in (0, xmax, 0, ymax):
        b.put(v, nb)
    return b.bytes()


def matrix(sx=None, tx=0, ty=0):
    b = Bits()
    if sx is None:
        b.put(0, 1)
    else:
        b.put(1, 1)
        b.put(20, 5)
        b.put(sx, 20)
        b.put(sx, 20)
    b.put(0, 1)
    nb = max(abs(tx).bit_length(), abs(ty).bit_length()) + 1
    b.put(nb, 5)
    b.put(tx, nb)
    b.put(ty, nb)
    return b.bytes()


def define_shape(code, shape_id, bitmap_id, version_rgba=False):
    body = struct.pack("<H", shape_id) + rect(400, 300)
    if code == 83:
        body += rect(400, 300) + b"\x00"
    body += b"\x02"                                       # two fill styles
    body += b"\x00" + (b"\x10\x20\x30\x40" if version_rgba else b"\x10\x20\x30")
    body += b"\x41" + struct.pack("<H", bitmap_id) + matrix(20 * 65536)
    body += b"\x00"                                       # line styles
    body += b"\x11\x00"                                   # numbits + end record
    return tag(code, body)


def sprite(sprite_id, children):
    inner = b""
    for depth, child in enumerate(children, 1):
        inner += tag(26, bytes([0x06]) + struct.pack("<HH", depth, child) + matrix(None, 20, 40))
    inner += tag(26, bytes([0x22]) + struct.pack("<HH", 50, children[0]) + b"named\x00")
    inner += tag(70, bytes([0x02, 0x08]) + struct.pack("<H", 60) + b"Cls\x00" + struct.pack("<H", children[-1]))
    inner += tag(1, b"") + tag(0, b"")
    return tag(39, struct.pack("<HH", sprite_id, 1) + inner)


def jpeg_of(w, h, seed, **kw):
    im = Image.frombytes("RGBA", (w, h), pattern(w, h, seed, "opaque")).convert("RGB")
    b = io.BytesIO()
    im.save(b, format="JPEG", **kw)
    return b.getvalue()


def lossless(code, cid, fmt, w, h, seed, colors=0):
    px = pattern(w, h, seed, "alpha")
    if fmt == 3:
        psize = 4 if code == 36 else 3
        pal = lcg_bytes(seed + 1, colors * psize)
        stride = (w + 3) & ~3
        rows = b"".join(bytes(px[y * w + x] % (colors + 3) for x in range(w)) + b"\0" * (stride - w)
                        for y in range(h))
        data = pal + rows
        return tag(code, struct.pack("<HBHHB", cid, 3, w, h, colors - 1) + zlib.compress(data))
    if fmt == 4:
        stride = (w * 2 + 3) & ~3
        data = b"".join(px[y * w * 2:y * w * 2 + w * 2] + b"\0" * (stride - w * 2) for y in range(h))
        return tag(code, struct.pack("<HBHH", cid, 4, w, h) + zlib.compress(data))
    data = bytearray(px)
    if code == 36:
        for i in range(0, len(data), 4):          # premultiplied ARGB
            a = data[i]
            for k in (1, 2, 3):
                data[i + k] = data[i + k] * a // 255
    return tag(code, struct.pack("<HBHH", cid, 5, w, h) + zlib.compress(bytes(data)))


def jpeg3(code, cid, jpeg, alpha):
    extra = b"\x00\x00" if code == 90 else b""
    return tag(code, struct.pack("<H", cid) + extra + struct.pack("<I", len(jpeg)) + jpeg
               + (zlib.compress(alpha) if alpha is not None else b""))


def movie(tags, version=10):
    body = rect(8000, 6000) + struct.pack("<BBH", 0x80, 29, 3) + b"".join(tags) + tag(0, b"")
    return b"FWS" + bytes([version]) + struct.pack("<I", 8 + len(body)) + body


def synthetic_movies():
    j_small = jpeg_of(48, 32, 21, quality=80)
    j_big = jpeg_of(136, 130, 22, quality=50)
    tables_src = jpeg_of(64, 40, 23, quality=70)
    # Split a whole JPEG into a JPEGTables tag (SOI + tables + EOI) and DefineBits scan data.
    sos = tables_src.index(b"\xff\xda")
    tables = tables_src[:sos] + b"\xff\xd9"
    scan = b"\xff\xd8" + tables_src[sos:]
    png = io.BytesIO()
    Image.frombytes("RGBA", (150, 20), pattern(150, 20, 24, "alpha")).save(png, format="PNG")
    gif = io.BytesIO()
    Image.frombytes("RGBA", (30, 20), pattern(30, 20, 25, "alpha")).convert("RGB").quantize(16).save(
        gif, format="GIF", transparency=2)
    a_soft = bytes((x * 9 + y * 5) & 0xFF for y in range(130) for x in range(136))
    a_full = b"\xff" * (48 * 32)
    stray = j_small[:-2] + b"\xff\xd9\xff\xd8" + b"\xff\xd9"
    tags = [
        tag(9, b"\x10\x20\x30"),
        tag(8, tables),
        tag(6, struct.pack("<H", 40) + scan),
        tag(21, struct.pack("<H", 3) + j_big),
        tag(21, struct.pack("<H", 4) + png.getvalue()),
        tag(21, struct.pack("<H", 5) + gif.getvalue()),
        jpeg3(35, 6, j_big, a_soft),
        jpeg3(35, 7, j_small, a_full),
        jpeg3(35, 8, j_small, None),
        jpeg3(90, 9, j_big, a_soft[: 136 * 70]),
        tag(21, struct.pack("<H", 10) + stray),
        lossless(20, 11, 3, 37, 23, 31, colors=17),
        lossless(20, 12, 4, 33, 21, 32),
        lossless(20, 13, 5, 140, 20, 33),
        lossless(36, 14, 3, 30, 30, 34, colors=40),
        lossless(36, 15, 5, 131, 30, 35),
        lossless(36, 2, 5, 3, 3, 36),
        tag(21, struct.pack("<H", 16) + b"not an image at all"),
        tag(36, struct.pack("<HBHH", 17, 5, 0, 0)),
        define_shape(2, 100, 11),
        define_shape(22, 101, 12),
        define_shape(32, 102, 6, version_rgba=True),
        define_shape(83, 103, 14, version_rgba=True),
        sprite(200, [100, 101]),
        sprite(201, [102, 200]),
        tag(10, b"\x01\x02"), tag(11, b"\x00"), tag(37, b"\x00"), tag(14, b"\x00"), tag(46, b"\x00"),
        tag(7, b"\x00"), tag(60, b"\x00"), tag(87, b"\x00"), tag(82, b"\x00\x00\x00\x00"), tag(39, b"\x01"),
        tag(76, struct.pack("<H", 3) + struct.pack("<H", 201) + b"ui.Panel\x00" + struct.pack("<H", 3)
            + b"DirectBitmap\x00" + struct.pack("<H", 103) + "Émoji ✓\x00".encode()),
        tag(56, struct.pack("<H", 1) + struct.pack("<H", 13) + b"exported\x00"),
        tag(76, struct.pack("<H", 1) + struct.pack("<H", 3) + b"Renamed\x00"),
    ]
    fws = movie(tags)
    cws = b"CWS" + fws[3:8] + zlib.compress(fws[8:])
    props = lzma._encode_filter_properties({"id": lzma.FILTER_LZMA1, "dict_size": 1 << 16})
    comp = lzma.compress(fws[8:], format=lzma.FORMAT_RAW,
                         filters=[{"id": lzma.FILTER_LZMA1, "dict_size": 1 << 16}])
    zws = b"ZWS" + fws[3:8] + struct.pack("<I", len(comp)) + props + comp
    crash = movie([tag(39, struct.pack("<HH", 300, 1) + tag(26, b"\x02\x01\x00\x05"))])
    return {
        "synthetic_fws.swf": fws, "synthetic_cws.swf": cws, "synthetic_zws.swf": zws,
        "sprite_crash.swf": crash,
    }


def manifest_case(name, raw):
    store_dir = TMP / f"store-{name}"
    swf_service._store = ContentStore(store_dir)
    rec = {"file": name}
    try:
        rec["manifest"] = swf_service._build_sync(raw)
    except extract.SwfError as exc:
        rec["swf_error"] = str(exc)
    except Exception as exc:  # noqa: BLE001
        rec["crash"] = type(exc).__name__
    try:
        h = extract.read_header(raw)
        rec["header"] = [h.version, h.compression, h.width, h.height, h.frame_rate, h.frame_count]
    except extract.SwfError as exc:
        rec["header_error"] = str(exc)
    return rec


def swf_cases():
    files = synthetic_movies()
    real = ["playerhealth.swf", "controllernotification.swf", "pvpplayercounterui.swf"]
    for n in real:
        p = GAME / "ui" / n
        if p.exists():
            files[n] = p.read_bytes()
        elif (OUT / n).exists():
            files[n] = (OUT / n).read_bytes()
    out = []
    for name, raw in files.items():
        (OUT / name).write_bytes(raw)
        out.append(manifest_case(name, raw))
    headers = []
    for raw in [b"", b"FWS", b"XYZ\x0a\0\0\0\0", b"CWS\x0a\0\0\0\0garbage", b"FWS\x0a\x10\0\0\0",
                b"FWS\x0a\x10\0\0\0\xff", b"ZWS\x0a\0\0\0\0\0\0\0\0\x5d\0\0\x01\0junk",
                b"CWS\x0a\0\0\0\0" + zlib.compress(b"x")[:3]]:
        try:
            h = extract.read_header(raw)
            headers.append({"raw": b64(raw), "ok": [h.version, h.compression, h.width, h.height,
                                                     h.frame_rate, h.frame_count]})
        except extract.SwfError as exc:
            headers.append({"raw": b64(raw), "error": str(exc)})
    return out, headers


def zip_case():
    real_time = zipfile.time
    zipfile.time = types.SimpleNamespace(time=lambda: 0.0, localtime=lambda _t: (2026, 10, 22, 13, 37, 59, 0, 0, 0))
    orig_init = zipfile.ZipInfo.__init__

    def init(self, *a, **k):
        orig_init(self, *a, **k)
        self.create_system = 3

    zipfile.ZipInfo.__init__ = init
    try:
        store_dir = TMP / "store-synthetic_fws.swf"
        swf_service._store = ContentStore(store_dir)
        manifest = swf_service._build_sync((OUT / "synthetic_fws.swf").read_bytes())
        assets = manifest["assets"] + [dict(manifest["assets"][0], sha="0" * 64)]
        swf_zip = swf_service.build_zip(assets)
        audio_service._store = ContentStore(TMP / "store-bank")
        bank_manifest = audio_service._build_sync((OUT / "synthetic.bnk").read_bytes(),
                                                  (OUT / "synthetic.txt").read_text(encoding="utf-8"))
        bnk_zip = audio_service.build_zip(bank_manifest["sounds"])
    finally:
        zipfile.time = real_time
        zipfile.ZipInfo.__init__ = orig_init
    return {"swf": sha(swf_zip), "swf_len": len(swf_zip), "bnk": sha(bnk_zip), "bnk_len": len(bnk_zip),
            "swf_names": [i.filename for i in zipfile.ZipFile(io.BytesIO(swf_zip)).infolist()],
            "bnk_names": [i.filename for i in zipfile.ZipFile(io.BytesIO(bnk_zip)).infolist()]}


# --- audio ----------------------------------------------------------------------

WEMS = {
    "vorbis_ui.wem": ("ui.bnk", 1040614984),
    "vorbis_mobs.wem": ("mobs.bnk", 921210715),
    "vorbis_biome.wem": ("amb_biome_01.bnk", 139572073),
    "adpcm_foley.wem": ("foley.bnk", 134783521),
    "pcm_foley.wem": ("foley.bnk", 13289078),
}


def real_wems():
    out = {}
    for name, (bnk, mid) in WEMS.items():
        if (OUT / name).exists():
            out[name] = (OUT / name).read_bytes()
            continue
        b = bank_reader.parse((GAME / "audio" / bnk).read_bytes())
        m = next(x for x in b.media if x.media_id == mid)
        out[name] = b.media_bytes(m)
        (OUT / name).write_bytes(out[name])
    return out


def pcm_samples(frames, channels, seed):
    raw = lcg_bytes(seed, frames * channels * 2)
    return bytes(raw)


def broken_wems(vorbis):
    out = {}
    out["not_riff.wem"] = b"JUNKJUNKJUNK"
    out["no_data.wem"] = b"RIFF\x10\0\0\0WAVEfmt \x10\0\0\0" + b"\0" * 16
    pcm = audio_encode.pcm_wem(pcm_samples(100, 1, 5), 1, 22050)
    out["pcm_ok.wem"] = pcm
    out["pcm8.wem"] = pcm.replace(b"\x02\x00\x10\x00\x06\x00", b"\x02\x00\x08\x00\x06\x00", 1)
    out["codec_bad.wem"] = pcm.replace(b"\xfe\xff\x01\x00", b"\x01\x00\x01\x00", 1)
    out["rate_bad.wem"] = pcm[:24] + struct.pack("<I", 500) + pcm[28:]
    adp = audio_encode.adpcm_wem(pcm_samples(300, 2, 6), 2, 44100)
    out["adpcm_ok.wem"] = adp
    out["adpcm_align.wem"] = adp[:32] + struct.pack("<H", 0x23) + adp[34:]
    fmt_off = vorbis.index(b"fmt ") + 8
    v = bytearray(vorbis)
    v[fmt_off + 0x18 + 0x28] = 5
    out["vorbis_blocks.wem"] = bytes(v)
    v = bytearray(vorbis)
    struct.pack_into("<I", v, fmt_off + 0x18 + 0x14, 0xFFFFFF)
    out["vorbis_offsets.wem"] = bytes(v)
    data_off = vorbis.index(b"data") + 8
    setup_at = struct.unpack_from("<I", vorbis, fmt_off + 0x18 + 0x10)[0]
    for i, junk in enumerate([b"\xff" * 8, b"\x00\x00", b"\x01\xff\xff\xff\xff\xff"]):
        v = bytearray(vorbis)
        v[data_off + setup_at + 2:data_off + setup_at + 2 + len(junk)] = junk
        out[f"vorbis_setup{i}.wem"] = bytes(v)
    first_at = struct.unpack_from("<I", vorbis, fmt_off + 0x18 + 0x14)[0]
    v = bytearray(vorbis[:data_off + first_at])
    struct.pack_into("<I", v, data_off - 4, first_at)
    out["vorbis_nopackets.wem"] = bytes(v)
    v = bytearray(vorbis)
    struct.pack_into("<I", v, fmt_off + 8, 0x7FFFFFFF)
    out["vorbis_avg_overflow.wem"] = bytes(v)
    return out


def convert_case(data):
    rec = {}
    try:
        info = wem_reader.parse(data)
        rec["info"] = [info.codec, info.channels, info.sample_rate, info.bits_per_sample, info.block_align,
                       info.avg_bytes_per_sec, info.sample_count, info.duration, info.data_offset, info.data_size]
    except wem_reader.WemError as exc:
        rec["parse_error"] = str(exc)
    try:
        b, mime, ext = wem_reader.convert(data)
        rec.update({"sha": sha(b), "len": len(b), "mime": mime, "ext": ext})
    except wem_reader.WemError as exc:
        rec["error"] = str(exc)
    except Exception as exc:  # noqa: BLE001
        rec["crash"] = type(exc).__name__
    return rec


SIDECAR = (
    "Event\tID\tName\t\t\tWwise Object Path\tNotes\r\n"
    "\t604671578\tPlay_ui_click\t\t\t\\Default Work Unit\\UI\\Play_ui_click\t\r\n"
    "\t686670768\tStop_ui_click\t\t\t\\Default Work Unit\\UI\\Stop_ui_click\tsome notes\r\n"
    "\t686670768\tStop_ui_click_dupe\t\t\t\\Default Work Unit\\UI\\x\t\r\n"
    "\t12x\tNotAnId\t\t\t\\x\t\r\n"
    "\r\n"
    "In Memory Audio\tID\tName\tAudio source file\t\tWwise Object Path\tNotes\tData Size\r\n"
    "\t{a}\tui_click_01\tC:\\Trove\\cache\\ui_click_01_54EC.wem\t\t\\Actor-Mixer Hierarchy\\Default Work Unit\\grp_UI\\ui_click\\ui_click_01\tfirst\t100\r\n"
    "\t{b}\tUI_Alpha\tC:/Trove/cache/alpha.wem\t\t\\Actor-Mixer Hierarchy\\grp\t\t5\r\n"
    "\t{c}\tzeta_thing\tplain.wem\t\tToplevel\t\t\r\n"
    "\t{d}\t\tempty_name.wem\t\t\\a\\b\\c\t\t\r\n"
    "\t{e}\tÉclair sound\tC:\\é.wem\t\t\\Actor-Mixer Hierarchy\\Ünïcode Group\\Éclair sound\tnote\u2028x\t1\r\n"
    "\n"
    "Streamed Audio\tID\tName\r\n"
    "\t1\tignored\r\n"
)


def build_bank(wems, sidecar_ids):
    """A BKHD/DIDX/DATA/HIRC/STID bank: Sound objects for most media, plus an
    action and an event, the layout Trove's banks use."""
    media = list(wems.items())
    didx, data = b"", b""
    for mid, blob in media:
        if len(data) % 16:
            data += b"\0" * (16 - len(data) % 16)
        didx += struct.pack("<III", mid, len(data), len(blob))
        data += blob
    objs = []
    for i, (mid, blob) in enumerate(media[:-1]):
        payload = struct.pack("<IIBII", 5000 + i, 0x00040001, 0, mid, len(blob) + i) + lcg_bytes(i, 20 + i * 3)
        objs.append(bytes([2]) + struct.pack("<I", len(payload)) + payload)
    objs.append(bytes([2]) + struct.pack("<I", len(struct.pack("<IIBII", 6000, 1, 0, media[0][0], 1)) + 2)
                + struct.pack("<IIBII", 6000, 1, 0, media[0][0], 1) + b"zz")
    act = struct.pack("<IHIBBBBI", 7000, 0x0403, 5000, 0, 0, 0, 4, 0xBA4C)
    objs.append(bytes([3]) + struct.pack("<I", len(act)) + act)
    ev = struct.pack("<IBI", 8000, 1, 7000)
    objs.append(bytes([4]) + struct.pack("<I", len(ev)) + ev)
    hirc = struct.pack("<I", len(objs)) + b"".join(objs)
    sec = lambda t, p: t + struct.pack("<I", len(p)) + p  # noqa: E731
    return (sec(b"BKHD", struct.pack("<IIII", 128, 0xBA4C, 0, 0)) + sec(b"DIDX", didx) + sec(b"DATA", data)
            + sec(b"HIRC", hirc) + sec(b"STID", b"\x01\0\0\0\x01\0\0\0\x04test"))


def audio_cases():
    real = real_wems()
    out = {"convert": {}}
    for name, data in real.items():
        out["convert"][name] = convert_case(data)
    broken = broken_wems(real["vorbis_ui.wem"])
    for name, data in broken.items():
        (OUT / name).write_bytes(data)
        out["convert"][name] = convert_case(data)
    ids = [11, 22, 33, 44, 55, 66, 77, 88]
    wems = dict(zip(ids, [real["vorbis_ui.wem"], real["adpcm_foley.wem"], real["pcm_foley.wem"],
                          broken["pcm_ok.wem"], broken["codec_bad.wem"], broken["adpcm_ok.wem"],
                          real["vorbis_mobs.wem"], broken["vorbis_blocks.wem"]]))
    bank = build_bank(wems, ids)
    sidecar = SIDECAR.format(a=22, b=33, c=44, d=55, e=66)
    (OUT / "synthetic.bnk").write_bytes(bank)
    (OUT / "synthetic.txt").write_bytes(sidecar.encode("utf-8"))
    audio_service._store = ContentStore(TMP / "store-bank")
    out["manifest"] = audio_service._build_sync(bank, sidecar)
    out["manifest_nosidecar"] = audio_service._build_sync(bank, None)
    out["names"] = {str(k): [v.name, v.path, v.group, v.source, v.notes]
                    for k, v in audio_names.parse(sidecar)[0].items()}
    out["events"] = len(audio_names.parse(sidecar)[1])
    out["event_ids"] = {n: audio_names.event_id(n) for n in ["Play_ui_button_select", "ÉVENT_x", "abc"]}
    out["titles"] = {t: audio_studio.safe_title(t) for t in
                     ["", "  My  Pack!! ", "a/b\\c:d*e", "x" * 100, "Ünïcode — title", " (1) & 'x', y ",
                      "tab\there", "\u3000spaced\u3000"]}
    bad_banks = {
        "empty": b"", "not_bkhd": b"DIDX\0\0\0\0", "short_bkhd": b"BKHD\x04\0\0\0\x80\0\0\0",
        "version": b"BKHD\x08\0\0\0\x7f\0\0\0\0\0\0\0", "overrun": b"BKHD\xff\0\0\0",
        "didx_odd": b"BKHD\x08\0\0\0\x80\0\0\0\0\0\0\0DIDX\x05\0\0\0abcde",
        "no_data": b"BKHD\x08\0\0\0\x80\0\0\0\0\0\0\0DIDX\x0c\0\0\0" + b"\0" * 12,
        "hirc_small": b"BKHD\x08\0\0\0\x80\0\0\0\0\0\0\0HIRC\x02\0\0\0ab",
        "hirc_mid": b"BKHD\x08\0\0\0\x80\0\0\0\0\0\0\0HIRC\x06\0\0\0\x05\0\0\0\x02\0",
        "nonascii": b"BKHD\x08\0\0\0\x80\0\0\0\0\0\0\0\xffXYZ\0\0\0\0",
    }
    out["bad_banks"] = {}
    for name, raw in bad_banks.items():
        try:
            bank_reader.parse(raw)
            out["bad_banks"][name] = None
        except bank_reader.BankError as exc:
            out["bad_banks"][name] = str(exc)
    enc = {}
    for name, (frames, ch, rate, codec) in {"mono_adpcm": (1000, 1, 24000, "adpcm"),
                                            "stereo_adpcm": (130, 2, 48000, "adpcm"),
                                            "mono_pcm": (77, 1, 11025, "pcm"),
                                            "stereo_pcm": (64, 2, 96000, "pcm")}.items():
        enc[name] = sha(audio_encode.encode(pcm_samples(frames, ch, frames), ch, rate, codec))
    enc["silence"] = sha(audio_encode.silence())
    out["encode"] = enc
    return out


# --- routes (raw ASGI through the real app) ----------------------------------------

async def asgi(target, method, url, headers, body=b""):
    path, _, qs = url.partition("?")
    scope = {
        "type": "http", "asgi": {"version": "3.0"}, "http_version": "1.1", "method": method,
        "scheme": "http", "path": urllib.parse.unquote(path), "raw_path": path.encode(),
        "query_string": qs.encode(), "root_path": "",
        "headers": [(b"host", b"testserver")] + headers + [(b"content-length", str(len(body)).encode())],
        "client": ("127.0.0.1", 50000), "server": ("testserver", 80),
    }
    done = asyncio.Event()
    pending = [{"type": "http.request", "body": body, "more_body": False}]
    sent = []

    async def receive():
        if pending:
            return pending.pop(0)
        await done.wait()
        return {"type": "http.disconnect"}

    async def send(message):
        sent.append(message)
        if message["type"] == "http.response.body" and not message.get("more_body"):
            done.set()

    await target(scope, receive, send)
    start = next(m for m in sent if m["type"] == "http.response.start")
    raw = b"".join(m.get("body", b"") for m in sent if m["type"] == "http.response.body")
    return start["status"], [(k.lower().decode(), v.decode("latin-1")) for k, v in start["headers"]], raw


KEPT = ("content-type", "cache-control", "etag", "content-disposition", "x-kiwi-sounds-replaced",
        "x-kiwi-sounds-added")


def multipart(fields):
    boundary = "kiwiBoundary7MA4YWxkTrZu0gW"
    out = b""
    for name, filename, data in fields:
        disp = f'form-data; name="{name}"' + (f'; filename="{filename}"' if filename is not None else "")
        out += f"--{boundary}\r\nContent-Disposition: {disp}\r\n".encode()
        if filename is not None:
            out += b"Content-Type: application/octet-stream\r\n"
        out += b"\r\n" + data + b"\r\n"
    out += f"--{boundary}--\r\n".encode()
    return f"multipart/form-data; boundary={boundary}", out


def route_cases():
    import app.site.router as site_router
    from app.main import app
    from app.trove.render import bp_cache

    upd = ContentStore(TMP / "updates")
    files = {}
    blobs = {}

    def put(branch, path, data, file=None):
        s = upd.put(data)[0]
        files[(branch, path)] = s
        blobs[s] = {"file": file} if file else {"b64": b64(data)}

    put("live-us", "ui/synthetic.swf", (OUT / "synthetic_fws.swf").read_bytes(), "synthetic_fws.swf")
    put("live-us", "ui/PlayerHealth.SWF", (OUT / "playerhealth.swf").read_bytes(), "playerhealth.swf")
    put("live-us", "ui/broken.swf", b"XYZ not a movie")
    put("pts", "ui/empty.swf", movie([]))
    put("live-us", "audio/synthetic.bnk", (OUT / "synthetic.bnk").read_bytes(), "synthetic.bnk")
    put("live-us", "audio/synthetic.txt", (OUT / "synthetic.txt").read_bytes(), "synthetic.txt")
    put("live-us", "audio/nosidecar.bnk", (OUT / "synthetic.bnk").read_bytes(), "synthetic.bnk")
    put("live-us", "audio/broken.bnk", b"BKHD\x08\0\0\0\x7f\0\0\0\0\0\0\0")
    put("live-us", "audio/silent.bnk", b"BKHD\x08\0\0\0\x80\0\0\0\0\0\0\0")
    files[("live-us", "ui/missing.swf")] = "f" * 64
    files[("live-us", "audio/missing.bnk")] = "e" * 64

    async def meta(branch, path):
        s = files.get((branch, path))
        return None if s is None else {"content_sha256": s}

    site_router.updates_read.get_file_meta = meta
    mods = ContentStore(TMP / "routes-mods")
    swf_service._store = mods
    audio_service._store = mods
    bp_cache._store = mods

    reqs = [
        ("swf_list", "GET", "/site/updates/live-us/file/swf?path=ui/synthetic.swf", []),
        ("swf_list_real", "GET", "/site/updates/live-us/file/swf?path=ui/PlayerHealth.SWF", []),
        ("swf_list_pts_empty", "GET", "/site/updates/pts/file/swf?path=ui/empty.swf", []),
        ("swf_branch", "GET", "/site/updates/nope/file/swf?path=ui/synthetic.swf", []),
        ("swf_not_swf", "GET", "/site/updates/live-us/file/swf?path=ui/x.png", []),
        ("swf_no_file", "GET", "/site/updates/live-us/file/swf?path=ui/none.swf", []),
        ("swf_blob_missing", "GET", "/site/updates/live-us/file/swf?path=ui/missing.swf", []),
        ("swf_broken", "GET", "/site/updates/live-us/file/swf?path=ui/broken.swf", []),
        ("swf_no_path", "GET", "/site/updates/live-us/file/swf", []),
        ("asset", "GET", "/site/updates/live-us/file/swf/asset?path=ui/synthetic.swf&id=3", []),
        ("asset_thumb", "GET", "/site/updates/live-us/file/swf/asset?path=ui/synthetic.swf&id=3&thumb=1", []),
        ("asset_thumb_none", "GET", "/site/updates/live-us/file/swf/asset?path=ui/synthetic.swf&id=11&thumb=true", []),
        ("asset_jpeg", "GET", "/site/updates/live-us/file/swf/asset?path=ui/PlayerHealth.SWF&id=1", []),
        ("asset_unknown", "GET", "/site/updates/live-us/file/swf/asset?path=ui/synthetic.swf&id=999", []),
        ("asset_bad_id", "GET", "/site/updates/live-us/file/swf/asset?path=ui/synthetic.swf&id=70000&thumb=maybe", []),
        ("asset_no_args", "GET", "/site/updates/live-us/file/swf/asset", []),
        ("swf_zip_empty", "GET", "/site/updates/pts/file/swf/zip?path=ui/empty.swf", []),
        ("bnk_list", "GET", "/site/updates/live-us/file/bnk?path=audio/synthetic.bnk", []),
        ("bnk_list_nosidecar", "GET", "/site/updates/live-us/file/bnk?path=audio/nosidecar.bnk", []),
        ("bnk_list_silent", "GET", "/site/updates/live-us/file/bnk?path=audio/silent.bnk", []),
        ("bnk_broken", "GET", "/site/updates/live-us/file/bnk?path=audio/broken.bnk", []),
        ("bnk_not_bnk", "GET", "/site/updates/live-us/file/bnk?path=audio/x.wem", []),
        ("bnk_no_file", "GET", "/site/updates/live-us/file/bnk?path=audio/none.bnk", []),
        ("bnk_blob_missing", "GET", "/site/updates/live-us/file/bnk?path=audio/missing.bnk", []),
        ("audio_vorbis", "GET", "/site/updates/live-us/file/bnk/audio?path=audio/synthetic.bnk&id=11", []),
        ("audio_adpcm", "GET", "/site/updates/live-us/file/bnk/audio?path=audio/synthetic.bnk&id=22", []),
        ("audio_raw", "GET", "/site/updates/live-us/file/bnk/audio?path=audio/synthetic.bnk&id=33&raw=1", []),
        ("audio_error", "GET", "/site/updates/live-us/file/bnk/audio?path=audio/synthetic.bnk&id=55", []),
        ("audio_decode_fail", "GET", "/site/updates/live-us/file/bnk/audio?path=audio/synthetic.bnk&id=88", []),
        ("audio_unknown", "GET", "/site/updates/live-us/file/bnk/audio?path=audio/synthetic.bnk&id=12", []),
        ("audio_bad_id", "GET", "/site/updates/live-us/file/bnk/audio?path=audio/synthetic.bnk&id=-1", []),
        ("audio_unnamed", "GET", "/site/updates/live-us/file/bnk/audio?path=audio/nosidecar.bnk&id=77", []),
        ("bnk_zip_silent", "GET", "/site/updates/live-us/file/bnk/zip?path=audio/silent.bnk", []),
    ]
    out = []
    for name, method, url, hdrs in reqs:
        status, rh, body = asyncio.run(asgi(app, method, url, hdrs))
        kept = [[k, v] for k, v in rh if k in KEPT]
        rec = {"name": name, "method": method, "url": url, "status": status, "headers": kept}
        ctype = dict(rh).get("content-type", "")
        if ctype.startswith("application/json"):
            rec["json"] = json.loads(body)
        else:
            rec["sha"] = sha(body)
            rec["len"] = len(body)
        out.append(rec)
        if name == "asset":
            etag = dict(rh)["etag"]
            status, rh, body = asyncio.run(asgi(app, method, url, [(b"if-none-match", etag.encode())]))
            out.append({"name": "asset_304", "method": method, "url": url, "if_none_match": etag,
                        "status": status, "headers": [[k, v] for k, v in rh if k in KEPT], "len": len(body)})
    studio = []
    clip = pcm_samples(500, 1, 9)
    stereo = pcm_samples(300, 2, 10)
    wem_ok = (OUT / "pcm_foley.wem").read_bytes()
    specs = [
        ("mute_bnk", {"branch": "live-us", "path": "audio/synthetic.bnk", "edits": [{"kind": "mute", "id": 22}]}, []),
        ("replace_adpcm", {"branch": "live-us", "path": "audio/synthetic.bnk",
                           "edits": [{"kind": "replace", "id": "33", "clip": "a", "channels": 1, "rate": 22050}]},
         [("clips", "a", clip)]),
        ("replace_pcm_stereo", {"branch": "live-us", "path": "audio/synthetic.bnk", "codec": "pcm",
                                "edits": [{"kind": "replace", "id": 11.9, "clip": "s", "channels": "2",
                                           "rate": 44100}]}, [("clips", "s", stereo)]),
        ("replace_wem", {"branch": "live-us", "path": "audio/synthetic.bnk",
                         "edits": [{"kind": "replace", "id": 44, "clip": "w", "wem": True}]}, [("clips", "w", wem_ok)]),
        ("add_tmod", {"branch": "live-us", "path": "audio/synthetic.bnk", "output": "tmod",
                      "mod": {"title": "  My: Pack ✓  ", "author": "Kiwi" * 30, "notes": "n" * 600},
                      "edits": [{"kind": "add", "event": "  Play_new_sound ", "clip": "a", "rate": 32000},
                                {"kind": "add", "event": "Play_cloned", "clip": "a", "rate": 8000,
                                 "clone_from": 22},
                                {"kind": "mute", "id": 66}, {"kind": "mute", "id": 66}]},
         [("clips", "a", clip), ("clips", "", b"ignored")]),
        ("add_dupe_event", {"branch": "live-us", "path": "audio/synthetic.bnk",
                            "edits": [{"kind": "add", "event": "Play_x1", "clip": "a", "rate": 8000},
                                      {"kind": "add", "event": "play_X1", "clip": "a", "rate": 8000}]},
         [("clips", "a", clip)]),
        ("unknown_sound", {"branch": "live-us", "path": "audio/synthetic.bnk",
                           "edits": [{"kind": "mute", "id": 1e20}]}, []),
        ("unknown_sound_big", {"branch": "live-us", "path": "audio/synthetic.bnk",
                               "edits": [{"kind": "mute", "id": "  1_000_000_000_000_000_000_000 "}]}, []),
        ("missing_id", {"branch": "live-us", "path": "audio/synthetic.bnk", "edits": [{"kind": "mute"}]}, []),
        ("missing_id_after_media", {"branch": "live-us", "path": "audio/synthetic.bnk",
                                    "edits": [{"kind": "replace", "clip": "zzz"}]}, []),
        ("bad_kind", {"branch": "live-us", "path": "audio/synthetic.bnk", "edits": [{"kind": 5}]}, []),
        ("not_a_dict_item", {"branch": "live-us", "path": "audio/synthetic.bnk", "edits": ["mute"]}, []),
        ("no_edits", {"branch": "live-us", "path": "audio/synthetic.bnk", "edits": []}, []),
        ("edits_not_list", {"branch": "live-us", "path": "audio/synthetic.bnk", "edits": {"a": 1}}, []),
        ("too_many", {"branch": "live-us", "path": "audio/synthetic.bnk", "edits": [{"kind": "mute", "id": 22}] * 201}, []),
        ("bad_codec", {"branch": "live-us", "path": "audio/synthetic.bnk", "codec": "vorbis",
                       "edits": [{"kind": "mute", "id": 22}]}, []),
        ("bad_channels", {"branch": "live-us", "path": "audio/synthetic.bnk",
                          "edits": [{"kind": "replace", "id": 22, "clip": "a", "channels": 3, "rate": 8000}]},
         [("clips", "a", clip)]),
        ("bad_rate", {"branch": "live-us", "path": "audio/synthetic.bnk",
                      "edits": [{"kind": "replace", "id": 22, "clip": "a", "rate": "fast"}]}, [("clips", "a", clip)]),
        ("low_rate", {"branch": "live-us", "path": "audio/synthetic.bnk",
                      "edits": [{"kind": "replace", "id": 22, "clip": "a", "rate": 999}]}, [("clips", "a", clip)]),
        ("empty_clip", {"branch": "live-us", "path": "audio/synthetic.bnk",
                        "edits": [{"kind": "replace", "id": 22, "clip": "t", "channels": 2, "rate": 8000}]},
         [("clips", "t", b"\x01\x02")]),
        ("bad_wem", {"branch": "live-us", "path": "audio/synthetic.bnk",
                     "edits": [{"kind": "replace", "id": 22, "clip": "w", "wem": 1}]}, [("clips", "w", b"RIFFnope")]),
        ("bad_event", {"branch": "live-us", "path": "audio/synthetic.bnk",
                       "edits": [{"kind": "add", "event": "no spaces!", "clip": "a", "rate": 8000}]},
         [("clips", "a", clip)]),
        ("nan_id", {"branch": "live-us", "path": "audio/synthetic.bnk", "edits": [{"kind": "mute", "id": "NaN"}]}, []),
        ("bad_branch", {"branch": ["x", 1, None, 2.5], "path": "audio/synthetic.bnk", "edits": []}, []),
        ("bad_path", {"branch": "live-us", "path": "audio/synthetic.wem", "edits": []}, []),
        ("no_file", {"branch": "live-us", "path": "audio/none.bnk", "edits": []}, []),
        ("blob_missing", {"branch": "live-us", "path": "audio/missing.bnk", "edits": []}, []),
        ("broken_bank", {"branch": "pts", "path": "audio/broken.bnk", "edits": [{"kind": "mute", "id": 1}]}, []),
        ("list_spec", [1, 2], []),
        ("too_long_path_tmod", None, []),
        ("mod_not_dict", {"branch": "live-us", "path": "audio/synthetic.bnk", "output": "tmod", "mod": "x",
                          "edits": [{"kind": "mute", "id": 22}]}, []),
        ("clone_bad", {"branch": "live-us", "path": "audio/synthetic.bnk",
                       "edits": [{"kind": "add", "event": "Play_cb", "clip": "a", "rate": 8000,
                                  "clone_from": "abc"}]}, [("clips", "a", clip)]),
    ]
    long_path = "audio/" + "d" * 260 + ".bnk"
    put("live-us", long_path, (OUT / "synthetic.bnk").read_bytes(), "synthetic.bnk")
    put("pts", "audio/broken.bnk", b"BKHD\x08\0\0\0\x80\0\0\0\0\0\0\0DIDX\x0c\0\0\0" + b"\0" * 12)
    raw_specs = [
        ("bad_json", "{nope", []),
        ("bom_json", "\ufeff{}", []),
        ("nan_json", '{"branch": NaN, "path": "a.bnk"}', []),
        ("deep_json", "[" * 12000 + "]" * 12000, []),
        ("deep_ok_json", "[" * 1500 + "]" * 1500, []),
    ]
    for name, spec, clips in specs:
        if name == "too_long_path_tmod":
            spec = {"branch": "live-us", "path": long_path, "output": "tmod", "edits": [{"kind": "mute", "id": 22}]}
        raw_specs.append((name, json.dumps(spec), clips))
    for name, spec, clips in raw_specs:
        ctype, body = multipart([("spec", None, spec.encode())] + clips)
        status, rh, rbody = asyncio.run(asgi(app, "POST", "/site/sound-studio/build",
                                             [(b"content-type", ctype.encode())], body))
        rec = {"name": name, "spec": spec, "clips": [[n, f, b64(d)] for n, f, d in clips], "status": status,
               "headers": [[k, v] for k, v in rh if k in KEPT]}
        if dict(rh).get("content-type", "").startswith("application/json"):
            rec["json"] = json.loads(rbody)
        else:
            rec["sha"] = sha(rbody)
            rec["len"] = len(rbody)
        studio.append(rec)
    forms = [
        ("form_no_spec", [("clips", "a", b"x")]),
        ("form_empty_spec", [("spec", None, b"")]),
        ("form_spec_file", [("spec", "s.json", b"{}")]),
        ("form_clip_text", [("spec", None, b"{}"), ("clips", None, b"text")]),
    ]
    for name, fields in forms:
        ctype, body = multipart(fields)
        status, rh, rbody = asyncio.run(asgi(app, "POST", "/site/sound-studio/build",
                                             [(b"content-type", ctype.encode())], body))
        studio.append({"name": name, "form": [[n, f, b64(d)] for n, f, d in fields], "status": status,
                       "headers": [[k, v] for k, v in rh if k in KEPT], "json": json.loads(rbody)})
    return out, studio, {"files": {f"{b}:{p}": s for (b, p), s in files.items()}, "blobs": blobs}


def cache_rows():
    """The blueprint_cache documents Beanie inserts (key order, value types)."""
    from beanie import init_beanie
    from beanie.odm.utils.dump import get_dict
    from bson import BSON
    from pymongo import AsyncMongoClient

    from app.trove.render.models import BlueprintCacheEntry

    async def run():
        client = AsyncMongoClient("mongodb://127.0.0.1:1", connect=False, serverSelectionTimeoutMS=200)
        await init_beanie(database=client["kiwi"], skip_indexes=True, document_models=[BlueprintCacheEntry])
        ok = BlueprintCacheEntry(key="s3:swf:abc", blob_sha="f" * 64, byte_len=123, voxel_count=7, size=None)
        err = BlueprintCacheEntry(key="b1:bnk:abc:-", err_status=422, err_code="bad_request", err_msg="Nope.")
        out = {}
        for name, e in [("ok", ok), ("err", err)]:
            d = get_dict(e, to_db=True, keep_nulls=e.get_settings().keep_nulls)
            d.pop("created_at")
            out[name] = {"keys": list(d), "bson": b64(BSON.encode(d))}
        return out

    return asyncio.run(run())


FFDEC_JAR = r"C:\Program Files (x86)\FFDec\ffdec-cli.jar"


def decompile_cases():
    """FFDec over a few movies, when this box has the jar and a JVM."""
    from app.core.config import settings
    from app.trove.swf import decompile

    settings.ffdec_jar = FFDEC_JAR
    if not decompile.available():
        return None
    out = {"jar": FFDEC_JAR, "cases": []}
    for name in ["playerhealth.swf", "synthetic_fws.swf", "pvpplayercounterui.swf"]:
        rec = {"file": name}
        try:
            r = decompile.decompile_scripts((OUT / name).read_bytes())
            rec["result"] = {"count": r["count"], "truncated": r["truncated"], "decompiler": r["decompiler"],
                             "scripts": [[s["path"], s["size"], sha(s["source"].encode())] for s in r["scripts"]]}
        except decompile.DecompileError as exc:
            rec["error"] = [type(exc).__name__, str(exc)]
        out["cases"].append(rec)
    for label, raw in [("not_swf", b"XYZ\x0a\0\0\0\0"), ("truncated", b"FWS\x0a\x10\0\0\0")]:
        try:
            decompile.decompile_scripts(raw)
        except decompile.DecompileError as exc:
            out["cases"].append({"label": label, "raw": b64(raw), "error": [type(exc).__name__, str(exc)]})
    old = settings.ffdec_max_scripts
    settings.ffdec_max_scripts = 1
    r = decompile.decompile_scripts((OUT / "playerhealth.swf").read_bytes())
    settings.ffdec_max_scripts = old
    out["capped"] = {"count": r["count"], "truncated": r["truncated"]}
    return out


def main():
    swf, headers = swf_cases()
    audio = audio_cases()
    golden = {
        "pil": {"tables": pil_tables(), "thumbs": pil_thumbs(), "jpeg": jpeg_cases(), "payloads": png_payloads()},
        "swf": swf,
        "swf_headers": headers,
        "audio": audio,
        "zips": zip_case(),
        "decompile": decompile_cases(),
        "cache_rows": cache_rows(),
    }
    routes, studio, files = route_cases()
    golden["routes"] = routes
    golden["studio"] = studio
    golden["archive"] = files
    (OUT / "golden.json").write_text(json.dumps(golden, indent=1, ensure_ascii=False) + "\n", encoding="utf-8",
                                     newline="\n")
    shutil.rmtree(TMP, ignore_errors=True)
    print("wrote", OUT)


if __name__ == "__main__":
    main()
