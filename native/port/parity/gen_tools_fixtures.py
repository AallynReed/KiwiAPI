"""Golden fixtures for the stateless file tools (/v1/mods/read, /v1/mods/build,
/site/unlock-debug), from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_tools_fixtures.py
Writes native/kiwi-server/testdata/tools/golden.json.

The routes run in the real app with their token / feature / throttle
dependencies overridden, so every reply after auth is Python's own. The codec
helpers (utf-8 errors, strict base64) are recorded straight from CPython.
"""
import base64
import hashlib
import json
import logging
import os
import sys
import warnings
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
sys.path.insert(0, os.getcwd())
os.environ.setdefault("SECRET_KEY", "tools-fixture-secret")

from fastapi.testclient import TestClient  # noqa: E402

from app.admin import runtime_config as rc  # noqa: E402

for _key, _spec in rc.REGISTRY.items():
    rc._CACHE[_key] = (_spec.default, float("inf"))

import app.site.router as site_router  # noqa: E402
import app.trove.router as trove_router  # noqa: E402
from app.main import app  # noqa: E402
from app.trove import tmod  # noqa: E402
from app.trove.troveio import write_leb128  # noqa: E402

assert tmod._native is None, "generate with the pure-Python reference"
OUT = Path(__file__).resolve().parents[2] / "kiwi-server" / "testdata" / "tools" / "golden.json"
KEPT = ("content-type", "content-disposition", "cache-control")
BIG = 4096


def lcg(seed, n):
    x = seed & 0xFFFFFFFF
    out = bytearray()
    for _ in range(n):
        x = (x * 1103515245 + 12345) & 0x7FFFFFFF
        out.append((x >> 16) & 0xFF)
    return bytes(out)


def body_record(raw: bytes) -> dict:
    if len(raw) <= BIG:
        return {"body_b64": base64.b64encode(raw).decode()}
    return {"body_sha256": hashlib.sha256(raw).hexdigest(), "body_len": len(raw)}


def reply(r) -> dict:
    return {"status": r.status_code, "headers": {k: r.headers[k] for k in KEPT if k in r.headers},
            **body_record(r.content)}


# --- codecs --------------------------------------------------------------------

UTF8 = [b"", b"abc", b"\xff", b"a\x80b", b"\xc2", b"\xc2A", b"\xe0\x80\x80", b"\xe0\xa0", b"\xe0\xa0A",
        b"\xed\xa0\x80", b"\xed\x9f\xbf", b"\xf0\x90\x80", b"\xf0\x8f\xbf\xbf", b"\xf4\x90\x80\x80",
        b"\xf4\x8f\xbf", b"\xf0\x90A", b"\xf0\x90\x80A", b"\xf5", b"\xc0\xaf", b"ok\xe2\x82\xacok\xe2\x82",
        b"\xe2\x82\xac\xf0\x9f\x98\x80", b"x\xf0\x9f\x98", b"\xf1\x80\x80\x80\x80\xff"]

B64 = ["", "QQ==", "QUI=", "QUJD", "=QUJD", "QUJD=", "QU==JD", "QU=", "Q", "QUJDR", "QUJD RA==", "QUJD\nRA==",
       "QUJD!", "QQ==QQ==", "QQ=a", "QUJ=x", "QQ===", "Q===", "QUI==", "ÄQQ==", "QQ", "QUJ", "abc_-", "AAAAAAA",
       "AAAAA===", "QUJDRA==\n"]


def codecs():
    utf8 = []
    for b in UTF8:
        try:
            b.decode("utf-8")
            utf8.append({"b64": base64.b64encode(b).decode(), "error": None})
        except UnicodeDecodeError as e:
            utf8.append({"b64": base64.b64encode(b).decode(), "error": str(e)})
    b64 = []
    for s in B64:
        try:
            b64.append({"s": s, "ok": base64.b64encode(base64.b64decode(s, validate=True)).decode()})
        except ValueError as e:
            b64.append({"s": s, "error": str(e)})
    return {"utf8": utf8, "b64": b64}


