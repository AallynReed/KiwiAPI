"""Golden fixtures for kiwi-server's BetterTroveTools routes and loops.

Pure functions (version compare, platform matching, commit grouping, GitHub and
Steam normalisation, blocklist parsing) are called directly; documents they
build are captured as pymongo BSON bytes. Route cases drive ``app.main.app`` at
the ASGI level with Beanie initialised offline, the stores replaced by the
fixture documents below, and the auth dependencies overridden where a case
says so. The Rust test (src/routes/btt/tests.rs) replays all of it.

Run from the repo root with the project venv:
    .venv/Scripts/python.exe native/port/parity/gen_btt_fixtures.py
"""

from __future__ import annotations

import asyncio
import copy
import datetime as dt
import json
import logging
import os
import re
import sys
import urllib.parse
import warnings
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "btt" / "golden.json"
os.chdir(ROOT)
sys.path.insert(0, str(ROOT))

import bson  # noqa: E402
from beanie import PydanticObjectId, init_beanie  # noqa: E402
from beanie.odm.utils import init as beanie_init  # noqa: E402
from pymongo import AsyncMongoClient  # noqa: E402
from pymongo.errors import DuplicateKeyError  # noqa: E402

from app.admin import runtime_config as rc  # noqa: E402
from app.btt_blocklist import service as blocklist  # noqa: E402
from app.btt_blocklist.models import BttModBlock  # noqa: E402
from app.core.dependencies import AccessContext, get_current_superuser  # noqa: E402
from app.steam_workshop import service as workshop  # noqa: E402
from app.steam_workshop.models import SteamWorkshopItem  # noqa: E402
from app.trove import btt_releases  # noqa: E402
from app.trove import router as trove_router  # noqa: E402
from app.trove.models import BttChangelog, BttRelease  # noqa: E402
from app.trove.mods_hub.models import TakenDownArtifact  # noqa: E402

EPOCH = dt.datetime(1970, 1, 1, tzinfo=dt.timezone.utc)


def D(ms: int) -> dict:
    return {"$date": ms}


def O(hex_: str) -> dict:
    return {"$oid": hex_}


def to_py(v):
    """Fixture JSON (with $date / $oid markers) to what pymongo hands Beanie."""
    if isinstance(v, list):
        return [to_py(x) for x in v]
    if isinstance(v, dict):
        if set(v) == {"$date"}:
            return EPOCH + dt.timedelta(milliseconds=v["$date"])
        if set(v) == {"$oid"}:
            return bson.ObjectId(v["$oid"])
        return {k: to_py(x) for k, x in v.items()}
    return v


# --- pure functions ---------------------------------------------------------------

VERSIONS = [
    ("v1.2.3", "v1.2.4"), ("1.2.3", "v1.2.3"), ("1.0", "1.0.0"), ("1.0.0", "1.0.0-beta"),
    ("1.0.0-beta", "1.0.0"), ("1.0.0-alpha", "1.0.0-beta"), ("1.0.0-rc.2", "1.0.0-rc.10"),
    ("v1.10", "v1.9"), ("  v2.0  ", "2.0"), ("garbage", "1.0"), ("1.0", ""), ("", ""),
    ("v", "1"), ("1234567890", "123456789"), ("1.2.3.4.5.6.7.8.9.10.11.12", "1.2.3.4.5.6.7.8.9.10"),
    ("1" * 64, "1"), ("1" * 65, "1"), ("1.2\n3", "1.2"), ("1.2x\u0085", "1.2x"), ("V1.0", "1.0"),
    ("\u0661.\u06f2", "1.2"), ("\U0001d7d9.0", "1.0"), ("\uff11.\uff10", "1.0"), ("1.0-\u00e9", "1.0-z"),
    ("01.002", "1.2"), ("1..2", "1"), ("1.", "1"), (".1", "1"), ("v1.2.3+build", "v1.2.3"),
    ("1.0\x1c", "1.0"), ("v\u0663\u0660", "30"),
]

PREFIXES = [
    "feat: add x", "fix(api): y", "Docs(readme): z", "FEAT(scope with space): w", "feat(): nope",
    "feat(a)b: nope", "1fix: nope", "fix : nope", "  chore: trim", "", "   ", "feat", "feat:",
    "feat(x(y)): z", "feat(x)(y): z", "\u00e9t\u00e9: no", "revert(\u00e9): yes", "fix\r: x",
    "feat(a\nb): c",
]

PLATFORM_NAMES = [
    ["app.msi", "app.exe", "app.AppImage", "app.deb", "app.rpm", "app.tar.gz", "app.apk", "notes.txt"],
    ["b.EXE", "a.exe", "c.MSI", "A.msi"],
    ["x.tar.GZ", "x.gz", "x.Deb", "y.appimage", "z.APK.apk"],
    ["", "exe", ".exe", "\u0130.exe", "K.APK", "\u212a.apk"],
    [],
]

