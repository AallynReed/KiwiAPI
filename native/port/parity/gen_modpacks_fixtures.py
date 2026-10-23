"""Golden fixtures for Modpacks (app/trove/modpacks), from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_modpacks_fixtures.py
Writes native/kiwi-server/testdata/modpacks/golden.json.

The service runs for real with its store calls replaced by in-memory fakes:
the string helpers, the document Beanie stores for a loaded / created / edited
pack (save() and insert() capture the encoded model), the card / detail /
catalog views, entry resolution, conflict messages, the per-mod configs and
the built .tpack / .zip / configs-zip bytes (zipfile's clock frozen). The
replies settled before any store is touched (validation, auth, form parsing,
405s) come from the real app over raw ASGI. The clock is frozen.
"""
import asyncio
import base64
import copy
import datetime as dt
import json
import logging
import os
import sys
import time
import types
import urllib.parse
import warnings
import zipfile
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
sys.path.insert(0, os.getcwd())
ENV = {
    "SECRET_KEY": "modpacks-fixture-secret",
    "API_URL": "https://api.example.test//",
    "APP_URL": "https://trove.example.test/",
}
os.environ.update(ENV)

import pydantic_core  # noqa: E402
from bson import ObjectId  # noqa: E402
from bson.json_util import CANONICAL_JSON_OPTIONS, dumps as bson_dumps, loads as bson_loads  # noqa: E402
from pymongo.asynchronous.database import AsyncDatabase  # noqa: E402

import app.core.utils as cu  # noqa: E402

NOW = dt.datetime(2026, 10, 23, 11, 22, 33, 456789, tzinfo=dt.timezone.utc)
ZIP_TIME = (2026, 10, 23, 11, 22, 34)


class _Frozen(dt.datetime):
    @classmethod
    def now(cls, tz=None):
        return NOW


cu.datetime = _Frozen

from app.admin import runtime_config as rc  # noqa: E402

for _key, _spec in rc.REGISTRY.items():
    rc._CACHE[_key] = (_spec.default, float("inf"))

from beanie.odm.utils.encoder import Encoder  # noqa: E402

from app.core.dependencies import get_current_superuser  # noqa: E402
from app.core.errors import APIError  # noqa: E402
from app.main import app  # noqa: E402
from app.site_auth.dependencies import get_current_site_user  # noqa: E402
from app.site_auth.models import SiteUser  # noqa: E402
from app.trove import tmod  # noqa: E402
from app.trove.modpacks import service as S  # noqa: E402
from app.trove.modpacks.models import ModpackProject, ModpackStar  # noqa: E402
from app.trove.modpacks.schemas import CreateModpackRequest, ModpackEntryInput, UpdateModpackRequest  # noqa: E402
from app.trove.mods_hub import service as hub  # noqa: E402
from app.trove.mods_hub.models import ModImageAsset, ModProject, ModRelease  # noqa: E402

assert tmod._native is None, "generate with the pure-Python reference"
OUT = Path(__file__).resolve().parents[2] / "kiwi-server" / "testdata" / "modpacks" / "golden.json"
KEPT = ("content-type", "allow", "www-authenticate", "cache-control")


async def _cmd(self, *a, **k):
    return {"version": "7.0.0"}


async def _lcn(self, *a, **k):
    return []


AsyncDatabase.command = _cmd
AsyncDatabase.list_collection_names = _lcn


async def _init():
    from beanie import init_beanie
    from pymongo import AsyncMongoClient
    client = AsyncMongoClient("mongodb://127.0.0.1:1", tz_aware=True, connect=False, serverSelectionTimeoutMS=200)
    await init_beanie(database=client["kiwi"], skip_indexes=True,
                      document_models=[ModpackProject, ModpackStar, ModProject, ModRelease, ModImageAsset, SiteUser])


asyncio.run(_init())


def run(coro):
    return asyncio.new_event_loop().run_until_complete(coro)


def ext(v):
    return json.loads(bson_dumps(v, json_options=CANONICAL_JSON_OPTIONS))


def from_ext(v):
    # tz-aware like the app's Mongo client (tz_aware=True).
    return bson_loads(json.dumps(v), json_options=CANONICAL_JSON_OPTIONS.with_options(tz_aware=True))


def to_json(v) -> str:
    return pydantic_core.to_json(v).decode()


def stored(doc) -> dict:
    return ext(Encoder(to_db=True, keep_nulls=True).encode(doc))


def err(e: APIError) -> dict:
    return {"status": e.status_code, "code": e.code.value if hasattr(e.code, "value") else str(e.code),
            "message": e.message}


def b64(b: bytes) -> str:
    return base64.b64encode(b).decode()


# --- string helpers --------------------------------------------------------------

STRINGS = ["", "Hello World", "  Café Ünïcode!!  ", "---", "A" * 70 + " tail", "İstanbul", "x/y\\z?",
           "日本語", "ﬁne straße", "ΣΑΣ", "a  b\tc\u3000d", "K-Kelvin", "9 lives", "Ǆungla"]