# --- tmod inputs ---------------------------------------------------------------


def raw_tmod(props, entries, stream, *, char_lengths=False, header_size=None, version=1):
    """A hand-framed tmod: ``entries`` are (path, index, offset, size, checksum)."""
    def s(text):
        b = text.encode("utf-8")
        return write_leb128(len(text) if char_lengths else len(b)) + b

    h = bytearray(8) + version.to_bytes(2, "little") + len(props).to_bytes(2, "little")
    for k, v in props:
        h += s(k) + s(v)
    for path, idx, off, size, chk in entries:
        b = path.encode("utf-8")
        h += bytes([len(path) if char_lengths else len(b)]) + b
        h += write_leb128(idx) + write_leb128(off) + write_leb128(size) + write_leb128(chk)
    h[0:8] = (header_size if header_size is not None else len(h)).to_bytes(8, "little")
    return bytes(h) + stream


def framed(files):
    """Trove's stored-block framing: 7-byte head, 32 KiB chunks split by 5-byte
    markers that lie about their lengths, 5-byte tail."""
    stream = bytearray()
    entries = []
    for path, content in files:
        size = len(content)
        entries.append((path, 0, len(stream) if size else 0, size, tmod.calculate_hash(content)))
        stream += content + b"\x00" * ((4 - size % 4) % 4)
    out = bytearray(b"\x78\x01\x00\xff\xff\x00\x00")
    for i in range(0, len(stream), 32768):
        if i:
            out += b"\x00\x13\x37\xec\xc8"
        out += stream[i:i + 32768]
    out += b"\x00\x00\x00\xff\xff"
    return entries, bytes(out)


def stream_of(files):
    built = tmod.build_tmod(1, {}, files)
    return built[int.from_bytes(built[:8], "little"):]


def tmod_inputs():
    files = [("data/a.txt", b"hello"), ("empty.bin", b""), ("x/one", b"1"), ("Mods/ÜBER.cfg", "été".encode())]
    built = tmod.build_tmod(1, {"title": "My Mod", "author": "Kiwi", "tags": "Dragons, fishing,bogus",
                                "notes": "naïve ✓"}, files)
    big = tmod.build_tmod(7, {"title": "Big"}, [("blob/big.bin", lcg(7, 70001)), ("blob/small", lcg(9, 33))])
    out = {
        "built": built,
        "big": big,
        "dupe_props": raw_tmod([("a", "1"), ("b", "2"), ("a", "3")], [("f", 0, 0, 3, tmod.calculate_hash(b"abc"))],
                               stream_of([("f", b"abc")])),
        "btt_chars": raw_tmod([("title", "Café"), ("notes", "größer ✓ — ok")],
                              [("ui/é.swf", 0, 0, 4, tmod.calculate_hash(b"swf!"))],
                              b"", char_lengths=True),
        "too_small": b"\x00" * 11,
        "header_outside": (999).to_bytes(8, "little") + b"\x01\x00\x00\x00",
        "header_below_12": (5).to_bytes(8, "little") + b"\x01\x00\x00\x00",
        "bad_utf8_prop": raw_tmod([("t\xff", "v")], [], b"").replace("tÿ".encode(), b"t\xff\xfe"),
        "truncated_varint": (13).to_bytes(8, "little") + b"\x01\x00\x01\x00" + b"\x85",
        "varint_too_long": (30).to_bytes(8, "little") + b"\x01\x00\x01\x00" + b"\x80" * 18,
        "table_overrun": raw_tmod([("k", "v")], [("p", 0, 0, 0, 0)], b"", header_size=17),
        "string_past_end": (40).to_bytes(8, "little") + b"\x01\x00\x01\x00" + b"\x7fabc" + b"\x00" * 24,
        "bad_cont": raw_tmod([("k", "v")], [], b"").replace(b"\x01v", b"\x02\xe2(") + b"",
        "no_files": raw_tmod([("title", "x")], [], b"x\x9c\x03\x00\x00\x00\x00\x01"),
    }
    entries, stream = framed([("ui/big.swf", lcg(3, 40000)), ("ui/tiny", b"abcde")])
    out["framed"] = raw_tmod([("title", "Framed")], entries, stream)
    out["framed_metadata"] = out["framed"]
    garbage = bytearray(out["built"])
    garbage[-3] ^= 0x55
    out["corrupt_stream"] = bytes(garbage)
    return out