BLOCK_INPUTS = [
    ("mod", "5, 6 6\t7", "  hi  "), ("author", "12\n13,,14", ""), ("hash", "ABCDEF0123456789ABCDEF0123456789", ""),
    ("hash", "abc", ""), ("mod", " , ", ""), ("mod", "", ""), ("mod", "a b c d e f g", ""),
    ("workshop", "123\u001f456", "note\u3000"), ("workshop_author", "76561198000000000 7656119800000000", ""),
    ("workshop_author", "76561198000000000", ""), ("mod", "\u0661\u0662", ""), ("mod", "1,1,1", "x"),
    ("hash", "0123456789abcdef0123456789abcdef 0123456789ABCDEF0123456789ABCDEF", ""),
    ("mod", "\u00c9", ""),
]
BLOCK_DUPS = {"6", "13"}

NORMALIZE = [
    {"id": 101, "tag_name": "v1.0.0", "html_url": "https://h/1", "published_at": "2026-01-02T03:04:05Z",
     "name": "  Release 1  ", "body": "notes", "prerelease": False, "draft": False,
     "assets": [{"name": "a.msi", "browser_download_url": "https://d/a.msi", "size": 10,
                 "content_type": "application/x-msi", "download_count": 3},
                {"name": "b.apk", "browser_download_url": "https://d/b.apk"},
                {"name": "", "browser_download_url": "https://d/c"},
                {"name": "c", "browser_download_url": None}, "junk",
                {"name": "d.exe", "browser_download_url": "https://d/d", "size": "12", "download_count": 2.9,
                 "content_type": 5}]},
    {"id": 3000000000, "tag_name": "v2", "html_url": "https://h/2", "published_at": "2026-01-02T03:04:05.123456Z",
     "name": None, "body": None, "prerelease": 1, "assets": None},
    {"id": 102, "tag_name": "v3", "html_url": "https://h/3", "published_at": "2026-01-02T03:04:05+02:00",
     "assets": {"a": 1}},
    {"id": 103, "tag_name": "v4", "html_url": "https://h/4", "published_at": "2026-01-02T03:04:05.5Z",
     "assets": "abc", "name": "\u3000x\u3000"},
    {"id": 104, "tag_name": "v5", "html_url": "https://h/5", "published_at": "2026-01-02T03:04:05Z", "draft": True},
    {"id": "105", "tag_name": "v6", "html_url": "https://h/6", "published_at": "2026-01-02T03:04:05Z"},
    {"id": 106.0, "tag_name": "v7", "html_url": "https://h/7", "published_at": "2026-01-02T03:04:05Z"},
    {"id": 107, "tag_name": "", "html_url": "https://h/8", "published_at": "2026-01-02T03:04:05Z"},
    {"id": 108, "tag_name": "v9", "html_url": "https://h/9", "published_at": "not a date"},
    {"id": 109, "tag_name": "v10", "html_url": "https://h/10", "published_at": None},
    {"id": 110, "tag_name": "v11", "html_url": "https://h/11"},
    {"id": 111, "tag_name": "v12", "html_url": "https://h/12", "published_at": 5},
    {"id": 112, "tag_name": "v13", "html_url": "https://h/13", "published_at": "2026-01-02T03:04:05Z",
     "assets": [{"name": "x.exe", "browser_download_url": "u", "size": "big"}]},
    {"id": 113, "tag_name": "v14", "html_url": "https://h/14", "published_at": "2026-01-02T03:04:05Z", "name": 7},
    {"id": 114, "tag_name": "v15", "html_url": "https://h/15", "published_at": "2026-01-02T03:04:05Z", "assets": 3},
    {"id": 115, "tag_name": "v16", "html_url": "https://h/16", "published_at": "2026-01-02 03:04:05",
     "assets": [{"name": 5, "browser_download_url": ["u"], "size": True, "download_count": False}]},
    "not a dict", None, [],
]

