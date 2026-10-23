"""Golden fixtures for kiwi-server's Mods Hub git store (src/mods_git), from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_mods_git_fixtures.py
Writes native/kiwi-server/testdata/mods_git/golden.json.

Runs the real gitstore (dulwich) over a temporary mods store: a script of
commits, branches, deletes and a fork whose resulting repository files (every
loose object and ref, byte for byte) are recorded, then the service reads
(branches, commits, trees, blobs, compare, placement, archive) and writes
(commit_files, create/delete branch, fix_placement) over fake projects, the
commit form's validation through the real app, the Basic-auth parsing, the
push authorization decisions, dulwich's receive-pack replies to the twin
spec's push bodies, and Beanie's save() of ModProject and ModGitToken. The
clock is frozen.
"""
import asyncio
import base64
import datetime as dt
import json
import logging
import os
import shutil
import sys
import tempfile
import time
import types
import urllib.parse
import warnings
import zipfile
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
os.chdir(ROOT)
os.environ.setdefault("SECRET_KEY", "mods-git-fixture-secret")
STORE = Path(tempfile.mkdtemp(prefix="kiwi-mods-git-"))
os.environ["MODS_STORE_DIR"] = str(STORE)

from bson import ObjectId  # noqa: E402
from bson.json_util import CANONICAL_JSON_OPTIONS, dumps as bson_dumps  # noqa: E402
from pymongo.asynchronous.database import AsyncDatabase  # noqa: E402

from app.admin import runtime_config as rc  # noqa: E402

for _key, _spec in rc.REGISTRY.items():
    rc._CACHE[_key] = (_spec.default, float("inf"))

from app.core.config import settings  # noqa: E402
from app.core.errors import APIError  # noqa: E402

settings.mods_store_dir = str(STORE)
settings.mods_hub_max_files_per_commit = 4
settings.mods_hub_max_file_bytes = 4096

from app.trove.mods_hub import git_http, gitstore, service, trove_layout  # noqa: E402
from app.trove.mods_hub.models import ModGitToken, ModProject  # noqa: E402

OUT = ROOT / "native" / "kiwi-server" / "testdata" / "mods_git" / "golden.json"
NOW = dt.datetime(2026, 10, 22, 9, 30, 15, 123456, tzinfo=dt.timezone.utc)
service.utcnow = lambda: NOW


def b64(b: bytes) -> str:
    return base64.b64encode(b).decode()


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
    client = AsyncMongoClient("mongodb://127.0.0.1:1", tz_aware=True, connect=False, serverSelectionTimeoutMS=200)
    await init_beanie(database=client["kiwi"], skip_indexes=True, document_models=[ModProject, ModGitToken])


asyncio.run(_init())

# --- the store script ---------------------------------------------------------

ALICE = "Alice Example"
ZOE = "Zoë_名-x.y"
OPS = [
    ["ensure", "p1"],
    ["commit", "p1", "main", [["ui/a.txt", b"hello\n"], ["blueprints/deco/x.blueprint", b"\x00\x01\x02bp"],
                              ["README.md", b"# Readme\n"], ["readme_root.txt", b"r"], ["ui/dup.txt", b"hello\n"]],
     [], ALICE, "first commit", 1700000000],
    ["commit", "p1", "main", [["ui/b.txt", b"b" * 3000], ["textures/t.dds", bytes(range(256)) * 20],
                              ["ui/b.txt", b"second write wins"]],
     ["README.md", "not/there.txt"], ALICE, "second", 1700000100],
    ["commit", "p1", "main", [["ui/a.txt", b"hello\n"]], [], ALICE, "noop", 1700000200],
    ["branch", "p1", "dev", "main"],
    ["commit", "p1", "dev", [["ui/a.txt/c", b"collide"]], [], ZOE, "dir wins", 1700000300],
    ["commit", "p1", "dev", [["ui", b"file over dir"]], [], ZOE, "file wins", 1700000300],
    ["commit", "p1", "feature/x", [["models/m.txt", b"m"]], [], "Multi\nLine", "orphan branch", 1700000050],
    ["branch", "p1", "fromsha", "@1"],
    ["branch", "p1", "fromupper", "@2U"],
    ["delete_branch", "p1", "feature/x"],
    ["delete_branch", "p1", "nope"],
    ["ensure", "p3"],
    ["branch", "p3", "x", "main"],
    ["fork", "p1", "p2", "main", "main", ZOE, "Fork of slug", 1700000400],
    ["fork", "p9", "p8", "main", "main", ZOE, "Fork of nothing", 1700000400],
    ["commit", "p2", "main", [], ["ui/b.txt"], ZOE, "", 1700000500],
    ["commit", "p2", "main", [["x.tmod", b"t"], ["Ui/Mixed.TXT", b"mixed"]], [], "", "msg\n with  newline ", 1700000600],
    ["commit", "p2", "main", [], ["x.tmod", "ui/a.txt", "ui/dup.txt", "Ui/Mixed.TXT", "readme_root.txt",
                                  "blueprints/deco/x.blueprint", "textures/t.dds"], ZOE, "empty tree", 1700000700],
]