TAGS = [[], ["  Fun ", "fun", "FUN  stuff", "a\u3000\u3000b", "x" * 40, "", "  "], [f"t{i}" for i in range(15)],
        ["İ", "ß", "Ω"]]
URLS = [None, "", "   ", "https://x.y", " HTTP://x ", "httpſ://x", "https://", "ftp://x", "https://a b",
        "http://" + "x" * 300, "http://" + "x" * 292, "https://x\u00a0y", "Https://ok.example/path?q=1"]
FILENAMES = [None, "", "  report  ", "a<b>c:d\"e/f\\g|h?i*j", "...", " .hidden. ", "x" * 130, "Ünï ✓", "\x01\x1f\x7f"]
ESCAPE = ["a.b*c", "(x)[y]{z}?+-|^$\\&~# \t\n\r\v\f", "plain", "ÄÖ.*"]
QUOTE = ["default", "spin off", "a/b", "é~_.-", "100%"]
CASEFOLD = ["Straße", "ﬁle", "ΣΑΣ ς", "İ", "ǅ", "Ꭰ", "ẞ", "Kelvin K", "ŉ", "ﬆ", "Ω", "MiXeD"]
SORT = ["banana", "Apple", "apple", "Äpfel", "straße", "STRASSE", "ﬁsh", "fish", "", "Zed", "zed", "ǆ", "Ǆ", "ǅ"]


def strings():
    def url_case(u, field):
        try:
            return {"ok": S._clean_url(u, field=field)}
        except APIError as e:
            return {"error": err(e)}

    return {
        "slugify": [[s, S._slugify(s)] for s in STRINGS],
        "tags": [[t, S._clean_tags(t)] for t in TAGS],
        "urls": [[u, f, url_case(u, f)] for i, u in enumerate(URLS) for f in [("Discord", "support")[i % 2]]],
        "filenames": [[n, S._safe_filename(n), hub._safe_filename(n)] for n in FILENAMES],
        "escape": [[s, __import__("re").escape(s)] for s in ESCAPE],
        "quote": [[s, urllib.parse.quote(s)] for s in QUOTE],
        "casefold": [[s, s.casefold()] for s in CASEFOLD],
        "sorted": sorted(SORT, key=lambda s: s.casefold()),
        "sort_input": SORT,
    }


# --- documents ---------------------------------------------------------------------

OWNER = ObjectId("65d000000000000000000001")
COLLAB = ObjectId("65d000000000000000000002")
OTHER = ObjectId("65d000000000000000000003")
MOD_A = ObjectId("65e000000000000000000001")
MOD_B = ObjectId("65e000000000000000000002")
PACK_ID = ObjectId("65f000000000000000000001")
T1 = dt.datetime(2026, 9, 1, 8, 0, 0, 125000, tzinfo=dt.timezone.utc)
T2 = dt.datetime(2026, 9, 2, 9, 30, 0, tzinfo=dt.timezone.utc)
SHA_X = "a" * 64
SHA_Y = "b" * 64


def entry(**kw):
    base = {"project_id": None, "handle": "", "slug": "", "title": "", "branch": "main", "version_locked": False,
            "locked_tag": None, "custom_sha": None, "custom_filename": None, "author": ""}
    base.update(kw)
    return base


FULL = {
    "_id": PACK_ID, "slug": "my-pack", "title": "My Pack ✓", "summary": "Sum", "description": "Desc",
    "warnings": "Careful<br>really", "tags": ["fun", "ui"], "owner_id": OWNER, "owner_username": "Owner Name",
    "owner_handle": "owner", "collaborators": [{"user_id": COLLAB, "username": "collab"}], "visibility": "public",
    "banner_sha": "c" * 64, "preview_shas": ["d" * 64, "e" * 64], "discord_url": "https://discord.gg/x",
    "website_url": None, "donation_urls": ["https://ko-fi.com/x"], "download_count": 41, "star_count": 3,
    "variants": [
        {"name": "default", "label": "Default", "entries": [
            entry(project_id=MOD_A, handle="alice", slug="mod-a", title="Mod A"),
            entry(custom_sha=SHA_X, custom_filename="Custom One.tmod", title="Custom One", author="Me"),
            entry(project_id=MOD_B, handle="bob", slug="mod-b", title="mod b", branch="beta", version_locked=True,
                  locked_tag="v2"),
        ]},
        {"name": "lite", "label": "", "entries": [entry(project_id=MOD_A, handle="alice", slug="mod-a",
                                                         title="Mod A")]},
        {"name": "spin off", "label": "Spin Off", "entries": []},
    ],
    "default_variant": "lite", "taken_down": True, "takedown_reason": "Bad stuff", "created_at": T1, "updated_at": T2,
}
MINIMAL = {"_id": ObjectId("65f000000000000000000002"), "slug": "bare", "title": "Bare", "owner_id": OWNER,
           "owner_username": "Owner", "created_at": T1, "updated_at": T1}
