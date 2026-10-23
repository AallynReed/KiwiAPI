"""Golden fixtures for kiwi-server's Mods Hub / Modpacks moderation routes
(``src/mods_moderation``), from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_mods_moderation_fixtures.py
Writes native/kiwi-server/testdata/mods_moderation/golden.json.

The real app runs at the ASGI level with ``get_current_superuser`` overridden
and every Beanie collection swapped for an in-memory fake that logs exactly
what Beanie sends to pymongo (filters, sorts, ``$set`` documents, inserts) and
what it read. ``utcnow`` is frozen. The stray import's per-mod upsert and the
run's catalog handling are driven with a fake HTTP client.
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

if os.environ.get("PYTHONHASHSEED") != "0":
    import subprocess
    sys.exit(subprocess.run([sys.executable, *sys.argv], env={**os.environ, "PYTHONHASHSEED": "0"}).returncode)

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "mods_moderation" / "golden.json"
os.chdir(ROOT)
sys.path.insert(0, str(ROOT))

from bson import ObjectId  # noqa: E402
from bson.int64 import Int64  # noqa: E402
from bson.json_util import CANONICAL_JSON_OPTIONS  # noqa: E402
from bson.json_util import dumps as ext_dumps  # noqa: E402
from beanie import init_beanie  # noqa: E402
from beanie.odm.utils import init as beanie_init  # noqa: E402
from beanie.odm.utils.dump import get_dict  # noqa: E402
from pymongo import AsyncMongoClient  # noqa: E402

import app.core.utils as core_utils  # noqa: E402
from app.auth.models import User  # noqa: E402
from app.core.dependencies import get_current_superuser  # noqa: E402
from app.site_auth.models import SiteUser  # noqa: E402
from app.trove import moderation, tmod  # noqa: E402
from app.trove.modpacks.models import ModpackProject, ModpackStar  # noqa: E402
from app.trove.mods_hub import gitstore, store, strayimport  # noqa: E402
from app.trove.mods_hub import service as hub  # noqa: E402
from app.trove.mods_hub.models import (  # noqa: E402
    ContentReport,
    ModClaimRequest,
    ModImageAsset,
    ModIssue,
    ModIssueEvent,
    ModProfile,
    ModProject,
    ModRelease,
    ModStar,
    RemixClaim,
    StrayImportState,
    TakenDownArtifact,
)

UTC = dt.timezone.utc
NOW = dt.datetime(2026, 10, 22, 14, 37, 21, 654321, tzinfo=UTC)
ADMIN_ID = "65a1b2c3d4e5f60718293a4b"
MODELS = [User, SiteUser, ModProject, ModRelease, ModProfile, ModClaimRequest, RemixClaim, ContentReport,
          TakenDownArtifact, StrayImportState, ModImageAsset, ModStar, ModIssue, ModIssueEvent, ModpackProject,
          ModpackStar]


class FrozenDatetime(dt.datetime):
    @classmethod
    def now(cls, tz=None):
        return NOW if tz is not None else NOW.replace(tzinfo=None)


core_utils.datetime = FrozenDatetime


def ext(value):
    return json.loads(ext_dumps(value, json_options=CANONICAL_JSON_OPTIONS))


def at(ms: int) -> dt.datetime:
    return dt.datetime.fromtimestamp(ms / 1000, UTC)


# --- the fake store --------------------------------------------------------------------

_oid_seq = [0]


def next_oid() -> ObjectId:
    _oid_seq[0] += 1
    return ObjectId(f"66f{_oid_seq[0]:021x}")


def _get(doc, key):
    cur = doc
    for part in key.split("."):
        if isinstance(cur, dict) and part in cur:
            cur = cur[part]
        else:
            return False, None
    return True, cur


def _cond(present, val, cond):
    if isinstance(cond, dict) and cond and all(k.startswith("$") for k in cond):
        for op, arg in cond.items():
            if op == "$in":
                vals = val if isinstance(val, list) else [val]
                if not any(v in arg for v in vals) and not (not present and None in arg):
                    return False
            elif op == "$nin":
                vals = val if isinstance(val, list) else [val]
                if any(v in arg for v in vals) or (not present and None in arg):
                    return False
            elif op == "$ne":
                if (val if present else None) == arg:
                    return False
            elif op == "$regex":
                flags = re.I if "i" in cond.get("$options", "") else 0
                vals = val if isinstance(val, list) else [val]
                if not any(isinstance(v, str) and re.search(arg, v, flags) for v in vals):
                    return False
            elif op == "$options":
                continue
            else:
                raise NotImplementedError(op)
        return True
    if cond is None:
        return not present or val is None
    if isinstance(val, list) and not isinstance(cond, list):
        return cond in val
    return present and val == cond


def match(doc, q) -> bool:
    for k, v in (q or {}).items():
        if k == "$and":
            if not all(match(doc, x) for x in v):
                return False
        elif k == "$or":
            if not any(match(doc, x) for x in v):
                return False
        else:
            present, val = _get(doc, k)
            if not _cond(present, val, v):
                return False
    return True


def _eq_fields(q, out):
    for k, v in (q or {}).items():
        if k == "$and":
            for x in v:
                _eq_fields(x, out)
        elif not k.startswith("$") and not (isinstance(v, dict) and any(x.startswith("$") for x in v)):
            out[k] = v
    return out


def bsonish(v):
    """What a BSON round trip makes of a value: datetimes floored to the millisecond."""
    if isinstance(v, dt.datetime):
        return v.replace(microsecond=v.microsecond // 1000 * 1000)
    if isinstance(v, dict):
        return {k: bsonish(x) for k, x in v.items()}
    if isinstance(v, list):
        return [bsonish(x) for x in v]
    return v


def apply_update(doc, update, inserting=False):
    for op, fields in update.items():
        if op == "$set" or (op == "$setOnInsert" and inserting):
            for k, v in fields.items():
                doc[k] = bsonish(copy.deepcopy(v))
        elif op == "$inc":
            for k, v in fields.items():
                doc[k] = doc.get(k, 0) + v
        elif op != "$setOnInsert":
            raise NotImplementedError(op)


def _sort_key(spec):
    def key(d):
        out = []
        for field, direction in spec:
            present, v = _get(d, field)
            rank = (0, 0) if v is None else (1, v)
            out.append(rank)
        return out
    return key


class FakeCursor:
    def __init__(self, docs):
        self.docs = docs

    async def to_list(self, length=None):
        return [copy.deepcopy(d) for d in self.docs]

    def __aiter__(self):
        self._it = iter(self.docs)
        return self

    async def __anext__(self):
        try:
            return copy.deepcopy(next(self._it))
        except StopIteration:
            raise StopAsyncIteration


class Result:
    def __init__(self, **kw):
        self.__dict__.update(kw)


class FakeDB:
    def __init__(self):
        self.colls: dict[str, list] = {}
        self.ops: list = []

    def coll(self, name):
        return self.colls.setdefault(name, [])

    def log(self, **entry):
        self.ops.append(ext(entry))


class FakeColl:
    def __init__(self, db: FakeDB, name: str):
        self.db, self.name = db, name

    @property
    def docs(self):
        return self.db.coll(self.name)

    async def find_one(self, filter=None, *a, **kw):
        hit = next((d for d in self.docs if match(d, filter)), None)
        self.db.log(op="find_one", coll=self.name, filter=filter or {}, result=hit)
        return copy.deepcopy(hit)

    def find(self, filter=None, sort=None, projection=None, skip=0, limit=0, *a, **kw):
        hits = [d for d in self.docs if match(d, filter)]
        spec = list(sort or [])
        for field, direction in reversed(spec):
            hits.sort(key=_sort_key([(field, direction)]), reverse=direction == -1)
        hits = hits[skip or 0:]
        if limit:
            hits = hits[:limit]
        self.db.log(op="find", coll=self.name, filter=filter or {}, sort=[[f, int(d)] for f, d in spec],
                    skip=skip or 0, limit=limit or 0, result=hits)
        return FakeCursor(hits)

    async def count_documents(self, filter, *a, **kw):
        n = sum(1 for d in self.docs if match(d, filter))
        self.db.log(op="count", coll=self.name, filter=filter, result=n)
        return n

    async def insert_one(self, document, *a, **kw):
        oid = document.get("_id") or next_oid()
        stored = {"_id": oid, **{k: v for k, v in document.items() if k != "_id"}}
        self.docs.append(bsonish(copy.deepcopy(stored)))
        self.db.log(op="insert_one", coll=self.name, doc=stored)
        return Result(inserted_id=oid)

    def _upsert(self, filter, update):
        doc = _eq_fields(filter, {})
        if "_id" not in doc:
            doc = {"_id": next_oid(), **doc}
        apply_update(doc, update, inserting=True)
        self.docs.append(doc)
        return doc

    async def find_one_and_update(self, filter, update, *a, upsert=False, **kw):
        hit = next((d for d in self.docs if match(d, filter)), None)
        before = copy.deepcopy(hit)
        if hit is not None:
            apply_update(hit, update)
        elif upsert:
            hit = self._upsert(filter, update)
        self.db.log(op="save", coll=self.name, filter=filter, update=update, upsert=upsert, before=before)
        return copy.deepcopy(hit)

    async def update_one(self, filter, update, upsert=False, *a, **kw):
        hit = next((d for d in self.docs if match(d, filter)), None)
        before = copy.deepcopy(hit)
        if hit is not None:
            apply_update(hit, update)
        elif upsert:
            self._upsert(filter, update)
        self.db.log(op="update_one", coll=self.name, filter=filter, update=update, upsert=upsert, before=before)
        return Result(matched_count=int(hit is not None))

    async def update_many(self, filter, update, *a, **kw):
        hits = [d for d in self.docs if match(d, filter)]
        for d in hits:
            apply_update(d, update)
        self.db.log(op="update_many", coll=self.name, filter=filter, update=update, matched=len(hits))
        return Result(matched_count=len(hits))

    async def delete_one(self, filter, *a, **kw):
        hit = next((d for d in self.docs if match(d, filter)), None)
        if hit is not None:
            self.docs.remove(hit)
        self.db.log(op="delete_one", coll=self.name, filter=filter)
        return Result(deleted_count=int(hit is not None))

    async def delete_many(self, filter, *a, **kw):
        hits = [d for d in self.docs if match(d, filter)]
        for d in hits:
            self.docs.remove(d)
        self.db.log(op="delete_many", coll=self.name, filter=filter, deleted=len(hits))
        return Result(deleted_count=len(hits))

    async def distinct(self, key, filter=None, *a, **kw):
        out = []
        for d in self.docs:
            if match(d, filter):
                present, v = _get(d, key)
                if present and v not in out:
                    out.append(v)
        self.db.log(op="distinct", coll=self.name, key=key, filter=filter or {})
        return out

    async def aggregate(self, pipeline, *a, **kw):
        rows = list(self.docs)
        for stage in pipeline:
            if "$match" in stage:
                rows = [d for d in rows if match(d, stage["$match"])]
            elif "$group" in stage:
                g = stage["$group"]
                field = g["_id"].lstrip("$")
                groups: dict = {}
                for d in rows:
                    k = d.get(field)
                    acc = groups.setdefault(str(k), {"_id": k, "s": 0})
                    acc["s"] += d.get(g["s"]["$sum"].lstrip("$"), 0)
                rows = list(groups.values())
            else:
                raise NotImplementedError(stage)
        self.db.log(op="aggregate", coll=self.name, pipeline=pipeline, result=rows)
        return FakeCursor(rows)


DB = FakeDB()
REAL_RUN = strayimport.run
EMAILS: list = []
REPOS: list = []
BLOBS: dict = {}


async def fake_queue_email(to, subject, text, html=None):
    EMAILS.append({"to": to, "subject": subject, "text": text, "html": html})


async def fake_delete_repo(project_id):
    REPOS.append(str(project_id))


async def fake_get_blob(sha):
    return BLOBS.get(sha)


async def fake_put_blob(data):
    sha = store.blob_sha(data)
    BLOBS[sha] = data
    return sha, True


# --- ASGI ------------------------------------------------------------------------------

async def call(app, method: str, url: str, body: bytes = b"", ctype: str | None = None, rid: str = "parity-mm"):
    path, _, qs = url.partition("?")
    headers = [(b"host", b"testserver"), (b"x-request-id", rid.encode()),
               (b"content-length", str(len(body)).encode())]
    if ctype:
        headers.append((b"content-type", ctype.encode()))
    scope = {
        "type": "http", "asgi": {"version": "3.0"}, "http_version": "1.1", "method": method,
        "scheme": "http", "path": urllib.parse.unquote(path), "raw_path": path.encode(),
        "query_string": qs.encode(), "root_path": "", "headers": headers,
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
    hdrs = {k.decode("latin-1").lower(): v.decode("latin-1") for k, v in start["headers"]}
    out = {"status": start["status"], "body": raw.decode("utf-8")}
    for name in ("allow", "www-authenticate", "content-type"):
        if name in hdrs:
            out[name] = hdrs[name]
    return out


# --- seed --------------------------------------------------------------------------------

def oid(n: int) -> ObjectId:
    return ObjectId(f"65b{n:021x}")


OWNER, CLAIMANT, NEWOWNER, QUIET = oid(1), oid(2), oid(3), oid(4)
P_PUB, P_DRAFT, P_WEB, S_PEND, S_APPR, S_REJ, N_CLASH, RX_T, RX_O = (oid(n) for n in range(0x10, 0x19))
PACK1, PACK2 = oid(0x30), oid(0x31)
PROF1 = oid(0x40)
REP1, REP2, REP3 = oid(0x50), oid(0x51), oid(0x52)
C1, C2, C3 = oid(0x60), oid(0x61), oid(0x62)
RC1, RC2 = oid(0x70), oid(0x71)
R1, R2, R3, R4 = oid(0x80), oid(0x81), oid(0x82), oid(0x83)
SHA = {k: (c * 64) for k, c in (("r1", "a"), ("r1p", "b"), ("r3", "c"), ("r4", "d"), ("ban", "e"), ("pre", "f"))}
T0 = at(1_760_000_000_123)


def site_user(uid, username, **kw):
    base = {"_id": uid, "username": username, "discord_handle": "", "notify_email": None, "display_name": None,
            "is_active": True, "is_verified": True, "is_deleted": False, "discord_id": None, "discord_avatar": None,
            "token_version": 0, "claimed_trove_name": None, "claimed_trove_display": None, "claimed_at": None,
            "claim_verified": False, "claim_baseline": {}, "claim_verified_at": None, "creator_token_hash": None,
            "creator_token_prefix": None, "creator_token_at": None, "mod_issues_seen_at": None,
            "created_at": T0, "updated_at": T0, "last_login_at": None}
    base.update(kw)
    return base


def project(pid, slug, title, **kw):
    base = {"_id": pid, "slug": slug, "title": title, "summary": "", "owner_id": OWNER,
            "owner_username": "Owner Name", "owner_handle": "owner", "visibility": "public",
            "created_at": at(1_750_000_000_000), "updated_at": at(1_760_000_000_000)}
    base.update(kw)
    return base


def seed():
    DB.colls = {
        "site_users": [
            site_user(OWNER, "owner", display_name="Owner Name", notify_email="owner@kiwi-mail.net"),
            site_user(CLAIMANT, "claimant", notify_email="claimant@kiwi-mail.net"),
            site_user(NEWOWNER, "newowner", display_name="New Owner"),
            site_user(QUIET, "quiet", display_name=""),
        ],
        "mod_projects": [
            project(P_PUB, "kiwi-sword", "Kiwi Sword", summary="A sword", tags=["weapon", "sword"],
                    banner_sha=SHA["ban"], preview_shas=[SHA["pre"], SHA["ban"]], download_count=Int64(42),
                    star_count=3, downloads_7d=7, popularity_score=0.1234567, is_beta=True,
                    title_i18n={"fr": "Épée Kiwi", "de": "Kiwi-Schwert"}, fork_count=1,
                    last_release_at=at(1_759_000_000_500), updated_at=at(1_761_000_000_001)),
            project(P_DRAFT, "draft-mod", "Draft Mod", visibility="draft",
                    collaborators=[{"user_id": CLAIMANT, "username": "claimant", "extra": 1}],
                    inspired_by_slug="kiwi-sword", inspired_by_handle="owner", inspired_by_title="Kiwi Sword",
                    inspired_by_owner="Owner Name", popularity_score=1, updated_at=at(1_760_500_000_000)),
            project(P_WEB, "web-only", "Web Only", visibility="public_website_only", popularity_score=0.5,
                    uploaded_on_behalf=True, author="Someone Else", updated_at=at(1_759_500_000_000)),
            project(S_PEND, "lost-mod", "Lost Mod", owner_id=None, owner_username="Ghost", owner_handle="stray",
                    is_stray=True, stray_status="pending", visibility="draft", author="Ghost", mode="releases",
                    source=strayimport.SOURCE, source_id="101", source_url="https://example.invalid/101",
                    updated_at=at(1_758_000_000_000)),
            project(S_APPR, "clash-mod", "Clash Mod", owner_id=None, owner_username="Wraith", owner_handle="stray",
                    is_stray=True, stray_status="approved", author="Wraith", mode="releases",
                    updated_at=at(1_757_000_000_000)),
            project(S_REJ, "nope-mod", "Nope Mod", owner_id=None, owner_username="Shade", owner_handle="stray",
                    is_stray=True, stray_status="rejected", visibility="draft", author="Shade",
                    updated_at=at(1_756_000_000_000)),
            project(N_CLASH, "clash-mod", "Theirs", owner_id=NEWOWNER, owner_username="New Owner",
                    owner_handle="newowner", updated_at=at(1_755_000_000_000)),
            project(RX_T, "remix-target", "Remix Target", owner_id=QUIET, owner_username="quiet",
                    owner_handle="quiet", updated_at=at(1_754_000_000_000)),
            project(RX_O, "remix-original", "Remix Original", fork_count=Int64(2), updated_at=at(1_753_000_000_000)),
        ],
        "mod_releases": [
            {"_id": R1, "project_id": P_PUB, "owner_id": OWNER, "tag": "v1", "tmod_sha": SHA["r1"],
             "prior_tmod_shas": [SHA["r1p"], ""], "tmod_size": 1000, "status": "published",
             "created_at": T0, "updated_at": T0, "published_at": T0, "announced_at": T0},
            {"_id": R2, "project_id": S_PEND, "owner_id": None, "tag": "3", "tmod_sha": SHA["r4"], "tmod_size": 10,
             "status": "published", "created_at": T0, "updated_at": T0, "published_at": T0, "announced_at": None},
            {"_id": R3, "project_id": P_PUB, "owner_id": OWNER, "tag": "v2", "branch": "beta", "tmod_sha": SHA["r3"],
             "tmod_size": Int64(500), "status": "draft", "created_at": T0, "updated_at": T0},
            {"_id": R4, "project_id": P_DRAFT, "owner_id": OWNER, "tag": "v0", "tmod_sha": SHA["r3"], "tmod_size": 5,
             "status": "published", "created_at": T0, "updated_at": T0},
        ],
        "mod_images": [
            {"_id": oid(0x90), "sha": SHA["ban"], "content_type": "image/png", "byte_size": 2048, "owner_id": OWNER,
             "width": 10, "height": 10, "variants": {}, "created_at": T0},
            {"_id": oid(0x91), "sha": SHA["pre"], "content_type": "image/webp", "byte_size": Int64(300),
             "owner_id": OWNER, "width": None, "height": None, "variants": {"200": SHA["r1"]}, "created_at": T0},
        ],
        "mod_profiles": [
            {"_id": PROF1, "site_user_id": OWNER, "handle": "owner", "display_name": "", "tagline": "hi",
             "created_at": T0, "updated_at": T0},
        ],
        "content_reports": [
            {"_id": REP1, "target_type": "mod", "target_id": P_PUB, "target_label": "Kiwi Sword",
             "target_url": "/mods/owner/kiwi-sword", "reason": "stolen", "resolved": False,
             "created_at": at(1_761_100_000_000)},
            {"_id": REP2, "target_type": "profile", "target_id": PROF1, "target_label": "owner",
             "target_url": "/mods/owner", "reason": "spam", "resolved": False, "created_at": at(1_761_200_000_000)},
            {"_id": REP3, "target_type": "modpack", "target_id": PACK1, "target_label": "Pack",
             "target_url": "/modpacks/owner/pack", "reason": "old", "resolved": True,
             "created_at": at(1_761_000_000_000)},
        ],
        "mod_claim_requests": [
            {"_id": C1, "project_id": S_PEND, "project_slug": "lost-mod", "project_title": "Lost Mod",
             "claimant_id": CLAIMANT, "claimant_username": "claimant", "message": "mine", "status": "pending",
             "resolved_by": None, "resolved_at": None, "created_at": at(1_761_300_000_000)},
            {"_id": C2, "project_id": S_PEND, "project_slug": "lost-mod", "claimant_id": QUIET,
             "claimant_username": "quiet", "status": "pending", "created_at": at(1_761_200_000_000)},
            {"_id": C3, "project_id": S_REJ, "project_slug": "nope-mod", "project_title": "Nope Mod",
             "claimant_id": CLAIMANT, "claimant_username": "claimant", "status": "approved",
             "resolved_by": ObjectId(ADMIN_ID), "resolved_at": at(1_761_000_000_500),
             "created_at": at(1_760_000_000_000)},
        ],
        "mod_remix_claims": [
            {"_id": RC1, "target_id": RX_T, "target_slug": "remix-target", "target_title": "Remix Target",
             "original_id": RX_O, "original_slug": "remix-original", "original_title": "Remix Original",
             "claimant_id": OWNER, "claimant_username": "owner", "message": "it's mine", "status": "pending",
             "created_at": at(1_761_400_000_000)},
            {"_id": RC2, "target_id": RX_T, "original_id": P_PUB, "claimant_id": OWNER, "claimant_username": "owner",
             "status": "pending", "created_at": at(1_761_300_000_000)},
        ],
        "mod_taken_down_artifacts": [
            {"_id": oid(0xA0), "sha256": SHA["r4"], "md5": None, "project_id": S_REJ, "title": "Nope Mod",
             "owner_handle": "stray", "created_at": T0},
        ],
        "mod_stars": [{"_id": oid(0xB0), "project_id": P_DRAFT, "site_user_id": CLAIMANT, "created_at": T0}],
        "mod_issues": [{"_id": oid(0xB1), "project_id": P_DRAFT}],
        "mod_issue_events": [{"_id": oid(0xB2), "project_id": P_DRAFT}],
        "modpack_projects": [
            {"_id": PACK1, "slug": "pack", "title": "Kiwi Pack", "summary": "best", "tags": ["pvp"], "owner_id": OWNER,
             "owner_username": "Owner Name", "owner_handle": "owner", "visibility": "public",
             "variants": [{"name": "default", "label": "", "entries": [
                 {"project_id": P_PUB, "handle": "owner", "slug": "kiwi-sword", "title": "Kiwi Sword", "junk": 1},
                 {"custom_sha": SHA["r3"], "custom_filename": "X.tmod", "title": "X"}]},
                 {"name": "lite", "label": "Lite", "entries": []}],
             "default_variant": "lite", "created_at": T0, "updated_at": at(1_761_000_000_000)},
            {"_id": PACK2, "slug": "pack-2", "title": "Second Pack", "owner_id": QUIET, "owner_username": "quiet",
             "visibility": "draft", "preview_shas": [SHA["pre"]], "created_at": T0, "updated_at": T0},
        ],
        "modpack_stars": [{"_id": oid(0xC0), "modpack_id": PACK2, "site_user_id": OWNER, "created_at": T0}],
    }
    BLOBS.clear()
    BLOBS[SHA["r1"]] = b"first release bytes"
    BLOBS[SHA["r3"]] = b""


SCENARIO = [
    ("reports", "GET", "/admin/mods/reports", None),
    ("reports_resolved", "GET", "/admin/mods/reports?resolved=true", None),
    ("dismiss", "POST", f"/admin/mods/reports/{REP2}/dismiss", None),
    ("dismiss_again", "POST", f"/admin/mods/reports/{REP2}/dismiss", None),
    ("dismiss_bad", "POST", "/admin/mods/reports/nope/dismiss", None),
    ("projects", "GET", "/admin/mods/projects", None),
    ("projects_size", "GET", "/admin/mods/projects?sort=size&limit=4&offset=1", None),
    ("projects_popular", "GET", "/admin/mods/projects?sort=popularity&q=+kiwi.&owner=Owner+Name&visibility=public", None),
    ("projects_stars", "GET", "/admin/mods/projects?sort=stars&q=&limit=2", None),
    ("projects_bogus", "GET", "/admin/mods/projects?sort=bogus&offset=7", None),
    ("takedown", "POST", f"/admin/mods/projects/{P_PUB}/takedown", b'{"reason": "  Stolen <b>assets</b>  "}'),
    ("takedown_default", "POST", f"/admin/mods/projects/{P_DRAFT}/takedown", b"{}"),
    ("takedown_missing", "POST", f"/admin/mods/projects/{oid(0xFFF)}/takedown", b"{}"),
    ("takedown_bad_id", "POST", "/admin/mods/projects/xyz/takedown", b'{"reason": "x"}'),
    ("restore", "POST", f"/admin/mods/projects/{P_PUB}/restore", None),
    ("restore_missing", "POST", "/admin/mods/projects/nope/restore", None),
    ("profile_takedown", "POST", f"/admin/mods/profiles/{PROF1}/takedown", b'{"reason": "Spam"}'),
    ("profile_restore", "POST", f"/admin/mods/profiles/{PROF1}/restore", None),
    ("profile_missing", "POST", f"/admin/mods/profiles/{oid(0xFFF)}/takedown", b"{}"),
    ("stray", "GET", "/admin/mods/stray", None),
    ("stray_all", "GET", "/admin/mods/stray?status=&q=ghost&limit=1", None),
    ("stray_rejected", "GET", "/admin/mods/stray?status=rejected", None),
    ("stray_approve", "POST", f"/admin/mods/stray/{S_PEND}/approve", None),
    ("stray_reject", "POST", f"/admin/mods/stray/{S_REJ}/reject", None),
    ("stray_not_stray", "POST", f"/admin/mods/stray/{P_PUB}/approve", None),
    ("claims", "GET", "/admin/mods/claims", None),
    ("claims_all", "GET", "/admin/mods/claims?status=", None),
    ("claim_approve", "POST", f"/admin/mods/claims/{C1}/approve", None),
    ("claim_approve_again", "POST", f"/admin/mods/claims/{C1}/approve", None),
    ("claim_reject", "POST", f"/admin/mods/claims/{C3}/reject", None),
    ("claim_missing", "POST", "/admin/mods/claims/zzz/reject", None),
    ("assign", "POST", "/admin/mods/stray/assign",
     json.dumps({"user_id": str(NEWOWNER), "project_ids": [str(S_APPR), str(P_PUB), "junk"]}).encode()),
    ("assign_no_user", "POST", "/admin/mods/stray/assign", b'{"user_id": "junk", "project_ids": ["x"]}'),
    ("remix_claims", "GET", "/admin/mods/remix-claims", None),
    ("remix_approve", "POST", f"/admin/mods/remix-claims/{RC1}/approve", None),
    ("remix_approve_again", "POST", f"/admin/mods/remix-claims/{RC1}/approve", None),
    ("remix_reject", "POST", f"/admin/mods/remix-claims/{RC2}/reject", None),
    ("remix_claims_after", "GET", "/admin/mods/remix-claims?status=approved", None),
    ("modpacks", "GET", "/admin/modpacks", None),
    ("modpacks_q", "GET", "/admin/modpacks?q=pack&owner=quiet&visibility=draft&limit=5&offset=0", None),
    ("pack_takedown", "POST", f"/admin/modpacks/{PACK1}/takedown", b'{"reason": "  "}'),
    ("pack_takedown2", "POST", f"/admin/modpacks/{PACK1}/takedown", b'{"reason": " Broken links "}'),
    ("pack_restore", "POST", f"/admin/modpacks/{PACK1}/restore", None),
    ("pack_delete", "DELETE", f"/admin/modpacks/{PACK2}", None),
    ("pack_missing", "DELETE", "/admin/modpacks/nope", None),
    ("project_delete", "DELETE", f"/admin/mods/projects/{P_DRAFT}", None),
    ("project_delete_missing", "DELETE", f"/admin/mods/projects/{P_DRAFT}", None),
    ("import_state", "GET", "/admin/mods/stray/import", None),
    ("import_owner_no_author", "POST", "/admin/mods/stray/import?owner_id=abc", None),
    ("import_owner_missing", "POST", "/admin/mods/stray/import?author_id=+5+&owner_id=65b000000000000000000fff", None),
    ("import_start", "POST", "/admin/mods/stray/import?resync=true&author_id=%207%20&owner_id=" + str(NEWOWNER), None),
    ("import_start_resync", "POST", "/admin/mods/stray/import?resync=true&author_id=+", None),
]


async def scenario_section(app) -> dict:
    seed()
    out = {"steps": []}
    started: list = []

    async def fake_run(resync, author_id=None, owner=None):
        started.append({"resync": resync, "author_id": author_id, "owner": str(owner.id) if owner else None})

    strayimport.run = fake_run
    for i, (name, method, url, body) in enumerate(SCENARIO):
        DB.ops = []
        EMAILS.clear()
        REPOS.clear()
        res = await call(app, method, url, body or b"", "application/json" if body is not None else None,
                         rid=f"parity-mm-{i}")
        await asyncio.sleep(0)
        out["steps"].append({"name": name, "method": method, "url": url,
                             "sent": body.decode() if body is not None else None, **res,
                             "ops": DB.ops, "emails": list(EMAILS), "repos": list(REPOS)})
    out["started"] = started
    # A run in progress refuses a second start.
    DB.colls["stray_import_state"][0].update(running=True, started_at=bsonish(NOW - dt.timedelta(minutes=5)))
    DB.ops = []
    out["already_running"] = {**await call(app, "POST", "/admin/mods/stray/import"), "ops": DB.ops}
    DB.colls["stray_import_state"][0].update(started_at=bsonish(NOW - dt.timedelta(hours=3, seconds=1)))
    DB.ops = []
    out["stale_run"] = {**await call(app, "POST", "/admin/mods/stray/import?force=false"), "ops": DB.ops}
    return out


# --- model normalization --------------------------------------------------------------------

def model_cases() -> dict:
    """What a load followed by ``save()`` writes (``$set``), or that the load fails."""
    pid = oid(0x500)
    cases = {
        "mod_project": (ModProject, [
            {"_id": pid, "slug": "s", "title": "t"},
            {"_id": pid, "slug": "s", "title": "t", "download_count": Int64(5), "popularity_score": 2,
             "star_count": 3.0, "taken_down": 1, "is_beta": 0, "owner_id": str(OWNER),
             "collaborators": [{"user_id": OWNER, "username": "o", "x": True}], "title_i18n": {"fr": "T"},
             "remix_requested_at": T0, "extra_field": "kept", "fork_count": True},
            {"_id": pid, "slug": "s", "title": "t", "visibility": "weird"},
            {"_id": pid, "slug": "s"},
            {"_id": pid, "slug": "s", "title": 5},
            {"_id": pid, "slug": "s", "title": "t", "popularity_score": True, "issues_enabled": 2},
            {"_id": pid, "slug": "s", "title": "t", "download_count": 2.5},
            {"_id": pid, "slug": "s", "title": "t", "tags": ["a", 1]},
            {"_id": pid, "slug": "s", "title": "t", "stray_status": None, "source": None, "mode": "releases",
             "created_at": T0},
        ]),
        "mod_release": (ModRelease, [
            {"_id": pid, "project_id": OWNER, "tag": "v1", "tmod_sha": "x"},
            {"_id": pid, "project_id": OWNER, "tag": "v1", "tmod_sha": "x", "tmod_properties": {"a": "b"},
             "prior_tmod_shas": ["p"], "tmod_size": Int64(3_000_000_000), "status": "draft"},
            {"_id": pid, "project_id": OWNER, "tag": "v1"},
        ]),
        "mod_profile": (ModProfile, [{"_id": pid, "site_user_id": OWNER}]),
        "claim": (ModClaimRequest, [
            {"_id": pid, "project_id": OWNER, "project_slug": "s", "claimant_id": OWNER, "claimant_username": "c"},
            {"_id": pid, "project_id": OWNER, "project_slug": "s", "claimant_id": OWNER, "claimant_username": "c",
             "status": "denied"},
        ]),
        "remix_claim": (RemixClaim, [
            {"_id": pid, "target_id": OWNER, "original_id": OWNER, "claimant_id": OWNER, "claimant_username": "c"},
        ]),
        "content_report": (ContentReport, [
            {"_id": pid, "target_type": "mod", "target_id": OWNER, "reason": "r", "created_at": T0},
            {"_id": pid, "target_type": "user", "target_id": OWNER, "reason": "r"},
        ]),
        "taken_down": (TakenDownArtifact, [{"_id": pid, "sha256": "x", "created_at": T0}]),
        "stray_state": (StrayImportState, [{"_id": pid}, {"_id": pid, "total": Int64(9), "running": True}]),
        "image_asset": (ModImageAsset, [
            {"_id": pid, "sha": "x", "content_type": "image/png", "byte_size": 4, "width": Int64(7), "created_at": T0},
        ]),
        "modpack": (ModpackProject, [
            {"_id": pid, "slug": "p", "title": "P", "owner_id": OWNER, "owner_username": "o",
             "variants": [{"name": "default", "entries": [{"slug": "x", "junk": 1}, {}]}], "created_at": T0},
            {"_id": pid, "slug": "p", "title": "P", "owner_username": "o"},
            {"_id": pid, "slug": "p", "title": "P", "owner_id": OWNER, "owner_username": "o",
             "variants": [{"label": "no name"}]},
        ]),
    }
    out = {}
    for name, (cls, raws) in cases.items():
        rows = []
        for raw in raws:
            try:
                saved = ext(get_dict(cls.model_validate(raw), to_db=True))
                rows.append({"raw": ext(raw), "saved": saved})
            except Exception:
                rows.append({"raw": ext(raw), "error": True})
        out[name] = rows
    return out


# --- emails, cards ---------------------------------------------------------------------------------

async def email_cases() -> list:
    out = []
    for args in (("a@x.io", "mod", "Kiwi <Sword>", "  Spam  ", "/mods/o/k"),
                 ("b@x.io", "profile", "owner", "", ""),
                 ("c@x.io", "weird", "Thing", "Because", "/x"),
                 (None, "mod", "Skipped", "r", "/u"),
                 ("", "mod", "Skipped", "r", "/u")):
        EMAILS.clear()
        await moderation.notify_takedown(*args)
        out.append({"args": list(args), "email": EMAILS[0] if EMAILS else None})
    return out


def slug_cases() -> list:
    titles = ["Kiwi Sword", "  --Hello, World!--  ", "Épée de Kiwi", "", "!!!", "\u212aelvin", "\u0130stanbul",
              "a" * 70 + " b", "Mod_Name 2.0", "日本語"]
    return [{"title": t, "slug": hub._slugify(t)} for t in titles]


def tag_cases() -> list:
    raws = [["Weapon", "weapon", " Two  Words ", "", "x" * 40], [f"t{i}" for i in range(15)], ["Tab\tSep\nNew"],
            ["\u00a0nbsp\u2003em\u00a0"], ["ΣΑΣ"]]
    return [{"raw": r, "clean": hub._clean_tags(r)} for r in raws]


# --- stray import -----------------------------------------------------------------------------

def tiny_tmod(title: str, author: str | None) -> bytes:
    props = {"title": title}
    if author is not None:
        props["author"] = author
    return tmod.build_tmod(1, props, [("ui/x.swf", b"FWS\x0a" + b"\x00" * 8)])


PNG = b"\x89PNG\r\n\x1a\n" + b"\x00\x00\x00\x0dIHDR" + (300).to_bytes(4, "big") + (150).to_bytes(4, "big") + b"\x08\x06" + b"\x00" * 20
GIF = b"GIF89a" + (64).to_bytes(2, "little") + (32).to_bytes(2, "little") + b"\x00" * 20
JPEG = (b"\xff\xd8\xff\xe0\x00\x10JFIF\x00\x01\x01\x00\x00\x01\x00\x01\x00\x00"
        b"\xff\xc0\x00\x11\x08\x01\x2c\x02\x58\x03\x01\x22\x00\x02\x11\x01\x03\x11\x01\xff\xd9")
WEBP_LOSSY = b"RIFF\x00\x00\x00\x00WEBPVP8 " + b"\x00" * 10 + (500).to_bytes(2, "little") + (250).to_bytes(2, "little") + b"\x00" * 4
WEBP_L = b"RIFF\x00\x00\x00\x00WEBPVP8L\x00\x00\x00\x00\x2f" + bytes([0x3F, 0xC0, 0x0F, 0x00]) + b"\x00" * 6
WEBP_X = b"RIFF\x00\x00\x00\x00WEBPVP8X" + b"\x00" * 8 + bytes([0x1F, 0x00, 0x00, 0x0F, 0x00, 0x00]) + b"\x00" * 2
WEBP_SHORT = b"RIFF\x00\x00\x00\x00WEBPVP8 " + b"\x00" * 9


class FakeResp:
    def __init__(self, status, content, url):
        self.status_code, self.content, self.url = status, content, url


class FakeClient:
    def __init__(self, routes):
        self.routes, self.requested = routes, []

    async def get(self, url, follow_redirects=False):
        self.requested.append(url)
        hit = self.routes.get(url)
        if hit is None:
            raise RuntimeError("unreachable")
        return FakeResp(hit[0], hit[1], url)




def FILE(fileid):
    return strayimport.FILE_URL.format(fileid=fileid)


TM_A = tiny_tmod("Kiwi Hat", "Real Author")
TM_B = tiny_tmod("  ", None)
TM_C = tiny_tmod("Updated Hat", "New Author")


def catalog_mod(mid, name, files, **kw):
    base = {"id": mid, "name": name, "author": {"ID": 77, "Username": " Listed "}, "description": "desc",
            "image": f"https://img.invalid/{mid}.png", "type": "Costume", "subtype": " Hat ", "totaldownloads": "12",
            "likes": 4, "downloads": files}
    base.update(kw)
    return base


STRAY_MODS = [
    ("new_tmod", catalog_mod(201, " Kiwi Hat ", [
        {"fileid": 5, "version": "1.0", "format": "tmod", "changes": "first", "date": 1_700_000_000},
        {"fileid": 9, "version": 2.5, "format": "TMOD", "changes": "", "date": "1700000500"},
        {"fileid": 12, "extra": 1, "format": "zip"}]), False),
    ("new_zip_pending", catalog_mod("202", "", [{"fileid": "7", "format": "zip", "date": 0}],
                                    author="nobody", type=None, subtype="Weapon", image=None,
                                    totaldownloads=3.9, likes=None), True),
    ("new_no_title", catalog_mod(203, "Kiwi Hat", [{"fileid": 8, "format": "rar", "changes": None,
                                                     "date": "x"}], image=5), False),
    ("no_downloads", catalog_mod(204, "Empty", []), False),
    ("bad_downloads_count", catalog_mod(205, "Bad", [{"fileid": 1}], totaldownloads="12.5"), False),
    ("no_id", catalog_mod("  ", "No id", [{"fileid": 1}]), False),
    ("download_fails", catalog_mod(206, "Gone", [{"fileid": 404}]), False),
    ("existing_refresh", catalog_mod(201, "Kiwi Hat Renamed", [{"fileid": 10, "version": "", "changes": "fixed",
                                                                 "date": 1_700_100_000}], description=""), True),
    ("existing_same_file", catalog_mod(201, "Kiwi Hat", [{"fileid": 10}], likes="8"), False),
    ("existing_rejected", catalog_mod(209, "Rejected", [{"fileid": 3}]), False),
    ("bad_name", catalog_mod(210, ["x"], [{"fileid": 3}]), False),
    ("bad_changes", catalog_mod(211, "Bad changes", [{"fileid": 13, "changes": 5}]), False),
]


async def stray_section() -> dict:
    DB.colls = {"mod_projects": [project(oid(0x209), "rejected", "Rejected", owner_id=None, owner_handle="stray",
                                         is_stray=True, stray_status="rejected", source=strayimport.SOURCE,
                                         source_id="209")],
                "mod_releases": [], "mod_images": []}
    routes = {
        FILE("9"): (200, TM_A), FILE("5"): (200, TM_A), FILE("7"): (200, b"PK\x03\x04zip"),
        FILE("8"): (200, TM_B), FILE("404"): (404, b"nope"), FILE("10"): (200, TM_C), FILE("13"): (200, TM_A),
        "https://img.invalid/201.png": (200, PNG), "https://img.invalid/203.png": (200, GIF),
        "https://img.invalid/202.png": (200, JPEG),
    }
    client = FakeClient(routes)
    out = []
    for name, mod, resync in STRAY_MODS:
        DB.ops = []
        try:
            outcome = await strayimport._upsert_mod(client, mod, resync=resync)
        except Exception as e:
            outcome = {"raised": type(e).__name__}
        out.append({"name": name, "mod": mod, "resync": resync, "outcome": outcome, "ops": DB.ops})
    return {"cases": out, "files": {k.rsplit("=", 1)[-1]: v[1].hex() for k, v in routes.items() if "fileid=" in k}}


def stray_helpers() -> dict:
    downloads = [
        [], None, "abc", {"a": 1}, [{"fileid": 3}, {"fileid": "10"}, {"fileid": 10, "extra": "0"}],
        [{"fileid": 3, "extra": "yes"}, {"fileid": 2, "extra": 1}, "junk"], [{"fileid": 3, "extra": 0.5}],
        [{"fileid": 1, "extra": True}], [{"fileid": " 4 "}, {"fileid": 4}], [{"fileid": None}, {}],
        [{"fileid": "x"}], 5, True,
    ]
    latest = []
    for d in downloads:
        try:
            latest.append({"in": d, "out": strayimport._latest_download(d)})
        except Exception as e:
            latest.append({"in": d, "error": type(e).__name__})
    times = []
    for d in ({"date": 1_700_000_000}, {"date": "1700000000"}, {"date": 0}, {"date": -5}, {"date": "x"},
              {}, {"date": 1.9}, {"date": None}, {"date": [1]}):
        try:
            t = strayimport._upload_time(d)
            times.append({"in": d, "out": t.isoformat() if t else None})
        except (ValueError, OverflowError, OSError) as e:
            times.append({"in": d, "error": type(e).__name__})
    values = [None, True, False, 0, 7, -3, 2.5, 1e16, 1.0, "s", "it's", [1, "a", None, [2.0]], {"k": "v", "n": 1}, []]
    py_str = [{"in": v, "out": str(v)} for v in values]
    ints = []
    for v in (True, 3, 2.9, -2.9, " 12 ", "1_000", "+5", "1.5", "", "x", None, [1], {"a": 1}, "\uff11\uff12"):
        try:
            ints.append({"in": v, "out": int(v)})
        except (TypeError, ValueError) as e:
            ints.append({"in": v, "error": str(e)})
    images = []
    for label, data in (("png", PNG), ("gif", GIF), ("jpeg", JPEG), ("webp", WEBP_LOSSY), ("webp_l", WEBP_L),
                        ("webp_x", WEBP_X), ("webp_short", WEBP_SHORT), ("short", PNG[:20]), ("text", b"x" * 40),
                        ("jpeg_nosof", b"\xff\xd8" + b"\x00" * 30)):
        images.append({"name": label, "hex": data.hex(), "out": store.sniff_image(data)})
    return {"latest": latest, "times": times, "py_str": py_str, "ints": ints, "images": images}


async def run_cases() -> list:
    """``run()``'s catalog handling: what ``last_error``/counters end up as."""
    out = []
    cases = [
        ("status_404", (404, b"missing"), None),
        ("status_503", (503, b""), None),
        ("bad_json", (200, b'{"a": '), None),
        ("bad_json_lines", (200, b'[\n  1,\n  2 x'), None),
        ("number", (200, b"5"), None),
        ("null", (200, b"null"), None),
        ("string", (200, b'"abc"'), None),
        ("author_not_dict", (200, json.dumps([{"id": 1, "author": "bob"}]).encode()), "1"),
        ("empty_dict", (200, b"{}"), None),
        ("filtered", (200, json.dumps({"a": {"id": 0}, "b": {"id": 1, "author": {"ID": 2}, "downloads": []},
                                       "c": "x", "d": {"id": 3, "author": None}}).encode()), "2"),
    ]

    import httpx

    class CatalogClient:
        def __init__(self, resp, **kw):
            self.resp = resp

        async def __aenter__(self):
            return self

        async def __aexit__(self, *a):
            return False

        async def get(self, url, follow_redirects=False):
            status, content = self.resp
            req = httpx.Request("GET", url)
            return httpx.Response(status, content=content, request=req)

    real = strayimport.httpx.AsyncClient
    for name, resp, author in cases:
        DB.colls = {"stray_import_state": [], "mod_projects": [], "mod_releases": [], "mod_images": []}
        DB.ops = []
        strayimport.httpx.AsyncClient = lambda *a, _r=resp, **k: CatalogClient(_r)
        await REAL_RUN(False, author)
        st = DB.colls["stray_import_state"][0]
        out.append({"name": name, "status": resp[0], "content": resp[1].decode(), "author_id": author,
                    "state": ext(st)})
    strayimport.httpx.AsyncClient = real
    return out