TMOD_RECIPES = {
    "big": {"version": 7, "properties": [["title", "Big"]],
            "files": [["blob/big.bin", 7, 70001], ["blob/small", 9, 33]]},
}

# --- routes ----------------------------------------------------------------------

client = TestClient(app, raise_server_exceptions=False)
app.dependency_overrides[trove_router._MODS.dependency] = lambda: None
app.dependency_overrides[site_router._unlock_debug_throttle] = lambda: None


def read_cases(inputs):
    cases = []
    for name, data in inputs.items():
        for meta in ((True,) if name == "framed_metadata" else (False, True)):
            r = client.post("/v1/mods/read" + ("?metadata_only=true" if meta else ""), content=data,
                            headers={"Content-Type": "application/octet-stream"})
            rec = {"input": name, "query": "metadata_only=true" if meta else None,
                   "input_sha256": hashlib.sha256(data).hexdigest(), **reply(r)}
            if len(data) <= BIG:
                rec["input_b64"] = base64.b64encode(data).decode()
            cases.append(rec)
    extra = [
        ("json_string", "", {"Content-Type": "application/json"}, json.dumps(base64.b64encode(b"x").decode())),
        ("json_empty_string", "", {"Content-Type": "application/json"}, '""'),
        ("json_number", "", {"Content-Type": "application/json"}, "12"),
        ("json_null", "", {"Content-Type": "application/json"}, "null"),
        ("json_object", "?metadata_only=maybe", {"Content-Type": "application/json"}, '{"a": [1, "\u00e9"]}'),
        ("json_bad", "", {"Content-Type": "application/json"}, '{"a": '),
        ("no_body", "", {}, ""),
        ("no_body_bad_bool", "?metadata_only=nah", {}, ""),
        ("bool_yes", "?metadata_only=YES", {"Content-Type": "text/plain"}, inputs["built"]),
        ("bool_off", "?metadata_only=off&metadata_only=1", {}, inputs["built"]),
        ("no_ctype_raw", "", {}, inputs["built"]),
    ]
    for name, query, headers, body in extra:
        content = body if isinstance(body, bytes) else body.encode()
        r = client.post("/v1/mods/read" + query, content=content, headers=headers)
        cases.append({"input": None, "name": name, "query": query, "req_headers": headers,
                      "req_body_b64": base64.b64encode(content).decode(), **reply(r)})
    return cases


def b64(b):
    return base64.b64encode(b).decode()