LEGACY = {"_id": ObjectId("65f000000000000000000003"), "slug": "legacy", "title": "Legacy", "owner_id": str(OWNER),
          "owner_username": "O", "owner_handle": "owner", "visibility": "unlisted", "extra_field": 7,
          "variants": [{"name": "x", "entries": [{"title": "only title", "unknown": 1}]}],
          "default_variant": "missing", "download_count": 2.0, "taken_down": 0, "created_at": T1, "updated_at": T2}
DOCS = {"full": FULL, "minimal": MINIMAL, "legacy": LEGACY}
INVALID = [
    {"_id": ObjectId("65ac00000000000000000001"), "title": "no slug", "owner_id": OWNER, "owner_username": "o"},
    {"_id": ObjectId("65ac00000000000000000002"), "slug": "s", "title": "t", "owner_id": OWNER, "owner_username": "o", "visibility": "hidden"},
    {"_id": ObjectId("65ac00000000000000000003"), "slug": "s", "title": 5, "owner_id": OWNER, "owner_username": "o"},
    {"_id": ObjectId("65ac00000000000000000004"), "slug": "s", "title": "t", "owner_id": "nope", "owner_username": "o"},
]


def bson_round(d):
    return from_ext(ext(d))


def load(d) -> ModpackProject:
    return ModpackProject.model_validate(bson_round(d))


def docs():
    out = {}
    for name, d in DOCS.items():
        p = load(d)
        out[name] = {"in": ext(bson_round(d)), "stored": stored(p)}
    bad = []
    for d in INVALID:
        try:
            ModpackProject.model_validate(bson_round(d))
            bad.append({"in": ext(d), "valid": True})
        except Exception:  # noqa: BLE001
            bad.append({"in": ext(d), "valid": False})
    out["invalid"] = bad
    return out


# --- views ---------------------------------------------------------------------------

MODS = {
    "alive": {"_id": MOD_A, "slug": "mod-a", "title": "Mod A Renamed", "owner_username": "Alice A",
              "owner_handle": "alice2", "visibility": "public", "is_beta": True, "taken_down": False},
    "draft": {"_id": MOD_A, "slug": "mod-a", "title": "Draft", "owner_handle": "alice", "visibility": "draft"},
    "down": {"_id": MOD_A, "slug": "mod-a", "title": "Down", "owner_handle": "alice", "visibility": "unlisted",
             "taken_down": True},
    "hidden": {"_id": MOD_A, "slug": "mod-a", "title": "Hidden", "owner_handle": "alice", "visibility": "hidden"},
}
RELS = {
    "tmod": {"_id": ObjectId("65ab00000000000000000001"), "project_id": MOD_A, "tag": "v1.0", "branch": "main", "release_format": "tmod",
             "tmod_sha": SHA_Y, "tmod_filename": "x.tmod", "tmod_properties": {"title": "  Mod A Title  "}},
    "zip": {"_id": ObjectId("65ab00000000000000000002"), "project_id": MOD_A, "tag": "v2", "branch": "beta", "release_format": "zip",
            "tmod_sha": SHA_Y, "tmod_filename": "mod-a-v2.zip"},
}
RESOLVE = [
    ("custom_present", entry(custom_sha=SHA_X, custom_filename="C.tmod", title="", author="Au"), None, None, True),
    ("custom_missing", entry(custom_sha=SHA_X, title="Named"), None, None, False),
    ("custom_empty_sha", entry(custom_sha="", title="Blank"), None, None, True),
    ("removed", entry(project_id=MOD_A, handle="alice", slug="mod-a", title="Old"), None, None, True),
    ("no_project", entry(handle="h", slug="s", title="T"), None, None, True),
    ("draft", entry(project_id=MOD_A, title="Old"), "draft", None, True),
    ("down", entry(project_id=MOD_A, title="Old"), "down", None, True),
    ("no_build", entry(project_id=MOD_A, title="Old"), "alive", None, True),
    ("latest", entry(project_id=MOD_A, title="Old"), "alive", "tmod", True),
    ("locked_zip", entry(project_id=MOD_A, title="Old", branch="beta", version_locked=True, locked_tag="v2"), "alive",
     "zip", True),
    ("locked_flag_no_tag", entry(project_id=MOD_A, version_locked=True, locked_tag=""), "hidden", "tmod", True),
    ("tag_not_locked", entry(project_id=MOD_A, version_locked=False, locked_tag="v9"), "alive", "tmod", True),
]


def _patch_resolution(mod, rel, present, calls):
    async def get(pid):
        calls.append("mod")
        return ModProject.model_validate(bson_round(MODS[mod])) if mod else None

    async def has_blob(sha):
        calls.append("blob")
        return present

    class Q:
        def sort(self, *_):
            return self

        async def first_or_none(self):
            calls.append("latest")
            return ModRelease.model_validate(bson_round(RELS[rel])) if rel else None

    async def find_one(*a, **k):
        calls.append("locked")
        return ModRelease.model_validate(bson_round(RELS[rel])) if rel else None

    ModProject.get = get
    ModRelease.find_one = find_one
    ModRelease.find = lambda *a, **k: Q()
    S.mods_store.has_blob = has_blob


