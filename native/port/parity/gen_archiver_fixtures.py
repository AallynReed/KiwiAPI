"""Golden fixtures for kiwi-server's update archiver (``src/archiver``).

Runs the real Python code: the CDN/manifest/TFI parsers and diffs, the
``sync_branch`` pipeline over a fake CDN and an in-memory repo (real CAS in a
temp dir), the ``MongoUpdateRepo`` writes captured at the pymongo/Beanie
boundary, the backfill, and the two admin routes through the ASGI app.

From the repo root, with the production-pinned venv:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_archiver_fixtures.py
Writes native/kiwi-server/testdata/archiver/golden.json.
"""

from __future__ import annotations

import asyncio
import datetime as dt
import hashlib
import json
import logging
import os
import sys
import tempfile
import urllib.parse
import warnings
import zlib
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "archiver" / "golden.json"
os.chdir(ROOT)
sys.path.insert(0, str(ROOT))

import httpx  # noqa: E402
from beanie import init_beanie  # noqa: E402
from beanie.odm.utils import init as beanie_init  # noqa: E402
from beanie.odm.utils.dump import get_dict  # noqa: E402
from bson import ObjectId  # noqa: E402
from bson.int64 import Int64  # noqa: E402
from bson.json_util import CANONICAL_JSON_OPTIONS  # noqa: E402
from bson.json_util import dumps as ext_dumps  # noqa: E402
from pymongo import AsyncMongoClient, DeleteOne, UpdateOne  # noqa: E402

from app.auth.models import User  # noqa: E402
from app.core.dependencies import get_current_superuser  # noqa: E402
from app.trove.troveio import calculate_hash, write_leb128  # noqa: E402
from app.trove.updates import archive, cdn, diff, maintenance  # noqa: E402
from app.trove.updates import repo as repo_mod  # noqa: E402
from app.trove.updates.cas import ContentStore  # noqa: E402
from app.trove.updates.ingest import sync_branch  # noqa: E402
from app.trove.updates.models import (  # noqa: E402
    UpdateBranch,
    UpdateChange,
    UpdateManifestEntry,
    UpdateState,
    UpdateVersion,
)

REAL_BACKFILL = maintenance.backfill_last_ordinal
UTC = dt.timezone.utc
NOW =dt.datetime(2026, 10, 22, 14, 37, 21, 654321, tzinfo=UTC)
ADMIN_ID = "65a1b2c3d4e5f60718293a4b"


def ext(value):
    return json.loads(ext_dumps(value, json_options=CANONICAL_JSON_OPTIONS))


def err_text(fn, *a, **k):
    try:
        return {"ok": fn(*a, **k)}
    except Exception as e:  # noqa: BLE001
        return {"error": str(e), "type": type(e).__name__}


# --- pure ----------------------------------------------------------------------------

POINTERS = [
    "STABLE-103-418-A-335805|content/patchkiwi-live-us01|May 21 15:16, STABLE-103-418-A-335805|"
    "http://alpha.triongames.com/installer/trove/trove-alpha-motd.html|kiwi_STABLE-103-418-A-335805_Patch|*",
    "", "only-one-field", "  V1 | content/cp |x\r\n", "V2|cp", "V3|cp|m", "V4|cp|m| motd \x1c",
    "|cp|x|y", "V5| |x|y", "\ufeffV6|content/content/cp|a|b", "V7|contentx/cp|a|\u00e9t\u00e9\u2003",
    "\x1fV8|cp\x1d", "V9\u3000|cp|||",
]

MANIFESTS = [
    "version STABLE-103-415-A-335760\r\nCrashHandler.exe:535853c2de3cc8da34f376c53850f66d00232723:2376192\r\n"
    "audio\\archive0.tfa:84f78fad45833a3735d15bff5f66bb19f5eb79e1:4039807\r\n",
    "version V1\r\nGood.exe:abc:123\r\ngarbage-no-colons\r\nBad.exe:def:notanint\r\n",
    "Trove_x64.exe:abc:1\r\n",
    " version V1\nA:b:1\n",
    "version   V2 \r\r\nc:d:e:f:9\nx: y :1_000\n z:w: +12 \nn:m:-4\nq:r:1__0\ns:t:_1\nu:v:\uff11\uff12\n",
    "version V3\n\n\n:sha:5\npath::6\npath:sha:\nx\\y\\z.tfi:s:7\r\n",
    "version V4\rwith-cr\nA:B:1\rC:D:2\n",
    "version",
    "version \n",
    "",
    "version V5\nbig:s:99999999999\nneg:s:-0\nws:s:\t3\n",
]

URLS = [
    ("http://trove-update.dyn.triongames.com", "/kiwi-live-client-patch/", "patchkiwi-live-us01",
     "ui/archive0.tfa", "deadbeef"),
    ("http://h", "/p/", "cp", "dir with space/f+g&h=i?#%.bin", "abc"),
    ("http://h", "", "cp", "\u00fcml\u00e4ut/\u65e5\u672c/~a_b-c.d", "0"),
    ("http://h", "/p/", "cp", "a//b/./c/../d;e:f@g$h!i'j(k)l*m,n", "s"),
]

LOOSE_TFI = [
    ("a.bin", 0, b"alpha"), ("sub\\b.bin", 0, b"bravo!"), ("c.bin", 1, b"charlie-x"),
]


