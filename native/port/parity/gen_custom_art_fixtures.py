"""Golden fixtures for custom art (app/custom_art), from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_custom_art_fixtures.py
Writes native/kiwi-server/testdata/custom_art/golden.json and the multipart
bodies native/port/parity/data/custom_art/*.bin the write specs send (the
twin runs with CUSTOM_ART_IMAGE_MAX_BYTES=1024, so over_limit.bin stays small).

Runs the real app (raw ASGI) for every reply settled before a store is touched
(form parsing, the 422s, the form settings), the real service functions for
the checks before the captcha, the inserted document, the views, changelogs
and emails, Pillow's Image.open over generated and broken pictures, and the
release worker's pure helpers.
"""
import asyncio
import base64
import datetime as dt
import io
import json
import logging
import os
import struct
import subprocess
import sys
import tempfile
import types
import urllib.parse
import warnings
import zlib
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
sys.path.insert(0, os.getcwd())

ENV = {
    "SECRET_KEY": "custom-art-fixture-secret",
    "APP_URL": "https://trove.example.test//",
    "CAPTCHA_PROVIDER": "hcaptcha",
    "CAPTCHA_SITEKEY": "parity-sitekey",
    "CUSTOM_ART_IMAGE_MAX_BYTES": "3145728",
}
os.environ.update(ENV)

from bson import ObjectId  # noqa: E402
from bson.json_util import CANONICAL_JSON_OPTIONS, dumps as bson_dumps  # noqa: E402
from PIL import Image  # noqa: E402
from pydantic import TypeAdapter  # noqa: E402
from pymongo.asynchronous.collection import AsyncCollection  # noqa: E402
from pymongo.asynchronous.database import AsyncDatabase  # noqa: E402

from app.admin import runtime_config as rc  # noqa: E402

for _key, _spec in rc.REGISTRY.items():
    rc._CACHE[_key] = (_spec.default, float("inf"))

import app.custom_art.builder as builder  # noqa: E402
import app.custom_art.service as svc  # noqa: E402
from app.core.config import settings  # noqa: E402
from app.core.errors import APIError  # noqa: E402
from app.custom_art.models import ArtRequest  # noqa: E402
from app.main import app  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "kiwi-server" / "testdata" / "custom_art" / "golden.json"
BODIES = Path(__file__).resolve().parent / "data" / "custom_art"
KEPT = ("content-type", "content-length", "cache-control")


def ext(v):
    return json.loads(bson_dumps(v, json_options=CANONICAL_JSON_OPTIONS))


def bson_round(doc):
    import bson
    from bson.codec_options import CodecOptions
    return bson.decode(bson.encode(doc), codec_options=CodecOptions(tz_aware=True))


async def _cmd(self, *a, **k):
    return {"version": "7.0.0"}


async def _lcn(self, *a, **k):
    return []


AsyncDatabase.command = _cmd
AsyncDatabase.list_collection_names = _lcn
INSERTED = []


async def _insert_one(self, doc, *a, **kw):
    INSERTED.append(dict(doc))
    return types.SimpleNamespace(inserted_id=ObjectId("6700000000000000000000aa"))


AsyncCollection.insert_one = _insert_one


async def _init():
    from beanie import init_beanie
    from pymongo import AsyncMongoClient
    client = AsyncMongoClient("mongodb://127.0.0.1:1", connect=False, serverSelectionTimeoutMS=200, tz_aware=True)
    await init_beanie(database=client["kiwi"], skip_indexes=True, document_models=[ArtRequest])


LOOP = asyncio.new_event_loop()
LOOP.run_until_complete(_init())


def run(coro):
    return LOOP.run_until_complete(coro)


async def asgi(method, url, headers, body=b""):
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

    await app(scope, receive, send)
    start = next(m for m in sent if m["type"] == "http.response.start")
    raw = b"".join(m.get("body", b"") for m in sent if m["type"] == "http.response.body")
    return start["status"], [(k.lower().decode(), v.decode("latin-1")) for k, v in start["headers"]], raw


# --- pictures ----------------------------------------------------------------------


def chunk(cid: bytes, data: bytes, crc=None) -> bytes:
    c = zlib.crc32(cid + data) & 0xFFFFFFFF if crc is None else crc
    return struct.pack(">I", len(data)) + cid + data + struct.pack(">I", c)


SIG = b"\x89PNG\r\n\x1a\n"


