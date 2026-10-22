"""Golden fixtures for file drops (app/drops), from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_drops_fixtures.py
Writes native/kiwi-server/testdata/drops/golden.json.

Runs the real app (raw ASGI) for every reply that is settled before a store is
touched (form parsing, the 422s, the admin body validation), the real view
builders over fixed documents, and Starlette's own FileResponse over a file
with a fixed mtime for the download headers and ranges.
"""
import asyncio
import datetime as dt
import json
import logging
import os
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

ENV = {
    "SECRET_KEY": "drops-fixture-secret",
    "APP_URL": "https://trove.example.test//",
}
os.environ.update(ENV)

from bson import ObjectId  # noqa: E402
from bson.json_util import CANONICAL_JSON_OPTIONS, dumps as bson_dumps  # noqa: E402
from pydantic import TypeAdapter  # noqa: E402
from pymongo.asynchronous.database import AsyncDatabase  # noqa: E402
from starlette.responses import FileResponse  # noqa: E402

from app.admin import runtime_config as rc  # noqa: E402

for _key, _spec in rc.REGISTRY.items():
    rc._CACHE[_key] = (_spec.default, float("inf"))

import app.drops.service as svc  # noqa: E402
from app.core.dependencies import get_current_superuser  # noqa: E402
from app.drops.models import DropUpload, FileDrop  # noqa: E402
from app.drops.schemas import DropCreated, DropPublicView, DropUploadView, DropView  # noqa: E402
from app.main import app  # noqa: E402

OUT = Path(__file__).resolve().parents[2] / "kiwi-server" / "testdata" / "drops" / "golden.json"
NOW = dt.datetime(2026, 10, 22, 11, 0, 0, 123456, tzinfo=dt.timezone.utc)
KEPT = ("content-type", "content-length", "cache-control", "accept-ranges", "content-disposition",
        "last-modified", "etag", "content-range", "allow", "retry-after")


def ext(v):
    return json.loads(bson_dumps(v, json_options=CANONICAL_JSON_OPTIONS))


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
    await init_beanie(database=client["kiwi"], skip_indexes=True, document_models=[FileDrop, DropUpload])


asyncio.run(_init())


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


# --- pure helpers ----------------------------------------------------------------

FILENAMES = [None, "", "  report.pdf  ", "C:\\Users\\me\\My File (1).PNG", "../../etc/passwd", "..", " . ",
             "résumé ✓.pdf", "a" * 130 + ".bin", "we!rd#name$.tar.gz", "[x] (y) - z.txt", "\\\\server\\share\\f.txt",
             "dir/sub/", "日本語.txt"]

RANGES = ["bytes=0-9", "bytes=-5", "bytes=290-", "bytes=0-1000", "bytes=50-60, 0-9, 55-70", "bytes",
          "items=0-1", " BYTES = 0-1", "bytes=x-y", "bytes=300-", "bytes=5-1", "bytes=-", "bytes=,,",
          "bytes=--5", "bytes=1-2-3", "bytes=" + ",".join(["0-1"] * 101), "bytes=0-0,1-1,2-2", "bytes= 3 - 4 ",
          "bytes=-0", "bytes=-1000"]


def ranges():
    out = []
    for r in RANGES:
        try:
            res = {"ok": [list(x) for x in FileResponse._parse_range_header(r, 300)]}
        except Exception as exc:  # noqa: BLE001
            res = {"error": type(exc).__name__, "content": getattr(exc, "content", None)}
        out.append({"range": r, **res})
    return out


def stat_headers():
    out = []
    for mtime, size in [(1761130800.1234567, 300), (1761130800.0, 0), (1.5, 7), (1761130800.999999, 12345678)]:
        st = os.stat_result((0o100644, 0, 0, 1, 0, 0, size, 0, int(mtime), 0, 0, mtime, 0))
        r = FileResponse("x", filename="x")
        r.set_stat_headers(st)
        out.append({"mtime": mtime, "size": size, "etag": r.headers["etag"],
                    "last_modified": r.headers["last-modified"]})
    return out


# --- downloads ---------------------------------------------------------------------

MTIME_NS = 1761130800_123456700
DATA = bytes(range(256)) + b"tail-of-the-file-" * 2 + b"!" * 10


