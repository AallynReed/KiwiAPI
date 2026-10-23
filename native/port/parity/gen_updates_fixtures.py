"""Golden fixtures for the update archive read API (/v1/updates/*, /site/updates/*), from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_updates_fixtures.py
Writes native/kiwi-server/testdata/updates/golden.json.

The pure parts (difflib hunks, blob decoding, the .pkfx parser, directory listing,
viewer classification) are captured straight from the Python functions. The HTTP
cases run the real routers and Beanie models against an in-memory pymongo layer
that records every call, with the CAS on a real temp directory whose files carry a
fixed mtime (FileResponse's ETag/Last-Modified come from os.stat).
"""
import asyncio
import base64
import datetime as dt
import hashlib
import json
import logging
import os
import random
import re
import sys
import tempfile
import types
import urllib.parse
import warnings
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
sys.path.insert(0, os.getcwd())
os.environ.setdefault("SECRET_KEY", "updates-fixture-secret")

import bson  # noqa: E402
from bson import json_util  # noqa: E402
from bson.codec_options import CodecOptions  # noqa: E402
from bson.json_util import CANONICAL_JSON_OPTIONS  # noqa: E402
from bson.objectid import ObjectId  # noqa: E402
from pymongo.asynchronous.collection import AsyncCollection  # noqa: E402
from pymongo.asynchronous.database import AsyncDatabase  # noqa: E402

from app.admin import runtime_config as rc  # noqa: E402

for _key, _spec in rc.REGISTRY.items():
    rc._CACHE[_key] = (_spec.default, float("inf"))
rc._CACHE["feature_updates_enabled"] = (True, float("inf"))

import app.core.dependencies as deps  # noqa: E402
from app.core.config import settings  # noqa: E402
from app.core.ratelimit import RateLimitInfo  # noqa: E402
from app.main import app  # noqa: E402
from app.trove.mods_hub import vfx as pkfx  # noqa: E402
from app.trove.updates import compare as cmp  # noqa: E402
from app.trove.updates import read as rd  # noqa: E402
from app.trove.updates import vfx as uvfx  # noqa: E402
from app.trove.updates.models import (  # noqa: E402
    UpdateBranch,
    UpdateChange,
    UpdateManifestEntry,
    UpdateState,
    UpdateVersion,
)

OUT = Path(__file__).resolve().parents[2] / "kiwi-server" / "testdata" / "updates" / "golden.json"
MTIME_NS = 1_760_659_200_500_000_000
KEPT_HEADERS = ("content-type", "cache-control", "etag", "content-disposition", "last-modified", "accept-ranges",
                "content-range", "x-ratelimit-limit", "x-ratelimit-remaining", "x-ratelimit-reset", "allow",
                "www-authenticate")
CODEC = CodecOptions(tz_aware=True, tzinfo=dt.timezone.utc)


def ext(v):
    return json.loads(json_util.dumps(v, json_options=CANONICAL_JSON_OPTIONS))


def bson_round(doc):
    return bson.decode(bson.encode(doc), codec_options=CODEC)


# --- pure cases -----------------------------------------------------------------


def run_hunks():
    rng = random.Random(20261023)
    cases = []
    crafted = [
        ([], []),
        (["a"], []),
        ([], ["a"]),
        (["a", "b", "c"], ["a", "b", "c"]),
        ([str(i) for i in range(30)], [str(i) for i in range(30) if i not in (5, 20)] + ["x"]),
        ([str(i) for i in range(40)], [str(i) if i % 9 else f"{i}!" for i in range(40)]),
        (["", "", "a", "", "", "b", ""], ["", "a", "", "", "", "b", "", ""]),
        (["x"] * 12, ["x"] * 5 + ["y"] + ["x"] * 7),
        (["a", "b"] * 10, ["b", "a"] * 10),
    ]
    for a, b in crafted:
        cases.append({"a": a, "b": b})
    for n in range(130):
        alpha = ["", "a", "b", "c", "dd", "{", "}"][: rng.randint(2, 7)]
        la, lb = rng.randint(0, 30 if n < 120 else 150), rng.randint(0, 30 if n < 120 else 150)
        a = [rng.choice(alpha) for _ in range(la)]
        if rng.random() < 0.6:
            b = list(a)
            for _ in range(rng.randint(0, 8)):
                op = rng.random()
                pos = rng.randint(0, len(b))
                if op < 0.4:
                    b.insert(pos, rng.choice(alpha + ["new"]))
                elif op < 0.8 and b:
                    del b[min(pos, len(b) - 1)]
                elif b:
                    b[min(pos, len(b) - 1)] = "chg"
        else:
            b = [rng.choice(alpha) for _ in range(lb)]
        cases.append({"a": a, "b": b})
    for c in cases:
        sm = cmp.difflib.SequenceMatcher(a=c["a"], b=c["b"], autojunk=False)
        c["opcodes"] = [list(o) for o in sm.get_opcodes()]
        c["hunks"] = cmp.make_hunks(c["a"], c["b"])
    return cases


DECODE_INPUTS = [
    None, b"", b"one\ntwo\n", b"one\r\ntwo\rthree", b"a\x0cb\x0bc\x1cd\x1de\x1ef", b"tab\there\x00nul\x7fdel",
    "caf\u00e9\nna\u00efve\u2028sep\u2029par\x85nel".encode(), b"latin \xe9\xe8\x85x\n", b"\xff\xfe\x00\x01",
    b"trailing\n\n", b"\n", b"x" * 64, "emoji \U0001f600 \u00ff".encode(), b"\xed\xa0\x80 surrogate",
    b"\r\r\n\n",
]