def tfi_bytes(entries, pad=0):
    out = bytearray()
    contents: dict[int, bytearray] = {}
    for name, arc, data in entries:
        buf = contents.setdefault(arc, bytearray())
        off = len(buf)
        buf += data
        nb = name.encode("utf-8") + b"\x00" * pad
        out += write_leb128(len(nb)) + nb + write_leb128(arc) + write_leb128(off)
        out += write_leb128(len(data)) + write_leb128(calculate_hash(data))
    return bytes(out)


TFIS = [
    tfi_bytes(LOOSE_TFI), tfi_bytes(LOOSE_TFI, pad=3), b"",
    b"\x03ab", b"\x02a\x00\x00\x00\x00", b"\x05ab\xffcd\x00\x00\x00\x00", b"\x03\xe2\x82\x00\x00\x00\x00",
    b"\x03\xe2\x82", b"\x02\xc0\x80\x00\x00\x00\x00", b"\x04\xf0\x90\x80A\x00\x00\x00\x00",
    b"\x01a" + b"\x80" * 10, b"\x01a" + b"\xff\xff\xff\xff\x7f" * 4,
    b"\x01a\x00\x00\x00", b"\x03a\x00b\x00\x00\x00\x00", b"\x09" + "\u00e9t\u00e9".encode() + b"\x00\x00\x00\x00",
    b"\x0aname\\x\\y\x00\x00\x01\x02\x03\x04", b"\x02\xed\xa0\x00\x00\x00\x00",
]


def tfi_case(raw):
    try:
        entries = archive.parse_tfi(raw)
    except archive.ArchiveError as e:
        return {"hex": raw.hex(), "error": str(e)}
    return {"hex": raw.hex(), "entries": [[e.name, e.archive_index, e.offset, e.size, e.fnv_hash] for e in entries]}


def inflate_cases():
    good = zlib.compress(b"0123456789" * 50)
    out = []
    for name, raw, hint in [
        ("exact", good, 500), ("hint-short", good, 400), ("hint-long", good, 512), ("no-hint", good, 0),
        ("trailing", good + b"garbage", 500), ("truncated", good[:-10], 500), ("truncated-nohint", good[:20], 0),
        ("bad-header", b"\x00\x01\x02\x03", 4), ("empty", b"", 0), ("bad-block", b"\x78\x9c\xff\xff\xff", 8),
        ("needs-dict", bytes.fromhex("78bb00000001") + b"\x00" * 8, 8),
    ]:
        r = err_text(archive.decompress_tfa, raw, hint)
        case = {"name": name, "hex": raw.hex(), "hint": hint}
        if "ok" in r:
            case["sha"] = hashlib.sha256(r["ok"]).hexdigest()
            case["len"] = len(r["ok"])
        else:
            case["error"] = r["error"]
        out.append(case)
    return out


CLASSIFY = [
    ({"Trove.exe": {"sha1": "a", "size": 1}, "ui/index.tfi": {"sha1": "t", "size": 1},
      "ui/archive0.tfa": {"sha1": "x", "size": 1}},
     [{"path": "Trove.exe", "sha1": "a", "size": 1}, {"path": "ui/index.tfi", "sha1": "T2", "size": 2},
      {"path": "ui/archive0.tfa", "sha1": "X2", "size": 2}, {"path": "NewLoose.dll", "sha1": "n", "size": 3}]),
    ({"a.exe": {"sha1": "x", "size": 1}}, [{"path": "a.exe", "sha1": "x", "size": 1}]),
    ({"ui/index.tfi": {"sha1": "t", "size": 1}, "ui/archive0.tfa": {"sha1": "x", "size": 1}, "z.dll": {"sha1": "z", "size": 1}}, []),
    ({}, [{"path": "b/archive2.tfa", "sha1": "1", "size": 1}, {"path": "b/x_archive2.tfa", "sha1": "1", "size": 1},
          {"path": "b/archive10.tfa", "sha1": "1", "size": 1}, {"path": "b/archive01.tfa", "sha1": "1", "size": 1},
          {"path": "b/two.tfi", "sha1": "1", "size": 1}, {"path": "b/index.tfi", "sha1": "1", "size": 1},
          {"path": "b/archive.tfa", "sha1": "1", "size": 1}, {"path": "b/archive\u0663.tfa", "sha1": "1", "size": 1},
          {"path": "root.tfi", "sha1": "1", "size": 1}, {"path": "archive7.tfa", "sha1": "1", "size": 1},
          {"path": "b/c/archive0.tfa", "sha1": "1", "size": 1}, {"path": "\u00e9/a.bin", "sha1": "1", "size": 1},
          {"path": "Z.bin", "sha1": "1", "size": 1}, {"path": "dup", "sha1": "1", "size": 1},
          {"path": "dup", "sha1": "2", "size": 9}]),
    ({"q/index.tfi": {"sha1": "1", "size": 1}, "q/archive0.tfa": {"sha1": "1", "size": 1},
      "q/archive1.tfa": {"sha1": "1", "size": 1}, "q/loose.txt": {"sha1": "1", "size": 1}},
     [{"path": "q/index.tfi", "sha1": "1", "size": 1}, {"path": "q/archive0.tfa", "sha1": "1", "size": 1},
      {"path": "q/loose.txt", "sha1": "2", "size": 1}]),
]