def run_ops():
    results = []
    for op in OPS:
        kind = op[0]
        try:
            if kind == "ensure":
                gitstore._ensure(op[1])
                res = None
            elif kind == "commit":
                _, pid, branch, adds, deletes, author, msg, ts = op
                res = gitstore._write_commit(pid, branch, [(p, c) for p, c in adds], deletes,
                                             gitstore.author_ident(author), msg, ts)
            elif kind == "branch":
                _, pid, name, start = op
                if start.startswith("@"):
                    idx = int(start[1:].rstrip("U"))
                    start = results[idx]["ok"]
                    if op[3].endswith("U"):
                        start = start.upper()
                res = gitstore._create_branch(pid, name, start)
            elif kind == "delete_branch":
                gitstore._delete_branch(op[1], op[2])
                res = None
            elif kind == "fork":
                _, src, dst, sb, b, author, msg, ts = op
                res = gitstore._fork(src, dst, sb, b, gitstore.author_ident(author), msg, ts)
            results.append({"ok": res})
        except gitstore.NothingToCommit:
            results.append({"nothing": True})
        except gitstore.GitStoreError as e:
            results.append({"refused": str(e)})
    return results


def dump_store():
    out = {}
    for repo in sorted((STORE / "git").iterdir()):
        files, dirs = {}, []
        for p in sorted(repo.rglob("*")):
            rel = p.relative_to(repo).as_posix()
            if p.is_dir():
                dirs.append(rel)
            else:
                files[rel] = b64(p.read_bytes())
        out[repo.name] = {"files": files, "dirs": dirs}
    return out


def encode_ops():
    out = []
    for op in OPS:
        op = list(op)
        if op[0] == "commit":
            op[3] = [[p, b64(c)] for p, c in op[3]]
        out.append(op)
    return out


# --- fake projects for the service layer ------------------------------------------------

OWNER = ObjectId("65b000000000000000000001")
OTHER = ObjectId("65b000000000000000000002")


class FakeProject(types.SimpleNamespace):
    async def save(self):
        self.saved = getattr(self, "saved", 0) + 1


def project(pid, **kw):
    base = dict(id=pid, slug="my mod: <v2>", default_branch="main", mode="files", owner_id=OWNER,
                collaborators=[], source_visibility="public", visibility="public", taken_down=False,
                is_remix=False, remix_status="pending", remix_requested_at=None)
    base.update(kw)
    return FakeProject(**base)


def actor(oid=OWNER, display="Alice Example", username="alice"):
    return types.SimpleNamespace(id=oid, display_name=display, username=username, is_active=True)


async def call(coro):
    try:
        res = await coro
    except APIError as e:
        return {"error": [e.status_code, e.code.value if hasattr(e.code, "value") else e.code, e.message]}
    except Exception as e:  # noqa: BLE001
        return {"exception": type(e).__name__}
    if isinstance(res, tuple) and isinstance(res[1], bytes):
        return {"ok": [res[0], b64(res[1])]}
    if isinstance(res, tuple):
        return {"ok": list(res)}
    if isinstance(res, bytes):
        return {"ok_bytes": b64(res)}
    return {"ok": res}


GAME = {"a.txt": "ui/hud/a.txt", "t.dds": "textures/t.dds", "readme_root.txt": "ui/readme_root.txt",
        "x.blueprint": "blueprints/deco/x.blueprint", "m.txt": "Models/Deep/M.TXT", "dup.txt": ""}