TAGS = [
    {"name": "v1.1.0", "commit": {"sha": "c3"}},
    {"name": "v1.0.0", "commit": {"sha": "c5"}},
    {"name": "v0.9", "commit": {"sha": "nope"}},
    {"name": "", "commit": {"sha": "c4"}},
    {"name": "dup", "commit": {"sha": "c3x"}},
    {"name": "v1.0.0-again", "commit": {"sha": "c5"}},
    None, {}, {"commit": None}, {"name": "x", "commit": {}},
]
COMMITS = [
    {"sha": "c1aaaaaaaaaa", "commit": {"message": "feat(ui): add thing\n\nlong body"}, "html_url": "https://c/1"},
    {"sha": "c2", "commit": {"message": "  Fix: trim me  \r\nmore"}, "html_url": None},
    {"sha": "c3", "commit": {"message": "chore: release 1.1"}, "html_url": "https://c/3"},
    {"sha": "c4", "commit": {"message": "no prefix here"}},
    {"sha": "", "commit": {"message": "skipped: no sha"}},
    {"sha": "c6", "commit": {"message": "   "}},
    {"sha": "c7", "commit": None},
    "junk",
    {"sha": "c5", "commit": {"message": "\u00e9t\u00e9: unicode first line \u2713"}, "html_url": "https://c/5"},
    {"sha": "c8\u00e9\u00e9\u00e9\u00e9\u00e9\u00e9", "commit": {"message": "docs: tail"}, "html_url": 12},
]
GROUP_CASES = [
    (TAGS, COMMITS), ([], COMMITS), (TAGS, []), ([{"name": "v0", "commit": {"sha": "c1aaaaaaaaaa"}}], COMMITS[:2]),
    (["bad tag"], COMMITS), ([{"name": "t", "commit": "bad"}], COMMITS), ([], [{"sha": 5, "commit": {"message": "x"}}]),
    ([], [{"sha": "s", "commit": {"message": 5}}]), ([], [{"sha": "s", "commit": "bad"}]),
    ([{"name": "t", "commit": {"sha": ["x"]}}], []),
    ([{"name": 5, "commit": {"sha": "c3"}}], COMMITS),
]

RATE_BODIES = [
    {"message": "API rate limit exceeded for 1.2.3.4."}, {"message": "Not Found"}, {"message": "RATE LIMIT"},
    {}, [], {"message": None}, {"message": ["rate limit"]}, {"message": 5}, "rate limit",
]

STEAM_DETAILS = [
    {"result": 1, "publishedfileid": "111", "title": "  Mod A  ", "short_description": " short ",
     "file_description": "long", "creator": "76561198000000001", "preview_url": "https://p/1", "file_size": "1024",
     "time_created": 1700000000, "time_updated": 1700000100, "subscriptions": 50, "favorited": 5, "visibility": 0,
     "tags": [{"tag": "Costume"}, {"tag": ""}, {"display_name": "x"}, "junk", {"tag": 5}]},
    {"result": 1, "publishedfileid": "222", "title": "B", "file_description": " only long ", "creator": "7656",
     "file_size": "3000000000", "subscriptions": "12", "visibility": "0"},
    {"result": 9, "publishedfileid": "333"},
    {"result": 1, "publishedfileid": "444", "banned": True},
    {"result": 1, "publishedfileid": "555", "visibility": 1},
    {"result": 1, "publishedfileid": "", "title": "no id"},
    {"publishedfileid": 666, "title": None, "creator": 76561198000000002, "time_created": 1.9, "favorited": "x",
     "subscriptions": True, "file_size": [1], "tags": None, "visibility": "  0  "},
    {"result": True, "publishedfileid": "777", "visibility": None, "tags": "abc"},
    {"result": 1.0, "publishedfileid": "888", "visibility": 0.0, "short_description": "", "file_description": 0},
    {"result": "1", "publishedfileid": "999"},
    "junk", None,
]


def pure_section() -> dict:
    out: dict = {}
    out["versions"] = [
        {"a": a, "b": b, "parse_a": (lambda p: None if p is None else [list(p[0]), p[1]])(btt_releases.parse_version(a)),
         "cmp": btt_releases.compare_versions(a, b)}
        for a, b in VERSIONS
    ]
    out["prefixes"] = [{"msg": m, "prefix": btt_releases.parse_conventional_prefix(m)} for m in PREFIXES]
    order = []
    for names in PLATFORM_NAMES:
        assets = [{"name": n, "i": i} for i, n in enumerate(names)]
        for p in btt_releases.PLATFORMS:
            order.append({"names": names, "platform": p,
                          "order": [a["i"] for a in btt_releases.assets_for_platform(assets, p)]})
    out["platform_order"] = order
    out["normalize"] = []
    for raw in NORMALIZE:
        try:
            norm = btt_releases.normalize_release(copy.deepcopy(raw))
            res = {"bson": None if norm is None else bson.encode({"doc": norm}).hex()}
        except Exception as e:  # noqa: BLE001
            res = {"error": type(e).__name__}
        out["normalize"].append({"raw": raw, **res})
    out["groups"] = []
    for tags, commits in GROUP_CASES:
        try:
            groups = btt_releases.build_changelog_groups(copy.deepcopy(tags), copy.deepcopy(commits))
            res = {"bson": bson.encode({"groups": groups}).hex()}
        except Exception as e:  # noqa: BLE001
            res = {"error": type(e).__name__}
        out["groups"].append({"tags": tags, "commits": commits, **res})
    out["rate_limited"] = [
        {"body": b, "value": isinstance(b, dict) and "rate limit" in str(b.get("message", "")).lower()}
        for b in RATE_BODIES
    ]
    items = workshop.parse_items(copy.deepcopy(STEAM_DETAILS))
    out["steam"] = {"details": STEAM_DETAILS, "bson": bson.encode({"items": items}).hex()}
    return out