def plan_json(plan):
    return {
        "changed_loose": plan["changed_loose"], "removed_loose": plan["removed_loose"],
        "dir_work": [[d, w["tfi_path"], w["changed_archives"]] for d, w in plan["dir_work"].items()],
        "new_map": [[p, e["sha1"], e["size"]] for p, e in plan["new_map"].items()],
        "any": plan["any"],
    }


def status_messages():
    out = []
    for code, reason, headers in [
        (500, b"Internal Server Error", {}), (403, b"Forbidden", {}), (404, b"File not found", {}),
        (301, b"Moved Permanently", {"Location": "http://x/y?z=1"}), (302, b"Found", {}),
        (304, b"Not Modified", {"Location": "/a"}), (199, b"Odd", {}), (600, b"Way Out", {}),
        (418, b"", {}), (503, "Servi\u00e7e".encode("latin-1"), {}),
    ]:
        url = "http://h/p//content/cp/recovery/a%20b.bin?sha1=x"
        resp = httpx.Response(code, headers=headers, extensions={"reason_phrase": reason},
                              request=httpx.Request("GET", url))
        try:
            resp.raise_for_status()
            msg = None
        except httpx.HTTPStatusError as e:
            msg = str(e)
        out.append({"status": code, "reason": resp.reason_phrase, "url": url,
                    "location": headers.get("Location"), "message": msg})
    return out


def text_cases():
    out = []
    for body, ctype in [
        (b"V1|cp", None), (b"\xff\xfeV|cp", "text/plain"), ("\u00e9|cp".encode("latin-1"), "text/plain; charset=ISO-8859-1"),
        ("\u00e9|cp".encode(), "text/plain; charset=utf-8"), (b"a\x80b", "text/plain; charset=us-ascii"),
        (b"\xef\xbb\xbfV|cp", "text/plain"), (b"\xe2\x82", "text/plain; charset=\"UTF-8\""),
    ]:
        headers = {"content-type": ctype} if ctype else {}
        out.append({"hex": body.hex(), "content_type": ctype, "text": httpx.Response(200, headers=headers, content=body).text})
    return out


def pure_section():
    return {
        "branches": list(cdn.BRANCHES.items()),
        "pointers": [{"text": t, **err_text(cdn.parse_pointer, t)} for t in POINTERS],
        "manifests": [{"text": t, **err_text(lambda x: list(cdn.parse_manifest(x)), t)} for t in MANIFESTS],
        "urls": [{"args": list(a), "pointer": cdn.pointer_url(a[0], a[1], "kiwi-pts.txt"),
                  "manifest": cdn.manifest_url(a[0], a[1], a[2], "V1"), "file": cdn.file_url(*a)} for a in URLS],
        "tfi": [tfi_case(t) for t in TFIS],
        "archive_size": [[[[e.name, e.archive_index, e.offset, e.size, e.fnv_hash] for e in archive.parse_tfi(tfi_bytes(LOOSE_TFI))],
                          ai, archive.archive_size(archive.parse_tfi(tfi_bytes(LOOSE_TFI)), ai)] for ai in (0, 1, 2)],
        "inflate": inflate_cases(),
        "classify": [{"old": [[p, v["sha1"], v["size"]] for p, v in old.items()], "new": new,
                      "plan": plan_json(diff.classify_manifest_diff(old, new))} for old, new in CLASSIFY],
        "diff_logical": [
            {"old": [["a", 1], ["b", 2], ["c", 3], ["n", None]], "new": [["a", 1], ["b", 99], ["d", 4], ["n", 5]],
             "out": diff.diff_logical({"a": 1, "b": 2, "c": 3, "n": None}, {"a": 1, "b": 99, "d": 4, "n": 5})},
        ],
        "archive_index_of": [[p, diff.archive_index_of(p)] for p in [
            "ui/archive12.tfa", "ui/index.tfi", "archivearchive3.tfa", "x/archive\u0661\u0662.tfa", "archive.tfa",
            "a/archive5.tfa.bak", "archive007.tfa", "Archive1.tfa"]],
        "status_messages": status_messages(),
        "text": text_cases(),
    }


# --- ingest scenarios ----------------------------------------------------------------

def _sha1(b: bytes) -> str:
    return hashlib.sha1(b).hexdigest()


def build_dir(dir_files):
    contents: dict[int, bytearray] = {}
    tfi = bytearray()
    for name, arc, data in dir_files:
        buf = contents.setdefault(arc, bytearray())
        offset = len(buf)
        buf += data
        nb = name.encode("utf-8") + b"\x00"
        tfi += write_leb128(len(nb)) + nb + write_leb128(arc) + write_leb128(offset)
        tfi += write_leb128(len(data)) + write_leb128(calculate_hash(data))
    return bytes(tfi), {arc: zlib.compress(bytes(buf)) for arc, buf in contents.items()}