def resolution():
    out = []
    for name, e, mod, rel, present in RESOLVE:
        calls = []
        _patch_resolution(mod, rel, present, calls)
        from app.trove.modpacks.models import ModpackEntry
        view, got = run(S._resolve_entry(ModpackEntry.model_validate(bson_round(e))))
        out.append({"name": name, "entry": ext(bson_round(e)), "mod": ext(bson_round(MODS[mod])) if mod else None,
                    "release": ext(bson_round(RELS[rel])) if rel else None, "present": present,
                    "calls": calls, "view": to_json(view), "has_release": got is not None,
                    "download_name": hub.release_download_filename(got) if got is not None else None})
    return out


VIEWS = {
    "a": {"custom": False, "handle": "alice2", "slug": "mod-a", "title": "Zebra Mod", "author": "Alice",
          "branch": "main", "version_locked": False, "locked_tag": None, "version": "v1", "is_beta": False,
          "available": True, "reason": None},
    "c": {"custom": True, "custom_sha": SHA_X, "custom_filename": "Custom One.tmod", "handle": "", "slug": "",
          "title": "ärger", "author": "Me", "branch": "", "version_locked": False, "locked_tag": None, "version": None,
          "is_beta": False, "available": False, "reason": "missing"},
    "b": {"custom": False, "handle": "bob", "slug": "mod-b", "title": "Apple", "author": "", "branch": "beta",
          "version_locked": True, "locked_tag": "v2", "version": "v2", "is_beta": True, "available": False,
          "reason": "not a .tmod"},
}
VARIANT_VIEWS = {"default": ["a", "c", "b"], "lite": ["a"], "spin off": []}
VIEWERS = {"anon": None, "owner": OWNER, "collab": COLLAB, "other": OTHER}


def views():
    out = []
    for name, d in DOCS.items():
        p = load(d)
        order = {}
        for v in p.variants:
            keys = VARIANT_VIEWS.get(v.name, ["a"] * len(v.entries))
            for e, k in zip(v.entries, keys):
                order[id(e)] = VIEWS[k]

        async def resolve(e, order=order):
            return dict(order[id(e)]), None

        S._resolve_entry = resolve
        case = {"doc": name, "card": to_json(S.pack_card(p)), "public_card": to_json(S.public_pack_card(p)),
                "public": to_json(run(S.public_pack_dto(p))), "details": {}, "can_view": {}}
        case["master"] = to_json({**S.pack_card(p), "id": str(p.id), "taken_down": p.taken_down,
                                  "owner_id": str(p.owner_id)})
        for vname, viewer in VIEWERS.items():
            user = types.SimpleNamespace(id=viewer) if viewer else None
            case["can_view"][vname] = S.can_view(p, user)
            for starred in [vname in ("owner", "other")]:
                async def has(v, pk, s=starred):
                    return s
                S.has_starred = has
                case["details"][f"{vname}:{starred}"] = to_json(run(S.pack_detail(p, user)))
        out.append(case)
    return {"cases": out, "views": {k: to_json(v) for k, v in VIEWS.items()}, "variant_views": VARIANT_VIEWS}


# --- writes ----------------------------------------------------------------------------

ACTOR = types.SimpleNamespace(id=OWNER, username="owner_now", display_name="Owner Display")
ACTOR_NO_DISPLAY = types.SimpleNamespace(id=OWNER, username="plainuser", display_name="")


class Captured(Exception):
    pass


def capture_save():
    box = {}

    async def save(self, *a, **k):
        box["doc"] = stored(self)
        return self

    async def insert(self, *a, **k):
        box["doc"] = stored(self)
        return self

    ModpackProject.save = save
    ModpackProject.insert = insert
    return box


def outcome(coro, box):
    try:
        res = run(coro)
    except APIError as e:
        return {"error": err(e)}
    out = {"stored": box.get("doc")}
    if isinstance(res, dict):
        out["reply"] = to_json(res)
    return out


UPDATES = [
    {"title": "  New Title  ", "summary": " s ", "description": " d ", "warnings": "w", "tags": ["A ", "a", "B"],
     "visibility": "unlisted"},
    {"discord_url": "", "website_url": " https://site.example ", "donation_urls": ["", "https://a", " http://b "]},
    {"donation_urls": [f"https://d{i}" for i in range(4)] + [""]},
    {"discord_url": "nope"},
    {"website_url": "https://ok", "donation_urls": ["https://ok", "bad"]},
    {"default_variant": "spin off", "variant_order": ["lite", "nope", "spin off", "lite"]},
    {"default_variant": "missing"},
    {"title": None, "tags": None},
    {"variant_order": []},
]