def run_decode():
    out = []
    for data in DECODE_INPUTS:
        d = cmp.decode_blob(data)
        out.append({"data": None if data is None else base64.b64encode(data).decode(),
                    "lines": d.lines, "is_text": d.is_text, "reason": d.reason})
    for n in (cmp.MAX_DIFF_BYTES + 1, cmp.MAX_DIFF_BYTES):
        d = cmp.decode_blob(b"x\n" * (n // 2) + b"x" * (n % 2))
        out.append({"len": n, "line_count": len(d.lines), "is_text": d.is_text, "reason": d.reason})
    return out


LOSSY = [b"ok", b"\xe2\x82", b"\xf0\x80\x80x", b"\xed\xa0\x80", b"a\xffb\xc3", b"\xc0\xaf", b"\xf4\x90\x80\x80",
         b"\xe0\x80\x80", b"\xf0\x9f\x98", "\u00e9".encode() + b"\x80\x80"]


def run_lossy():
    return [{"data": base64.b64encode(b).decode(), "text": b.decode("utf-8", "replace")} for b in LOSSY]


PKFX_TEXTS = [
    'Version = 1.10.0.38425;\nCParticleEffect\t$LOCAL$/Resource\n{\n\tTexture = "Textures/fire_01.dds";\n'
    '\tMesh = "Meshes/Ring.PKMM";\n}\n',
    'CParticleSamplerShape\t$LOCAL$/A\n{\n\tAnimResource = "Meshes/path_spline.pkmm";\n'
    '\tOther = "$LOCAL$/thing.dds";\n\tDup = "textures/FIRE_01.DDS";\n}\n',
    'CNEdEditorParticle_PreviewRoom $LOCAL$/X\n{\n\tModel = "Editor/room.fbx";\n}\n'
    'CParticleRenderer $LOCAL$/Y\n{\n\tTex = "Textures\\\\Back\\\\slash.tga";\n\tTwo = "a.png" "b.pkat";\n}\n',
    'Line = "no ext here";\nRef = "x.dds.png";\nBad = "multi\nline.dds";\nAnimResourceX = "m/a.pkmm";\n'
    'q = "a.dds"b.dds"";\r\nCRLF = "crlf.tga";\x85NEL = "nel.png";\u2028LS = "ls.pkfx";',
    'AnimResource = "dir.v2/mesh";\nAnimResource = "C:\\\\path.d\\\\mesh.pkmm";\nx = "K\u212a.pkfx";',
    "",
]


def run_pkfx():
    out = []
    for t in PKFX_TEXTS:
        out.append({"text": t, "refs": pkfx.extract_refs(t)})
    names = ["a/b/c.dds", "a\\b\\c.PNG", "noext", "dir.v/file", ".hidden", "x.pkat", "x.PKFX", "y.pkan", "z.fbx",
             "w.TGA", "", "trailing/"]
    media = [{"path": p, "basename": pkfx.basename(p), "media": pkfx.media_type_for(p),
              "asset": p.lower().endswith(uvfx._ASSET_SUFFIXES)} for p in names]
    return out, media


def run_listing():
    rows = [
        {"path": "a.txt", "size": 3, "last_ordinal": 2},
        {"path": "dir/x.txt", "size": 10, "last_ordinal": 1},
        {"path": "dir/sub/y.txt", "size": 5, "last_ordinal": 4},
        {"path": "dir", "size": 7, "last_ordinal": 0},
        {"path": "B.txt", "size": 1},
        {"path": "zdir/q", "size": 2, "last_ordinal": None},
        {"path": "\u00e9t\u00e9/f", "size": 1, "last_ordinal": 3},
        {"path": "dir/", "size": 1, "last_ordinal": 9},
    ]
    out = []
    for prefix in ["", "dir/", "dir/sub/", "nope/", "d"]:
        out.append({"rows": rows, "prefix": prefix, "listing": rd.directory_listing(rows, prefix)})
    return out


def run_view_kinds():
    paths = ["a.png", "A.JPEG", "x.dds", "m.blueprint", "ui.swf", "s.bnk", "fx.pkfx", "readme.txt", "noext",
             "dir.v/file", "big.txt", "big.png", "x.apng", "x.ico", "trailing.", ".gitignore", "a.b/c.PkFx"]
    out = []
    for p in paths:
        for size in (10, rd.VIEW_MAX_BYTES, rd.VIEW_MAX_BYTES + 1):
            async def meta(branch, path, size=size):
                return {"path": path, "content_sha256": "0" * 64, "size": size, "archive": None,
                        "archive_index": None, "removed": False, "removed_ordinal": None}
            rd.get_file_meta = meta
            payload = LOOP.run_until_complete(rd.read_file_text("live-us", p))
            out.append({"path": p, "size": size, "kind": payload["kind"]})
    rd.get_file_meta = REAL_GET_FILE_META
    return out


# --- in-memory pymongo ----------------------------------------------------------

DB: dict[str, list[dict]] = {}
CALLS: list[dict] = []


def _get(doc, key):
    return [doc[key]] if key in doc else []


def _match_value(values, cond):
    if isinstance(cond, dict) and any(k.startswith("$") for k in cond):
        for op, arg in cond.items():
            if op == "$ne":
                if arg in values or (arg is None and not values):
                    return False
            elif op in ("$lt", "$lte", "$gte"):
                cmpf = {"$lt": lambda v: v < arg, "$lte": lambda v: v <= arg, "$gte": lambda v: v >= arg}[op]
                if not any(v is not None and type(v) is type(arg) and cmpf(v) for v in values):
                    return False
            elif op == "$in":
                if not any(v in arg for v in values):
                    return False
            elif op == "$regex":
                flags = re.I if "i" in cond.get("$options", "") else 0
                if not any(isinstance(v, str) and re.search(arg, v, flags) for v in values):
                    return False
            elif op == "$options":
                continue
            else:
                raise NotImplementedError(op)
        return True
    return any(v == cond for v in values)


def _matches(doc, flt):
    return all(_match_value(_get(doc, k), cond) for k, cond in (flt or {}).items())


def _project(doc, projection):
    if not projection:
        return dict(doc)
    keep_id = projection.get("_id", 1)
    return {k: v for k, v in doc.items() if (k == "_id" and keep_id) or (k != "_id" and projection.get(k))}


def _run(coll, flt, sort=None, skip=0, limit=0, projection=None):
    docs = [d for d in DB.get(coll, []) if _matches(d, flt)]
    for key, direction in reversed(sort or []):
        docs.sort(key=lambda d: d.get(key), reverse=direction < 0)
    if skip:
        docs = docs[skip:]
    if limit:
        docs = docs[:limit]
    return [_project(d, projection) for d in docs]


def _sort_spec(sort):
    if sort is None:
        return []
    return [[k, d] for k, d in sort]


class FakeCursor:
    def __init__(self, coll, flt, projection, sort=None, skip=0, limit=0):
        self.coll, self.flt, self.projection = coll, flt, projection
        self.sort_, self.skip_, self.limit_ = _sort_spec(sort), skip or 0, limit or 0

    def sort(self, key, direction=1):
        self.sort_ = [[key, direction]]
        return self

    def skip(self, n):
        self.skip_ = n
        return self

    def limit(self, n):
        self.limit_ = n
        return self

    def _results(self):
        res = _run(self.coll, self.flt, self.sort_, self.skip_, self.limit_, self.projection)
        CALLS.append({"op": "find", "coll": self.coll, "filter": ext(self.flt or {}),
                      "projection": ext(self.projection), "sort": self.sort_, "skip": self.skip_,
                      "limit": self.limit_, "result": ext(res)})
        return res

    async def to_list(self, length=None):
        return self._results()

    def __aiter__(self):
        self._it = iter(self._results())
        return self

    async def __anext__(self):
        try:
            return next(self._it)
        except StopIteration:
            raise StopAsyncIteration from None


def _find(self, filter=None, projection=None, *a, **kw):
    return FakeCursor(self.name, filter, projection, kw.get("sort"), kw.get("skip"), kw.get("limit"))


async def _find_one(self, filter=None, *a, projection=None, **kw):
    res = _run(self.name, filter, projection=projection)
    hit = res[0] if res else None
    CALLS.append({"op": "find_one", "coll": self.name, "filter": ext(filter or {}),
                  "projection": ext(projection), "result": ext(hit)})
    return hit


async def _count(self, filter, *a, **kw):
    n = len(_run(self.name, filter))
    CALLS.append({"op": "count", "coll": self.name, "filter": ext(filter), "result": n})
    return n


AsyncCollection.find = _find
AsyncCollection.find_one = _find_one
AsyncCollection.count_documents = _count


async def _cmd(self, *a, **k):
    return {"version": "7.0.0"}


async def _lcn(self, *a, **k):
    return []


AsyncDatabase.command = _cmd
AsyncDatabase.list_collection_names = _lcn


async def _init():
    from beanie import init_beanie
    from pymongo import AsyncMongoClient
    client = AsyncMongoClient("mongodb://127.0.0.1:1", connect=False, serverSelectionTimeoutMS=200)
    await init_beanie(database=client["kiwi"], skip_indexes=True, document_models=[
        UpdateBranch, UpdateVersion, UpdateChange, UpdateState, UpdateManifestEntry])


LOOP = asyncio.new_event_loop()
LOOP.run_until_complete(_init())
REAL_GET_FILE_META = rd.get_file_meta

# --- auth + limits --------------------------------------------------------------

LIMIT = RateLimitInfo(limit=600, remaining=599, reset=dt.datetime(2026, 1, 2, 3, 4, 5, tzinfo=dt.timezone.utc))


async def _limit(*_a, **_k):
    return LIMIT


async def _no_use(*_a, **_k):
    return None


USER = types.SimpleNamespace(id=ObjectId("65a1b2c3d4e5f60718293a4b"), email="master@aallyn.net",
                             is_superuser=True, is_active=True)
TOKENS = {
    "tok_all": types.SimpleNamespace(id=ObjectId("65a1b2c3d4e5f60718293a51"), name="Updates bot",
                                     scopes=0, allowed_ip_hashes=[], ip_salt=None),
    "tok_feeds": types.SimpleNamespace(id=ObjectId("65a1b2c3d4e5f60718293a52"), name="feeds only",
                                       scopes=2, allowed_ip_hashes=[], ip_salt=None),
}


async def _resolve(creds):
    tok = TOKENS.get(creds.credentials)
    if tok is None:
        raise deps._not_authenticated("Invalid or revoked API token")
    return USER, tok


deps._resolve_token = _resolve
deps.check_rate_limit = _limit
deps.record_token_use = _no_use


async def call(method, url, headers):
    path, _, qs = url.partition("?")
    scope = {
        "type": "http", "asgi": {"version": "3.0"}, "http_version": "1.1", "method": method,
        "scheme": "http", "path": urllib.parse.unquote(path), "raw_path": path.encode(),
        "query_string": qs.encode(), "root_path": "",
        "headers": [(b"host", b"testserver")] + headers + [(b"content-length", b"0")],
        "client": ("127.0.0.1", 50000), "server": ("testserver", 80),
    }
    done = asyncio.Event()
    pending = [{"type": "http.request", "body": b"", "more_body": False}]
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
    start = next(x for x in sent if x["type"] == "http.response.start")
    raw = b"".join(x.get("body", b"") for x in sent if x["type"] == "http.response.body")
    return start["status"], [(k.lower().decode(), v.decode("latin-1")) for k, v in start["headers"]], raw


# --- dataset --------------------------------------------------------------------


def utc(*a):
    return dt.datetime(*a, tzinfo=dt.timezone.utc)


def lines(n, edits=()):
    out = [f"line {i}: the quick brown fox" for i in range(n)]
    for i, text in edits:
        if text is None:
            out[i] = None
        else:
            out[i] = text
    return ("\n".join(x for x in out if x is not None) + "\n").encode()


FIRE = (b'Version = 1.10.0;\nCParticleEffect\t$LOCAL$/Resource\n{\n\tDiffuse = "Textures/fire_01.dds";\n'
        b'\tAnimResource = "Meshes/Ring.pkmm";\n\tGone = "Textures/nothere.png";\n'
        b'\tDup = "TEXTURES/FIRE_01.DDS";\n\tFull = "particles/vfx/textures/fire_01.dds";\n}\n'
        b'CNEdEditorParticle_Backdrop $LOCAL$/Room\n{\n\tMesh = "Editor/room.fbx";\n}\n')
SPARK = b'CParticleEffect $LOCAL$/S\n{\n\tTex = "Textures/spark.tga";\n\tName = "caf\xe9 \xff";\n}\n'
OLD_FX = b'CParticleEffect $LOCAL$/O\n{\n\tTex = "Textures/old_only.dds";\n}\n'

CONTENT = {
    "readme_v1": lines(40),
    "readme_v2": lines(40, [(3, "line 3: CHANGED"), (25, None), (39, "line 39: end changed")]),
    "readme_v3": lines(40, [(3, "line 3: CHANGED"), (10, "line 10: \tTAB\x01ctl"), (25, None)]) + b"tail\r\nmore",
    "a_binfab": b"binfab \x00\x01\x02 packed",
    "b_binfab": b"second \x00 blob",
    "logo": b"\x89PNG\r\n\x1a\nfake",
    "old_txt": b"old file\nremoved later\n",
    "latin": b"caf\xe9 latin-1 \x85 next\x0bvt\n",
    "nul": b"text with \x00 nul",
    "fire": FIRE,
    "spark": SPARK,
    "old_fx": OLD_FX,
    "fire_dds": b"DDS fire texture",
    "ring_pkan": b"pkan animation data",
    "spark_tga": b"TGA spark",
    "thumb_png": b"\x89PNG thumb",
    "stray_pkfx": b'CParticleEffect $LOCAL$/Q\n{\n\tTex = "Textures/fire_01.dds";\n}\n',
    "project": b"<popcornproject/>",
    "blueprint": b"bp",
    "swf": b"FWS",
    "bnk": b"BKHD",
    "dds": b"DDS ",
    "unicode": "na\u00efve unicode file\n".encode(),
    "pts_txt": b"pts only\n",
    "big_view": b"pretend this is big",
    "huge": b"y" * (1024 * 1024 + 5),
    "missing": b"this blob is never written",
}
SHA = {k: hashlib.sha256(v).hexdigest() for k, v in CONTENT.items()}
NOT_WRITTEN = {"missing"}
GENERATED = {"huge": ("y", 1024 * 1024 + 5)}


def change(branch, ordinal, path, typ, key=None, size=None):
    return {"_id": ObjectId(), "branch": branch, "ordinal": ordinal, "path": path, "type": typ,
            "content_sha256": SHA[key] if key else None, "fnv_hash": None,
            "size": (size if size is not None else len(CONTENT[key])) if key else 0}


def build_dataset():
    oid = iter(ObjectId(f"66{i:022x}") for i in range(1, 10_000))
    branches = [
        {"_id": next(oid), "branch": "live-us", "content_path": "live", "current_version": "LIVE-3",
         "current_ordinal": 3, "last_probe_at": utc(2026, 10, 20, 11, 0, 0, 123000), "status": "idle",
         "last_error": None},
        {"_id": next(oid), "branch": "pts", "content_path": "pts", "current_version": None,
         "current_ordinal": 1, "last_probe_at": None, "status": "syncing", "last_error": "boom"},
    ]
    versions = []
    for o, tag, status, cap, comp in [
        (1, "LIVE-1", "complete", utc(2026, 9, 1, 11, 0, 0), utc(2026, 9, 1, 11, 5, 0, 250000)),
        (2, "LIVE-2", "complete", utc(2026, 9, 15, 11, 0, 0, 500000), None),
        (3, "LIVE-3", "complete", utc(2026, 10, 1, 11, 0, 0, 1000), utc(2026, 10, 1, 11, 2, 0)),
        (4, "LIVE-4", "in_progress", utc(2026, 10, 20, 11, 0, 0), None),
    ]:
        versions.append({"_id": next(oid), "branch": "live-us", "ordinal": o, "version_tag": tag, "status": status,
                         "captured_at": cap, "completed_at": comp, "content_path": "live", "motd": "",
                         "files_added": o * 3, "files_modified": o, "files_removed": o - 1,
                         "bytes_added": o * 1000})
    versions.append({"_id": next(oid), "branch": "pts", "ordinal": 1, "version_tag": "PTS-1", "status": "complete",
                     "captured_at": utc(2026, 9, 2, 0, 0, 0), "completed_at": None, "content_path": "pts",
                     "motd": "hi", "files_added": 1, "files_modified": 0, "files_removed": 0, "bytes_added": 9})
    L = "live-us"
    changes = [
        change(L, 1, "ui/readme.txt", "added", "readme_v1"),
        change(L, 1, "prefabs/a.binfab", "added", "a_binfab"),
        change(L, 1, "prefabs/b.binfab", "added", "b_binfab"),
        change(L, 1, "ui/logo.png", "added", "logo"),
        change(L, 1, "data/old.txt", "added", "old_txt"),
        change(L, 1, "data/latin.txt", "added", "latin"),
        change(L, 1, "data/nul.bin", "added", "nul"),
        change(L, 1, "particles/vfx/popcornproject.xml", "added", "project"),
        change(L, 1, "particles/vfx/Effects/Fire.pkfx", "added", "fire"),
        change(L, 1, "particles/vfx/old_gone.pkfx", "added", "old_fx"),
        change(L, 1, "particles/vfx/textures/FIRE_01.dds", "added", "fire_dds"),
        change(L, 1, "particles/vfx/meshes/ring.pkan", "added", "ring_pkan"),
        change(L, 1, "particles/vfx/editor/thumbnails/particles/fire.pkfx.png", "added", "thumb_png"),
        change(L, 1, "data/huge.txt", "added", "huge"),
        change(L, 2, "ui/readme.txt", "modified", "readme_v2"),
        change(L, 2, "data/old.txt", "removed"),
        change(L, 2, "models/ship.blueprint", "added", "blueprint"),
        change(L, 2, "ui/menu.swf", "added", "swf"),
        change(L, 2, "audio/bank.bnk", "added", "bnk"),
        change(L, 2, "ui/tex.DDS", "added", "dds"),
        change(L, 2, "particles/vfx/Effects/aSpark.pkfx", "added", "spark"),
        change(L, 2, "particles/vfx/textures/spark.tga", "added", "spark_tga"),
        change(L, 2, "data/huge.txt", "modified", "readme_v1"),
        change(L, 3, "ui/readme.txt", "modified", "readme_v3"),
        change(L, 3, "particles/vfx/old_gone.pkfx", "removed"),
        change(L, 3, "other/stray.pkfx", "added", "stray_pkfx"),
        change(L, 3, "data/na\u00efve \u00fcnicode.txt", "added", "unicode"),
        change(L, 3, "data/big.txt", "added", "big_view", size=2_000_000),
        change(L, 3, "data/missing.txt", "added", "missing"),
        change(L, 3, "data/huge.txt", "modified", "huge"),
        change(L, 3, "data/a.b+c (1).txt", "added", "readme_v1"),
        change("pts", 1, "pts/only.txt", "added", "pts_txt"),
    ]
    state = {}
    for c in sorted(changes, key=lambda c: c["ordinal"]):
        key = (c["branch"], c["path"])
        if c["type"] == "removed":
            state.pop(key, None)
            continue
        state[key] = {"_id": next(oid), "branch": c["branch"], "path": c["path"], "content_sha256": c["content_sha256"],
                      "fnv_hash": None, "size": c["size"], "archive": "data" if c["path"].startswith("prefabs/") else None,
                      "archive_index": 2 if c["path"].startswith("prefabs/") else None, "last_ordinal": c["ordinal"]}
    state[("live-us", "prefabs/b.binfab")]["last_ordinal"] = 0
    del state[("live-us", "prefabs/b.binfab")]["last_ordinal"]
    rows = list(state.values())
    return {
        "update_branches": [bson_round(d) for d in branches],
        "update_versions": [bson_round(d) for d in versions],
        "update_changes": [bson_round(d) for d in changes],
        "update_state": [bson_round(d) for d in rows],
    }


DATASET = build_dataset()
STORE = Path(tempfile.mkdtemp(prefix="kiwi-updates-"))
settings.trove_update_store_dir = str(STORE)
for key, data in CONTENT.items():
    if key in NOT_WRITTEN:
        continue
    sha = SHA[key]
    p = STORE / "objects" / sha[:2] / sha
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_bytes(data)
    os.utime(p, ns=(MTIME_NS, MTIME_NS))

TOK = {"authorization": "Bearer tok_all"}
V, S = "/v1/updates", "/site/updates"
Q = urllib.parse.quote

HTTP = [
    ("v1_branches", f"{V}/branches", TOK),
    ("v1_versions", f"{V}/live-us/versions", TOK),
    ("v1_versions_page", f"{V}/live-us/versions?limit=1&offset=1", TOK),
    ("v1_versions_pts", f"{V}/pts/versions", TOK),
    ("v1_versions_far", f"{V}/live-us/versions?offset=50", TOK),
    ("v1_versions_unknown", f"{V}/nope/versions", TOK),
    ("v1_versions_422", f"{V}/nope/versions?limit=0&offset=-1", TOK),
    ("v1_versions_422_b", f"{V}/live-us/versions?limit=201&offset=x", TOK),
    ("v1_changes", f"{V}/live-us/changes", TOK),
    ("v1_changes_ordinal", f"{V}/live-us/changes?ordinal=1", TOK),
    ("v1_changes_tag", f"{V}/live-us/changes?version=LIVE-2", TOK),
    ("v1_changes_tag_and_ordinal", f"{V}/live-us/changes?version=LIVE-1&ordinal=2", TOK),
    ("v1_changes_empty_tag", f"{V}/live-us/changes?version=&ordinal=2", TOK),
    ("v1_changes_type", f"{V}/live-us/changes?ordinal=2&type=removed", TOK),
    ("v1_changes_page", f"{V}/live-us/changes?ordinal=1&limit=3&offset=2", TOK),
    ("v1_changes_bad_type", f"{V}/live-us/changes?type=bogus", TOK),
    ("v1_changes_empty_type", f"{V}/live-us/changes?type=", TOK),
    ("v1_changes_no_version", f"{V}/live-us/changes?ordinal=99", TOK),
    ("v1_changes_in_progress", f"{V}/live-us/changes?ordinal=4", TOK),
    ("v1_changes_unknown_tag", f"{V}/live-us/changes?version=NOPE", TOK),
    ("v1_changes_422", f"{V}/live-us/changes?ordinal=x&limit=1001", TOK),
    ("v1_changes_unknown_branch", f"{V}/x/changes?type=bogus", TOK),
    ("v1_changes_pts", f"{V}/pts/changes", TOK),
    ("v1_tree_root", f"{V}/live-us/tree", TOK),
    ("v1_tree_prefabs", f"{V}/live-us/tree?prefix=prefabs", TOK),
    ("v1_tree_slash", f"{V}/live-us/tree?prefix=particles/vfx/", TOK),
    ("v1_tree_none", f"{V}/live-us/tree?prefix=nope/", TOK),
    ("v1_tree_data", f"{V}/live-us/tree?prefix=data", TOK),
    ("v1_tree_unknown", f"{V}/nope/tree", TOK),
    ("v1_meta", f"{V}/live-us/file/meta?path=prefabs/a.binfab", TOK),
    ("v1_meta_removed", f"{V}/live-us/file/meta?path=data/old.txt", TOK),
    ("v1_meta_missing", f"{V}/live-us/file/meta?path=never/was.txt", TOK),
    ("v1_meta_422", f"{V}/live-us/file/meta", TOK),
    ("v1_file", f"{V}/live-us/file?path=ui/readme.txt", {}),
    ("v1_file_token", f"{V}/live-us/file?path=prefabs/a.binfab", TOK),
    ("v1_file_removed", f"{V}/live-us/file?path=data/old.txt", {}),
    ("v1_file_unicode", f"{V}/live-us/file?path={Q('data/na\u00efve \u00fcnicode.txt')}", {}),
    ("v1_file_spaces", f"{V}/live-us/file?path={Q('data/a.b+c (1).txt')}", {}),
    ("v1_file_missing_blob", f"{V}/live-us/file?path=data/missing.txt", {}),
    ("v1_file_none", f"{V}/live-us/file?path=x/y", {}),
    ("v1_file_unknown_branch", f"{V}/nope/file?path=x", {}),
    ("v1_file_422", f"{V}/live-us/file", {}),
    ("v1_file_range", f"{V}/live-us/file?path=ui/readme.txt", {"range": "bytes=0-9"}),
    ("v1_file_range_suffix", f"{V}/live-us/file?path=ui/readme.txt", {"range": "bytes=-5"}),
    ("v1_file_range_bad", f"{V}/live-us/file?path=ui/readme.txt", {"range": "items=0-1"}),
    ("v1_file_range_416", f"{V}/live-us/file?path=ui/readme.txt", {"range": "bytes=99999-"}),
    ("v1_file_if_range_ok", f"{V}/live-us/file?path=ui/readme.txt",
     {"range": "bytes=2-4", "if-range": "Fri, 17 Oct 2025 00:00:00 GMT"}),
    ("v1_file_if_range_stale", f"{V}/live-us/file?path=ui/readme.txt",
     {"range": "bytes=2-4", "if-range": "\"stale\""}),
    ("v1_view_text", f"{V}/live-us/file/view?path=ui/readme.txt", TOK),
    ("v1_view_image", f"{V}/live-us/file/view?path=ui/logo.png", TOK),
    ("v1_view_dds", f"{V}/live-us/file/view?path=ui/tex.DDS", TOK),
    ("v1_view_blueprint", f"{V}/live-us/file/view?path=models/ship.blueprint", TOK),
    ("v1_view_swf", f"{V}/live-us/file/view?path=ui/menu.swf", TOK),
    ("v1_view_bnk", f"{V}/live-us/file/view?path=audio/bank.bnk", TOK),
    ("v1_view_pkfx", f"{V}/live-us/file/view?path=particles/vfx/Effects/Fire.pkfx", TOK),
    ("v1_view_too_large", f"{V}/live-us/file/view?path=data/big.txt", TOK),
    ("v1_view_missing", f"{V}/live-us/file/view?path=data/missing.txt", TOK),
    ("v1_view_binary", f"{V}/live-us/file/view?path=prefabs/a.binfab", TOK),
    ("v1_view_latin", f"{V}/live-us/file/view?path=data/latin.txt", TOK),
    ("v1_view_removed", f"{V}/live-us/file/view?path=data/old.txt", TOK),
    ("v1_view_none", f"{V}/live-us/file/view?path=nope", TOK),
    ("v1_history", f"{V}/live-us/file/history?path=ui/readme.txt", TOK),
    ("v1_history_removed", f"{V}/live-us/file/history?path=data/old.txt", TOK),
    ("v1_history_none", f"{V}/live-us/file/history?path=nope", TOK),
    ("v1_compare", f"{V}/live-us/file/compare?path=ui/readme.txt&from=1&to=3", TOK),
    ("v1_compare_reverse", f"{V}/live-us/file/compare?path=ui/readme.txt&from=3&to=2", TOK),
    ("v1_compare_identical", f"{V}/live-us/file/compare?path=prefabs/a.binfab&from=1&to=3", TOK),
    ("v1_compare_added", f"{V}/live-us/file/compare?path=ui/menu.swf&from=1&to=2", TOK),
    ("v1_compare_removed", f"{V}/live-us/file/compare?path=data/old.txt&from=1&to=3", TOK),
    ("v1_compare_neither", f"{V}/live-us/file/compare?path=nope&from=1&to=2", TOK),
    ("v1_compare_too_large", f"{V}/live-us/file/compare?path=data/huge.txt&from=2&to=3", TOK),
    ("v1_compare_missing_blob", f"{V}/live-us/file/compare?path=data/missing.txt&from=2&to=3", TOK),
    ("v1_compare_no_ordinal", f"{V}/live-us/file/compare?path=ui/readme.txt&from=1&to=9", TOK),
    ("v1_compare_in_progress", f"{V}/live-us/file/compare?path=ui/readme.txt&from=2&to=4", TOK),
    ("v1_compare_422", f"{V}/live-us/file/compare?from=x", TOK),
    ("site_branches", f"{S}/branches", {}),
    ("site_versions", f"{S}/live-us/versions", {}),
    ("site_versions_page", f"{S}/live-us/versions?limit=2&offset=1", {}),
    ("site_versions_unknown", f"{S}/zz/versions", {}),
    ("site_versions_422", f"{S}/live-us/versions?limit=500", {}),
    ("site_changes", f"{S}/live-us/changes", {}),
    ("site_changes_type", f"{S}/live-us/changes?ordinal=3&type=added&limit=2000", {}),
    ("site_changes_bad_type", f"{S}/live-us/changes?type=zz", {}),
    ("site_changes_no_version", f"{S}/live-us/changes?version=nope", {}),
    ("site_changes_422", f"{S}/live-us/changes?limit=2001", {}),
    ("site_tree", f"{S}/live-us/tree", {}),
    ("site_tree_prefix", f"{S}/live-us/tree?prefix=particles", {}),
    ("site_tree_pts", f"{S}/pts/tree?prefix=pts/", {}),
    ("site_search", f"{S}/live-us/search?q=readme", {}),
    ("site_search_case", f"{S}/live-us/search?q=FIRE", {}),
    ("site_search_regex", f"{S}/live-us/search?q={Q('a.b+c (')}", {}),
    ("site_search_dot", f"{S}/live-us/search?q=.", {}),
    ("site_search_limit", f"{S}/live-us/search?q=a&limit=2", {}),
    ("site_search_blank", f"{S}/live-us/search?q=%20%20", {}),
    ("site_search_padded", f"{S}/live-us/search?q=%20readme%20", {}),
    ("site_search_none", f"{S}/live-us/search?q=zzzz", {}),
    ("site_search_422", f"{S}/live-us/search?limit=501", {}),
    ("site_search_422_long", f"{S}/live-us/search?q={'x' * 201}", {}),
    ("site_search_unknown", f"{S}/qq/search?q=x", {}),
    ("site_meta", f"{S}/live-us/file/meta?path=prefabs/a.binfab", {}),
    ("site_meta_removed", f"{S}/live-us/file/meta?path=particles/vfx/old_gone.pkfx", {}),
    ("site_meta_none", f"{S}/live-us/file/meta?path=nope", {}),
    ("site_view_text", f"{S}/live-us/file/view?path=ui/readme.txt", {}),
    ("site_view_unicode", f"{S}/live-us/file/view?path={Q('data/na\u00efve \u00fcnicode.txt')}", {}),
    ("site_view_binary", f"{S}/live-us/file/view?path=data/nul.bin", {}),
    ("site_view_image", f"{S}/live-us/file/view?path=ui/logo.png", {}),
    ("site_view_removed", f"{S}/live-us/file/view?path=data/old.txt", {}),
    ("site_view_none", f"{S}/live-us/file/view?path=zz", {}),
    ("site_view_422", f"{S}/live-us/file/view", {}),
    ("site_history", f"{S}/live-us/file/history?path=ui/readme.txt", {}),
    ("site_history_none", f"{S}/live-us/file/history?path=zz", {}),
    ("site_compare", f"{S}/live-us/file/compare?path=ui/readme.txt&from=1&to=2", {}),
    ("site_compare_identical", f"{S}/live-us/file/compare?path=prefabs/b.binfab&from=1&to=2", {}),
    ("site_compare_neither", f"{S}/live-us/file/compare?path=zz&from=1&to=3", {}),
    ("site_compare_too_large", f"{S}/live-us/file/compare?path=data/huge.txt&from=1&to=3", {}),
    ("site_compare_latin", f"{S}/live-us/file/compare?path=data/latin.txt&from=1&to=3", {}),
    ("site_compare_added", f"{S}/live-us/file/compare?path=other/stray.pkfx&from=2&to=3", {}),
    ("site_compare_no_ordinal", f"{S}/live-us/file/compare?path=ui/readme.txt&from=0&to=1", {}),
    ("site_compare_422", f"{S}/live-us/file/compare?path=x&to=1.5", {}),
    ("site_vfx", f"{S}/live-us/vfx", {}),
    ("site_vfx_q", f"{S}/live-us/vfx?q=%20FIRE%20", {}),
    ("site_vfx_page", f"{S}/live-us/vfx?limit=1&offset=1", {}),
    ("site_vfx_far", f"{S}/live-us/vfx?offset=99999999999999999999999", {}),
    ("site_vfx_pts", f"{S}/pts/vfx", {}),
    ("site_vfx_422", f"{S}/live-us/vfx?limit=0&q={'y' * 201}", {}),
    ("site_vfx_unknown", f"{S}/zz/vfx", {}),
    ("site_vfx_manifest", f"{S}/live-us/vfx/manifest?path=particles/vfx/Effects/Fire.pkfx", {}),
    ("site_vfx_manifest_rel", f"{S}/live-us/vfx/manifest?path=%5CEffects%5Cfire.PKFX", {}),
    ("site_vfx_manifest_lossy", f"{S}/live-us/vfx/manifest?path=particles/vfx/Effects/aSpark.pkfx", {}),
    ("site_vfx_manifest_stray", f"{S}/live-us/vfx/manifest?path=other/stray.pkfx", {}),
    ("site_vfx_manifest_removed", f"{S}/live-us/vfx/manifest?path=/particles/vfx/old_gone.pkfx", {}),
    ("site_vfx_manifest_none", f"{S}/live-us/vfx/manifest?path=particles/vfx/nope.pkfx", {}),
    ("site_vfx_manifest_not_pkfx", f"{S}/live-us/vfx/manifest?path=popcornproject.xml", {}),
    ("site_vfx_manifest_empty", f"{S}/live-us/vfx/manifest?path=", {}),
    ("site_vfx_asset", f"{S}/live-us/vfx/asset?path=Textures/fire_01.dds", {}),
    ("site_vfx_asset_full", f"{S}/live-us/vfx/asset?path=particles/vfx/textures/spark.tga", {}),
    ("site_vfx_asset_pkan", f"{S}/live-us/vfx/asset?path=%2FMeshes%2Fring.pkan", {}),
    ("site_vfx_asset_png", f"{S}/live-us/vfx/asset?path=editor/thumbnails/particles/fire.pkfx.png", {}),
    ("site_vfx_asset_304", f"{S}/live-us/vfx/asset?path=Textures/fire_01.dds",
     {"if-none-match": f"\"{SHA['fire_dds']}\""}),
    ("site_vfx_asset_weak", f"{S}/live-us/vfx/asset?path=Textures/fire_01.dds",
     {"if-none-match": f"W/\"{SHA['fire_dds']}\""}),
    ("site_vfx_asset_not_asset", f"{S}/live-us/vfx/asset?path=popcornproject.xml", {}),
    ("site_vfx_asset_missing", f"{S}/live-us/vfx/asset?path=Textures/nothere.png", {}),
    ("site_vfx_asset_outside", f"{S}/live-us/vfx/asset?path=other/stray.pkfx", {}),
]


def run_http():
    out = []
    for i, (name, url, headers) in enumerate(HTTP):
        DB.clear()
        DB.update({k: [dict(d) for d in v] for k, v in DATASET.items()})
        CALLS.clear()
        uvfx._CACHE.clear()
        hdrs = [(k.encode(), v.encode("latin-1")) for k, v in headers.items()]
        hdrs.append((b"x-request-id", f"req_u_{i:03d}".encode()))
        status, rh, raw = LOOP.run_until_complete(call("GET", url, hdrs))
        binary = name.startswith("v1_file") or name.startswith("site_vfx_asset")
        out.append({
            "name": name, "url": url,
            "headers": [[k.decode(), v.decode("latin-1")] for k, v in hdrs],
            "status": status,
            "resp_headers": {k: v for k, v in rh if k in KEPT_HEADERS},
            "content_length": next((v for k, v in rh if k == "content-length"), None),
            "resp": base64.b64encode(raw).decode() if binary else raw.decode("utf-8"),
            "calls": list(CALLS),
        })
    return out


GATED = [
    ("v1_disabled", "GET", f"{V}/live-us/versions?limit=0", {}),
    ("v1_file_disabled", "GET", f"{V}/live-us/file", {}),
    ("site_disabled", "GET", f"{S}/live-us/tree", {}),
    ("site_disabled_branches", "GET", f"{S}/branches", {}),
    ("v1_noauth", "GET", f"{V}/live-us/versions?limit=0", {}),
    ("v1_branches_noauth", "GET", f"{V}/branches", {}),
    ("v1_badtoken", "GET", f"{V}/branches", {"authorization": "Bearer nope"}),
    ("v1_scope", "GET", f"{V}/live-us/tree", {"authorization": "Bearer tok_feeds"}),
    ("v1_versions_405", "POST", f"{V}/live-us/versions", TOK),
    ("v1_file_405", "DELETE", f"{V}/live-us/file?path=x", {}),
    ("site_tree_405", "POST", f"{S}/live-us/tree", {}),
    ("site_vfx_asset_405", "PUT", f"{S}/live-us/vfx/asset", {}),
]


def run_gated():
    out = []
    for i, (name, method, url, headers) in enumerate(GATED):
        DB.clear()
        DB.update({k: [dict(d) for d in v] for k, v in DATASET.items()})
        flag = rc._CACHE["feature_updates_enabled"]
        if "disabled" in name:
            rc._CACHE["feature_updates_enabled"] = (False, float("inf"))
        hdrs = [(k.encode(), v.encode("latin-1")) for k, v in headers.items()]
        hdrs.append((b"x-request-id", f"req_g_{i:03d}".encode()))
        status, rh, raw = LOOP.run_until_complete(call(method, url, hdrs))
        rc._CACHE["feature_updates_enabled"] = flag
        out.append({"name": name, "method": method, "url": url,
                    "headers": [[k.decode(), v.decode("latin-1")] for k, v in hdrs], "status": status,
                    "resp_headers": {k: v for k, v in rh if k in KEPT_HEADERS}, "resp": raw.decode("utf-8")})
    return out


def main():
    pk, media = run_pkfx()
    data = {
        "python": sys.version.split()[0],
        "mtime_ns": MTIME_NS,
        "blobs": {SHA[k]: base64.b64encode(v).decode() for k, v in CONTENT.items()
                  if k not in NOT_WRITTEN and k not in GENERATED},
        "generated_blobs": {SHA[k]: {"repeat": r, "len": n} for k, (r, n) in GENERATED.items()},
        "hunks": run_hunks(),
        "decode": run_decode(),
        "lossy": run_lossy(),
        "pkfx": pk,
        "media": media,
        "listing": run_listing(),
        "view_kinds": run_view_kinds(),
        "http": run_http(),
        "gated": run_gated(),
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(data, ensure_ascii=False, separators=(",", ":")) + "\n", encoding="utf-8")
    print(f"wrote {OUT} ({len(data['hunks'])} hunks, {len(data['http'])} http, {len(data['gated'])} gated)")


if __name__ == "__main__":
    main()