def make_game(loose, dirs, extra_manifest=(), drop=()):
    manifest, files = [], {}
    for path, data in loose.items():
        files[path] = data
        manifest.append({"path": path, "sha1": _sha1(data), "size": len(data)})
    for d, dfiles in dirs.items():
        tfi, tfas = build_dir(dfiles)
        tfi_path = f"{d}/index.tfi" if d else "index.tfi"
        files[tfi_path] = tfi
        manifest.append({"path": tfi_path, "sha1": _sha1(tfi), "size": len(tfi)})
        for arc, tfa in tfas.items():
            tfa_path = f"{d}/archive{arc}.tfa" if d else f"archive{arc}.tfa"
            files[tfa_path] = tfa
            manifest.append({"path": tfa_path, "sha1": _sha1(tfa), "size": len(tfa)})
    manifest += list(extra_manifest)
    manifest = [m for m in manifest if m["path"] not in drop]
    return manifest, files


class FakeCdn:
    def __init__(self, version, content_path, manifest, files, fail=None, pointer_error=None):
        self.pointer = {"version": version, "content_path": content_path, "motd": "m-" + version, "fields": []}
        self.manifest = manifest
        self.files = files
        self.fail = fail or {}
        self.pointer_error = pointer_error

    async def fetch_pointer(self, pointer_file):
        if self.pointer_error:
            raise cdn.CdnError(self.pointer_error)
        return self.pointer

    async def fetch_manifest(self, content_path, version):
        return "M-" + self.pointer["version"], self.manifest

    async def download_file(self, content_path, path, sha1, expected_size=None):
        await asyncio.sleep(0)
        if path in self.fail:
            raise cdn.CdnError(self.fail[path])
        data = self.files[path]
        if expected_size is not None and len(data) != expected_size:
            raise cdn.CdnError(f"size mismatch for {path}: got {len(data)}, manifest {expected_size}")
        return data


class FakeRepo:
    """Python's ingest-test repo, writes applied at once, dict order kept."""

    def __init__(self):
        self.manifest, self.state, self.changes, self.versions, self.branch = {}, {}, [], [], {}

    async def get_manifest_sidecar(self, branch):
        return {p: dict(v) for p, v in self.manifest.items()}

    async def begin_version(self, branch, version_tag, pointer):
        for v in self.versions:
            if v["status"] == "in_progress":
                return v["ordinal"], True
        ordinal = max((v["ordinal"] for v in self.versions), default=0) + 1
        self.versions.append({"ordinal": ordinal, "version_tag": version_tag, "status": "in_progress",
                              "content_path": pointer["content_path"], "motd": pointer["motd"]})
        return ordinal, False

    async def state_get(self, branch, path):
        st = self.state.get(path)
        return dict(st) if st else None

    async def get_archive_state(self, branch, directory):
        return {p: {"fnv_hash": st["fnv_hash"], "content_sha256": st["content_sha256"]}
                for p, st in self.state.items() if st.get("archive") == directory}

    async def record_change(self, branch, ordinal, path, change_type, content_sha256, fnv_hash, size):
        self.changes.append({"ordinal": ordinal, "path": path, "type": change_type,
                             "content_sha256": content_sha256, "fnv_hash": fnv_hash, "size": size})

    async def upsert_state(self, branch, path, content_sha256, fnv_hash, size, archive, archive_index, last_ordinal):
        self.state[path] = {"content_sha256": content_sha256, "fnv_hash": fnv_hash, "size": size,
                            "archive": archive, "archive_index": archive_index, "last_ordinal": last_ordinal}

    async def remove_state(self, branch, path):
        self.state.pop(path, None)

    async def set_manifest_entry(self, branch, path, sha1, size):
        self.manifest[path] = {"sha1": sha1, "size": size}

    async def remove_manifest_entry(self, branch, path):
        self.manifest.pop(path, None)

    async def finish_version(self, branch, ordinal, version_tag, pointer, counts):
        for v in self.versions:
            if v["ordinal"] == ordinal:
                v["status"] = "complete"
                v.update(counts)
        self.branch = {"current_version": version_tag, "current_ordinal": ordinal,
                       "content_path": pointer["content_path"]}

    async def touch_probe(self, branch, content_path, version_tag):
        self.branch["current_version"] = version_tag
        self.branch["content_path"] = content_path

    def snapshot(self):
        return {
            "manifest": sorted([p, v["sha1"], v["size"]] for p, v in self.manifest.items()),
            "state": sorted([p, s["content_sha256"], s["fnv_hash"], s["size"], s["archive"], s["archive_index"],
                             s["last_ordinal"]] for p, s in self.state.items()),
            "changes": sorted(([c["ordinal"], c["path"], c["type"], c["content_sha256"], c["fnv_hash"], c["size"]]
                               for c in self.changes), key=lambda r: (r[0], r[1], r[2])),
            "versions": [dict(v) for v in self.versions],
            "branch": dict(self.branch),
        }


def cas_listing(root: Path):
    out = []
    for p in sorted((root / "objects").rglob("*")):
        if p.is_file():
            out.append([p.relative_to(root).as_posix(), p.stat().st_size])
    return out


async def drain():
    me = asyncio.current_task()
    for _ in range(50):
        pending = [t for t in asyncio.all_tasks() if t is not me and not t.done()]
        if not pending:
            return
        await asyncio.gather(*pending, return_exceptions=True)


def game_json(manifest, files):
    return {"cdn_manifest": manifest, "cdn_files": {p: d.hex() for p, d in files.items()}}


BIG = bytes(range(256)) * 9