def writes():
    out = {}
    box = capture_save()

    async def unique(owner_id, title):
        return S._slugify(title) + "-2"

    S._unique_slug = unique
    out["create"] = []
    for actor in (ACTOR, ACTOR_NO_DISPLAY):
        for body in ({"title": "  Shiny Pack  ", "summary": " sum ", "description": " d ", "tags": [" X ", "x", "y"],
                      "visibility": "public"}, {"title": "日本"}):
            req = CreateModpackRequest.model_validate(body)
            box.clear()
            res = outcome(S.create_modpack(actor, title=req.title, summary=req.summary, description=req.description,
                                           tags=req.tags, visibility=req.visibility), box)
            out["create"].append({"actor": [str(actor.id), actor.username, actor.display_name], "body": body, **res})

    out["update"] = []
    for body in UPDATES:
        p = load(FULL)
        box.clear()
        req = UpdateModpackRequest.model_validate(body)
        res = outcome(S.update_modpack(p, ACTOR, **req.model_dump(exclude_unset=True)), box)
        out["update"].append({"body": body, **res})

    out["variants"] = []
    for op, args in [("add", ("  Hard Mode!! ", "default")), ("add", ("lite", None)), ("add", ("New", "nope")),
                     ("update", ("lite", "  Lite  ")), ("update", ("lite", None)), ("update", ("nope", "x")),
                     ("delete", ("lite",)), ("delete", ("default",)), ("delete", ("nope",))]:
        p = load(FULL)
        box.clear()
        if op == "add":
            res = outcome(S.add_variant(p, ACTOR, name=args[0], copy_from=args[1]), box)
        elif op == "update":
            res = outcome(S.update_variant(p, ACTOR, args[0], **({} if args[1] is None else {"label": args[1]})), box)
        else:
            res = outcome(S.delete_variant(p, ACTOR, args[0]), box)
        out["variants"].append({"op": op, "args": list(args), **res})
    full = load(FULL)
    full.variants = full.variants * 10
    box.clear()
    out["variants"].append({"op": "add_31", "args": ["x"], **outcome(S.add_variant(full, ACTOR, name="Another"), box)})
    single = load(FULL)
    single.variants = single.variants[:1]
    box.clear()
    out["variants"].append({"op": "delete_last", "args": ["default"], **outcome(S.delete_variant(single, ACTOR,
                                                                                                 "default"), box)})

    out["collaborators"] = []

    class FakeSiteUser:
        class _Field:
            def __eq__(self, other):
                return other

        username = _Field()
        users = {"collab": types.SimpleNamespace(id=COLLAB, username="collab"),
                 "newbie": types.SimpleNamespace(id=OTHER, username="newbie"),
                 "owner": types.SimpleNamespace(id=OWNER, username="owner")}

        @classmethod
        async def find_one(cls, name):
            return cls.users.get(name)

    async def detail(pk, actor):
        return {"detail": True}

    real_detail, real_user = S.pack_detail, S.SiteUser
    S.pack_detail, S.SiteUser = detail, FakeSiteUser
    for name in ("  @Newbie ", "@@collab", "owner", "ghost", "  "):
        box.clear()
        out["collaborators"].append({"op": "add", "name": name, **outcome(S.add_collaborator(load(FULL), ACTOR, name),
                                                                          box)})
    crowded = load(FULL)
    crowded.collaborators = [type(crowded.collaborators[0])(user_id=ObjectId(f"65c0000000000000000000{i:02x}"),
                                                            username=f"u{i}") for i in range(20)]
    box.clear()
    out["collaborators"].append({"op": "add_21", "name": "newbie", "pack_collaborators": [
        ext(c.model_dump()) for c in crowded.collaborators], **outcome(S.add_collaborator(crowded, ACTOR, "newbie"),
                                                                      box)})
    for uid in (str(COLLAB), "not-an-id", str(OTHER)):
        box.clear()
        out["collaborators"].append({"op": "remove", "name": uid,
                                     **outcome(S.remove_collaborator(load(FULL), ACTOR, uid), box)})
    box.clear()
    out["collaborators"].append({"op": "remove_as_collab", "name": str(COLLAB),
                                 **outcome(S.remove_collaborator(load(FULL), types.SimpleNamespace(id=COLLAB),
                                                                 str(COLLAB)), box)})
    S.pack_detail, S.SiteUser = real_detail, real_user
    return out


# --- tmods, configs, conflicts, artifacts ------------------------------------------------

def build(title, files, extra=None, version=1):
    props = {"title": title, "author": "Au"} if title is not None else {"author": "Au"}
    props.update(extra or {})
    return tmod.build_tmod(version, props, files, lowercase_paths=False)