BUILD = [
    ("basic", {"version": 2, "properties": {"title": "Shiny: Wings!! (v2)", "author": "K", "tags": "Wings,GUI"},
               "files": [{"path": "\\UI\\Hud.SWF", "content_base64": b64(b"swfdata")},
                         {"path": "//Mods/My Mod.CFG", "content_base64": b64(b"[cfg]\n")},
                         {"path": "data/Empty", "content_base64": ""}]}),
    ("loader_in_place", {"properties": {"modLoader": "BTT", "title": "   ", "flags": "7", "tags": "Dragons"},
                         "files": [{"path": "a", "content_base64": b64(b"abc")}]}),
    ("no_title", {"properties": {"tags": "nothing,here"}, "files": [{"path": "ÄÖ/ü.TXT", "content_base64": b64(b"z")}]}),
    ("long_title", {"properties": {"title": "x" * 120 + "é"}, "files": [{"path": "p", "content_base64": "QQ=="}]}),
    ("version_str", {"version": "3", "files": [{"path": "p", "content_base64": "QUJD"}]}),
    ("version_float", {"version": 4.0, "files": [{"path": "p", "content_base64": "QUJD"}]}),
    ("version_big", {"version": 70000, "files": [{"path": "p", "content_base64": "QUJD"}]}),
    ("version_neg", {"version": -1, "files": [{"path": "p", "content_base64": "QUJD"}]}),
    ("version_huge", {"version": 10 ** 30, "files": [{"path": "p", "content_base64": "QUJD"}]}),
    ("version_bool", {"version": True, "files": [{"path": "p", "content_base64": "QUJD"}]}),
    ("no_files", {"files": []}),
    ("path_255", {"files": [{"path": "a" * 255, "content_base64": ""}]}),
    ("path_too_long", {"files": [{"path": "/" + "é" * 128, "content_base64": ""}]}),
    ("bad_b64", {"files": [{"path": "ok", "content_base64": "QQ=="}, {"path": "Bad", "content_base64": "QQ=x"}]}),
    ("bad_b64_ascii", {"files": [{"path": "p", "content_base64": "ü"}]}),
    ("bad_b64_len", {"files": [{"path": "p", "content_base64": "QUJDR"}]}),
    ("b64_before_paths", {"files": [{"path": "x" * 300, "content_base64": "QQ="}]}),
    ("prop_not_str", {"properties": {"a": 1, "b": None}, "files": [{"path": "p", "content_base64": ""}]}),
    ("files_missing", {"properties": {}}),
    ("file_fields_missing", {"files": [{"path": "p"}, {"content_base64": ""}, "x"]}),
    ("not_object", [1, 2]),
    ("version_frac", {"version": 1.5, "files": [{"path": "p", "content_base64": ""}]}),
    ("big", {"version": 7, "properties": {"title": "Big"},
             "files": [{"path": "blob/big.bin", "content_base64": b64(lcg(7, 70001))},
                       {"path": "blob/small", "content_base64": b64(lcg(9, 33))}]}),
]


def build_cases():
    cases = []
    for name, body in BUILD:
        raw = json.dumps(body).encode()
        r = client.post("/v1/mods/build", content=raw, headers={"Content-Type": "application/json"})
        rec = {"name": name, **reply(r)}
        rec["req_body"] = None if name == "big" else body
        cases.append(rec)
    for name, raw, headers in [("bad_json", b'{"files": [', {"Content-Type": "application/json"}),
                               ("empty", b"", {"Content-Type": "application/json"}),
                               ("text_plain", b'{"files": []}', {"Content-Type": "text/plain"})]:
        r = client.post("/v1/mods/build", content=raw, headers=headers)
        cases.append({"name": name, "req_raw_b64": base64.b64encode(raw).decode(), "req_headers": headers,
                      **reply(r)})
    return cases


def filenames():
    names = ["My Mod", "  ", "", "a/b\\c:d*e?f\"g<h>i|j", "Ünïcödé ✓ Mod", "x" * 100, " lead.trail. ", "-_.",
             "\u3000wide\u3000", "tab\tname"]
    return [{"in": n, "out": trove_router._safe_filename(n)} for n in names]


EXE = b"MZ\x90\x00" + b"\x00" * 60 + b"\x7c\x39\x68\xe0\x02\x00\x00" + b"middle" + b"\x7c\x39\x68\xe0\x02\x00\x00" + b"end"


def multipart(parts, boundary="kiwiBoundary7x"):
    out = bytearray()
    for name, filename, ctype, data in parts:
        out += f"--{boundary}\r\n".encode()
        disp = f'form-data; name="{name}"'
        if filename is not None:
            disp += f'; filename="{filename}"'
        out += f"Content-Disposition: {disp}\r\n".encode()
        if ctype:
            out += f"Content-Type: {ctype}\r\n".encode()
        out += b"\r\n" + data + b"\r\n"
    out += f"--{boundary}--\r\n".encode()
    return bytes(out), f"multipart/form-data; boundary={boundary}"