async def reads(results):
    sha1 = results[1]["ok"]
    sha2 = results[2]["ok"]
    p1, p2, p3, p4 = project("p1"), project("p2"), project("p3"), project("p4")

    async def game_map(branch=trove_layout.LIVE_BRANCH):
        return dict(GAME)

    trove_layout.game_file_map = game_map
    out = {"sha1": sha1, "sha2": sha2}
    out["branches"] = {k: await call(service.list_branches(v)) for k, v in
                       (("p1", p1), ("p2", p2), ("p3", p3), ("p4", p4))}
    refs = ["", "main", "dev", "fromsha", sha1, sha1.upper(), "nope", "0" * 40, "z" * 40, "é" * 20, sha1[:7],
            "refs/heads/main", "feature/x"]
    out["trees"] = [[pid, r, await call(service.get_tree(project(pid), r))] for pid in ("p1", "p2", "p4") for r in refs]
    commits = [("p1", None, 50, 0), ("p1", "dev", 50, 0), ("p1", "dev", 1, 1), ("p1", "dev", 2, 0), ("p1", "", 1, 5),
               ("p1", "fromsha", 10, 0), ("p1", sha2, 10, 0), ("p1", "nope", 10, 0), ("p2", None, 50, 0),
               ("p4", None, 50, 0)]
    out["commits"] = [[pid, b, lim, off, await call(service.list_commits(project(pid), b, lim, off))]
                      for pid, b, lim, off in commits]
    blobs = [("p1", "main", "ui/a.txt"), ("p1", "main", "UI\\A.txt "), ("p1", sha1, "/readme.md"),
             ("p1", "main", "README.md"), ("p1", "main", "../x"), ("p1", "main", ""), ("p1", "main", "a//b"),
             ("p1", "main", "x" * 256), ("p1", "dev", "ui"), ("p1", "dev", "ui/a.txt/c"), ("p4", "main", "x")]
    out["blobs"] = [[pid, r, path, await call(service.get_file_bytes(project(pid), r, path))]
                    for pid, r, path in blobs]
    pairs = [("main", "dev"), (sha1, "main"), ("dev", sha1), ("main", "nope"), ("nope", "main")]
    out["compare"] = [[b, h, await call(service.compare(p1, b, h))] for b, h in pairs]
    out["placement"] = [[pid, r, await call(service.placement_report(project(pid), r))]
                        for pid, r in (("p1", ""), ("p1", sha1), ("p1", "dev"), ("p2", ""), ("p4", ""))]

    async def empty_map(branch=trove_layout.LIVE_BRANCH):
        return {}

    trove_layout.game_file_map = empty_map
    out["placement_no_game"] = await call(service.placement_report(p1, ""))
    trove_layout.game_file_map = game_map

    real_localtime = time.localtime
    real_init = zipfile.ZipInfo.__init__

    def init(self, *a, **k):
        real_init(self, *a, **k)
        self.create_system = 3

    zipfile.ZipInfo.__init__ = init
    time.localtime = lambda *a: time.struct_time((2026, 10, 22, 9, 30, 15, 3, 295, 0))
    try:
        out["archives"] = [[pid, r, await call(service.source_archive(project(pid), r))]
                           for pid, r in (("p1", ""), ("p1", sha1), ("p1", "dev"), ("p2", ""), ("p3", ""),
                                          ("p4", ""), ("p1", "nope"))]
    finally:
        time.localtime = real_localtime
        zipfile.ZipInfo.__init__ = real_init
    out["archive_clock"] = [2026, 10, 22, 9, 30, 15]
    return out