SWF = b"FWS\x0a" + bytes(range(40))
TMODS = {
    "declared": build("Declared Mod", [("ui/a.swf", SWF), ("ModCfgs/One.cfg", b"[a]\nx=1\n"),
                                       ("ModCfgs/Two.cfg", b"[b]\n")], {"configPath": "\\ModCfgs/TWO.CFG "}),
    "single": build("Single: Cfg?", [("ui/Main.SWF", SWF), ("thing.CFG", b"k=v\n")]),
    "many": build("Many", [("ui/b.swf", SWF), ("UI/A.swf", SWF), ("ui/sub/b.swf", SWF), ("a.cfg", b"1"),
                           ("b.cfg", b"2")]),
    "swf_only": build("Swf Only", [("x/Menu.swf", SWF)]),
    "none": build("Plain", [("blocks/a.blueprint", b"\x01\x02")]),
    "untitled": build("   ", [("x/a.swf", SWF)]),
    "no_title": build(None, [("x/a.swf", SWF)]),
    "dup_title": build("Declared Mod", [("ui/other.swf", SWF)]),
    "preview": build("Preview Mod", [("preview.png", b"png"), ("blocks/a.blueprint", b"x"), ("Shared/X.dds", b"d")],
                     {"previewPath": " PREVIEW.PNG "}),
    "overlap": build("Overlap", [("shared/x.dds", b"d"), ("blocks/A.blueprint", b"y"), ("p1", b"1"), ("p2", b"2"),
                                 ("p3", b"3")]),
    "overlap2": build("overlap ", [("p1", b"1"), ("p2", b"2"), ("p3", b"3"), ("preview.png", b"x")]),
    "garbage": b"\x05\x00\x00\x00\x00\x00\x00\x00\x01\x00",
}


def configs():
    out = []
    for name, data in TMODS.items():
        made = hub.pack_config_for_artifact(data)
        fp = S._fingerprint_bytes(data)
        out.append({"name": name, "tmod": b64(data),
                    "config": None if made is None else [made[0], b64(made[1])],
                    "fingerprint": None if fp is None else [fp[0], sorted(fp[1])]})
    pairs = []
    for a, b in [("preview", "overlap"), ("overlap", "overlap2"), ("declared", "dup_title"), ("none", "swf_only")]:
        fa, fb = S._fingerprint_bytes(TMODS[a]), S._fingerprint_bytes(TMODS[b])
        pairs.append({"a": a, "b": b, "messages": S._pair_conflicts(f"“{a}”", fa, b, fb)})
    return {"tmods": out, "pairs": pairs}


ART_ENTRIES = [
    ("hub", entry(project_id=MOD_A, handle="alice", slug="mod-a", title="Declared"), "a", "declared"),
    ("custom", entry(custom_sha=SHA_X, custom_filename="Single Cfg.tmod", title="Single"), "c", "single"),
    ("custom_noname", entry(custom_sha="1" * 64, title="Bare/Name"), "c2", "swf_only"),
    ("hub_missing_blob", entry(project_id=MOD_B, title="Gone"), "b", None),
    ("unavailable", entry(project_id=MOD_B, title="Nope"), "u", None),
    ("dup_title", entry(custom_sha="2" * 64, custom_filename="Declared Mod.tmod", title="Dup"), "c3", "dup_title"),
    ("plain", entry(custom_sha="3" * 64, custom_filename="Plain.tmod", title="Plain"), "c4", "none"),
    ("hub_empty_sha", entry(project_id=MOD_B, title="Empty sha"), "e", None),
]
ART_VIEWS = {
    "a": ({"handle": "alice2", "slug": "mod-a", "title": "Declared Mod", "branch": "main", "version": "v1",
           "version_locked": False, "available": True},
          {"project_id": MOD_A, "tag": "v1", "tmod_sha": "4" * 64, "tmod_properties": {"title": "Declared Mod"},
           "tmod_filename": "ignored.tmod"}),
    "c": ({"handle": "", "slug": "", "title": "Single", "branch": "", "version": None, "version_locked": False,
           "available": True}, None),
    "c2": ({"handle": "", "slug": "", "title": "Bare/Name", "branch": "", "version": None, "version_locked": False,
            "available": True}, None),
    "b": ({"handle": "bob", "slug": "mod-b", "title": "Gone", "branch": "main", "version": "v3",
           "version_locked": True, "available": True},
          {"project_id": MOD_B, "tag": "v3", "tmod_sha": "5" * 64, "tmod_properties": {}, "tmod_filename": "g.tmod"}),
    "u": ({"handle": "bob", "slug": "mod-b", "title": "Nope", "branch": "main", "version": None,
           "version_locked": False, "available": False}, None),
    "c3": ({"handle": "", "slug": "", "title": "Dup", "branch": "", "version": None, "version_locked": False,
            "available": True}, None),
    "c4": ({"handle": "", "slug": "", "title": "Plain", "branch": "", "version": None, "version_locked": False,
            "available": True}, None),
    "e": ({"handle": "bob", "slug": "mod-b", "title": "Empty sha", "branch": "main", "version": "v4",
           "version_locked": False, "available": True},
          {"project_id": MOD_B, "tag": "v4", "tmod_sha": "", "tmod_filename": "e.tmod"}),
}
ART_BLOBS = {"4" * 64: "declared", SHA_X: "single", "1" * 64: "swf_only", "2" * 64: "dup_title", "3" * 64: "none"}


class _FakeTime:
    @staticmethod
    def time():
        return 0.0

    @staticmethod
    def localtime(t=None):
        return time.struct_time(ZIP_TIME + (0, 0, -1))


_zipinfo_init = zipfile.ZipInfo.__init__


def _linux_zipinfo(self, *a, **k):
    _zipinfo_init(self, *a, **k)
    self.create_system = 3  # what the Linux container writes


