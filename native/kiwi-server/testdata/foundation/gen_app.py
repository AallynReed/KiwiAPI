"""Golden fixtures from pydantic and the real FastAPI app (run AFTER gen_stdlib.py,
whose float/value/datetime corpus it reuses), from the repo root:
    .venv/Scripts/python.exe native/kiwi-server/testdata/foundation/gen_app.py
Writes app.json next to this file.

`http` cases are raw ASGI calls into `app.main.app` itself (runtime_config cache
pre-filled with registry defaults, so no Mongo is needed) and a small app built
with the same error handlers + request-id middleware that exercises every
validation shape the Rust extractors implement.
"""
import asyncio
import datetime as dt
import json
import logging
import os
import struct
import sys
import urllib.parse
import warnings
from enum import Enum
from pathlib import Path
from typing import Any, Literal

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
sys.path.insert(0, os.getcwd())

import bson  # noqa: E402
from beanie import PydanticObjectId  # noqa: E402
from bson.codec_options import CodecOptions  # noqa: E402
from fastapi import Body, FastAPI, Header, HTTPException, Path as PathParam, Query  # noqa: E402
from fastapi.responses import JSONResponse  # noqa: E402
from pydantic import BaseModel, Field, TypeAdapter, field_validator  # noqa: E402

from app.admin import runtime_config as rc  # noqa: E402
from app.core import errors  # noqa: E402
from app.core.errors import APIError, ErrorCode, raise_from_value_error, register_error_handlers  # noqa: E402
from app.core.observability import add_request_context_middleware  # noqa: E402

HERE = Path(__file__).parent
STD = json.loads((HERE / "stdlib.json").read_text(encoding="utf-8"))
ANY = TypeAdapter(Any)
EPOCH = dt.datetime(1970, 1, 1, tzinfo=dt.timezone.utc)


def unbits(h: str) -> float:
    return struct.unpack("<d", struct.pack("<Q", int(h, 16)))[0]


def dec(v):
    if isinstance(v, list):
        return [dec(x) for x in v]
    if isinstance(v, dict):
        if "i" in v:
            return int(v["i"])
        if "f" in v:
            return unbits(v["f"])
        return {k: dec(x) for k, x in v["d"]}
    return v


def pydantic_section():
    floats = [[b, TypeAdapter(float).dump_json(unbits(b)).decode()] for b, _ in STD["float_repr"]]
    dumps = [[c["v"], ANY.dump_json(dec(c["v"])).decode()] for c in STD["dumps"]]
    dts = []
    opts = CodecOptions(tz_aware=True)
    for c in STD["datetimes"]:
        base = EPOCH + dt.timedelta(microseconds=c["us"])
        d = base.astimezone(dt.timezone(dt.timedelta(seconds=c["off"])))
        back = bson.decode(bson.encode({"d": base}), codec_options=opts)["d"]
        dts.append({
            "us": c["us"], "off": c["off"],
            "aware": ANY.dump_json(d).decode().strip('"'),
            "utc_bson_tz": ANY.dump_json(base.astimezone(bson.tz_util.utc)).decode().strip('"'),
            "naive": ANY.dump_json(d.replace(tzinfo=None)).decode().strip('"'),
            "date": ANY.dump_json(d.date()).decode().strip('"'),
            "mongo_us": (back - EPOCH) // dt.timedelta(microseconds=1),
        })
    return floats, dumps, dts


# --- a small app with the production error handlers ------------------------

class Color(str, Enum):
    red = "red"
    green = "green"
    blue = "blue"


class Inner(BaseModel):
    n: int
    tags: list[str] = Field(default_factory=list, max_length=2)


class M(BaseModel):
    name: str = Field(min_length=1, max_length=5)
    age: int = Field(ge=0, le=150)
    score: float | None = Field(default=None, ge=0.05, le=1.0)
    flag: bool = False
    kind: Literal["a", "b"] = "a"
    color: Color = Color.red
    inner: Inner | None = None
    items: list[int] = Field(default_factory=list, max_length=3)
    mp: dict[str, int] = Field(default_factory=dict)
    oid: PydanticObjectId | None = None
    ratio: float = Field(default=0.5, gt=0, lt=1.5)
    n2: int = Field(default=0, multiple_of=5)
    note: str | None = Field(default=None, max_length=10)
    nums: list[float] = Field(default_factory=list)
    req_list: list[str] | None = Field(default=None, min_length=2)

    @field_validator("name")
    @classmethod
    def no_x(cls, v):
        if v == "x":
            raise ValueError("no x allowed")
        return v