def admin_user() -> User:
    return User.model_validate({"_id": ObjectId(ADMIN_ID), "email": "master@aallyn.net", "hashed_password": "x",
                                "is_superuser": True, "is_active": True, "is_verified": True})


UNAUTH = [
    ("GET", "/admin/mods/reports"), ("POST", f"/admin/mods/reports/{REP1}/dismiss"), ("GET", "/admin/mods/projects"),
    ("DELETE", f"/admin/mods/projects/{P_PUB}"), ("POST", f"/admin/mods/projects/{P_PUB}/takedown"),
    ("POST", f"/admin/mods/projects/{P_PUB}/restore"), ("POST", f"/admin/mods/profiles/{PROF1}/takedown"),
    ("POST", f"/admin/mods/profiles/{PROF1}/restore"), ("GET", "/admin/mods/stray/import"),
    ("POST", "/admin/mods/stray/import"), ("GET", "/admin/mods/stray"), ("POST", "/admin/mods/stray/assign"),
    ("POST", f"/admin/mods/stray/{S_PEND}/approve"), ("POST", f"/admin/mods/stray/{S_PEND}/reject"),
    ("GET", "/admin/mods/claims"), ("POST", f"/admin/mods/claims/{C1}/approve"),
    ("POST", f"/admin/mods/claims/{C1}/reject"), ("GET", "/admin/mods/remix-claims"),
    ("POST", f"/admin/mods/remix-claims/{RC1}/approve"), ("POST", f"/admin/mods/remix-claims/{RC1}/reject"),
    ("GET", "/admin/modpacks"), ("DELETE", f"/admin/modpacks/{PACK1}"), ("POST", f"/admin/modpacks/{PACK1}/takedown"),
    ("POST", f"/admin/modpacks/{PACK1}/restore"),
    ("PUT", "/admin/mods/stray/import"), ("DELETE", "/admin/mods/stray/import"), ("POST", "/admin/mods/reports"),
    ("GET", f"/admin/mods/projects/{P_PUB}"), ("GET", f"/admin/mods/projects/{P_PUB}/takedown"),
    ("GET", "/admin/mods/stray/assign"), ("PATCH", "/admin/modpacks"), ("GET", f"/admin/modpacks/{PACK1}"),
    ("HEAD", "/admin/mods/stray"), ("HEAD", "/admin/mods/stray/import"), ("PUT", f"/admin/mods/claims/{C1}/approve"),
]


