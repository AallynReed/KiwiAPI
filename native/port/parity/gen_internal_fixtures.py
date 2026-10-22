"""Golden fixtures for the api's /internal routes (page-view ingest and the
regional maintenance list), from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_internal_fixtures.py
Writes native/kiwi-server/testdata/internal/golden.json.

Every reply comes from the real app (raw ASGI). The page_views collection is a
recorder, so the inserted documents and per-pid updates are captured as Python
builds them. Country codes are placeholders, never a configured value.
"""
import asyncio
import datetime as dt
import json
import logging
import os
import sys
import types
import urllib.parse
import warnings
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
sys.path.insert(0, os.getcwd())

ENV = {"SECRET_KEY": "internal-fixture-secret", "INTERNAL_KEY": "k3y-for-fixtures"}
os.environ.update(ENV)

from bson.json_util import CANONICAL_JSON_OPTIONS, dumps as bson_dumps  # noqa: E402

from app.admin import runtime_config as rc  # noqa: E402

for _key, _spec in rc.REGISTRY.items():
    rc._CACHE[_key] = (_spec.default, float("inf"))

import app.pageviews.ingest as ingest  # noqa: E402
from app.geo_maintenance.router import parse_countries  # noqa: E402
from app.main import app  # noqa: E402

OUT = Path(__file__).resolve().parents[2] / "kiwi-server" / "testdata" / "internal" / "golden.json"
NOW = dt.datetime(2026, 10, 22, 11, 0, 0, 123456, tzinfo=dt.timezone.utc)
KEY = ENV["INTERNAL_KEY"]


def ext(v):
    return json.loads(bson_dumps(v, json_options=CANONICAL_JSON_OPTIONS))


class Recorder:
    def __init__(self):
        self.inserted = []
        self.updates = []

    async def insert_many(self, docs, ordered=True):
        self.inserted.append({"docs": ext(docs), "ordered": ordered})

    async def bulk_write(self, ops, ordered=True):
        self.updates.append({"ops": [ext({"filter": op._filter, "update": op._doc}) for op in ops],
                             "ordered": ordered})


REC = Recorder()
ingest._collection = lambda: REC
ingest.utcnow = lambda: NOW


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


def ev(**kw):
    return kw


GOOD_BATCH = {"events": [
    ev(kind="pv", pid="p-0001-aaaa", route="/mods/{handle}/{slug}", path="/mods/aallyn/thing ✓",
       visitor_hash="0123456789abcdef0123456789abcdef", channel="search", source="Google", device="mobile",
       browser="Chrome", os="Android", lang="en", engaged_s=99, perf={"ttfb": 5}),
    ev(pid="p-0002-bbbb", route="/player/{name}", path="/player/x", visitor_hash="short"),
    ev(kind="pv", route="", path="/x", visitor_hash="0123456789"),
    ev(kind="pv", route="/r", path="/p", visitor_hash="01234567"),
    ev(kind="end", pid="p-0001-aaaa", engaged_s=40),
    ev(kind="perf", pid="p-0001-aaaa", perf={"ttfb": 120, "fcp": 300, "cls": 25}),
    ev(kind="end", pid="p-0001-aaaa", engaged_s=12),
    ev(kind="perf", pid="p-0001-aaaa", perf={"lcp": 900, "cls": 10, "fcp": 0}),
    ev(kind="perf", pid="p-0003-cccc"),
    ev(kind="end", pid="p-0003-cccc", engaged_s=7),
    ev(kind="perf", pid="p-0004-dddd", perf={"dcl": 1, "load": 2}),
    ev(kind="end", pid=""),
    ev(kind="end"),
    ev(kind="perf", pid="p-0005-eeee", perf={}),
]}

H_KEY = [(b"x-internal-key", KEY.encode())]
JSON = [(b"content-type", b"application/json")]