def m_json(m: M) -> dict:
    d = m.model_dump(mode="python")
    d["color"] = m.color.value
    d["oid"] = str(m.oid) if m.oid else None
    return d


test_app = FastAPI()
register_error_handlers(test_app)
add_request_context_middleware(test_app)


@test_app.get("/q")
async def q_route(
    limit: int = Query(default=50, ge=1, le=200),
    fmt: str = Query(default="json", pattern="^(json|bin)$"),
    name: str = Query(..., min_length=1, max_length=4),
    f: float | None = Query(default=None, ge=0.05, le=1.0),
    b: bool = Query(default=False),
    kind: Literal["x", "y", "z"] = Query(default="x"),
    color: Color = Query(default=Color.red),
    ids: list[int] = Query(default=[]),
    u: int = Query(..., ge=0, le=0xFFFFFFFF),
    cursor: str | None = Query(default=None, max_length=40),
    oid: PydanticObjectId | None = Query(default=None),
    tags: list[str] = Query(default=[], max_length=2),
):
    return JSONResponse({"limit": limit, "fmt": fmt, "name": name, "f": f, "b": b, "kind": kind,
                         "color": color.value, "ids": ids, "u": u, "cursor": cursor,
                         "oid": str(oid) if oid else None, "tags": tags})


@test_app.get("/p/{oid}/{n}")
async def p_route(oid: PydanticObjectId, n: int = PathParam(..., ge=1)):
    return JSONResponse({"oid": str(oid), "n": n})


@test_app.get("/h")
async def h_route(x_internal_key: str = Header(default=""), x_num: int = Header(default=0)):
    return JSONResponse({"key": x_internal_key, "num": x_num})


@test_app.post("/b")
async def b_route(body: M):
    return JSONResponse({"m": m_json(body)})


@test_app.post("/b2/{n}")
async def b2_route(n: int, body: M, x: int = Query(...)):
    return JSONResponse({"n": n, "x": x, "m": m_json(body)})


@test_app.post("/opt")
async def opt_route(body: M | None = None):
    return JSONResponse({"m": m_json(body) if body else None})


@test_app.post("/lst")
async def lst_route(body: list[int] = Body(..., min_length=1, max_length=3)):
    return JSONResponse({"items": body})


@test_app.post("/dict")
async def dict_route(body: dict[str, Any]):
    return JSONResponse({"d": body})


@test_app.get("/only-get")
async def only_get():
    return JSONResponse({"ok": True})


@test_app.get("/e/{kind}")
async def e_route(kind: str):
    if kind == "http404":
        raise HTTPException(status_code=404)
    if kind == "http404d":
        raise HTTPException(status_code=404, detail="texture not found")
    if kind == "http400":
        raise HTTPException(status_code=400, detail="Bad thing ☕ é   \"q\"")
    if kind == "http422":
        raise HTTPException(status_code=422, detail="Blueprint not renderable: x")
    if kind == "http418":
        raise HTTPException(status_code=418)
    if kind == "http503":
        raise HTTPException(status_code=503, detail="down")
    if kind == "http409":
        raise HTTPException(status_code=409, detail="You already have a creator token.")
    if kind == "http401":
        raise HTTPException(status_code=401, detail="nope")
    if kind == "api":
        raise APIError(403, ErrorCode.ip_not_allowed, "Requests from this IP are not allowed for this token")
    if kind == "api404":
        raise APIError(404, ErrorCode.not_found, "The Mods Hub is currently disabled.")
    if kind == "details":
        raise APIError(429, ErrorCode.rate_limited, "Rate limit exceeded. Slow down and try again later.",
                       details={"limit": 120, "window_seconds": 60, "f": 1e16, "g": 1e-05, "h": 0.1,
                                "s": "é \x7f\x1f", "n": None, "l": [1, 2.0, True]})
    if kind == "adhoc":
        raise APIError(400, "weird_code", "Café")
    if kind == "exists":
        raise_from_value_error(ValueError("Name already exists"))
    if kind == "valueerr":
        raise_from_value_error(ValueError("bad value"))
    if kind == "boom":
        raise RuntimeError("boom")
    return JSONResponse({"kind": kind})