async def writes():
    out = []
    p5 = project("p5")
    me = actor()
    cases = [
        ("commit_ok", service.commit_files(p5, me, branch_name="main", message="  add files ",
                                           adds=[("UI\\HUD\\A.txt", b"a"), ("textures/T.dds", b"t" * 10)],
                                           deletes=[])),
        ("commit_same", service.commit_files(p5, me, branch_name="main", message="again",
                                             adds=[("ui/hud/a.txt", b"a")], deletes=[])),
        ("commit_blank_message", service.commit_files(p5, me, branch_name="main", message=" \n ", adds=[],
                                                      deletes=[])),
        ("commit_too_many", service.commit_files(p5, me, branch_name="main", message="m",
                                                 adds=[(f"ui/{i}.txt", b"x") for i in range(5)], deletes=[])),
        ("commit_bad_branch", service.commit_files(p5, me, branch_name="a..b", message="m", adds=[], deletes=[])),
        ("commit_big_file", service.commit_files(p5, me, branch_name="main", message="m",
                                                 adds=[("ui/ok.txt", b"x"), ("Ui/Big 'one'.bin", b"x" * 4097)],
                                                 deletes=[])),
        ("commit_bad_path", service.commit_files(p5, me, branch_name="main", message="m",
                                                 adds=[("ui/./x", b"x")], deletes=[])),
        ("commit_tmod", service.commit_files(p5, me, branch_name="main", message="m",
                                             adds=[("ui/Built.TMOD ", b"x")], deletes=[])),
        ("commit_bad_delete", service.commit_files(p5, me, branch_name="main", message="m", adds=[],
                                                   deletes=["ui/"])),
        ("commit_releases_mode", service.commit_files(project("p5", mode="releases"), me, branch_name="main",
                                                      message="m", adds=[], deletes=[])),
        ("commit_not_owner", service.commit_files(p5, actor(OTHER, None, "bob"), branch_name="main", message="m",
                                                  adds=[], deletes=[])),
        ("commit_collaborator", service.commit_files(
            project("p5", collaborators=[types.SimpleNamespace(user_id=OTHER, username="bob")]),
            actor(OTHER, "", "bob"), branch_name="side/one", message="bob was here",
            adds=[("ui/hud/a.txt", b"a2"), ("particles/p.pkfx", b"p")], deletes=["textures/t.dds"])),
        ("branch_ok", service.create_branch(p5, me, " feat-1 ", None)),
        ("branch_from_sha_bad", service.create_branch(p5, me, "feat-2", "nope")),
        ("branch_exists", service.create_branch(p5, me, "feat-1", "main")),
        ("branch_invalid", service.create_branch(p5, me, "/lead", None)),
        ("branch_dotlock_500", service.create_branch(p5, me, "x.lock", None)),
        ("branch_empty_repo", service.create_branch(project("p6"), me, "feat", None)),
        ("branch_from_side", service.create_branch(p5, me, "feat-3", "side/one")),
        ("delete_default", service.delete_branch(p5, me, "main")),
        ("delete_ok", service.delete_branch(p5, me, "feat-1")),
        ("fix_ok", service.fix_placement(p5, me, "")),
        ("fix_nothing", service.fix_placement(p5, me, "main")),
        ("fix_no_branch", service.fix_placement(p5, me, "ghost")),
        ("fix_bad_branch", service.fix_placement(p5, me, "..")),
        ("fix_side", service.fix_placement(p5, me, "side/one")),
    ]

    async def game_map(branch=trove_layout.LIVE_BRANCH):
        return dict(GAME, **{"p.pkfx": "particles/fx/p.pkfx"})

    trove_layout.game_file_map = game_map
    for name, coro in cases:
        out.append([name, await call(coro)])

    async def empty_map(branch=trove_layout.LIVE_BRANCH):
        return {}

    trove_layout.game_file_map = empty_map
    out.append(["fix_no_game", await call(service.fix_placement(p5, me, "side/one"))])
    return out


# --- HTTP: the commit form and the git routes, through the real app -------------------

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
KEPT = ("content-type", "cache-control", "pragma", "www-authenticate", "content-disposition", "allow")

FORMS = [
    ("form_empty", MP, mp([("x", "y", None, None)])),
    ("form_no_body", None, b""),
    ("form_json", "application/json", b'{"branch": "main", "message": "m"}'),
    ("form_blank_fields", MP, mp([("branch", "", None, None), ("message", "", None, None)])),
    ("form_fields_are_files", MP, mp([("branch", b"main", "b.txt", "text/plain"),
                                      ("message", b"m\nx", "m.txt", None)])),
    ("form_files_text", MP, mp([("branch", "main", None, None), ("message", "m", None, None),
                                ("files", "not a file", None, None), ("files", b"ok", "ok.txt", None),
                                ("files", "", None, None)])),
    ("form_paths_file", MP, mp([("branch", "main", None, None), ("message", "m", None, None),
                                ("paths", b"p", "p.txt", None), ("paths", "ui/x.txt", None, None),
                                ("deletes", b"d", "d.bin", "application/octet-stream")])),
    ("form_garbage", MP, b"garbage"),
    ("form_no_boundary", "multipart/form-data", b"--x\r\n"),
    ("form_ok", MP, mp([("branch", "web", None, None), ("message", " from the form ", None, None),
                        ("files", b"one", "One.TXT", "text/plain"), ("files", b"two", "two.bin", None),
                        ("files", b"three", "", None), ("paths", "ui/one.txt", None, None)])),
]