async def unauth_section(app) -> list:
    out = []
    for i, (method, url) in enumerate(UNAUTH):
        res = await call(app, method, url, rid=f"parity-mm-u{i}")
        out.append({"method": method, "url": url, **res})
    for i, url in enumerate([f"/admin/mods/projects/{P_PUB}/takedown", "/admin/mods/stray/assign",
                             f"/admin/modpacks/{PACK1}/takedown"]):
        res = await call(app, "POST", url, b'{"reason": ', "application/json", rid=f"parity-mm-j{i}")
        out.append({"method": "POST", "url": url, "body_sent": '{"reason": ', **res})
    return out


async def params_section(app) -> list:
    """422s answered after auth, before the handler."""
    reqs = [
        ("GET", "/admin/mods/reports?resolved=maybe"),
        ("GET", "/admin/mods/projects?limit=0&offset=-1"),
        ("GET", "/admin/mods/projects?limit=201&offset=x"),
        ("GET", "/admin/mods/stray?limit=abc"),
        ("POST", "/admin/mods/stray/import?resync=2&force=nah"),
        ("GET", "/admin/modpacks?offset=-5&limit=999"),
    ]
    out = []
    for i, (method, url) in enumerate(reqs):
        DB.ops = []
        res = await call(app, method, url, rid=f"parity-mm-p{i}")
        out.append({"method": method, "url": url, **res})
    bodies = [
        (f"/admin/mods/projects/{P_PUB}/takedown", b'{"reason": ' + json.dumps("r" * 2001).encode() + b"}"),
        (f"/admin/mods/projects/{P_PUB}/takedown", b""),
        (f"/admin/mods/projects/{P_PUB}/takedown", b"[]"),
        (f"/admin/mods/projects/{P_PUB}/takedown", b'{"reason": 5}'),
        ("/admin/mods/stray/assign", b"{}"),
        ("/admin/mods/stray/assign", b'{"user_id": "", "project_ids": []}'),
        ("/admin/mods/stray/assign", b'{"user_id": 5, "project_ids": "abc"}'),
        ("/admin/mods/stray/assign", b'{"user_id": "u", "project_ids": [1, "a", null]}'),
    ]
    for i, (url, body) in enumerate(bodies):
        res = await call(app, "POST", url, body, "application/json", rid=f"parity-mm-b{i}")
        out.append({"method": "POST", "url": url, "sent": body.decode(), **res})
    return out