def github_sample() -> dict:
    """Real payloads from the configured repo (refreshed on every run)."""
    import httpx

    from app.core.config import settings

    base = f"https://api.github.com/repos/{settings.btt_releases_repo}/"
    get = lambda path: httpx.get(base + path, headers=btt_releases._gh_headers(), timeout=15).raise_for_status().json()  # noqa: E731
    releases, tags, commits = get("releases?per_page=10"), get("tags?per_page=100"), get("commits?per_page=40")
    keep = ("id", "tag_name", "html_url", "published_at", "name", "body", "prerelease", "draft")
    keep_asset = ("name", "browser_download_url", "size", "content_type", "download_count")
    releases = [{**{k: r[k] for k in keep if k in r},
                 "assets": [{k: a[k] for k in keep_asset if k in a} for a in r.get("assets") or []]}
                for r in releases]
    tags = [{"name": t.get("name"), "commit": {"sha": (t.get("commit") or {}).get("sha")}} for t in tags]
    commits = [{"sha": c.get("sha"), "commit": {"message": (c.get("commit") or {}).get("message")},
                "html_url": c.get("html_url")} for c in commits]
    norm = [btt_releases.normalize_release(r) for r in releases]
    return {
        "releases": releases, "tags": tags, "commits": commits,
        "releases_bson": [None if n is None else bson.encode({"doc": n}).hex() for n in norm],
        "groups_bson": bson.encode({"groups": btt_releases.build_changelog_groups(tags, commits)}).hex(),
    }


async def block_section() -> list:
    out = []
    orig_insert = BttModBlock.insert

    async def fake_insert(self, *a, **k):
        if self.value in BLOCK_DUPS:
            raise DuplicateKeyError("E11000 duplicate key")
        self.id = PydanticObjectId("0123456789abcdef01234567")
        return self

    BttModBlock.insert = fake_insert
    try:
        for kind, raw, note in BLOCK_INPUTS:
            try:
                added = await blocklist.add(kind, raw, note)
                res = {"added": [[a["value"], a["note"]] for a in added]}
            except Exception as e:  # noqa: BLE001
                res = {"status": e.status_code, "message": e.message}
            out.append({"kind": kind, "raw": raw, "note": note, **res})
    finally:
        BttModBlock.insert = orig_insert
    return out


# --- route cases ---------------------------------------------------------------------

def asset(name, size=100, ctype="application/octet-stream", count=1, **extra):
    a = {"name": name, "url": f"https://dl.example/{name}", "size": size}
    if ctype is not ...:
        a["content_type"] = ctype
    if count is not ...:
        a["download_count"] = count
    a.update(extra)
    return a


# Newest first, as the store returns them.
RELEASES = [
    {"_id": O("65a000000000000000000005"), "release_id": 205, "tag_name": "v1.4.0-beta.1", "name": "Beta \u2713",
     "body": "Beta \"notes\"\n- one\t\u00e9", "html_url": "https://github.com/x/releases/tag/v1.4.0-beta.1",
     "prerelease": True, "published_at": D(1759320000000),
     "assets": [asset("BTT_1.4.0_x64.msi"), asset("BTT_1.4.0_x64.exe", ctype=None)],
     "fetched_at": D(1759406400123)},
    {"_id": O("65a000000000000000000004"), "release_id": 3000000004, "tag_name": "v1.3.0", "name": "1.3.0",
     "body": "", "html_url": "https://github.com/x/releases/tag/v1.3.0", "prerelease": False,
     "published_at": D(1756728000000),
     "assets": [asset("btt_1.3.0_amd64.AppImage", size=3000000000, count=4000000000),
                asset("btt_1.3.0_amd64.deb", extra_key="ignored"), asset("btt.apk")],
     "fetched_at": D(1759406400000)},
    {"_id": O("65a000000000000000000003"), "release_id": 203, "tag_name": "v1.2.0", "name": "1.2.0",
     "body": "two", "html_url": "https://github.com/x/releases/tag/v1.2.0", "prerelease": False,
     "published_at": D(1754049600500),
     "assets": [asset("b.exe"), asset("a.exe"), asset("BTT.MSI", ctype=..., count=...), asset("btt.tar.gz"),
                asset("btt-old.apk")],
     "fetched_at": D(1759406400999)},
    {"_id": O("65a000000000000000000002"), "release_id": 202, "tag_name": "v1.1.0-rc.1", "html_url": "https://h/rc",
     "prerelease": True, "published_at": D(1751371200000), "assets": [asset("rc.apk")],
     "fetched_at": D(1751371200001)},
    {"_id": O("65a000000000000000000001"), "release_id": 201, "tag_name": "v1.0.0", "html_url": "https://h/1",
     "published_at": D(1748779200000), "fetched_at": D(1748779200000)},
]