def ihdr(w, h, depth=8, color=6, filt=0, interlace=0):
    return chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, depth, color, 0, filt, interlace))


IDAT = chunk(b"IDAT", zlib.compress(b"\x00" * 20))
IEND = chunk(b"IEND", b"")


def pil(img, fmt, **kw):
    buf = io.BytesIO()
    img.save(buf, fmt, **kw)
    return buf.getvalue()


def gif_raw(screen, frames, gct=True, trailer=True, extra=b""):
    out = b"GIF89a" + struct.pack("<HH", *screen) + bytes([0xF0 if gct else 0x00, 0, 0])
    if gct:
        out += bytes(range(6))
    out += extra
    for (x, y, w, h) in frames:
        out += b"," + struct.pack("<HHHHB", x, y, w, h, 0) + b"\x02\x02\x44\x01\x00"
    return out + (b";" if trailer else b"")


def pictures():
    rgba = Image.new("RGBA", (4, 4), (10, 20, 30, 128))
    rgb = Image.new("RGB", (8, 3), (200, 100, 50))
    pal = Image.new("P", (5, 5), 1)
    pal.info["transparency"] = 0
    anim = [Image.new("RGB", (6, 2), c) for c in ((255, 0, 0), (0, 255, 0))]
    big_text = zlib.compress(b"x" * (1024 * 1024 + 5))
    zi = zlib.compress(b"hello")
    apng_ok = SIG + ihdr(10, 10) + chunk(b"acTL", struct.pack(">II", 1, 0)) + \
        chunk(b"fcTL", struct.pack(">IIIIIHHBB", 0, 10, 10, 0, 0, 1, 10, 0, 0)) + IDAT + IEND
    apng_bad = SIG + ihdr(10, 10) + chunk(b"acTL", struct.pack(">II", 1, 0)) + \
        chunk(b"fcTL", struct.pack(">IIIIIHHBB", 0, 11, 10, 0, 0, 1, 10, 0, 0)) + IDAT + IEND
    apng_seq = SIG + ihdr(10, 10) + chunk(b"fcTL", struct.pack(">IIIIIHHBB", 3, 5, 5, 0, 0, 1, 10, 0, 0)) + IDAT
    cases = {
        "png_rgba": pil(rgba, "PNG"),
        "png_rgb": pil(rgb, "PNG"),
        "png_palette_trns": pil(pal, "PNG"),
        "png_16bit": pil(Image.new("I;16", (3, 3)), "PNG"),
        "png_text_first": SIG + chunk(b"tEXt", b"Title\x00abc") + ihdr(7, 9) + IDAT + IEND,
        "png_ztxt_itxt": SIG + ihdr(3, 3) + chunk(b"zTXt", b"k\x00\x00" + zi) +
        chunk(b"iTXt", b"k\x00\x01\x00en\x00t\x00" + zi) + IDAT + IEND,
        "png_ztxt_bad_method": SIG + ihdr(3, 3) + chunk(b"zTXt", b"k\x00\x01" + zi) + IDAT,
        "png_ztxt_too_large": SIG + ihdr(3, 3) + chunk(b"zTXt", b"k\x00\x00" + big_text) + IDAT,
        "png_ztxt_corrupt": SIG + ihdr(3, 3) + chunk(b"zTXt", b"k\x00\x00garbage") + IDAT,
        "png_itxt_too_large": SIG + ihdr(3, 3) + chunk(b"iTXt", b"k\x00\x01\x00en\x00t\x00" + big_text) + IDAT,
        "png_iccp_bad_method": SIG + ihdr(3, 3) + chunk(b"iCCP", b"icc\x00\x01" + zi) + IDAT,
        "png_iccp_corrupt": SIG + ihdr(3, 3) + chunk(b"iCCP", b"icc\x00\x00junk") + IDAT,
        "png_iccp_no_nul": SIG + ihdr(3, 3) + chunk(b"iCCP", b"\x00") + IDAT,
        "png_bad_ihdr_crc": SIG + chunk(b"IHDR", struct.pack(">IIBBBBB", 4, 4, 8, 6, 0, 0, 0), crc=1) + IDAT,
        "png_bad_ancillary_crc": SIG + ihdr(4, 4) + chunk(b"gAMA", b"\x00\x00\xb1\x8f", crc=7) + IDAT,
        "png_truncated": SIG + ihdr(4, 4),
        "png_truncated_crc": SIG + ihdr(4, 4)[:-2],
        "png_filter": SIG + ihdr(4, 4, filt=1) + IDAT,
        "png_bad_mode": SIG + ihdr(4, 4, depth=3, color=2) + IDAT,
        "png_ihdr_short": SIG + chunk(b"IHDR", b"\x00" * 12) + IDAT,
        "png_zero_width": SIG + ihdr(0, 4) + IDAT,
        "png_bomb": SIG + ihdr(20000, 20000) + IDAT,
        "png_bad_cid": SIG + chunk(b"IH-R", b"\x00" * 13) + IDAT,
        "png_gama_short": SIG + ihdr(4, 4) + chunk(b"gAMA", b"\x00\x01") + IDAT,
        "png_trns_short_rgb": SIG + ihdr(4, 4, color=2) + chunk(b"tRNS", b"\x00\x01") + IDAT,
        "png_iend_first": SIG + ihdr(2, 2) + IEND,
        "png_no_ihdr": SIG + chunk(b"tEXt", b"a\x00b") + IDAT + b"\x00" * 8,
        "png_apng": apng_ok,
        "png_apng_bad_frame": apng_bad,
        "png_apng_bad_seq": apng_seq,
        "png_unknown_chunk": SIG + ihdr(6, 2) + chunk(b"zzZz", b"data") + IDAT,
        "png_unknown_truncated": SIG + ihdr(6, 2) + struct.pack(">I", 100) + b"zzZz" + b"short",
        "gif_pil": pil(Image.new("P", (5, 7), 3), "GIF"),
        "gif_anim": pil(anim[0], "GIF", save_all=True, append_images=anim[1:], duration=50, loop=0),
        "gif_grow": gif_raw((4, 4), [(2, 1, 6, 6)]),
        "gif_zero_screen": gif_raw((0, 0), [(0, 0, 3, 2)]),
        "gif_no_image": gif_raw((4, 4), []),
        "gif_trailer_only": b"GIF89a" + struct.pack("<HH", 4, 4) + b"\x00\x00\x00" + b";" + b"\x00" * 16,
        "gif_truncated_desc": gif_raw((4, 4), [], gct=False, trailer=False) + b",\x00\x00\x00\x00\x04" + b"\x00" * 4,
        "gif_ext": gif_raw((4, 4), [(0, 0, 4, 4)], extra=b"!\xf9\x04\x05\x0a\x00\x01\x00!\xfe\x03abc\x00" +
                           b"!\xff\x0bNETSCAPE2.0\x03\x01\x00\x00\x00"),
        "gif_gce_short": gif_raw((4, 4), [(0, 0, 4, 4)], extra=b"!\xf9\x01\x05\x00"),
        "gif_bad_palette": b"GIF89a" + struct.pack("<HH", 4, 4) + b"\x80\x00\x00" + b"\x00\x00",
        "gif_bomb": gif_raw((60000, 60000), [(0, 0, 1, 1)]),
        "gif_grow_bomb": gif_raw((1, 1), [(0, 0, 60000, 60000)]),
        "webp_lossy": pil(rgb, "WEBP", quality=80),
        "webp_lossless": pil(rgba, "WEBP", lossless=True),
        "webp_anim": pil(anim[0], "WEBP", save_all=True, append_images=anim[1:], duration=50),
        "webp_alpha_lossy": pil(rgba, "WEBP", quality=50),
        "webp_bad_mode": b"RIFF" + struct.pack("<I", 20) + b"WEBPVP8Z" + b"\x00" * 16,
        "jpeg_baseline": pil(rgb, "JPEG", quality=90),
        "jpeg_progressive": pil(rgb, "JPEG", progressive=True),
        "jpeg_gray": pil(Image.new("L", (9, 4), 77), "JPEG"),
        "jpeg_cmyk": pil(Image.new("CMYK", (3, 5), (1, 2, 3, 4)), "JPEG"),
        "jpeg_no_third_ff": b"\xff\xd8\x00" + b"\x00" * 30,
        "short": b"\x89PNG\r\n\x1a\n" + b"\x00" * 10,
        "text": b"this is not a picture at all, sorry",
    }
    lossy = cases["webp_lossy"]
    cases["webp_truncated"] = lossy[:-6]
    cases["webp_riff_short"] = lossy[:4] + struct.pack("<I", len(lossy)) + lossy[8:]
    jpg = cases["jpeg_baseline"]
    cases["jpeg_truncated_header"] = jpg[:jpg.index(b"\xff\xc0") + 4]
    cases["jpeg_no_sos"] = jpg[:jpg.index(b"\xff\xda")]
    return cases