def artifacts():
    zipfile.time = _FakeTime
    zipfile.ZipInfo.__init__ = _linux_zipinfo
    blobs = {sha: TMODS[name] for sha, name in ART_BLOBS.items()}

    async def get_blob(sha):
        return blobs.get(sha)

    S.mods_store.get_blob = get_blob
    cases = []
    sets = {"everything": [0, 1, 2, 3, 4, 5, 6, 7], "plain_only": [6], "none": [4, 3], "configless": [6, 4]}
    titles = [("My Pack: Ünï?", "default", "Warn ✓\nline"), ("..", "spin-off", ""), ("日本 pack", "default", "")]
    for set_name, idx in sets.items():
        chosen = [ART_ENTRIES[i] for i in idx]
        for title, variant, warnings_ in titles if set_name == "everything" else titles[:1]:
            pack = load({**FULL, "title": title, "warnings": warnings_,
                         "variants": [{"name": variant, "label": "", "entries": [c[1] for c in chosen]}],
                         "default_variant": variant})
            lookup = {}
            for e, c in zip(pack.variants[0].entries, chosen):
                view, rel = ART_VIEWS[c[2]]
                lookup[id(e)] = (view, None if rel is None else ModRelease.model_validate(
                    {"_id": ObjectId(), "branch": "main", **rel}))

            async def resolve(e, lookup=lookup):
                return lookup[id(e)]

            S._resolve_entry = resolve
            for fmt in ("tpack", "zip", "configs"):
                try:
                    blob, filename, media = run(S.build_artifact(pack, None, fmt))
                    res = {"blob": b64(blob), "filename": filename, "media": media}
                except APIError as e:
                    res = {"error": err(e)}
                cases.append({"set": set_name, "title": title, "variant": variant, "warnings": warnings_,
                              "format": fmt, **res})
    entries = [{"name": n, "entry": ext(bson_round(e)), "view": ART_VIEWS[v][0],
                "release": None if ART_VIEWS[v][1] is None else ext(bson_round(
                    {"_id": ObjectId("65aa00000000000000000000"), "branch": "main", **ART_VIEWS[v][1]}))}
               for n, e, v, _ in ART_ENTRIES]
    return {"zip_time": list(ZIP_TIME), "entries": entries, "sets": sets,
            "blobs": {sha: name for sha, name in ART_BLOBS.items()}, "cases": cases}


# --- HTTP: everything settled before a store is touched ------------------------------------

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
JSON = "application/json"
H = "/v1/modpacks/hub/projects/owner/my-pack"
LONG = "x" * 121

# (name, method, url, content type, body, signed in)
HTTP = [
    ("site_list_bad_params", "GET", f"/site/modpacks/projects?limit=0&offset=-1&q={LONG}&tag={'t' * 41}", None, b"",
     None),
    ("site_list_bad_limit_type", "GET", "/site/modpacks/projects?limit=abc&author=" + "a" * 81, None, b"", None),
    ("site_download_bad", "GET", f"/site/modpacks/projects/owner/my-pack/download?format=rar&variant={'v' * 81}",
     None, b"", None),
    ("site_download_bad_format_case", "GET", "/site/modpacks/projects/owner/my-pack/download?format=ZIP", None, b"",
     None),
    ("site_mine_anon", "GET", "/site/modpacks/me/projects", None, b"", None),
    ("create_bad_json", "POST", "/v1/modpacks/hub/projects", JSON, b'{"title": ', None),
    ("create_anon", "POST", "/v1/modpacks/hub/projects", JSON, b'{"title": "x"}', None),
    ("mine_anon", "GET", "/v1/modpacks/hub/me/projects", None, b"", None),
    ("delete_anon", "DELETE", H, None, b"", None),
    ("star_anon", "POST", H + "/star", None, b"", None),
    ("banner_garbage", "POST", H + "/banner", MP, b"garbage", None),
    ("banner_anon", "POST", H + "/banner", MP, mp([("file", b"\x89PNG", "a.png", "image/png")]), None),
    ("upload_no_boundary", "POST", H + "/variants/default/upload", "multipart/form-data", b"--x\r\n", None),
    ("entries_bad_json", "PUT", H + "/variants/default/entries", JSON, b"[", None),
    ("put_on_list", "PUT", "/v1/modpacks/hub/projects", JSON, b"{}", None),
    ("post_on_detail", "POST", H, None, b"", None),
    ("patch_star", "PATCH", H + "/star", None, b"", None),
    ("get_variant", "GET", H + "/variants/x", None, b"", None),
    ("post_variant_entries", "POST", H + "/variants/x/entries", None, b"", None),
    ("get_collaborator", "GET", H + "/collaborators/abc", None, b"", None),
    ("post_hub_projects_download", "POST", "/v1/modpacks/hub/projects/download", None, b"", None),
    ("post_hub_me_download", "POST", "/v1/modpacks/hub/me/download", None, b"", None),
    ("post_for_mod_download", "POST", "/v1/modpacks/for-mod/x/download", None, b"", None),
    ("post_public_detail", "POST", "/v1/modpacks/owner/my-pack", None, b"", None),
    ("delete_public_list", "DELETE", "/v1/modpacks", None, b"", None),
    ("post_site_list", "POST", "/site/modpacks/projects", None, b"", None),
    ("post_admin_delete", "POST", "/admin/modpacks/abc", None, b"", None),
    ("get_admin_takedown", "GET", "/admin/modpacks/abc/takedown", None, b"", None),
    ("admin_anon", "GET", "/admin/modpacks", None, b"", None),
    ("upload_missing_file", "POST", H + "/variants/default/upload", MP, mp([("other", "x", None, None)]), "user"),
    ("upload_file_is_text", "POST", H + "/variants/default/upload", MP, mp([("file", "text", None, None)]), "user"),
    ("banner_missing_file", "POST", H + "/banner", "application/x-www-form-urlencoded", b"file=", "user"),
    ("admin_list_bad", "GET", "/admin/modpacks?limit=201&offset=-2", None, b"", "admin"),
]