CHANGELOG = {
    "_id": O("65a0000000000000000000c1"), "repo": "AallynReed/BetterTroveTools",
    "groups": [
        {"version": "Unreleased", "commits": [
            {"sha": "aaaaaaaaaaaa1", "short_sha": "aaaaaaa", "message": "feat(ui): new \u2713", "type": "feat",
             "url": "https://c/1"},
            {"sha": "bbbbbbbbbbbb2", "short_sha": "bbbbbbb", "message": "no prefix", "type": None, "url": ""}]},
        {"version": "v1.3.0", "commits": [
            {"sha": "cccccccccccc3", "short_sha": "ccccccc", "message": "fix: \"quoted\"", "type": "fix",
             "url": "https://c/3"}]},
        {"version": "v1.2.0", "commits": [
            {"sha": "d4", "short_sha": "d4", "message": "a", "type": "chore", "url": "u4"},
            {"sha": "e5", "short_sha": "e5", "message": "b", "url": "u5"},
            {"sha": "f6", "short_sha": "f6", "message": "c", "type": "docs", "url": "u6", "extra": 1}]},
    ],
    "rate_limited": True, "fetched_at": D(1759406400456),
}

# Most subscribed first, as the store returns them.
WORKSHOP = [
    {"_id": O("65a0000000000000000000a1"), "published_file_id": "222", "title": "Blocked item",
     "creator": "76561198000000009", "subscriptions": 90, "seen_at": D(1759406400000)},
    {"_id": O("65a0000000000000000000a2"), "published_file_id": "111", "title": "Mod A \u2713",
     "description": "desc \"q\"", "creator": "76561198000000001", "creator_name": "Alice", "preview_url": "https://p/1",
     "file_size": 3000000000, "created": 1700000000, "updated": 1700000100, "subscriptions": 50, "favorites": 5,
     "tags": ["Costume", "Mount"], "seen_at": D(1759406400456)},
    {"_id": O("65a0000000000000000000a3"), "published_file_id": "333", "title": "By blocked author",
     "creator": "76561198000000077", "subscriptions": 40, "seen_at": D(1759406400100)},
    {"_id": O("65a0000000000000000000a4"), "published_file_id": "444", "seen_at": D(1759406400000)},
]