CASES = [
    ("ingest_no_key", "POST", "/internal/pageviews", JSON, GOOD_BATCH),
    ("ingest_wrong_key", "POST", "/internal/pageviews", JSON + [(b"x-internal-key", b"nope")], GOOD_BATCH),
    ("ingest_non_ascii_key", "POST", "/internal/pageviews", JSON + [(b"x-internal-key", "k\xe9y".encode("latin-1"))],
     GOOD_BATCH),
    ("ingest_ok", "POST", "/internal/pageviews", JSON + H_KEY, GOOD_BATCH),
    ("ingest_empty", "POST", "/internal/pageviews", JSON + H_KEY, {"events": []}),
    ("ingest_bad_no_key", "POST", "/internal/pageviews", JSON, {"events": [{"kind": "click"}]}),
    ("ingest_missing_events", "POST", "/internal/pageviews", JSON + H_KEY, {}),
    ("ingest_no_body", "POST", "/internal/pageviews", H_KEY, None),
    ("ingest_bad_json", "POST", "/internal/pageviews", JSON + H_KEY, b'{"events": ['),
    ("ingest_not_list", "POST", "/internal/pageviews", JSON + H_KEY, {"events": {"a": 1}}),
    ("ingest_too_many", "POST", "/internal/pageviews", JSON + H_KEY, {"events": [{}] * 501}),
    ("ingest_too_many_bad", "POST", "/internal/pageviews", JSON + H_KEY, {"events": [{"kind": 1}] * 501}),
    ("ingest_bad_fields", "POST", "/internal/pageviews", JSON + H_KEY, {"events": [
        {"kind": "click", "pid": "x" * 41, "route": 5, "engaged_s": 86401, "channel": "c" * 81,
         "perf": {"ttfb": -1, "cls": 10001, "lcp": "fast", "fcp": 1.5}},
        {"perf": "nope", "engaged_s": "12", "visitor_hash": None},
        "not an object",
    ]}),
    ("ingest_extra_fields", "POST", "/internal/pageviews", JSON + H_KEY,
     {"events": [{"kind": "end", "pid": "p-9", "engaged_s": "15", "unknown": 1}], "more": True}),
    ("geo_no_key", "GET", "/internal/geo-maintenance", [], None),
    ("geo_wrong_key", "GET", "/internal/geo-maintenance", [(b"x-internal-key", b"k3y-for-fixtures ")], None),
    ("geo_ok_default", "GET", "/internal/geo-maintenance", H_KEY, None),
    ("geo_wrong_method", "POST", "/internal/geo-maintenance", H_KEY, None),
    ("ingest_wrong_method", "GET", "/internal/pageviews", H_KEY, None),
]


def http():
    out = []
    for i, (name, method, url, headers, body) in enumerate(CASES):
        REC.inserted.clear()
        REC.updates.clear()
        raw_body = body if isinstance(body, bytes) else (b"" if body is None else json.dumps(body).encode())
        hdrs = headers + [(b"x-request-id", f"req_internal_{i:03d}".encode())]
        status, rh, raw = asyncio.run(asgi(method, url, hdrs, raw_body))
        out.append({
            "name": name, "method": method, "url": url, "request_id": f"req_internal_{i:03d}",
            "headers": [[k.decode("latin-1"), v.decode("latin-1")] for k, v in headers],
            "body": raw_body.decode("utf-8"), "status": status,
            "resp_headers": [[k, v] for k, v in rh if k in ("content-type", "allow")],
            "resp": raw.decode("utf-8"),
            "inserted": list(REC.inserted), "updates": list(REC.updates),
        })
    return out


COUNTRY_INPUTS = ["", " ", "zz", "ZZ, qq ,zz,,  ", "xa,XB , xa", "  q1 , ,Q1"]


def main():
    data = {
        "env": ENV,
        "now": NOW.isoformat(),
        "http": http(),
        "countries": [[raw, parse_countries(raw)] for raw in COUNTRY_INPUTS],
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(data, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