UNLOCK = [
    ("patched", [("trove_exe", "Trove.exe", "application/octet-stream", EXE)]),
    ("last_wins", [("trove_exe", "a.exe", None, b"nope"), ("trove_exe", "Trove.exe", None, EXE)]),
    ("not_mz", [("trove_exe", "Trove.exe", None, b"ZM" + EXE[2:])]),
    ("no_sequence", [("trove_exe", "Trove.exe", None, b"MZ" + b"\x7c\x39\x68\xe0\x02\x00")]),
    ("empty_file", [("trove_exe", "Trove.exe", None, b"")]),
    ("no_filename", [("trove_exe", "", None, EXE)]),
    ("missing", [("other", "x.exe", None, EXE)]),
    ("text_field", [("trove_exe", None, None, b"just text")]),
    ("empty_text", [("trove_exe", None, None, b"")]),
]


FLAG = {"on": True}


async def _is_enabled(flag):
    return FLAG["on"] if flag == "feature_unlock_debug_enabled" else True


site_router.feature_flags.is_enabled = _is_enabled


def unlock_cases():
    cases = []
    for flag in (True, False):
        FLAG["on"] = flag
        for name, parts in UNLOCK if flag else UNLOCK[:1]:
            body, ctype = multipart(parts)
            r = client.post("/site/unlock-debug", content=body, headers={"Content-Type": ctype})
            cases.append({"name": name, "enabled": flag, "req_b64": base64.b64encode(body).decode(),
                          "req_ctype": ctype, **reply(r)})
    FLAG["on"] = True
    for name, body, ctype in [("urlencoded", b"trove_exe=abc", "application/x-www-form-urlencoded"),
                              ("json", b'{"trove_exe": 1}', "application/json"),
                              ("no_boundary", b"--x--", "multipart/form-data"),
                              ("none", b"", None)]:
        headers = {"Content-Type": ctype} if ctype else {}
        r = client.post("/site/unlock-debug", content=body, headers=headers)
        cases.append({"name": name, "enabled": True, "req_b64": base64.b64encode(body).decode(), "req_ctype": ctype,
                      **reply(r)})
    return cases


app.dependency_overrides[trove_router._OCR.dependency] = lambda: None
PNG_1PX = bytes.fromhex("89504e470d0a1a0a0000000d4948445200000001000000010802000000907753de0000000c4944415408d763f8ffff3f0005fe02fea7d6a4f30000000049454e44ae426082")

OCR_ERRORS = [
    ("unsupported", [("file", "a.txt", "text/plain", b"hi")]),
    ("type_params", [("file", "a.png", "image/PNG; charset=x", PNG_1PX)]),
    ("upper_ok_but_empty", [("file", "a.png", "IMAGE/PNG", b"")]),
    ("empty", [("file", "a.png", "image/png", b"")]),
    ("missing", [("other", "a.png", "image/png", PNG_1PX)]),
    ("text_field", [("file", None, None, b"words")]),
    ("empty_text", [("file", None, None, b"")]),
    ("garbage", [("file", "a.png", "image/png", b"definitely not an image")]),
    ("garbage_no_type", [("file", "a.bin", None, b"\x00\x01\x02")]),
]


def ocr_cases():
    cases = []
    for name, parts in OCR_ERRORS:
        body, ctype = multipart(parts)
        r = client.post("/v1/ocr/character", content=body, headers={"Content-Type": ctype})
        cases.append({"name": name, "req_b64": base64.b64encode(body).decode(), "req_ctype": ctype, **reply(r)})
    for name, body, ctype in [("json", b'{"file": 1}', "application/json"), ("no_boundary", b"--x--", "multipart/form-data")]:
        r = client.post("/v1/ocr/character", content=body, headers={"Content-Type": ctype})
        cases.append({"name": name, "req_b64": base64.b64encode(body).decode(), "req_ctype": ctype, **reply(r)})
    return cases


def main():
    inputs = tmod_inputs()
    golden = {
        "codecs": codecs(),
        "recipes": TMOD_RECIPES,
        "read": read_cases(inputs),
        "build": build_cases(),
        "filenames": filenames(),
        "unlock": unlock_cases(),
        "ocr_errors": ocr_cases(),
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(golden, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