# Oldest first, as the store returns them.
BLOCKLIST = [
    {"_id": O("65a0000000000000000000b1"), "kind": "author", "value": "17", "note": "spam",
     "created_at": D(1756728000000)},
    {"_id": O("65a0000000000000000000b2"), "kind": "hash", "value": "0123456789abcdef0123456789abcdef", "note": "",
     "created_at": D(1756728000001)},
    {"_id": O("65a0000000000000000000b3"), "kind": "workshop", "value": "222", "note": "n \u2713",
     "created_at": D(1756728000002)},
    {"_id": O("65a0000000000000000000b4"), "kind": "workshop_author", "value": "76561198000000077",
     "created_at": D(1756728000003)},
    {"_id": O("65a0000000000000000000b5"), "kind": "mod", "value": "42", "note": "x",
     "created_at": D(1756728000123)},
    {"_id": O("65a0000000000000000000b6"), "kind": "hash", "value": "ffffffffffffffffffffffffffffffff", "note": "",
     "created_at": D(1756728000124)},
]
TAKEDOWN_MD5S = ["eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee", "0123456789abcdef0123456789abcdef",
                 "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]
EXISTING_ENTRY = "65a0000000000000000000b5"

J = {"content-type": "application/json"}
A = {"x-test-auth": "1"}
AJ = {**A, **J}


def jb(v) -> bytes:
    return json.dumps(v).encode()


# (name, method, url, headers, body, flags). Flags: "real" = the Rust test uses
# the production router (no handler runs); "nochangelog" = empty changelog store;
# "mask" = JSON paths whose values are generated per call.
CASES = [
    ("releases", "GET", "/v1/btt/releases", {}, None, {}),
    ("releases_release", "GET", "/v1/btt/releases?channel=release", {}, None, {}),
    ("releases_beta", "GET", "/v1/btt/releases?channel=beta", {}, None, {}),
    ("releases_page", "GET", "/v1/btt/releases?limit=2&offset=1", {}, None, {}),
    ("releases_past_end", "GET", "/v1/btt/releases?offset=100", {}, None, {}),
    ("releases_last_wins", "GET", "/v1/btt/releases?limit=1&limit=3", {}, None, {}),
    ("releases_limit_str_float", "GET", "/v1/btt/releases?limit=2.0", {}, None, {}),
    ("releases_bad_channel", "GET", "/v1/btt/releases?channel=nightly", {}, None, {}),
    ("releases_empty_channel", "GET", "/v1/btt/releases?channel=", {}, None, {}),
    ("releases_limit_0", "GET", "/v1/btt/releases?limit=0", {}, None, {}),
    ("releases_limit_201", "GET", "/v1/btt/releases?limit=201&offset=-1", {}, None, {}),
    ("releases_limit_abc", "GET", "/v1/btt/releases?limit=abc&offset=1.5&channel=nope", {}, None, {}),
    ("latest", "GET", "/v1/btt/latest", {}, None, {}),
    ("latest_beta", "GET", "/v1/btt/latest?channel=beta", {}, None, {}),
    ("latest_bad", "GET", "/v1/btt/latest?channel=Release", {}, None, {}),
    ("check_windows_old", "GET", "/v1/btt/check?installed=v1.1.0&platform=windows", {}, None, {}),
    ("check_linux_same", "GET", "/v1/btt/check?installed=1.3&platform=linux", {}, None, {}),
    ("check_android_newer", "GET", "/v1/btt/check?installed=v9.0.0&platform=android", {}, None, {}),
    ("check_garbage", "GET", "/v1/btt/check?installed=garbage&platform=windows", {}, None, {}),
    ("check_beta", "GET", "/v1/btt/check?installed=v1.4.0-alpha&platform=windows&channel=beta", {}, None, {}),
    ("check_beta_linux_none", "GET", "/v1/btt/check?installed=1&platform=linux&channel=beta", {}, None, {}),
    ("check_missing", "GET", "/v1/btt/check", {}, None, {}),
    ("check_empty_installed", "GET", "/v1/btt/check?installed=&platform=windows", {}, None, {}),
    ("check_bad_platform", "GET", "/v1/btt/check?installed=1&platform=mac&channel=bad", {}, None, {}),
    ("check_bad_channel", "GET", "/v1/btt/check?installed=1&platform=windows&channel=bad", {}, None, {}),
    ("check_unicode", "GET", "/v1/btt/check?installed=%E2%9C%93&platform=android", {}, None, {}),
    ("platform_windows", "GET", "/v1/btt/latest/windows", {}, None, {}),
    ("platform_linux", "GET", "/v1/btt/latest/linux", {}, None, {}),
    ("platform_android_beta", "GET", "/v1/btt/latest/android?channel=beta", {}, None, {}),
    ("platform_linux_beta_none", "GET", "/v1/btt/latest/linux?channel=beta", {}, None, {}),
    ("platform_unknown", "GET", "/v1/btt/latest/mac?channel=bad", {}, None, {}),
    ("platform_upper", "GET", "/v1/btt/latest/WINDOWS", {}, None, {}),
    ("platform_unicode", "GET", "/v1/btt/latest/%E2%9C%93%20x", {}, None, {}),
    ("platform_bad_channel", "GET", "/v1/btt/latest/windows?channel=x", {}, None, {}),
    ("changelog", "GET", "/v1/btt/changelog", {}, None, {}),
    ("changelog_limit", "GET", "/v1/btt/changelog?limit_groups=2&commits_per_group=1", {}, None, {}),
    ("changelog_commits", "GET", "/v1/btt/changelog?commits_per_group=2", {}, None, {}),
    ("changelog_bad", "GET", "/v1/btt/changelog?limit_groups=51&commits_per_group=-1", {}, None, {}),
    ("changelog_none", "GET", "/v1/btt/changelog?limit_groups=1", {}, None,
     {"nochangelog": True, "mask": ["fetched_at"]}),
    ("workshop", "GET", "/v1/btt/workshop", {}, None, {}),
    ("mod_blocklist", "GET", "/v1/btt/mod-blocklist", {}, None, {}),
    ("admin_list", "GET", "/admin/btt/mod-blocklist", A, None, {}),
    ("admin_list_noauth", "GET", "/admin/btt/mod-blocklist", {}, None, {"real": True}),
    ("admin_add_noauth", "POST", "/admin/btt/mod-blocklist", J, jb({"kind": "mod", "values": "1"}), {"real": True}),
    ("admin_add_noauth_badjson", "POST", "/admin/btt/mod-blocklist", J, b'{"kind": ', {"real": True}),
    ("admin_add_noauth_missing", "POST", "/admin/btt/mod-blocklist", {}, None, {"real": True}),
    ("admin_add_badjson", "POST", "/admin/btt/mod-blocklist", AJ, b'{"kind": "mod" "values"}', {}),
    ("admin_add_missing", "POST", "/admin/btt/mod-blocklist", A, None, {}),
    ("admin_add_not_object", "POST", "/admin/btt/mod-blocklist", AJ, jb([1]), {}),
    ("admin_add_invalid", "POST", "/admin/btt/mod-blocklist", AJ,
     jb({"kind": "user", "values": "x" * 20001, "note": 5}), {}),
    ("admin_add_kind_null", "POST", "/admin/btt/mod-blocklist", AJ, jb({"kind": None}), {}),
    ("admin_add_bad_hash", "POST", "/admin/btt/mod-blocklist", AJ, jb({"kind": "hash", "values": "abc, xyz"}), {}),
    ("admin_add_empty", "POST", "/admin/btt/mod-blocklist", AJ, jb({"kind": "mod", "values": " ,\n"}), {}),
    ("admin_add_ok", "POST", "/admin/btt/mod-blocklist", AJ,
     jb({"kind": "mod", "values": "5, 6 6\t7", "note": "  hi \u2713 "}),
     {"mask": ["added.*.id", "added.*.created_at"]}),
    ("admin_add_all_dups", "POST", "/admin/btt/mod-blocklist", AJ, jb({"kind": "author", "values": "13 6"}), {}),
    ("admin_remove", "DELETE", f"/admin/btt/mod-blocklist/{EXISTING_ENTRY}", A, None, {}),
    ("admin_remove_unknown", "DELETE", "/admin/btt/mod-blocklist/65a0000000000000000000ff", A, None, {}),
    ("admin_remove_bad_id", "DELETE", "/admin/btt/mod-blocklist/not-an-id", A, None, {}),
    ("admin_remove_upper", "DELETE", f"/admin/btt/mod-blocklist/{EXISTING_ENTRY.upper()}", A, None, {}),
    ("admin_remove_noauth", "DELETE", f"/admin/btt/mod-blocklist/{EXISTING_ENTRY}", {}, None, {"real": True}),
    ("put_405", "PUT", "/admin/btt/mod-blocklist", {}, None, {"real": True}),
    ("delete_405", "DELETE", "/admin/btt/mod-blocklist", {}, None, {"real": True}),
    ("get_entry_405", "GET", f"/admin/btt/mod-blocklist/{EXISTING_ENTRY}", {}, None, {"real": True}),
    ("post_releases_405", "POST", "/v1/btt/releases", {}, None, {"real": True}),
    ("post_platform_405", "POST", "/v1/btt/latest/windows", {}, None, {"real": True}),
]


class FakeQuery:
    def __init__(self, rows, minus_reverses: bool):
        self.rows = rows
        self.minus_reverses = minus_reverses

    def sort(self, key):
        if self.minus_reverses and isinstance(key, str) and key.startswith("-"):
            return FakeQuery(list(reversed(self.rows)), False)
        return self

    async def to_list(self):
        return list(self.rows)


class FakeColl:
    async def distinct(self, field, filt):
        return list(TAKEDOWN_MD5S)


class FakeEntry:
    async def delete(self):
        return None


STATE = {"changelog": True}


def install_fakes() -> None:
    releases = [BttRelease.model_validate(to_py(d)) for d in RELEASES]

    async def list_releases(channel, limit, offset):
        docs = [r for r in releases if channel not in btt_releases.CHANNELS
                or r.prerelease == btt_releases.CHANNELS[channel]]
        return docs[offset:offset + limit], len(docs)

    async def latest_per_platform(channel):
        docs = [r for r in releases if r.prerelease == btt_releases.CHANNELS[channel]]
        return {p: btt_releases.walk_latest(docs, p) for p in btt_releases.PLATFORMS}

    async def get_changelog():
        return BttChangelog.model_validate(to_py(CHANGELOG)) if STATE["changelog"] else None

    btt_releases.list_releases = list_releases
    btt_releases.latest_per_platform = latest_per_platform
    btt_releases.get_changelog = get_changelog
    rows = [SteamWorkshopItem.model_validate(to_py(d)) for d in WORKSHOP]
    SteamWorkshopItem.find = classmethod(lambda cls, *a, **k: FakeQuery(rows, False))
    entries = [BttModBlock.model_validate(to_py(d)) for d in BLOCKLIST]
    BttModBlock.find = classmethod(lambda cls, *a, **k: FakeQuery(entries, True))
    TakenDownArtifact.get_pymongo_collection = classmethod(lambda cls: FakeColl())

    async def get(cls, oid, *a, **k):
        return FakeEntry() if str(oid) == EXISTING_ENTRY else None

    BttModBlock.get = classmethod(get)

    async def fake_insert(self, *a, **k):
        if self.value in BLOCK_DUPS:
            raise DuplicateKeyError("E11000 duplicate key")
        self.id = PydanticObjectId()
        return self

    BttModBlock.insert = fake_insert


async def call(app, method: str, url: str, headers: list[tuple[bytes, bytes]], body: bytes):
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
    return start["status"], [(k.decode("latin-1").lower(), v.decode("latin-1")) for k, v in start["headers"]], raw


def mask(value, path: list[str]):
    if not path:
        return "<masked>"
    head, rest = path[0], path[1:]
    if head == "*":
        return [mask(v, rest) for v in value]
    return {k: (mask(v, rest) if k == head else v) for k, v in value.items()}


MASK_FORMATS = {
    "fetched_at": r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(\.\d{6})?Z$",
    "added.*.created_at": r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(\.\d{6})?\+00:00$",
    "added.*.id": r"^[0-9a-f]{24}$",
}


def check_format(value, path: list[str], pattern: str):
    if not path:
        assert re.match(pattern, value), (value, pattern)
        return
    head, rest = path[0], path[1:]
    for v in (value if head == "*" else [value[head]]):
        check_format(v, rest, pattern)


async def http_section(app) -> list:
    async def superuser():
        return object()

    async def anonymous():
        return AccessContext(user=None, token=None, ip=None)

    out = []
    for i, (name, method, url, headers, body, flags) in enumerate(CASES):
        STATE["changelog"] = not flags.get("nochangelog")
        app.dependency_overrides.clear()
        app.dependency_overrides[trove_router._BTT.dependency] = anonymous
        if "x-test-auth" in headers:
            app.dependency_overrides[get_current_superuser] = superuser
        rid = f"parity-btt-{i:03d}"
        sent = [(k.encode(), v.encode()) for k, v in headers.items() if k != "x-test-auth"]
        sent.append((b"x-request-id", rid.encode()))
        status, rh, raw = await call(app, method, url, sent, body or b"")
        get = lambda k: next((v for n, v in rh if n == k), None)  # noqa: E731
        text = raw.decode("utf-8")
        for m in flags.get("mask", []):
            parsed = json.loads(text)
            check_format(parsed, m.split("."), MASK_FORMATS[m])
            text = json.dumps(mask(parsed, m.split(".")), ensure_ascii=False, separators=(",", ":"))
        out.append({
            "name": name, "method": method, "url": url, "headers": headers,
            "body": None if body is None else body.decode("utf-8"),
            "real": bool(flags.get("real")), "nochangelog": bool(flags.get("nochangelog")),
            "mask": flags.get("mask", []), "request_id": rid,
            "status": status, "content_type": get("content-type"), "allow": get("allow"),
            "content_length": get("content-length"), "response": text,
        })
    app.dependency_overrides.clear()
    return out


async def main() -> None:
    async def no_server(self):
        self._database_major_version = 7
        self._existing_collections = []

    beanie_init.Initializer._load_cached_info = no_server
    client = AsyncMongoClient("mongodb://127.0.0.1:1/", tz_aware=True, connect=False)
    await init_beanie(database=client["fixtures"], document_models=[
        BttRelease, BttChangelog, BttModBlock, SteamWorkshopItem, TakenDownArtifact,
    ], skip_indexes=True)
    for key, spec in rc.REGISTRY.items():
        rc._CACHE[key] = (spec.default, float("inf"))
    data = {"python": sys.version.split()[0], **pure_section(), "block_values": await block_section(),
            "github_sample": github_sample()}
    install_fakes()
    from app.main import app
    data["store"] = {"releases": RELEASES, "changelog": CHANGELOG, "workshop": WORKSHOP, "blocklist": BLOCKLIST,
                     "takedown_md5s": TAKEDOWN_MD5S, "existing_entry": EXISTING_ENTRY,
                     "dups": sorted(BLOCK_DUPS), "repo": "AallynReed/BetterTroveTools"}
    data["http"] = await http_section(app)
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(data, ensure_ascii=True, indent=1) + "\n", encoding="utf-8")
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes) with Python {data['python']}")


if __name__ == "__main__":
    asyncio.run(main())