async def call(app, method: str, url: str, headers: list[tuple[bytes, bytes]], body: bytes):
    """One raw ASGI request (TestClient cannot carry non-ASCII response headers)."""
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

    try:
        await app(scope, receive, send)
    except Exception:
        pass
    start = next(m for m in sent if m["type"] == "http.response.start")
    raw = b"".join(m.get("body", b"") for m in sent if m["type"] == "http.response.body")
    return start["status"], [(k.lower(), v) for k, v in start["headers"]], raw


def run(app, cases, prefix):
    out = []
    for i, (name, method, url, headers, body) in enumerate(cases):
        hdrs = [(k.encode(), v if isinstance(v, bytes) else v.encode("latin-1")) for k, v in headers.items()]
        if "x-request-id" not in headers and not name.startswith("noid"):
            hdrs.append((b"x-request-id", f"req_{prefix}_{i:03d}".encode()))
        status, rh, raw = asyncio.run(call(app, method, url, hdrs, body or b""))
        get = lambda k: next((v for n, v in rh if n == k), None)  # noqa: E731
        out.append({
            "name": name, "method": method, "url": url,
            "headers": [[k.decode(), v.hex()] for k, v in hdrs],
            "body_hex": (body or b"").hex(),
            "status": status,
            "content_type": (get(b"content-type") or b"").decode(),
            "allow": (get(b"allow") or b"").decode(),
            "x_request_id_hex": (get(b"x-request-id") or b"").hex(),
            "resp": raw.decode("utf-8"),
        })
    return out


J = {"content-type": "application/json"}


def jb(v) -> bytes:
    return json.dumps(v).encode()


OID = "507f1f77bcf86cd799439011"