def measure_cases(cases):
    out = []
    for name, data in cases.items():
        got = svc._measure(data)
        out.append({"name": name, "b64": base64.b64encode(data).decode(), "result": list(got) if got else None})
    return out


# --- pure helpers --------------------------------------------------------------------

NAMES = ["", "   ", "Ok Name", "  trim me  ", "a" * 64, "a" * 65, "name.", "name. ", "na,me", 'na"me', "tab\tname",
         "é" * 64, "é" * 65, "\u00a0nbsp\u00a0", "\u2028line\u2029", "x:y", "<x>", "a|b", "a/b", "a\\b", "a*b",
         "a?b", "dot.inside", "Kiwi ✓ 日本", "\x1cfs\x1f", "new\nline", "cr\rx"]

EMAILS = ["Foo@Example.COM", "info@x.io", "Support@Example.org", "bad", "a@b", "a@mailinator.com",
          "x@YOPMAIL.com", "Bob <bob@x.com>", " a@b.co ", "a@b.test", '"quoted"@x.com', "a@[1.2.3.4]",
          "é@x.com", "user@xn--bcher-kva.de", "a..b@x.com", ".a@x.com", "a@-x.com", "a@x.c0m", "a b@x.com",
          "a@x..com", "postmaster@Mail.Example.COM", "x" * 65 + "@x.com", "a@" + "b" * 64 + ".com",
          "first.last+tag@sub.domain.co.uk", "a@localhost", "\tpad@x.io\n"]