async def main() -> None:
    async def no_server(self):
        self._database_major_version = 7
        self._existing_collections = []

    beanie_init.Initializer._load_cached_info = no_server
    client = AsyncMongoClient("mongodb://127.0.0.1:1/", tz_aware=True, connect=False)
    await init_beanie(database=client["fixtures"], document_models=MODELS, skip_indexes=True)
    for cls in MODELS:
        cls.get_settings().pymongo_collection = FakeColl(DB, cls.get_settings().name)
    moderation.queue_email = fake_queue_email
    gitstore.delete_repo = fake_delete_repo
    store.get_blob = fake_get_blob
    store.put_blob = fake_put_blob

    from app.admin.runtime_config import REGISTRY, _CACHE
    for key, spec in REGISTRY.items():
        _CACHE[key] = (spec.default, float("inf"))
    from app.main import app
    app.dependency_overrides[get_current_superuser] = admin_user
    data = {"python": sys.version.split()[0], "now": NOW.isoformat(), "admin_id": ADMIN_ID}
    data["params"] = await params_section(app)
    data["scenario"] = await scenario_section(app)
    app.dependency_overrides.clear()
    data["unauth"] = await unauth_section(app)
    data["models"] = model_cases()
    data["emails"] = await email_cases()
    data["slugs"] = slug_cases()
    data["tags"] = tag_cases()
    data["stray_helpers"] = stray_helpers()
    data["stray"] = await stray_section()
    data["runs"] = await run_cases()
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(data, ensure_ascii=True, indent=1, default=str) + "\n", encoding="utf-8")
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes) with Python {data['python']}")


if __name__ == "__main__":
    asyncio.run(main())