def http_cases():
    from app.main import app
    from app.trove.mods_hub.write_auth import get_mod_write_user

    me = actor()
    app.dependency_overrides[get_mod_write_user] = lambda: me
    p7 = project("p7", slug="web")

    async def get_project(handle, slug):
        return p7 if (handle, slug) == ("alice", "web") else None

    service.get_project = get_project
    out = []
    for i, (name, ctype, body) in enumerate(FORMS):
        hdrs = [(b"x-request-id", f"req_mg_{i:03d}".encode())]
        if ctype:
            hdrs.append((b"content-type", ctype.encode()))
        status, rh, raw = asyncio.run(asgi(app, "POST", "/v1/mods/hub/projects/alice/web/commits", hdrs, body))
        out.append({"name": name, "content_type": ctype, "body": b64(body), "status": status,
                    "headers": [[k, v] for k, v in rh if k in KEPT], "resp": raw.decode("utf-8")})
    git = [
        ("refs_no_service", "GET", "/git/mods/alice/web.git/info/refs", []),
        ("refs_bad_service", "GET", "/git/mods/alice/web.git/info/refs?service=git-annex", []),
        ("refs_unknown_anon", "GET", "/git/mods/alice/ghost.git/info/refs?service=git-upload-pack", []),
        ("push_anon", "POST", "/git/mods/alice/web.git/git-receive-pack", []),
    ]
    for i, (name, method, url, extra) in enumerate(git):
        hdrs = [(b"x-request-id", f"req_mg_git_{i:03d}".encode())] + extra
        status, rh, raw = asyncio.run(asgi(app, method, url, hdrs, b""))
        out.append({"name": name, "method": method, "url": url, "status": status,
                    "headers": [[k, v] for k, v in rh if k in KEPT], "resp": raw.decode("utf-8")})
    return out


def basic_cases():
    from starlette.requests import Request
    values = ["Basic " + b64(b"user:kgit_abc"), "Basic " + b64(b":kgit_only"), "Basic " + b64(b"nocolon"),
              "Basic " + b64(b"u:"), "basic " + b64(b"u:p"), "Bearer x", "Basic !!!", "Basic dTpw", "Basic dTpwYQ",
              "Basic dTpwYQ=", "Basic dT pw\tYQ==", "Basic " + b64("ü:pässwörd".encode()), "Basic " + b64(b"u:\xff\xfe"),
              "Basic dTpwYQ==garbage", "Basic dTpwYQ==Zg==", "Basic dTpwYQ=a=", "Basic dTpwY=Q==", "Basic dTpwY=Q=", "Basic\u00e9 x", "Basic ", ""]
    out = []
    for v in values:
        headers = [(b"authorization", v.encode("latin-1"))] if v else []
        req = Request({"type": "http", "headers": headers})
        out.append([v, git_http._basic_token(req)])
    return out


def authorize_cases():
    user_owner = actor()
    user_other = actor(OTHER, None, "bob")
    projects = {
        "none": None,
        "public": project("x"),
        "private_src": project("x", source_visibility="private"),
        "releases": project("x", mode="releases"),
        "draft": project("x", visibility="draft"),
        "collab_draft": project("x", visibility="draft",
                                collaborators=[types.SimpleNamespace(user_id=OTHER, username="bob")]),
    }
    users = {"anon": None, "owner": user_owner, "other": user_other}
    out = []
    for pname, proj in projects.items():
        for uname, user in users.items():
            for push in (False, True):
                async def gp(handle, slug, proj=proj):
                    return proj

                async def auth(token, user=user):
                    return user

                service.get_project = gp
                service.authenticate_git = auth
                from starlette.requests import Request
                req = Request({"type": "http", "headers": [(b"authorization", b"Basic dTpw")]})
                try:
                    asyncio.run(git_http._authorize("h", "s", req, push=push))
                    res = {"ok": True}
                except APIError as e:
                    res = {"error": [e.status_code, getattr(e.code, "value", e.code), e.message, dict(e.headers or {})]}
                out.append([pname, uname, push, res])
    return out