SHAPES = [("player", "pfp", 10, 10), ("player", "pfp", 10, 11), ("club", "pfp", 3, 2), ("club", "banner", 6, 1),
          ("club", "banner", 7, 1), ("club", "banner", 5, 5), ("club", "banner", 4, 5), ("club", "banner", 600, 100),
          ("club", "banner", 601, 100)]


def err(e: APIError):
    return {"status": e.status_code, "code": str(e.code.value if hasattr(e.code, "value") else e.code),
            "message": e.message}


def try_(fn, *a):
    try:
        return {"ok": fn(*a)}
    except APIError as e:
        return err(e)


def shapes():
    out = []
    for kind, slot, w, h in SHAPES:
        out.append([kind, slot, w, h, try_(svc.check_shape, kind, slot, w, h)])
    return out


# --- submit (up to the captcha, and the inserted document) ---------------------------

class Reached(Exception):
    pass


SUBMITS = [
    ("ok_player", "player", " Kiwi ", "Foo@Example.COM", "  hello  ", [("pfp", "png_rgba")], None),
    ("ok_club_both", "club", "Club ✓", "club@x.io", None, [("pfp", "png_palette_trns"), ("banner", "jpeg_baseline")], None),
    ("ok_club_banner_only", "club", "C", "c@x.io", "n" * 600, [("banner", "png_rgb")], None),
    ("ok_webp_gif", "club", "W", "w@x.io", "", [("pfp", "webp_lossless"), ("banner", "gif_grow")], None),
    ("bad_name", "player", "bad/name", "Foo@Example.COM", None, [("pfp", "png_rgba")], None),
    ("bad_email_before_files", "player", "ok", "nope", None, [], None),
    ("disposable", "player", "ok", "a@mailinator.com", None, [("pfp", "png_rgba")], None),
    ("no_files_player", "player", "ok", "a@x.io", None, [], None),
    ("no_files_club", "club", "ok", "a@x.io", None, [], None),
    ("empty_files", "club", "ok", "a@x.io", None, [("pfp", "empty"), ("banner", "empty")], None),
    ("player_banner", "player", "ok", "a@x.io", None, [("pfp", "png_rgba"), ("banner", "png_rgb")], None),
    ("player_banner_only", "player", "ok", "a@x.io", None, [("banner", "png_rgb")], None),
    ("over_limit", "club", "ok", "a@x.io", None, [("pfp", "png_rgba"), ("banner", "png_rgb")], 20),
    ("over_limit_mb", "player", "ok", "a@x.io", None, [("pfp", "jpeg_baseline")], 2 * 1024 * 1024 + 7),
    ("not_picture", "player", "ok", "a@x.io", None, [("pfp", "text")], None),
    ("broken_png", "club", "ok", "a@x.io", None, [("banner", "png_bad_ihdr_crc")], None),
    ("not_square", "player", "ok", "a@x.io", None, [("pfp", "png_rgb")], None),
    ("banner_tall", "club", "ok", "a@x.io", None, [("pfp", "png_rgba"), ("banner", "jpeg_cmyk")], None),
    ("measure_before_shape", "club", "ok", "a@x.io", None, [("pfp", "png_rgb"), ("banner", "text")], None),
]