def downloads():
    tmp = Path(tempfile.mkdtemp())
    path = tmp / "stored.bin"
    path.write_bytes(DATA)
    os.utime(path, ns=(MTIME_NS, MTIME_NS))
    head = asyncio.run(asgi(FileResponse(path, filename="x"), "GET", "/", []))[1]
    etag = dict(head)["etag"]
    lm = dict(head)["last-modified"]
    cases = [
        ("plain", "application/octet-stream", "my file (1).bin", []),
        ("text", "text/plain", "notes.txt", []),
        ("text_charset", "text/csv; charset=latin-1", "a.csv", []),
        ("range", "application/pdf", "doc.pdf", [("range", "bytes=10-19")]),
        ("suffix", "application/pdf", "doc.pdf", [("range", "bytes=-7")]),
        ("open_end", "image/png", "a.png", [("range", "bytes=290-")]),
        ("multi", "application/octet-stream", "m.bin", [("range", "bytes=0-1, 5-6, 6-9")]),
        ("malformed", "application/octet-stream", "m.bin", [("range", "pages=1")]),
        ("unsatisfiable", "application/octet-stream", "m.bin", [("range", "bytes=999-")]),
        ("start_after_end", "application/octet-stream", "m.bin", [("range", "bytes=9-3")]),
        ("if_range_etag", "application/octet-stream", "m.bin", [("range", "bytes=0-3"), ("if-range", etag)]),
        ("if_range_date", "application/octet-stream", "m.bin", [("range", "bytes=0-3"), ("if-range", lm)]),
        ("if_range_stale", "application/octet-stream", "m.bin", [("range", "bytes=0-3"), ("if-range", '"nope"')]),
        ("if_range_weak", "application/octet-stream", "m.bin", [("range", "bytes=0-3"), ("if-range", "W/" + etag)]),
        ("too_many", "application/octet-stream", "m.bin", [("range", "bytes=" + ",".join(["0-1"] * 101))]),
        ("empty_ranges", "application/octet-stream", "m.bin", [("range", "bytes=,")]),
    ]
    out = []
    for name, media, filename, hdrs in cases:
        resp = FileResponse(path, media_type=media, filename=filename, headers={"Cache-Control": "no-store"})
        status, rh, body = asyncio.run(asgi(resp, "GET", "/", [(k.encode(), v.encode()) for k, v in hdrs]))
        ctype = dict(rh).get("content-type", "")
        m = re.search(r"boundary=([0-9a-f]+)", ctype)
        if m:
            body = body.replace(m.group(1).encode(), b"BOUNDARY")
            rh = [(k, v.replace(m.group(1), "BOUNDARY")) for k, v in rh]
        out.append({"name": name, "media_type": media, "filename": filename, "headers": hdrs, "status": status,
                    "resp_headers": [[k, v] for k, v in rh if k in KEPT], "body": body.decode("latin-1")})
    return {"mtime_ns": MTIME_NS, "data": DATA.decode("latin-1"), "cases": out}


# --- views ----------------------------------------------------------------------------

D1 = ObjectId("66f000000000000000000001")
D2 = ObjectId("66f000000000000000000002")


def drop_doc(_id, **kw):
    base = {"_id": _id, "slug": "AbCdEfGhIjKlMnOp", "label": "Logs ✓ \"x\"", "pin_hash": "$argon2id$...",
            "max_uploads": 3, "upload_count": 1, "max_file_bytes": 268435456,
            "expires_at": dt.datetime(2026, 10, 23, 11, 0, 0, 123000, tzinfo=dt.timezone.utc),
            "revoked": False, "created_at": dt.datetime(2026, 10, 22, 10, 0, tzinfo=dt.timezone.utc)}
    base.update(kw)
    return base


UPLOAD_DOCS = [
    {"_id": ObjectId("66f100000000000000000001"), "drop_id": D1, "filename": "a b.txt",
     "stored_name": "0011223344556677-a b.txt", "size": 12, "content_type": "text/plain",
     "sha256": "ab" * 32, "note": "hi <there>", "uploaded_at": dt.datetime(2026, 10, 22, 10, 30, 0, 5000,
                                                                              tzinfo=dt.timezone.utc)},
    {"_id": ObjectId("66f100000000000000000002"), "drop_id": D1, "filename": "upload.bin",
     "stored_name": "8899aabbccddeeff-upload.bin", "size": 3_000_000_000, "content_type": None,
     "sha256": "cd" * 32, "note": None, "uploaded_at": dt.datetime(2026, 10, 22, 10, 20, tzinfo=dt.timezone.utc)},
]
DROP_DOCS = [
    drop_doc(D1),
    drop_doc(D2, slug="zzzzzzzzzzzzzzzz", revoked=True, upload_count=0, max_uploads=1),
    drop_doc(ObjectId("66f000000000000000000003"), slug="expired", expires_at=NOW - dt.timedelta(seconds=1)),
    drop_doc(ObjectId("66f000000000000000000004"), slug="spent", upload_count=5, max_uploads=3),
    drop_doc(ObjectId("66f000000000000000000005"), slug="justnow", expires_at=NOW),
]


def bson_round(doc):
    import bson
    from bson.codec_options import CodecOptions
    return bson.decode(bson.encode(doc), codec_options=CodecOptions(tz_aware=True))


def views():
    svc.utcnow = lambda: NOW
    out = []
    for d in map(bson_round, DROP_DOCS):
        drop = FileDrop.model_validate(d)
        raw_ups = [bson_round(u) for u in UPLOAD_DOCS if u["drop_id"] == d["_id"]]
        ups = [DropUpload.model_validate(u) for u in raw_ups]
        out.append({
            "drop": ext(d), "uploads": ext(raw_ups),
            "view": TypeAdapter(DropView).dump_json(svc._view(drop, ups)).decode(),
            "public": TypeAdapter(DropPublicView).dump_json(svc.public_view(drop)).decode(),
            "uploads_view": [TypeAdapter(DropUploadView).dump_json(svc._upload_view(u)).decode() for u in ups],
            "open": svc.is_open(drop),
        })
    created = DropCreated(**svc._view(FileDrop.model_validate(bson_round(DROP_DOCS[0]))).model_dump(), pin="0042")
    return {"now": NOW.isoformat(), "cases": out, "created": TypeAdapter(DropCreated).dump_json(created).decode()}