TEST_CASES = [
    ("q_ok_min", "GET", "/q?u=1&name=ab", {}, None),
    ("q_missing", "GET", "/q", {}, None),
    ("q_many", "GET", "/q?limit=abc&fmt=xml&name=&f=2&b=maybe&kind=w&color=pink&ids=1&ids=x&u=-1&cursor=" + "c" * 41
     + "&oid=nope&tags=a&tags=b&tags=c", {}, None),
    ("q_bounds", "GET", "/q?limit=0&name=abcde&f=abc&b=YES&u=4294967296", {}, None),
    ("q_floaty", "GET", "/q?limit=1.5&name=ab&f=0.01&u=1e3", {}, None),
    ("q_lax", "GET", "/q?limit=+5&name=ab&u=%201%20&f=5e-1&b=on&ids=007&ids=-0&kind=z&color=blue&oid=" + OID.upper(), {}, None),
    ("q_unders", "GET", "/q?limit=1_000&name=ab&u=007", {}, None),
    ("q_big", "GET", "/q?limit=9999999999999999999999&name=ab&u=1.0", {}, None),
    ("q_repeat_last_wins", "GET", "/q?limit=abc&limit=7&name=x&name=yy&u=3", {}, None),
    ("q_plus_pct", "GET", "/q?name=a+b&u=2&cursor=%E2%82%AC%zz%41+", {}, None),
    ("q_bad_utf8", "GET", "/q?name=%FF%FE&u=2", {}, None),
    ("q_f_str", "GET", "/q?name=a&u=2&f=1_0.5&b=0", {}, None),
    ("q_f_ok", "GET", "/q?name=a&u=2&f=.5&b=FALSE", {}, None),
    ("q_blank", "GET", "/q?name=a&u=2&&cursor&tags=", {}, None),
    ("p_bad", "GET", "/p/abc/0", {}, None),
    ("p_ok", "GET", f"/p/{OID}/12", {}, None),
    ("p_int", "GET", f"/p/{OID}/x", {}, None),
    ("h_ok", "GET", "/h", {"x-internal-key": "k1", "x-num": " 42 "}, None),
    ("h_bad", "GET", "/h", {"x-num": "4x"}, None),
    ("h_latin1", "GET", "/h", {"x-internal-key": b"caf\xe9"}, None),
    ("b_many", "POST", "/b", J, jb({"name": "", "age": -1, "score": 5, "flag": "nah", "kind": "c", "color": "pink",
                                    "inner": {"tags": ["a", "b", "c"]}, "items": [1, "x", 2.5, 4],
                                    "mp": {"a": "x", "b": 2, "c": 1.5}, "oid": 5, "ratio": 0, "n2": 7,
                                    "note": "y" * 11, "nums": [1, "x", True, None], "req_list": ["a"]})),
    ("b_validator", "POST", "/b", J, jb({"name": "x", "age": 1.5})),
    ("b_types", "POST", "/b", J, jb({"name": "abcdef", "age": "12", "inner": 5, "items": "x", "mp": [], "ratio": 2,
                                     "n2": "10", "req_list": "ab"})),
    ("b_list", "POST", "/b", J, jb([1, 2])),
    ("b_str", "POST", "/b", J, jb("str")),
    ("b_empty", "POST", "/b", {}, b""),
    ("b_no_ct", "POST", "/b", {}, b'{"name":"a"}'),
    ("b_text_ct", "POST", "/b", {"content-type": "text/plain"}, b'{"name":"a"}'),
    ("b_bad_utf8", "POST", "/b", J, b'{"name":"a\xff"}'),
    ("b_bad_json", "POST", "/b", J, b'{"name":"a" "age":1}'),
    ("b_bad_json_unicode", "POST", "/b", J, '{"name":"é\U0001F600", age}'.encode()),
    ("b_nan", "POST", "/b", J, b'{"name": NaN, "age": 1}'),
    ("b_null", "POST", "/b", J, b"null"),
    ("b_nested_inner_nulls", "POST", "/b", J, jb({"name": None, "age": None, "flag": None, "items": None, "inner": None})),
    ("b_lax_ok", "POST", "/b", J, jb({"name": "ok", "age": True, "flag": 1, "items": [True, 2.0, "3"], "score": "0.5",
                                      "mp": {"k": "7"}, "oid": OID, "ratio": "1_0e-1", "n2": 15.0, "note": None,
                                      "nums": [1, 2.5, "3", False], "req_list": ["a", "b"], "kind": "b",
                                      "inner": {"n": "4", "tags": []}})),
    ("b_vnd_json", "POST", "/b", {"content-type": "application/vnd.api+json; charset=utf-8"}, jb({"name": "ok", "age": 3})),
    ("b_ct_upper", "POST", "/b", {"content-type": "Application/JSON"}, jb({"age": 3})),
    ("b_ct_weird", "POST", "/b", {"content-type": "application/json/x"}, jb({"age": 3})),
    ("b_utf16", "POST", "/b", J, json.dumps({"name": "ok", "age": 2}).encode("utf-16")),
    ("b_bom", "POST", "/b", J, b"\xef\xbb\xbf" + jb({"name": "ok", "age": 2})),
    ("b_dup_keys", "POST", "/b", J, b'{"name":"a","age":1,"name":"bb"}'),
    ("b_deep_err", "POST", "/b", J, b'{"name":"ok","age":1,"inner":{"n":1,"tags":[1]}}'),
    ("b2_order", "POST", "/b2/zz?x=q", J, jb({"age": True})),
    ("b2_ok", "POST", "/b2/3?x=4", J, jb({"name": "ok", "age": 1})),
    ("b2_badjson_first", "POST", "/b2/zz?x=q", J, b"[1,"),
    ("opt_empty", "POST", "/opt", {}, b""),
    ("opt_null", "POST", "/opt", J, b"null"),
    ("opt_bad", "POST", "/opt", J, b"{}"),
    ("lst_ok", "POST", "/lst", J, b"[1, 2]"),
    ("lst_short", "POST", "/lst", J, b"[]"),
    ("lst_long", "POST", "/lst", J, b'[1, "x", 3, 4]'),
    ("lst_items", "POST", "/lst", J, b'[1, "x", 2.5]'),
    ("lst_type", "POST", "/lst", J, b'{"a": 1}'),
    ("dict_ok", "POST", "/dict", J, b'{"a": [1, 2.5, "\\u00e9"], "b": {"c": null}}'),
    ("dict_bad", "POST", "/dict", J, b"[1]"),
    ("e_http404", "GET", "/e/http404", {}, None),
    ("e_http404d", "GET", "/e/http404d", {}, None),
    ("e_http404d_html", "GET", "/e/http404d", {"accept": "text/html"}, None),
    ("e_api404_html", "GET", "/e/api404", {"accept": "text/html"}, None),
    ("e_http400", "GET", "/e/http400", {}, None),
    ("e_http422", "GET", "/e/http422", {}, None),
    ("e_http418", "GET", "/e/http418", {}, None),
    ("e_http503", "GET", "/e/http503", {}, None),
    ("e_http409", "GET", "/e/http409", {}, None),
    ("e_http401", "GET", "/e/http401", {}, None),
    ("e_api", "GET", "/e/api", {}, None),
    ("e_details", "GET", "/e/details", {}, None),
    ("e_adhoc", "GET", "/e/adhoc", {}, None),
    ("e_exists", "GET", "/e/exists", {}, None),
    ("e_valueerr", "GET", "/e/valueerr", {}, None),
    ("e_boom", "GET", "/e/boom", {}, None),
    ("method_405", "POST", "/only-get", {}, None),
    ("unknown_json", "GET", "/nope", {}, None),
    ("unknown_html", "GET", "/nope/deeper", {"accept": "text/html,application/xhtml+xml"}, None),
    ("unknown_api_html", "GET", "/v1/nope", {"accept": "text/html"}, None),
    ("unknown_v1x_html", "GET", "/v1x", {"accept": "text/html"}, None),
    ("unknown_post_html", "POST", "/nope", {"accept": "text/html"}, None),
    ("rid_latin1", "GET", "/nope", {"x-request-id": b"caf\xe9 \xff"}, None),
    ("rid_empty", "GET", "/nope", {"x-request-id": ""}, None),
    ("noid_generated", "GET", "/nope", {}, None),
]