def submits(cases):
    cases = dict(cases, empty=b"")
    out = []
    orig_limit = settings.custom_art_image_max_bytes
    for name, kind, nm, email, note, files, limit in SUBMITS:
        settings.custom_art_image_max_bytes = limit or orig_limit
        data = {slot: cases[key] for slot, key in files}

        async def stop(*a, **k):
            raise Reached

        svc.verify_captcha = stop
        try:
            run(svc.submit(kind=kind, name=nm, email=email, note=note, files=dict(data), captcha_token=None, ip=None))
            res = {"ok": None}
        except Reached:
            res = {"ok": "captcha"}
        except APIError as e:
            res = err(e)
        doc = None
        if res.get("ok") == "captcha":
            async def yes(*a, **k):
                return True

            async def put(data):
                import hashlib
                return hashlib.sha256(data).hexdigest(), True

            svc.verify_captcha = yes
            svc.store.put_blob = put
            INSERTED.clear()
            run(svc.submit(kind=kind, name=nm, email=email, note=note, files=dict(data), captcha_token="t", ip="1.2.3.4"))
            doc = ext(bson_round({"_id": ObjectId("6700000000000000000000aa"), **INSERTED[-1]}))
        out.append({"name": name, "kind": kind, "input_name": nm, "email": email, "note": note,
                    "files": [[slot, key] for slot, key in files], "limit": limit or orig_limit,
                    "result": res, "doc": doc})
    settings.custom_art_image_max_bytes = orig_limit
    return out


# --- views, changelogs, emails ---------------------------------------------------------