def scenarios():
    loose1 = {"Trove_x64.exe": b"exe-v1", "Trove.cfg": b"[cfg]"}
    ui1 = [("a.bin", 0, b"alpha-content"), ("b.bin", 0, b"bravo-content"), ("deep\\c.bin", 1, b"charlie")]
    ui2 = [("a.bin", 0, b"alpha-content"), ("b.bin", 0, b"BRAVO-CHANGED!"), ("deep\\c.bin", 1, b"charlie")]
    ui4 = [("b.bin", 0, b"BRAVO-CHANGED!"), ("deep\\c.bin", 1, b"charlie")]
    yield "lifecycle", "live-us", [
        ("V1", make_game(loose1, {"ui": ui1, "audio": [("s.wem", 0, BIG), ("t.wem", 2, b"tt")]}), {}),
        ("V2", make_game(loose1, {"ui": ui2, "audio": [("s.wem", 0, BIG), ("t.wem", 2, b"tt")]}), {}),
        ("V2", make_game(loose1, {"ui": ui2, "audio": [("s.wem", 0, BIG), ("t.wem", 2, b"tt")]}), {}),
        ("V3", make_game(loose1, {"ui": ui4, "audio": [("s.wem", 0, BIG), ("t.wem", 2, b"tt")]}), {}),
        ("V4", make_game({"Trove_x64.exe": b"exe-v2"}, {"ui": ui4}), {}),
        ("V5", make_game({"Trove_x64.exe": b"exe-v2", "dup.bin": b"alpha-content"},
                         {"ui": ui4 + [("e.bin", 3, b"echo"), ("f.bin", 3, b"alpha-content")], "": [("rootf", 0, b"r")]}), {}),
    ]
    yield "loose", "pts", [
        ("V1", make_game({"keep.dll": b"k", "drop.dll": b"d", "x/y z.txt": b"sp"}, {}), {}),
        ("V2", make_game({"keep.dll": b"k2", "x/y z.txt": b"sp"}, {}), {}),
    ]
    many = {f"f{i:02d}.dat": f"payload-{i}".encode() for i in range(14)}
    yield "loose-failure", "live-us", [
        ("V1", make_game(many, {"d": [("q", 0, b"q")]}), {"fail": {"f03.dat": "boom f03", "f09.dat": "boom f09"}}),
        ("V1", make_game(many, {"d": [("q", 0, b"q")]}), {}),
    ]
    yield "archive-edge", "live-us", [
        ("V1", make_game({}, {"pk": [("a", 0, b"aaaa"), ("b", 1, b"bbbb"), ("c", 2, b"cc")]}), {}),
        # archive1 loses its only file; archive2's file changes but archive2.tfa is missing from the manifest.
        ("V2", make_game({}, {"pk": [("a", 0, b"aaaa"), ("c", 2, b"CC")]}, drop={"pk/archive2.tfa"}), {}),
        ("V3", make_game({}, {"pk": [("a", 0, b"aaaa"), ("c", 2, b"CC")]}), {}),
        ("V4", make_game({}, {"pk": [("a", 0, b"aaaa"), ("a", 1, b"second-a"), ("z", 0, b"zz")]}), {}),
    ]
    tfi_bad = {"path": "bad/index.tfi", "sha1": "s1", "size": 4}
    yield "errors", "live-us", [
        ("V1", make_game({"ok.txt": b"ok"}, {}, extra_manifest=[tfi_bad]), {"files": {"bad/index.tfi": b"\x05ab"}}),
        ("V1", make_game({"ok.txt": b"ok"}, {"good": [("g", 0, b"gg")]}), {"pointer_error": "malformed bootstrap pointer"}),
        ("V1", make_game({"ok.txt": b"ok"}, {"good": [("g", 0, b"gg")]}), {"corrupt": "good/archive0.tfa"}),
        ("V1", make_game({"ok.txt": b"ok"}, {"good": [("g", 0, b"gg")]}), {}),
        ("V2", make_game({"ok.txt": b"ok"}, {"good": [("g", 0, b"gg"), ("h", 0, b"hh" * 40)]}), {"truncate": "good/archive0.tfa"}),
    ]
    yield "dir-removed", "pts", [
        ("V1", make_game({"top.bin": b"t"}, {"gone": [("x", 0, b"x1"), ("y", 0, b"y1")], "stay": [("s", 0, b"s")]},
                         extra_manifest=[{"path": "gone/readme.txt", "sha1": "r", "size": 1}]), {"files": {"gone/readme.txt": b"r"}}),
        ("V2", make_game({"top.bin": b"t"}, {"stay": [("s", 0, b"s")]}), {}),
    ]


async def ingest_section():
    out = []
    for name, branch, steps in scenarios():
        with tempfile.TemporaryDirectory() as tmp:
            store, repo = ContentStore(tmp), FakeRepo()
            recorded = []
            for version, (manifest, files), opts in steps:
                files = dict(files)
                files.update(opts.get("files", {}))
                if "corrupt" in opts:
                    files[opts["corrupt"]] = b"\x78\x9c\xff\xff" + files[opts["corrupt"]][4:]
                if "truncate" in opts:
                    files[opts["truncate"]] = files[opts["truncate"]][:12]
                for m in manifest:
                    if m["path"] in files:
                        m["size"] = len(files[m["path"]])
                fake = FakeCdn(version, "cp-" + branch, manifest, files, opts.get("fail"), opts.get("pointer_error"))
                try:
                    s = await sync_branch(branch, cdn.BRANCHES[branch], fake, store, repo, download_concurrency=3)
                    result = {"summary": s}
                except Exception as e:  # noqa: BLE001
                    result = {"error": str(e), "type": type(e).__name__}
                await drain()
                recorded.append({"version": version, "pointer_error": opts.get("pointer_error"),
                                 "fail": opts.get("fail", {}), **game_json(manifest, files), **result,
                                 **repo.snapshot(), "cas": cas_listing(Path(tmp))})
            out.append({"name": name, "branch": branch, "steps": recorded})
    return out