BODIES = [
    ("create", "POST", "/v1/modpacks/hub/projects",
     [b"{}", b"[]", b'{"title": ""}', b'{"title": 5, "summary": null, "tags": "x", "visibility": "secret"}',
      ('{"title": "' + "é" * 121 + '", "summary": "' + "s" * 281 + '", "tags": ' + json.dumps(
          [str(i) for i in range(13)]) + '}').encode(),
      b'{"title": "ok", "tags": [1, "a", null], "description": 7}', b"null"]),
    ("update", "PATCH", H,
     [b'{"title": "", "warnings": 5, "visibility": "draft2", "donation_urls": ["a","b","c","d","e","f"]}',
      b'{"discord_url": "' + b"d" * 301 + b'", "default_variant": "' + b"v" * 81 + b'", "variant_order": 3}',
      b'{"tags": [1], "summary": ["x"]}', b'"str"']),
    ("add_variant", "POST", H + "/variants", [b"{}", b'{"name": "", "copy_from": 5}',
                                               b'{"name": "' + b"n" * 81 + b'", "copy_from": "' + b"c" * 81 + b'"}']),
    ("update_variant", "PATCH", H + "/variants/default", [b'{"label": 5}', b'{"label": "' + b"l" * 121 + b'"}']),
    ("set_entries", "PUT", H + "/variants/default/entries",
     [b'{"entries": 5}', b'{"entries": [5, {"handle": 1, "version_locked": "maybe", "branch": null}]}',
      ('{"entries": [' + ",".join(['{}'] * 301) + ']}').encode(),
      b'{"entries": [{"handle": "' + b"h" * 81 + b'", "slug": "' + b"s" * 121 + b'", "locked_tag": "'
      + b"t" * 61 + b'", "custom_sha": "' + b"c" * 129 + b'", "custom_filename": "' + b"f" * 201
      + b'", "title": "' + b"t" * 201 + b'", "author": "' + b"a" * 201 + b'"}]}']),
    ("collaborator", "POST", H + "/collaborators", [b"{}", b'{"username": ""}',
                                                    b'{"username": "' + b"u" * 81 + b'"}']),
    ("takedown", "POST", "/admin/modpacks/65f000000000000000000001/takedown",
     [b'{"reason": 5}', b'{"reason": "' + b"r" * 2001 + b'"}', b"[1]"]),
]


def http():
    out = []
    for name, method, url, ctype, body, who in HTTP + [
            (f"{kind}_{i}", m, u, JSON, b, "admin" if kind == "takedown" else "user")
            for kind, m, u, bodies in BODIES for i, b in enumerate(bodies)]:
        i = len(out)
        rid = f"req_modpacks_{i:03d}"
        hdrs = [(b"x-request-id", rid.encode())]
        if ctype:
            hdrs.append((b"content-type", ctype.encode()))
        app.dependency_overrides.clear()
        if who == "user":
            app.dependency_overrides[get_current_site_user] = lambda: ACTOR
        if who == "admin":
            app.dependency_overrides[get_current_superuser] = lambda: types.SimpleNamespace(id=OWNER,
                                                                                            is_superuser=True)
        status, rh, raw = asyncio.run(asgi(app, method, url, hdrs, body))
        if status >= 500 or status == 200:
            raise SystemExit(f"{name}: unexpected {status} {raw!r}")
        out.append({"name": name, "method": method, "url": url, "content_type": ctype, "request_id": rid,
                    "body": body.decode("latin-1"), "who": who, "status": status,
                    "resp_headers": [[k, v] for k, v in rh if k in KEPT], "resp": raw.decode("utf-8")})
    app.dependency_overrides.clear()
    return out


def main():
    data = {
        "env": ENV,
        "now": NOW.isoformat(),
        "strings": strings(),
        "docs": docs(),
        "resolution": resolution(),
        "views": views(),
        "configs": configs(),
        "artifacts": artifacts(),
        "http": http(),
    }
    data["writes"] = writes()
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(data, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