REAL_CASES = [
    ("unknown_json", "GET", "/nope", {}, None),
    ("unknown_html", "GET", "/nope", {"accept": "text/html"}, None),
    ("unknown_v1_html", "GET", "/v1/nope", {"accept": "text/html"}, None),
    ("unknown_site_html", "GET", "/site/nope", {"accept": "text/html"}, None),
    ("health_405", "POST", "/health", {}, None),
    ("health_ok", "GET", "/health", {}, None),
    ("login_bad_json", "POST", "/auth/login", J, b"{bad"),
    ("login_bad_json2", "POST", "/auth/login", J, b'{"email": "a@b.co" "password": 1}'),
    ("login_ctrl", "POST", "/auth/login", J, b'{"email": "\x01"}'),
    ("login_bad_utf8", "POST", "/auth/login", J, b'{"email": "\xc3"}'),
    ("login_nan", "POST", "/auth/login", J, b'{"email": NaN}'),
    ("login_missing", "POST", "/auth/login", J, jb({"email": "user@gmail.com"})),
    ("login_empty_obj", "POST", "/auth/login", J, b"{}"),
    ("login_list", "POST", "/auth/login", J, b"[]"),
    ("login_no_ct", "POST", "/auth/login", {}, jb({"email": "user@gmail.com", "password": "x"})),
    ("signup_lengths", "POST", "/auth/signup", J, jb({"email": "user@gmail.com", "password": "short", "display_name": "x" * 81})),
    ("signup_types", "POST", "/auth/signup", J, jb({"email": "user@gmail.com", "password": 12345678, "captcha_token": 5})),
    ("stat_range", "GET", "/site/gems/stat-range?tier=x&type=1&level=0&extra_containers=-1&element=e", {}, None),
    ("stat_range_missing", "GET", "/site/gems/stat-range", {}, None),
    ("news_limit", "GET", "/site/feeds/news?limit=0", {}, None),
    ("star_chart_long", "GET", "/site/gems/parse-star-chart?code=" + "a" * 8193, {}, None),
    ("eval_simple", "POST", "/site/gems/evaluate-simple", J, jb({"tier": "four", "level": 1.5, "power_rank": None})),
    ("eval_stats", "POST", "/site/gems/evaluate", J, jb({"tier": 4, "stats": [{"x": 1}, 5, {"type": 1, "value": "v"}]})),
    ("eval_missing_stats", "POST", "/site/gems/evaluate", J, jb({"auto_guess_procs": "maybe"})),
]


def http_section():
    for key, spec in rc.REGISTRY.items():
        rc._CACHE[key] = (spec.default, float("inf"))
    from app.main import app as real_app

    return run(test_app, TEST_CASES, "t"), run(real_app, REAL_CASES, "r")


def main():
    floats, dumps, dts = pydantic_section()
    test, real = http_section()
    data = {
        "python": sys.version.split()[0],
        "pydantic_float": floats,
        "pydantic_dumps": dumps,
        "pydantic_datetimes": dts,
        "error_codes": [c.value for c in ErrorCode],
        "status_to_code": {str(k): v.value for k, v in errors._STATUS_TO_CODE.items()},
        "not_found_html": errors._NOT_FOUND_HTML,
        "http_test_app": test,
        "http_real_app": real,
    }
    out = HERE / "app.json"
    out.write_text(json.dumps(data, ensure_ascii=True, separators=(",", ":")) + "\n", encoding="utf-8")
    print(f"wrote {out} ({out.stat().st_size} bytes) with Python {data['python']}")


if __name__ == "__main__":
    main()