# --- HTTP: form parsing and validation ------------------------------------------------------

USER = types.SimpleNamespace(id=ObjectId("65a1b2c3d4e5f60718293a4b"), is_superuser=True, is_active=True)
app.dependency_overrides[get_current_superuser] = lambda: USER


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
JSON = "application/json"

HTTP = [
    ("verify_missing_pin", "POST", "/site/drops/abc/verify", MP, mp([("other", "x", None, None)])),
    ("verify_empty_pin", "POST", "/site/drops/abc/verify", FORM, b"pin="),
    ("verify_json_body", "POST", "/site/drops/abc/verify", JSON, b'{"pin":"1234"}'),
    ("verify_no_boundary", "POST", "/site/drops/abc/verify", "multipart/form-data", b"--x\r\n"),
    ("verify_garbage", "POST", "/site/drops/abc/verify", MP, b"garbage"),
    ("verify_pin_is_file", "POST", "/site/drops/abc/verify", MP, mp([("pin", b"1234\n56", "pin.txt", "text/plain")])),
    ("upload_missing_all", "POST", "/site/drops/abc/upload", MP, mp([("x", "y", None, None)])),
    ("upload_no_body", "POST", "/site/drops/abc/upload", None, b""),
    ("upload_file_is_text", "POST", "/site/drops/abc/upload", MP, mp([("pin", "1234", None, None),
                                                                       ("file", "not a file", None, None)])),
    ("upload_file_empty_text", "POST", "/site/drops/abc/upload", MP, mp([("pin", "1234", None, None),
                                                                          ("file", "", None, None)])),
    ("upload_missing_pin", "POST", "/site/drops/abc/upload", MP, mp([("file", b"\x00\x01", "a.bin", None)])),
    ("upload_note_file", "POST", "/site/drops/abc/upload", MP, mp([("note", b"n", "n.txt", "text/plain")])),
    ("upload_header_name_missing", "POST", "/site/drops/abc/upload", MP,
     b"--XyZ\r\nContent-Disposition: form-data\r\n\r\nx\r\n--XyZ--\r\n"),
    ("create_empty", "POST", "/admin/drops", JSON, b"{}"),
    ("create_bad_json", "POST", "/admin/drops", JSON, b'{"label": '),
    ("create_no_body", "POST", "/admin/drops", None, b""),
    ("create_not_object", "POST", "/admin/drops", JSON, b"[1]"),
    ("create_bad_pin", "POST", "/admin/drops", JSON, b'{"label": "x", "pin": "12ab"}'),
    ("create_short_pin_and_more", "POST", "/admin/drops", JSON,
     b'{"label": "", "pin": "12", "max_uploads": 0, "expires_in_hours": 2161, "max_file_mb": "abc"}'),
    ("create_types", "POST", "/admin/drops", JSON,
     b'{"label": 5, "pin": 1234, "max_uploads": 1.5, "expires_in_hours": true, "max_file_mb": "12"}'),
    ("create_unicode_pin", "POST", "/admin/drops", JSON, '{"label": "x", "pin": "١٢٣٤"}'.encode()),
    ("create_long", "POST", "/admin/drops", JSON, ('{"label": "' + "é" * 121 + '", "pin": "1234567890123"}').encode()),
    ("update_bad", "PATCH", "/admin/drops/66f000000000000000000001", JSON,
     b'{"label": "", "max_uploads": 101, "extend_hours": 0, "revoked": "maybe"}'),
    ("update_valid_bad_id_500", "PATCH", "/admin/drops/x", JSON, b'{"label": null, "revoked": "yes", "max_uploads": "7"}'),
    ("update_not_object", "PATCH", "/admin/drops/x", JSON, b'"x"'),
]


def http():
    out = []
    for i, (name, method, url, ctype, body) in enumerate(HTTP):
        hdrs = [(b"x-request-id", f"req_drops_{i:03d}".encode())]
        if ctype:
            hdrs.append((b"content-type", ctype.encode()))
        status, rh, raw = asyncio.run(asgi(app, method, url, hdrs, body))
        if status not in (400, 413, 422) and not name.endswith("_500"):
            raise SystemExit(f"{name}: unexpected {status} {raw!r}")
        out.append({"name": name, "method": method, "url": url, "content_type": ctype,
                    "request_id": f"req_drops_{i:03d}", "body": body.decode("latin-1"), "status": status,
                    "resp_headers": [[k, v] for k, v in rh if k in KEPT], "resp": raw.decode("utf-8")})
    return out


def main():
    data = {
        "env": ENV,
        "filenames": [[n, svc.safe_filename(n)] for n in FILENAMES],
        "ranges": ranges(),
        "stat_headers": stat_headers(),
        "downloads": downloads(),
        "views": views(),
        "http": http(),
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(data, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