def req_doc(i, kind, name, slots, **kw):
    pics = {s: {"sha": f"{i:02x}" * 32, "content_type": "image/png", "width": w, "height": h} for s, (w, h) in slots}
    base = {"_id": ObjectId(f"6710000000000000000000{i:02x}"), "kind": kind, "name": name, "email": f"u{i}@x.io",
            "note": None, "pictures": pics, "status": "pending", "reason": None, "batch": None, "log": None,
            "versions": {}, "steam_pending": False,
            "created_at": dt.datetime(2026, 10, 20, 9, i // 60, i % 60, 123000, tzinfo=dt.timezone.utc),
            "decided_at": None, "released_at": None}
    base.update(kw)
    return base


DOCS = [
    req_doc(1, "player", "Kiwi", [("pfp", (64, 64))]),
    req_doc(2, "club", "Club <One> & \"Co\"", [("pfp", (32, 32)), ("banner", (120, 40))], note="hi\nthere",
            status="released", versions={svc.CHAT: "1.2.3", svc.CLUBS: "0.9", svc.NAMEPLATE: "4.0"},
            steam_pending=True, decided_at=dt.datetime(2026, 10, 21, 8, 0, tzinfo=dt.timezone.utc),
            released_at=dt.datetime(2026, 10, 22, 7, 30, 1, 5000, tzinfo=dt.timezone.utc), log="$ git\nok"),
    req_doc(3, "club", "Banner Only", [("banner", (300, 50))], status="denied", reason="Too blurry ✓"),
    req_doc(4, "club", "Pic Only", [("pfp", (10, 10))], status="failed", batch="abcdef012345",
            versions={svc.CHAT: "2.0"}),
    req_doc(5, "club", "Swapped", [("banner", (30, 10)), ("pfp", (5, 5))], status="approved"),
    req_doc(6, "player", "Odd 'quote'", [("pfp", (8, 8))], status="queued"),
]


def views():
    out = []
    for d in map(bson_round, DOCS):
        r = ArtRequest.model_validate(d)
        out.append({
            "doc": ext(d),
            "view": TypeAdapter(dict).dump_json(svc.view(r)).decode(),
            "lanes": svc.lanes(r), "mods": svc.mods_of(r),
            "what": {str(m): (svc.what(r, m) if any(m in svc.LANE_MODS[ln] for ln in svc.lanes(r)) else None)
                     for m in svc.MODS},
            "saved": ext(bson_round(__import__("beanie").odm.utils.encoder.Encoder(to_db=True, keep_nulls=True)
                                    .encode(r))),
        })
    return out


def changelogs():
    reqs = [ArtRequest.model_validate(bson_round(d)) for d in DOCS]
    many = [ArtRequest.model_validate(bson_round(req_doc(40 + i, "club" if i % 3 else "player", f"Name {i:02d}",
                                                         [("pfp", (5, 5))] + ([("banner", (10, 5))] if i % 2 else []))))
            for i in range(14)]
    sets = {"all": reqs, "first_two": reqs[:2], "one": reqs[:1], "many": many, "mixed": reqs + many[:4]}
    out = []
    for key, rs in sets.items():
        for m in svc.MODS:
            if any(m in svc.mods_of(r) for r in rs):
                out.append({"set": key, "mod": m, "docs": [ext(bson_round(r.model_dump(by_alias=True))) for r in rs],
                            "text": svc.changelog(rs, m)})
    return out


def mails():
    sent = []

    async def capture(to, subject, text, html):
        sent.append({"to": to, "subject": subject, "text": text, "html": html})

    svc.queue_email = capture
    out = []
    for d in map(bson_round, DOCS):
        r = ArtRequest.model_validate(d)
        sent.clear()
        if r.reason is None:
            r.reason = "  Needs a <clearer> picture & more \"contrast\"  ".strip()
        run(svc._notify_denied(r))
        denied = sent[-1]
        released = None
        if all(m in r.versions for m in svc.mods_of(r)):
            pages = {svc.CHAT: "https://trove.example.test/mods/aallyn/chat?x=1&y='2'", svc.CLUBS: ""}
            sent.clear()
            run(svc.notify_released(r, pages))
            released = {"pages": pages, "mail": sent[-1]}
        out.append({"doc": ext(bson_round(r.model_dump(by_alias=True))), "denied": denied, "released": released})
    return out


# --- HTTP: form parsing and validation, the form settings ------------------------------


def mp(parts, boundary="XyZ"):
    out = b""
    for name, value, filename, ctype in parts:
        out += b"--" + boundary.encode() + b"\r\n"
        disp = f'form-data; name="{name}"'
        if filename is not None:
            disp += f'; filename="{filename}"'
        out += b"Content-Disposition: " + disp.encode() + b"\r\n"
        if ctype:
            out += b"Content-Type: " + ctype.encode() + b"\r\n"
        out += b"\r\n" + (value if isinstance(value, bytes) else value.encode()) + b"\r\n"
    return out + b"--" + boundary.encode() + b"--\r\n"


MP = "multipart/form-data; boundary=XyZ"
FORM = "application/x-www-form-urlencoded"
BASE = [("kind", "player", None, None), ("name", "Kiwi", None, None), ("email", "a@x.io", None, None)]

HTTP = [
    ("no_body", None, b""),
    ("json_body", "application/json", b'{"kind": "player", "name": "x", "email": "a@x.io"}'),
    ("empty_strings", FORM, b"kind=&name=&email="),
    ("bad_kind", FORM, b"kind=team&name=x&email=a%40x.io"),
    ("kind_file", MP, mp([("kind", b"player", "k.txt", "text/plain"), ("name", "x", None, None),
                          ("email", "a@x.io", None, None)])),
    ("too_long", MP, mp([("kind", "club", None, None), ("name", "n" * 201, None, None),
                         ("email", "e" * 255, None, None), ("note", "é" * 501, None, None),
                         ("captcha_token", "t" * 4097, None, None)])),
    ("pfp_text", MP, mp(BASE + [("pfp", "not a file", None, None)])),
    ("banner_text_pfp_empty", MP, mp(BASE + [("pfp", "", None, None), ("banner", "x", None, None)])),
    ("name_file", MP, mp([("kind", "player", None, None), ("name", b"abc\ndef\n", "n.txt", None),
                          ("email", "a@x.io", None, None)])),
    ("email_file_note_file", MP, mp([("kind", "player", None, None), ("name", "x", None, None),
                                     ("email", b"\x00\x01", "e.bin", "application/octet-stream"),
                                     ("note", b"ab\ncd\n", "n.txt", "text/plain")])),
    ("garbage_multipart", MP, b"garbage"),
    ("no_boundary", "multipart/form-data", b"--x\r\n"),
    ("missing_email_only", MP, mp(BASE[:2])),
    ("repeated_last_wins", FORM, b"kind=player&kind=bad&name=x&email=a%40x.io"),
]


def http():
    out = []
    for i, (name, ctype, body) in enumerate(HTTP):
        rid = f"req_cart_{i:03d}"
        hdrs = [(b"x-request-id", rid.encode())]
        if ctype:
            hdrs.append((b"content-type", ctype.encode()))
        status, rh, raw = run(asgi("POST", "/site/custom-art", hdrs, body))
        if status not in (400, 422):
            raise SystemExit(f"{name}: unexpected {status} {raw!r}")
        out.append({"name": name, "content_type": ctype, "request_id": rid, "body": body.decode("latin-1"),
                    "status": status, "resp_headers": [[k, v] for k, v in rh if k in KEPT],
                    "resp": raw.decode("utf-8")})
    status, rh, raw = run(asgi("GET", "/site/custom-art/config", [(b"x-request-id", b"req_cart_cfg")]))
    config = {"status": status, "resp_headers": [[k, v] for k, v in rh if k in KEPT], "resp": raw.decode()}
    return out, config


ADMIN_BODIES = [
    ("approve", b"{}"), ("approve", b'{"name": null}'), ("approve", b'{"name": "' + b"n" * 201 + b'"}'),
    ("approve", b'{"name": 5}'), ("approve", b""), ("approve", b"[1]"), ("approve", b'{"name": "ok", "x": 1}'),
    ("deny", b"{}"), ("deny", b'{"reason": "no"}'), ("deny", b'{"reason": "' + b"r" * 2001 + b'"}'),
    ("deny", b'{"reason": null}'), ("deny", b'{"reason": "fine reason"}'), ("deny", b'{"reason": '),
]


def admin_bodies():
    from app.core.dependencies import get_current_superuser
    app.dependency_overrides[get_current_superuser] = lambda: types.SimpleNamespace(
        id=ObjectId("65a1b2c3d4e5f60718293a4b"), is_superuser=True, is_active=True)
    out = []
    for i, (route, body) in enumerate(ADMIN_BODIES):
        rid = f"req_cart_admin_{i:03d}"
        hdrs = [(b"x-request-id", rid.encode()), (b"content-type", b"application/json")]
        try:
            status, rh, raw = run(asgi("POST", f"/admin/custom-art/not-an-id/{route}", hdrs, body))
        except Exception as exc:  # noqa: BLE001
            raise SystemExit(f"{route} {body!r}: {exc!r}")
        out.append({"route": route, "body": body.decode(), "request_id": rid, "status": status, "resp": raw.decode()})
    app.dependency_overrides.clear()
    return out


# --- the release worker's helpers ------------------------------------------------------


def worker_helpers():
    reqs = [ArtRequest.model_validate(bson_round(req_doc(60 + i, "player", f"N{i}", [("pfp", (5, 5))])))
            for i in range(7)]
    texts = ['x = {"modVersion": "1.2.3"}', '"modVersion":\t\n "a b"', '"modVersion" : "1"', '"modVersion":"9"\n"modVersion":"8"',
             "nothing here", '"modVersion":\x1c"fs"', '"modVersion":\u00a0"nbsp"', '"modVersion": "with \\"esc"']
    raw = [b"plain", b"a\r\nb\rc\n", b"\xff\xfe bad utf8 \xe2\x9c\x93", b"trail   \n\n", b""]
    run_obj = builder.Run({})
    run_obj.lines = ["$ one", "x" * 59990, "é" * 20]
    return {
        "art_message": [[[r.name for r in reqs[:n]], builder._art_message(reqs[:n])] for n in (1, 2, 4, 5, 7)],
        "short": [[m, builder._short(m)] for m in svc.MODS + ("Other",)],
        "vdf_safe": [[t, builder._vdf_safe(t)] for t in ['plain', 'a\\b "c"\nd', "Added a profile picture for X."]],
        "versions": [[t, (__import__("re").search(r'"modVersion":\s*"([^"]*)"', t) or [None, None])[1]] for t in texts],
        "newlines": [[r.decode("latin-1"), subprocess.Popen._translate_newlines(None, r, "utf-8", "replace")]
                     for r in raw],
        "log_text": {"lines": run_obj.lines, "text": run_obj.text()},
        "vdf": steam_vdf(),
    }


def steam_vdf():
    ws = Path(tempfile.mkdtemp())
    tree = ws / "TroveUI"
    mod = svc.CHAT
    (tree / mod / "dist").mkdir(parents=True)
    (tree / mod / "dist" / f"{mod}.tmod").write_bytes(b"tmod")
    (tree / "steam_ids.json").write_text(json.dumps({mod: 123456}), encoding="utf-8")
    old = (settings.custom_art_workspace, settings.custom_art_steam_user)
    settings.custom_art_workspace = str(ws)
    settings.custom_art_steam_user = "parity"
    r = builder.Run({})
    r.__class__ = type("FakeRun", (builder.Run,), {"__call__": lambda self, args, cwd, check=True, env=None: (
        self.say("$ " + " ".join(args)), self.lines.append("Success."), True)[-1]})
    note = 'Added a "pic" for A\\B\nand more.'
    ok = builder._steam(tree, mod, note, r)
    text = (ws / "steam" / "item.vdf").read_text(encoding="utf-8")
    settings.custom_art_workspace, settings.custom_art_steam_user = old
    content = (ws / "steam" / "content").as_posix()
    return {"note": note, "item": "123456", "ok": ok, "vdf": text.replace(content, "<CONTENT>"),
            "lines": [ln.replace(str(ws), "<WS>") for ln in r.lines]}


def bodies(cases):
    BODIES.mkdir(parents=True, exist_ok=True)
    big = pil(Image.frombytes("RGB", (24, 24), __import__("random").Random(7).randbytes(24 * 24 * 3)), "PNG")
    assert len(big) > 1024 and all(len(cases[k]) <= 1024 for k in ("png_rgba", "png_palette_trns", "jpeg_baseline"))
    specs = {
        "player_ok": mp(BASE + [("note", "  Parity note ✓  ", None, None),
                                ("pfp", cases["png_rgba"], "pic.png", "image/png")]),
        "club_ok": mp([("kind", "club", None, None), ("name", "Parity Club", None, None),
                       ("email", "Club@Example.COM", None, None),
                       ("pfp", cases["png_palette_trns"], "p.png", "image/png"),
                       ("banner", cases["jpeg_baseline"], "b.jpg", "image/jpeg")]),
        "club_banner_only": mp([("kind", "club", None, None), ("name", "Banner Club", None, None),
                                ("email", "banner@x.io", None, None),
                                ("banner", cases["gif_grow"], "b.gif", "image/gif")]),
        "bad_name": mp([("kind", "player", None, None), ("name", "bad/name", None, None),
                        ("email", "a@x.io", None, None), ("pfp", cases["png_rgba"], "p.png", "image/png")]),
        "disposable": mp([("kind", "player", None, None), ("name", "ok", None, None),
                          ("email", "a@mailinator.com", None, None), ("pfp", cases["png_rgba"], "p.png", None)]),
        "not_square": mp(BASE + [("pfp", cases["png_rgb"], "p.png", "image/png")]),
        "not_picture": mp(BASE + [("pfp", cases["text"], "p.txt", "text/plain")]),
        "over_limit": mp(BASE + [("pfp", big, "big.png", "image/png")]),
        "player_banner": mp(BASE + [("pfp", cases["png_rgba"], "p.png", None), ("banner", cases["png_rgb"], "b.png", None)]),
        "unicode_email": mp([("kind", "player", None, None), ("name", "Uni", None, None),
                             ("email", "josé@exämple.com", None, None), ("pfp", cases["png_rgba"], "p.png", None)]),
        "validation_422": mp([("kind", "team", None, None), ("pfp", "text", None, None)]),
    }
    for name, body in specs.items():
        (BODIES / f"{name}.bin").write_bytes(body)


def main():
    cases = pictures()
    http_cases, config = http()
    data = {
        "env": ENV,
        "names": [[n, try_(svc.clean_name, n)] for n in NAMES],
        "emails": [[e, try_(svc.clean_email, e)] for e in EMAILS],
        "shapes": shapes(),
        "measure": measure_cases(cases),
        "submits": submits(cases),
        "views": views(),
        "changelogs": changelogs(),
        "mails": mails(),
        "http": http_cases,
        "config": config,
        "admin_bodies": admin_bodies(),
        "worker": worker_helpers(),
    }
    bodies(cases)
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(data, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