# --- MongoUpdateRepo ------------------------------------------------------------------

class Recorder:
    def __init__(self, name, log, modified=0):
        self.name = name
        self.log = log
        self.modified = modified

    async def bulk_write(self, ops, ordered=True):
        rows = []
        for op in ops:
            if isinstance(op, UpdateOne):
                rows.append({"update": ext(op._filter), "set": ext(op._doc["$set"]), "upsert": op._upsert})
            elif isinstance(op, DeleteOne):
                rows.append({"delete": ext(op._filter)})
        self.log.append({"coll": self.name, "ordered": ordered, "ops": rows})

        class R:
            modified_count = self.modified
        return R()

    async def insert_one(self, doc, session=None):
        self.log.append({"coll": self.name, "insert": ext(doc)})

        class R:
            inserted_id = ObjectId("65f000000000000000000001")
        return R()


class Q:
    def __init__(self, docs):
        self.docs = docs

    def sort(self, *a, **k):
        return self

    def limit(self, *a, **k):
        return self

    async def to_list(self, *a, **k):
        return self.docs


async def repo_section():
    log: list = []
    for cls, name in [(UpdateChange, "update_changes"), (UpdateState, "update_state"),
                      (UpdateManifestEntry, "update_manifest"), (UpdateVersion, "update_versions"),
                      (UpdateBranch, "update_branches")]:
        rec = Recorder(name, log)
        cls.get_pymongo_collection = classmethod(lambda c, rec=rec: rec)

    async def save(self, *a, **k):
        log.append({"coll": self.get_collection_name(), "save": ext(get_dict(self, to_db=True, keep_nulls=True))})
        return self

    UpdateVersion.save = save
    UpdateBranch.save = save
    repo_mod.utcnow = lambda: NOW

    r = repo_mod.MongoUpdateRepo()
    await r.record_change("live-us", 3, "ui/a.bin", "added", "ab" * 32, 4_000_000_000, 12)
    await r.record_change("live-us", 4, "ui/b.bin", "removed", None, None, 0)
    await r.record_change("pts", 2**31, "x", "modified", "cd" * 32, 5, 2**33)
    await r.upsert_state("live-us", "ui/a.bin", "ab" * 32, 4_000_000_000, 12, "ui", 0, 3)
    await r.upsert_state("live-us", "Trove.exe", "ef" * 32, None, 2**31, None, None, 2**31 - 1)
    await r.remove_state("live-us", "gone.bin")
    await r.upsert_state("live-us", "gone.bin", "01" * 32, 1, 1, "d", 7, 9)
    await r.set_manifest_entry("live-us", "ui/index.tfi", "s1", 2**40)
    await r.remove_manifest_entry("live-us", "old.tfa")
    await r.set_manifest_entry("live-us", "ui/archive0.tfa", "s2", 3)
    await r._flush()
    ops_log = list(log)
    log.clear()

    # begin_version: nothing in progress, last ordinal 6, branch absent -> insert.
    async def none(*a, **k):
        return None
    last = UpdateVersion.model_validate({"_id": ObjectId("65f0000000000000000000a6"), "branch": "live-us", "ordinal": Int64(6),
                                         "version_tag": "OLD", "status": "complete"})
    UpdateVersion.find_one = classmethod(lambda c, *a, **k: none())
    UpdateVersion.find = classmethod(lambda c, *a, **k: Q([last]))
    UpdateBranch.find_one = classmethod(lambda c, *a, **k: none())
    pointer = {"version": "PV", "content_path": "patchkiwi-live-us01", "motd": "Motd \u00e9", "fields": []}
    begin = await r.begin_version("live-us", "MV-7", pointer)
    begin_log = list(log)
    log.clear()
    # captured_at is the model's own utcnow() default: replaced by a marker.
    for row in begin_log:
        if "insert" in row and "captured_at" in row["insert"]:
            row["insert"]["captured_at"] = "<now>"

    # begin_version resuming an in-progress version.
    inprog = UpdateVersion.model_validate({"_id": ObjectId("65f0000000000000000000a7"), "branch": "pts", "ordinal": 9,
                                           "version_tag": "X", "status": "in_progress"})

    async def found(*a, **k):
        return inprog
    UpdateVersion.find_one = classmethod(lambda c, *a, **k: found())
    resumed = await r.begin_version("pts", "MV-9", pointer)
    resumed_log = list(log)
    log.clear()

    # finish_version over a legacy version doc (missing defaults) and an existing branch doc.
    vdoc = {"_id": ObjectId("65f0000000000000000000a8"), "branch": "live-us", "ordinal": 7, "version_tag": "MV-7",
            "status": "in_progress", "captured_at": dt.datetime(2026, 10, 1, 1, 2, 3, 456000, tzinfo=UTC),
            "content_path": "cp", "motd": "m", "files_added": Int64(1)}
    bdoc = {"_id": ObjectId("65f0000000000000000000b1"), "branch": "live-us", "content_path": "old",
            "current_version": "MV-6", "current_ordinal": Int64(6),
            "last_probe_at": dt.datetime(2026, 9, 1, tzinfo=UTC), "status": "syncing", "last_error": "earlier"}

    async def vfound(*a, **k):
        return UpdateVersion.model_validate(vdoc)

    async def bfound(*a, **k):
        return UpdateBranch.model_validate(bdoc)
    UpdateVersion.find_one = classmethod(lambda c, *a, **k: vfound())
    UpdateBranch.find_one = classmethod(lambda c, *a, **k: bfound())
    import app.events.bus as bus

    async def publish_type(name):
        log.append({"publish": name})
    bus.publish_type = publish_type
    await r.finish_version("live-us", 7, "MV-7", pointer, {"added": 3, "modified": 2**31, "removed": 0, "bytes_added": 2**35})
    finish_log = list(log)
    log.clear()

    await r.touch_probe("live-us", "cp2", "MV-8")
    touch_log = list(log)
    log.clear()
    long_msg = "\u00e9" * 300 + "x" * 300
    await r.mark_branch_error("live-us", long_msg)
    error_log = list(log)
    log.clear()
    UpdateBranch.find_one = classmethod(lambda c, *a, **k: none())
    await r.mark_branch_error("pts", "Server error '500 Internal Server Error' for url 'http://h'")
    error_new_log = list(log)
    log.clear()
    return {
        "now": NOW.isoformat(), "pointer": pointer,
        "ops": ops_log, "begin": {"result": list(begin), "log": begin_log, "last": ext(get_dict(last, to_db=True))},
        "resumed": {"result": list(resumed), "log": resumed_log},
        "finish": {"version": ext(vdoc), "branch": ext(bdoc), "log": finish_log},
        "touch": {"log": touch_log}, "error": {"message": long_msg, "log": error_log},
        "error_new": {"log": error_new_log},
    }