def saves():
    from beanie.odm.utils.dump import get_dict
    raw_full = {
        "_id": ObjectId("66a000000000000000000001"), "slug": "s", "title": "T", "summary": "", "tags": ["a", "b"],
        "owner_id": OWNER, "owner_username": "Alice", "owner_handle": "alice",
        "collaborators": [{"user_id": OTHER, "username": "bob", "extra": 1}], "visibility": "public",
        "download_count": 3.0, "popularity_score": 1, "star_count": 2**40, "title_i18n": {"fr": "Le T"},
        "remix_requested_at": dt.datetime(2026, 1, 1, tzinfo=dt.timezone.utc), "legacy_field": "kept by $set",
        "created_at": dt.datetime(2025, 5, 6, 7, 8, 9, 123000, tzinfo=dt.timezone.utc),
        "updated_at": dt.datetime(2025, 5, 6, 7, 8, 9, 456000, tzinfo=dt.timezone.utc),
    }
    raw_sparse = {"_id": ObjectId("66a000000000000000000002"), "slug": "z", "title": "Z",
                  "created_at": dt.datetime(2025, 1, 1, tzinfo=dt.timezone.utc),
                  "updated_at": dt.datetime(2025, 1, 2, tzinfo=dt.timezone.utc), "is_stray": True,
                  "stray_status": "pending", "source": "x", "mode": "releases"}
    out = []
    for raw in (raw_full, raw_sparse):
        p = ModProject.model_validate(raw)
        out.append({"raw": ext(raw), "set": ext(get_dict(p, to_db=True, keep_nulls=True))})
    tok = ModGitToken.model_validate({
        "_id": ObjectId("66b000000000000000000001"), "site_user_id": OWNER, "token_hash": "ab" * 32,
        "prefix": "kgit_abcdefg", "name": "laptop",
        "created_at": dt.datetime(2026, 2, 3, 4, 5, 6, 789000, tzinfo=dt.timezone.utc)})
    tok.last_used_at = NOW
    token = {"set": ext(get_dict(tok, to_db=True, keep_nulls=True)), "dto": service._git_token_dto(tok)}
    fresh = ModGitToken(site_user_id=OWNER, token_hash="cd" * 32, prefix="kgit_1234567", name="x")
    fresh.created_at = NOW
    token["fresh_dto"] = service._git_token_dto(fresh)
    return {"projects": out, "token": token}


def pure():
    paths = ["UI\\Hud\\A.txt", "/lead", "  Pad  ", "a/./b", "a/../b", "", "/", "x/", "é" * 128, "é" * 127,
             "ok/path.TXT", "'quote", "tab\there"]
    norm = []
    for raw in paths:
        try:
            norm.append([raw, {"ok": service._norm_path(raw)}])
        except APIError as e:
            norm.append([raw, {"error": [e.status_code, e.message]}])
    branches = []
    for raw in ["main", " main ", "a/b-c_d.e", "", "x" * 80, "x" * 81, "/a", "a..b", "a b", "é", "a//b", "x.lock"]:
        try:
            branches.append([raw, {"ok": service._valid_branch_name(raw)}])
        except APIError as e:
            branches.append([raw, {"error": e.message}])
    names = ["my mod: <v2>", "...dots...", "  ", 'a"b|c?d*e\x01f', "x" * 130, "é" * 125]
    idents = ["Alice Example", "", "Zoë_名-x.y", "a<b>c", "١٢٣", "Ǆ x", "ab\u0345c", "\u093e"]
    return {
        "norm_path": norm, "branch_names": branches,
        "safe_filename": [[n, service._safe_filename(n)] for n in names],
        "author_ident": [[n, gitstore.author_ident(n).decode()] for n in idents],
    }


def smart():
    """dulwich's receive-pack replies to the twin spec's push bodies
    (gen_mods_git_push_bodies.py), one after the other on a fresh repo."""
    data = ROOT / "native" / "port" / "parity" / "data" / "mods_git"
    out = []
    for name in ("push_ok", "push_blocked"):
        reply = gitstore._service("smart", b"git-receive-pack", (data / f"{name}.bin").read_bytes())
        out.append([name, b64(reply)])
    refs = {k.decode(): v.decode() for k, v in gitstore._open("smart").get_refs().items()}
    return {"replies": out, "refs": refs}


def main():
    results = run_ops()
    smart_replies = smart()
    data = {
        "ops": encode_ops(),
        "results": results,
        "reads": asyncio.run(reads(results)),
        "writes": asyncio.run(writes()),
        "now": NOW.isoformat(),
    }
    shutil.rmtree(gitstore.repo_dir("smart"), ignore_errors=True)
    data["store"] = dump_store()
    data["smart"] = smart_replies
    data["http"] = http_cases()
    data["basic"] = basic_cases()
    data["authorize"] = authorize_cases()
    data["saves"] = saves()
    data["pure"] = pure()
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(data, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    shutil.rmtree(STORE, ignore_errors=True)
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