# --- backfill ----------------------------------------------------------------------------

async def backfill_section():
    rows = [{"_id": f"p{i}", "last": (Int64(i) if i % 3 == 0 else i % 11)} for i in range(2003)]
    log: list = []

    class Agg:
        def __init__(self, rows):
            self.rows = rows

        def __aiter__(self):
            async def gen():
                for r in self.rows:
                    yield r
            return gen()

    class Changes:
        async def aggregate(self, pipeline, **kw):
            log.append({"pipeline": ext(pipeline), "kw": kw})
            return Agg(rows)

    calls = []

    class State:
        async def bulk_write(self, ops, ordered=True):
            calls.append({"n": len(ops), "ordered": ordered,
                          "first": {"filter": ext(ops[0]._filter), "update": ext(ops[0]._doc), "upsert": ops[0]._upsert},
                          "last": {"filter": ext(ops[-1]._filter), "update": ext(ops[-1]._doc)}})

            class R:
                modified_count = len(ops) - 1
            return R()

    UpdateChange.get_pymongo_collection = classmethod(lambda c: Changes())
    UpdateState.get_pymongo_collection = classmethod(lambda c: State())
    maintenance.utcnow = lambda: NOW
    maintenance._status.clear()
    n = await REAL_BACKFILL("pts")
    status = maintenance.get_backfill_status("pts")
    return {"updated": n, "log": log, "calls": calls, "status": ext(status),
            "rows_rule": "path p{i}, last Int64(i) when i % 3 == 0 else int32 i % 11, i < 2003"}


# --- routes -------------------------------------------------------------------------------

async def call(app, method: str, url: str, rid: str):
    path, _, qs = url.partition("?")
    headers = [(b"host", b"testserver"), (b"x-request-id", rid.encode()), (b"content-length", b"0")]
    scope = {
        "type": "http", "asgi": {"version": "3.0"}, "http_version": "1.1", "method": method,
        "scheme": "http", "path": urllib.parse.unquote(path), "raw_path": path.encode(),
        "query_string": qs.encode(), "root_path": "", "headers": headers,
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
    start = next(m for m in sent if m["type"] == "http.response.start")
    raw = b"".join(m.get("body", b"") for m in sent if m["type"] == "http.response.body")
    hdrs = {k.decode("latin-1").lower(): v.decode("latin-1") for k, v in start["headers"]}
    out = {"method": method, "url": url, "status": start["status"], "body": raw.decode("utf-8"),
           "content-type": hdrs.get("content-type")}
    for name in ("allow", "www-authenticate"):
        if name in hdrs:
            out[name] = hdrs[name]
    return out


async def routes_section():
    from app.main import app
    admin = User.model_validate({"_id": ObjectId(ADMIN_ID), "email": "master@aallyn.net", "hashed_password": "x",
                                 "is_superuser": True, "is_active": True, "is_verified": True,
                                 "created_at": NOW, "updated_at": NOW})
    started = []

    async def fake_backfill(branch):
        started.append(branch)
        maintenance._status[branch] = {"running": True, "updated": 0, "done": False, "started_at": NOW,
                                       "finished_at": None, "error": None}

    maintenance.backfill_last_ordinal = fake_backfill
    maintenance._status.clear()
    app.dependency_overrides[get_current_superuser] = lambda: admin
    out = []
    i = 0

    async def c(method, url):
        nonlocal i
        i += 1
        out.append(await call(app, method, url, f"parity-arc-{i}"))

    await c("GET", "/admin/updates/backfill-modified/status")
    await c("GET", "/admin/updates/backfill-modified/status?branch=pts")
    await c("GET", "/admin/updates/backfill-modified/status?branch=nope")
    await c("GET", "/admin/updates/backfill-modified/status?branch=")
    await c("GET", "/admin/updates/backfill-modified/status?branch=%C3%A9t%C3%A9")
    await c("POST", "/admin/updates/backfill-modified?branch=x%27y")
    await c("POST", "/admin/updates/backfill-modified?branch=LIVE-US")
    await c("POST", "/admin/updates/backfill-modified?branch=pts&branch=live-us")
    await c("GET", "/admin/updates/backfill-modified/status?branch=live-us")
    await c("POST", "/admin/updates/backfill-modified")
    maintenance._status["pts"] = {"running": False, "updated": 2003, "done": True,
                                  "started_at": NOW, "finished_at": NOW.replace(microsecond=0), "error": None}
    await c("GET", "/admin/updates/backfill-modified/status?branch=pts")
    maintenance._status["pts"] = {"running": False, "updated": 1000, "done": False,
                                  "started_at": NOW.replace(microsecond=1000), "finished_at": NOW,
                                  "error": "boom \u00e9 \"quoted\"\n"}
    await c("GET", "/admin/updates/backfill-modified/status?branch=pts")
    app.dependency_overrides.clear()
    unauth = []
    for j, (method, url) in enumerate([
        ("GET", "/admin/updates/backfill-modified/status"), ("POST", "/admin/updates/backfill-modified"),
        ("GET", "/admin/updates/backfill-modified"), ("POST", "/admin/updates/backfill-modified/status"),
        ("HEAD", "/admin/updates/backfill-modified/status"), ("PUT", "/admin/updates/backfill-modified"),
        ("DELETE", "/admin/updates/backfill-modified/status?branch=nope"),
    ]):
        unauth.append(await call(app, method, url, f"parity-arc-u{j}"))
    return {"authed": out, "started": started, "unauth": unauth}


CDN = ROOT / "native" / "port" / "parity" / "data" / "archiver_cdn"


def write_cdn() -> None:
    """The twin sandbox's fake update CDN (``sandbox.sh`` serves it as
    http://kp-twin:8099 with TROVE_UPDATE_PREFIX=/cdn/): live-us only, so pts
    answers 404 and is skipped as unpublished."""
    loose = {"Trove.exe": b"MZ-twin-exe", "Trove.cfg": b"[twin]", "dir with space/a b.txt": b"spaced",
             "x+y.bin": b"plus", "dup.bin": b"alpha-twin"}
    dirs = {
        "ui": [("a.bin", 0, b"alpha-twin"), ("sub\\b.bin", 0, b"bravo-twin"), ("c.bin", 1, b"charlie" * 9),
               ("same.bin", 1, b"alpha-twin")],
        "audio": [("s.wem", 0, bytes(range(256)) * 12), ("t.wem", 2, b"tango")],
    }
    manifest, files = make_game(loose, dirs)
    cp = "patchkiwi-live-us01"
    lines = ["version TWIN-1"] + [f"{m['path'].replace('/', chr(92))}:{m['sha1']}:{m['size']}" for m in manifest]
    lines.insert(3, "garbage-line-without-colons")
    text = "\r\n".join(lines) + "\r\n"
    out = {
        "cdn/public/kiwi-live-us.txt": b"TWIN-1|content/" + cp.encode() + b"|Oct 23, TWIN-1|motd|kiwi_TWIN-1_Patch|*\r\n",
        f"cdn/content/{cp}/TWIN-1.manifest": text.encode(),
    }
    for path, data in files.items():
        out[f"cdn/content/{cp}/recovery/{path}"] = data
    if CDN.exists():
        import shutil
        shutil.rmtree(CDN)
    for rel, data in out.items():
        p = CDN / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_bytes(data)
    (CDN / ".gitattributes").write_text("* binary\n", encoding="utf-8")


async def main() -> None:
    write_cdn()

    async def no_server(self):
        self._database_major_version = 7
        self._existing_collections = []

    beanie_init.Initializer._load_cached_info = no_server
    client = AsyncMongoClient("mongodb://127.0.0.1:1/", tz_aware=True, connect=False)
    await init_beanie(database=client["fixtures"], document_models=[
        User, UpdateBranch, UpdateVersion, UpdateChange, UpdateState, UpdateManifestEntry,
    ], skip_indexes=True)
    from app.admin import runtime_config as rc
    for key, spec in rc.REGISTRY.items():
        rc._CACHE[key] = (spec.default, float("inf"))
    data = {"python": sys.version.split()[0]}
    data["pure"] = pure_section()
    data["ingest"] = await ingest_section()
    data["routes"] = await routes_section()
    data["repo"] = await repo_section()
    data["backfill"] = await backfill_section()
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(data, ensure_ascii=True, indent=1) + "\n", encoding="utf-8")
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes) with Python {data['python']}")


if __name__ == "__main__":
    asyncio.run(main())
