"""Golden fixtures for the Mods Hub catalog (app/trove/mods_hub), from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_mods_catalog_fixtures.py
Writes native/kiwi-server/testdata/mods_catalog/golden.json.

The real service helpers and DTO builders over fixed documents, Beanie's
get_dict for the shapes written to Mongo, dulwich's repo layout, and pydantic's
errors for the request bodies (FastAPI's 422 details).
"""
import asyncio
import base64
import datetime as dt
import io
import json
import logging
import os
import re
import struct
import sys
import tempfile
import warnings
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
sys.path.insert(0, os.getcwd())

ENV = {
    "SECRET_KEY": "mods-catalog-fixture-secret",
    "API_URL": "https://api.example.test/",
    "APP_URL": "https://trove.example.test//",
    "SITE_MASTER_HANDLE": "aallyn",
}
os.environ.update(ENV)

from bson import ObjectId, decode, encode  # noqa: E402
from bson.codec_options import CodecOptions  # noqa: E402
from bson.json_util import CANONICAL_JSON_OPTIONS, dumps as bson_dumps  # noqa: E402
from pydantic import ValidationError  # noqa: E402
from pymongo.asynchronous.database import AsyncDatabase  # noqa: E402

from app.site_auth.models import SiteUser  # noqa: E402
from app.trove.mods_hub import gitstore, schemas, store  # noqa: E402
from app.trove.mods_hub import service as svc  # noqa: E402
from app.trove.mods_hub.models import (  # noqa: E402
    ModClaimRequest, ModImageAsset, ModProfile, ModProject, ModRelease, ModStar, RemixClaim,
)
from beanie.odm.utils.dump import get_dict  # noqa: E402

OUT = Path(__file__).resolve().parents[2] / "kiwi-server" / "testdata" / "mods_catalog" / "golden.json"
UTC = dt.timezone.utc
NOW = dt.datetime(2026, 10, 22, 11, 0, 0, 123456, tzinfo=UTC)
T0 = dt.datetime(2026, 9, 1, 12, 0, 0, 250000, tzinfo=UTC)
T1 = dt.datetime(2026, 10, 1, 8, 30, 0, tzinfo=UTC)


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
        ModProject, ModRelease, ModProfile, ModImageAsset, ModClaimRequest, RemixClaim, ModStar, SiteUser])


asyncio.run(_init())
svc.utcnow = lambda: NOW


def ext(v):
    return json.loads(bson_dumps(v, json_options=CANONICAL_JSON_OPTIONS))


def db(model):
    return ext(get_dict(model, to_db=True, keep_nulls=True))


def jsonable(v):
    return json.loads(json.dumps(v, default=str))


# --- pure helpers --------------------------------------------------------------------

TITLES = ["My Cool Mod", "  --Hello, World!--  ", "", "!!!", "Ünïcödé Mod ✓", "a" * 59 + " b c",
          "x" * 70, "İstanbul Ships", "Mod_v2.0 (beta)", "ДРАКОН", "-" * 5 + "z"]
TAGS = [["Dragons", " dragons ", "GUI"], ["  Long   tag\twith   spaces  " * 2], ["", "   ", "a"],
        [f"t{i}" for i in range(20)], ["MiXeD", "mixed", "Ünï", "x\u00a0\u2003y"]]
FILENAMES = ["My Mod", "  a<b>c:d\"e/f\\g|h?i*j  ", "...dots...", "", "\x01\x1fctl", "é" * 130, " . ", "a.b.tmod"]
ESCAPES = ["plain", "a.b*c", "  spaced  ", "(x)[y]{z}?+-|^$\\&~#", "tab\there\nnl", "ü-ö"]
IDENTS = ["Modder", "", "Mr. Robot!", "名前", "a b<c>", "x-y_z.w"]
URLS = [("", "link"), ("  https://x.io/a  ", "website"), ("HTTP://X.IO", "donation"), ("ftp://x", "Discord"),
        ("https://", "website"), ("http://a b", "website"), ("https://" + "a" * 293, "website"),
        ("https://" + "a" * 292, "website"), ("http://x\u3000y", "Discord invite"), ("http://ok\x1cno", "donation")]
I18N = [
    ([("fr", "Bonjour"), ("de", "  ")], "title", 120),
    ([("en", "Hello")], "summary", 280),
    ([("xx", "nope")], "readme_text", 60000),
    ([(" de ", "Hallo"), ("de", "Zweite")], "description", 40000),
    ([("ja", "あ" * 121)], "title", 120),
    ([("ko", "x" * 40001)], "description", 40000),
    ([("pt-PT", " ola ")], "tagline", 160),
    ([("zh-CN", "y" * 4001)], "warnings", 4000),
    ([("en", "x")], "readme", 40000),
]


def i18n_cases():
    out = []
    for pairs, base, mx in I18N:
        try:
            res = {"ok": list(svc._clean_i18n_map(dict(pairs), base=base, max_len=mx).items())}
        except svc.APIError as e:
            res = {"error": e.message}
        out.append({"map": pairs, "base": base, "max": mx, **res})
    return out


def url_cases():
    out = []
    for u, field in URLS:
        try:
            res = {"ok": svc._clean_url(u, field=field)}
        except svc.APIError as e:
            res = {"error": e.message}
        out.append({"in": u, "field": field, **res})
    return out


def png(w, h):
    return b"\x89PNG\r\n\x1a\n" + struct.pack(">I", 13) + b"IHDR" + struct.pack(">II", w, h) + b"\x08\x06\x00\x00\x00"


def sniff_cases():
    jpeg = (b"\xff\xd8\xff\xe0" + struct.pack(">H", 16) + b"JFIF\x00" + b"\x00" * 9
            + b"\xff\xc0" + struct.pack(">HBHH", 17, 8, 480, 640) + b"\x03" + b"\x00" * 12)
    vp8 = b"RIFF" + struct.pack("<I", 100) + b"WEBPVP8 " + b"\x00" * 10 + struct.pack("<HH", 320 | 0xC000, 200)
    vp8l = b"RIFF" + struct.pack("<I", 100) + b"WEBPVP8L" + b"\x00" * 5 + struct.pack("<I", (99) | (49 << 14)) + b"\x00" * 4
    vp8x = b"RIFF" + struct.pack("<I", 100) + b"WEBPVP8X" + b"\x00" * 8 + bytes([0x3f, 1, 0, 0x1f, 2, 0]) + b"\x00" * 4
    blobs = [png(1, 2), png(70000, 3), b"GIF89a" + struct.pack("<HH", 10, 20) + b"\x00" * 20, jpeg, vp8, vp8l, vp8x,
             b"RIFF" + b"\x00" * 4 + b"WEBPZZZZ" + b"\x00" * 16, b"\xff\xd8" + b"\x00" * 30, b"\x89PNG" + b"\x00" * 5,
             b"not an image at all, no....", b"RIFF\x00\x00\x00\x00WEBPVP8 " + b"\x00" * 6]
    out = []
    for b in blobs:
        got = store.sniff_image(b)
        out.append({"b64": base64.b64encode(b).decode(), "out": list(got) if got else None})
    return out


def facets_cases():
    rows_sets = [
        [{"_id": "dragons", "n": 3}, {"_id": "gui", "n": 1}, {"_id": "zeta", "n": 2}, {"_id": "Alpha", "n": 2},
         {"_id": "alpha", "n": 1}, {"_id": " Dragons ", "n": 2}, {"_id": "", "n": 9}, {"_id": "beta", "n": 2}],
        [],
        [{"_id": "Radar", "n": 1}, {"_id": "allies", "n": 4}, {"_id": "ü", "n": 1}, {"_id": "U", "n": 1}],
    ]
    out = []
    for rows in rows_sets:
        class Agg:
            def __init__(self, r):
                self.r = r

            async def to_list(self):
                return self.r
        orig = ModProject.aggregate
        ModProject.aggregate = classmethod(lambda cls, *a, **k: Agg(rows))
        try:
            res = asyncio.run(svc.tag_facets())
        finally:
            ModProject.aggregate = orig
        out.append({"rows": rows, "out": res})
    return out


# --- documents ------------------------------------------------------------------------

OWNER = ObjectId("65f0000000000000000000a1")
OTHER = ObjectId("65f0000000000000000000a9")


def project_docs():
    base = dict(
        _id=ObjectId("66a000000000000000000001"), slug="dragon-pack", title="Dragon Pack", summary="Dragons!",
        description="# Long\nreadme", readme_text="", warnings="Careful<br>Really",
        title_i18n={"fr": "Pack de dragons"}, summary_i18n={}, description_i18n={"de": "Lang"},
        readme_i18n={}, warnings_i18n={}, tags=["dragons", "gui", "custom tag"], owner_id=OWNER,
        owner_username="Parity Linked", owner_handle="paritylinked",
        collaborators=[{"user_id": OTHER, "username": "helper"}], is_stray=False, stray_status=None, author="",
        source=None, source_id=None, source_url=None, source_author_id=None, source_file_id=None, source_likes=0,
        uploaded_on_behalf=False, issues_enabled=True, issue_seq=3, open_issue_count=1, visibility="public",
        is_beta=True, mode="files", source_visibility="public", default_branch="main", banner_sha="b" * 64,
        preview_shas=["c" * 64, "d" * 64], discord_url="https://discord.gg/x", website_url=None,
        donation_urls=["https://ko-fi.com/x"], download_count=42, star_count=7, downloads_7d=5,
        popularity_score=0.8123, hidden_release_branches=["alt"], branch_order=["main", "alt"],
        forked_from_id=None, forked_from_slug=None, forked_from_handle=None, forked_from_title=None,
        forked_from_owner=None, inspired_by_slug="orig", inspired_by_handle="someone", inspired_by_title="Original",
        inspired_by_owner="Someone", fork_count=2, is_remix=False, remix_status="pending", remix_requested_at=None,
        remix_resolved_at=None, remix_resolved_by=None, taken_down=False, takedown_reason=None, created_at=T0,
        updated_at=T1, last_release_at=T1,
    )
    website_only = {**base, "_id": ObjectId("66a000000000000000000002"), "visibility": "public_website_only",
                    "preview_shas": [], "banner_sha": None, "inspired_by_slug": None, "last_release_at": None,
                    "popularity_score": 1.0, "author": "Real Author", "uploaded_on_behalf": True}
    remix = {**base, "_id": ObjectId("66a000000000000000000003"), "is_remix": True, "remix_status": "pending",
             "remix_requested_at": NOW - dt.timedelta(days=8), "forked_from_id": base["_id"],
             "forked_from_slug": "dragon-pack", "forked_from_handle": "paritylinked",
             "forked_from_title": "Dragon Pack", "forked_from_owner": "Parity Linked", "popularity_score": 0.0}
    fresh_remix = {**remix, "_id": ObjectId("66a000000000000000000004"),
                   "remix_requested_at": NOW - dt.timedelta(days=6, hours=23)}
    denied = {**remix, "_id": ObjectId("66a000000000000000000005"), "remix_status": "denied"}
    approved_draft = {**remix, "_id": ObjectId("66a000000000000000000006"), "remix_status": "approved",
                      "visibility": "draft", "remix_requested_at": None}
    stray = {**base, "_id": ObjectId("66a000000000000000000007"), "owner_id": None, "owner_handle": "stray",
             "owner_username": "", "is_stray": True, "stray_status": "approved", "author": "Old Author",
             "collaborators": [], "tags": [], "popularity_score": 0.5}
    sparse = {"_id": ObjectId("66a000000000000000000008"), "slug": "old", "title": "Old Mod",
              "owner_id": OWNER, "visibility": "public", "download_count": 3.0, "popularity_score": 2,
              "created_at": T0, "updated_at": T0}
    return [base, website_only, remix, fresh_remix, denied, approved_draft, stray, sparse]


def release_docs(pid):
    r1 = dict(
        _id=ObjectId("66b000000000000000000001"), project_id=pid, owner_id=OWNER, tag="v1.0", branch="main",
        title="First", changelog="- init", title_i18n={"fr": "Premier"}, changelog_i18n={},
        source_commit_sha="e" * 40, release_format="tmod", tmod_sha="f" * 64, prior_tmod_shas=["9" * 64],
        tmod_size=1234, tmod_filename="dragon-pack-v1.0.tmod",
        tmod_properties={"title": "  Dragon: Pack?  ", "author": "Parity", "modVersion": "1.0"},
        banner_sha="b" * 64, download_count=10, status="published", created_at=T0, updated_at=T0,
        published_at=T0, announced_at=None, silent=False,
    )
    r2 = {**r1, "_id": ObjectId("66b000000000000000000002"), "tag": "v1.1", "tmod_size": 2000,
          "tmod_properties": {"modVersion": ""}, "banner_sha": None, "published_at": T1, "created_at": T1,
          "tmod_sha": "f" * 64, "source_commit_sha": None}
    z = {**r1, "_id": ObjectId("66b000000000000000000003"), "release_format": "zip", "tmod_filename": "x-v2.zip",
         "status": "draft", "published_at": None, "tmod_properties": {}}
    return [r1, r2, z]


def users():
    return [
        dict(_id=OWNER, username="paritylinked", discord_handle="paritylinked", notify_email=None,
             display_name=None, is_active=True, is_verified=True, is_deleted=False,
             discord_id=312345678901234567, discord_avatar=None, token_version=0, created_at=T0, updated_at=T0),
        dict(_id=OTHER, username="helper", discord_handle="helper", display_name="  Helpful Person  ",
             is_active=True, discord_id=312345678901234999, discord_avatar="a_deadbeef", created_at=T1,
             updated_at=T1),
        dict(_id=ObjectId("65f0000000000000000000b0"), username="nodiscord", display_name="",
             is_active=True, discord_id=None, discord_avatar="abc", created_at=T0, updated_at=T0),
        dict(_id=ObjectId("65f0000000000000000000b1"), username="pngavatar", display_name=None,
             is_active=True, discord_id=1, discord_avatar="abc123", created_at=T0, updated_at=T0),
    ]


def from_mongo(d):
    return decode(encode(d), codec_options=CodecOptions(tz_aware=True, tzinfo=UTC))


def model(cls, d):
    return cls.model_validate(from_mongo(d))


def reread(m):
    return type(m).model_validate(from_mongo(get_dict(m, to_db=True, keep_nulls=True)))


def projects_section():
    out = []
    for d in project_docs():
        p = model(ModProject, d)
        rels = [model(ModRelease, r) for r in release_docs(p.id)[:2]]
        out.append({
            "doc": ext(d),
            "card": svc.project_card(p),
            "card_reveal": svc.project_card(p, reveal=True),
            "public": svc.public_mod_dto(p),
            "public_releases": svc.public_mod_dto(p, releases=rels),
            "remix": svc._remix_card(p),
            "effective": svc.effective_visibility(p),
            "event_project": svc._event_project(p),
            "to_db": db(p),
        })
    return out


def releases_section():
    out = []
    pid = ObjectId("66a000000000000000000001")
    for d in release_docs(pid):
        r = model(ModRelease, d)
        out.append({"doc": ext(d), "dto": svc._release_dto(r), "public": svc.public_release_dto(r),
                    "event": svc._event_release(r), "to_db": db(r)})
    rs = [model(ModRelease, d) for d in release_docs(pid)]
    change = [
        svc._event_change(rs[1], rs[0], 2),
        svc._event_change(rs[0], None, 1),
        svc._event_change(rs[1], rs[1], 5),
        svc._event_change(rs[2], rs[0], 3),
    ]
    return out, change


def profiles_section():
    us = [model(SiteUser, u) for u in users()]
    projects = [model(ModProject, d) for d in project_docs()]
    prof = model(ModProfile, dict(
        _id=ObjectId("66c000000000000000000001"), site_user_id=OWNER, handle="paritylinked",
        display_name="  Parity  ", tagline="Makes mods", readme="About", tagline_i18n={"fr": "Fait des mods"},
        readme_i18n={}, avatar_sha="a" * 64, banner_sha="b" * 64, discord_url=None, website_url="https://x.io",
        donation_urls=[], mod_order=["old", "missing", "dragon-pack", "old"], featured_slug="dragon-pack",
        taken_down=True, takedown_reason="Spam", created_at=T0, updated_at=T1))
    bare = model(ModProfile, dict(_id=ObjectId("66c000000000000000000002"), site_user_id=OTHER, handle="helper",
                                  featured_slug="nope", created_at=T0, updated_at=T0))
    cases = [(us[0], prof, True, [projects[0], projects[7]]), (us[0], prof, False, [projects[0]]),
             (us[1], bare, False, [projects[1], projects[0]]), (us[1], None, False, []), (us[2], None, True, [])]
    out = []
    for user, profile, owner, mods in cases:
        out.append({"user": ext(get_dict(user, to_db=True)), "profile": db(profile) if profile else None,
                    "is_owner": owner, "mods": [ext(get_dict(m, to_db=True)) for m in mods],
                    "out": svc.profile_dto(user, profile, owner, mods)})
    avatars = []
    for u in us:
        for sha in (None, "a" * 64):
            for small in (False, True):
                p = ModProfile(site_user_id=u.id, avatar_sha=sha) if sha else None
                avatars.append({"user": ext(get_dict(u, to_db=True)), "avatar_sha": sha, "small": small,
                                "out": svc._profile_avatar_url(u, p, small=small)})
    return out, avatars, db(prof)


def claims_section():
    c = ModClaimRequest(id=ObjectId("66d000000000000000000001"), project_id=ObjectId("66a000000000000000000007"),
                        project_slug="dragon-pack", project_title="Dragon Pack", claimant_id=OWNER,
                        claimant_username="paritylinked", message="it's me", created_at=NOW)
    r = RemixClaim(id=ObjectId("66d000000000000000000002"), target_id=ObjectId("66a000000000000000000003"),
                   target_slug="t", target_title="T", original_id=ObjectId("66a000000000000000000001"),
                   original_slug="o", original_title="O", claimant_id=OWNER, claimant_username="paritylinked",
                   message="", created_at=NOW)
    r2 = r.model_copy(update={"resolved_at": T1, "status": "approved"})
    return [{"dto": svc._claim_dto(reread(c)), "fresh": svc._claim_dto(c), "to_db": db(c)}], [
        {"dto": svc._remix_claim_dto(reread(x)), "fresh": svc._remix_claim_dto(x), "to_db": db(x)} for x in (r, r2)]


def new_docs():
    """The documents create_project / fork_project / store_image / star insert."""
    actor = model(SiteUser, users()[1])
    p = ModProject(slug="my-cool-mod", title="My Cool Mod", summary="s", description="d", tags=["x"],
                   visibility="draft", is_beta=False, mode="files", source_visibility="private",
                   owner_id=actor.id, owner_username=actor.display_name or actor.username,
                   owner_handle=actor.username, uploaded_on_behalf=False, author="",
                   inspired_by_slug="o", inspired_by_handle="h", inspired_by_title="T", inspired_by_owner="O")
    p.id = ObjectId("66e000000000000000000001")
    p.created_at = p.updated_at = NOW
    orig = model(ModProject, project_docs()[0])
    f = ModProject(slug="dragon-pack-2", title=orig.title, summary=orig.summary, description=orig.description,
                   tags=list(orig.tags), visibility="draft", mode=orig.mode, owner_id=actor.id,
                   owner_username="Helpful Person", owner_handle="helper", forked_from_id=orig.id,
                   forked_from_slug=orig.slug, forked_from_handle=orig.owner_handle, forked_from_title=orig.title,
                   forked_from_owner=orig.owner_username, is_remix=True, remix_status="pending")
    f.id = ObjectId("66e000000000000000000002")
    f.created_at = f.updated_at = NOW
    img = ModImageAsset(sha="a" * 64, content_type="image/png", byte_size=123, owner_id=actor.id,
                        width=70000, height=None)
    img.id = ObjectId("66e000000000000000000003")
    img.created_at = NOW
    star = ModStar(project_id=orig.id, site_user_id=actor.id)
    star.id = ObjectId("66e000000000000000000004")
    star.created_at = NOW
    prof = ModProfile(site_user_id=actor.id, handle=actor.username)
    prof.id = ObjectId("66e000000000000000000005")
    prof.created_at = prof.updated_at = NOW
    return {"project": db(p), "project_card": svc.project_card(p), "fork": db(f), "image": db(img),
            "star": db(star), "profile": db(prof)}


BODIES = [
    ("CreateProject", schemas.CreateProjectRequest, {}),
    ("CreateProject", schemas.CreateProjectRequest, {"title": ""}),
    ("CreateProject", schemas.CreateProjectRequest,
     {"title": "x" * 121, "summary": 5, "tags": [f"t{i}" for i in range(13)], "visibility": "secret",
      "is_beta": "maybe", "mode": "zip", "source_visibility": None, "inspired_by": "y" * 81, "on_behalf": 1,
      "credited_author": ["a"]}),
    ("CreateProject", schemas.CreateProjectRequest, {"title": "ok", "tags": ["a", 3], "visibility": "public"}),
    ("CreateProject", schemas.CreateProjectRequest, "not an object"),
    ("UpdateProject", schemas.UpdateProjectRequest,
     {"title": "", "title_i18n": {"fr": 3}, "summary_i18n": "x", "tags": "a", "visibility": None,
      "hidden_release_branches": ["b"] * 101, "donation_urls": ["u"] * 6, "discord_url": "d" * 301,
      "issues_enabled": "yes", "readme_text": "r" * 60001, "warnings": "w" * 4001}),
    ("UpdateProject", schemas.UpdateProjectRequest, {"title": None, "mode": "files", "branch_order": [1]}),
    ("UpdateProfile", schemas.UpdateProfileRequest,
     {"display_name": "d" * 81, "tagline": "t" * 161, "mod_order": ["s"] * 501, "featured_slug": "f" * 81,
      "readme_i18n": {"de": None}}),
    ("Lookup", schemas.HashLookupRequest, {}),
    ("Lookup", schemas.HashLookupRequest, {"hashes": []}),
    ("Lookup", schemas.HashLookupRequest, {"hashes": ["a"] * 201, "releases": "some", "include_releases": "x"}),
    ("Lookup", schemas.HashLookupRequest, {"hashes": ["a", 1, None]}),
    ("RemixStatus", schemas.RemixStatusRequest, {"status": "maybe"}),
    ("RemixClaim", schemas.RemixClaimRequest, {"target_handle": "", "target_slug": "s" * 121, "message": 1}),
    ("Collaborator", schemas.CollaboratorRequest, {"username": ""}),
    ("Claim", schemas.ClaimRequest, {"message": "m" * 2001}),
]


def bodies():
    out = []
    for name, cls, body in BODIES:
        try:
            cls.model_validate(body, from_attributes=True)
            errors = None
        except ValidationError as e:
            errors = [{**err, "loc": ["body", *err["loc"]]} for err in e.errors(include_url=False)]
            errors = jsonable(errors)
        out.append({"model": name, "body": body, "errors": errors})
    return out


def lookup_modes():
    out = []
    for body in ({"hashes": ["a"]}, {"hashes": ["a"], "include_releases": True},
                 {"hashes": ["a"], "releases": "all", "include_releases": True},
                 {"hashes": ["a"], "releases": "none", "include_releases": True}):
        out.append({"body": body, "mode": schemas.HashLookupRequest.model_validate(body).release_mode})
    return out


def repo_layout():
    with tempfile.TemporaryDirectory() as tmp:
        d = Path(tmp) / "x.git"
        gitstore.settings.mods_store_dir = tmp
        orig = gitstore._root
        gitstore._root = lambda: Path(tmp)
        try:
            gitstore._ensure("x")
        finally:
            gitstore._root = orig
        files = {}
        dirs = []
        for root, ds, fs in os.walk(d):
            for name in ds:
                dirs.append(os.path.relpath(os.path.join(root, name), d).replace(os.sep, "/"))
            for name in fs:
                p = os.path.join(root, name)
                files[os.path.relpath(p, d).replace(os.sep, "/")] = open(p, "rb").read().decode()
        return {"dirs": sorted(dirs), "files": files}


def fork_commit():
    from dulwich.objects import Commit
    c = Commit()
    c.tree = b"4b825dc642cb6eb9a060e54bf8d69288fbee4904"
    c.author = c.committer = gitstore.author_ident("Helpful Person")
    c.author_time = c.commit_time = 1792000000
    c.author_timezone = c.commit_timezone = 0
    c.encoding = b"UTF-8"
    c.message = b"Fork of dragon-pack"
    return {"raw": c.as_raw_string().decode(), "sha": c.id.decode()}


FORK_FILES = [("ui/main.swf", 0o100644, b"\x00swf\x01"), ("ui/a b/\u00fc.cfg", 0o100755, b"x=1\n"),
              ("readme.md", 0o100644, b"# hi\n"), ("ui/z", 0o100644, b""), ("ui-x/y.txt", 0o100644, b"y")]


def fork_repo():
    """gitstore._fork over a source repo holding FORK_FILES."""
    from dulwich.index import commit_tree
    from dulwich.objects import Blob, Commit
    with tempfile.TemporaryDirectory() as tmp:
        orig = gitstore._root
        gitstore._root = lambda: Path(tmp)
        try:
            src = gitstore._ensure("src")
            entries = []
            for path, mode, data in FORK_FILES:
                b = Blob.from_string(data)
                src.object_store.add_object(b)
                entries.append((path.encode(), b.id, mode))
            c = Commit()
            c.tree = commit_tree(src.object_store, entries)
            c.author = c.committer = b"src <src@x>"
            c.author_time = c.commit_time = 1
            c.author_timezone = c.commit_timezone = 0
            c.message = b"init"
            src.object_store.add_object(c)
            src.refs[b"refs/heads/main"] = c.id
            sha = gitstore._fork("src", "dst", "main", "main", gitstore.author_ident("Helpful Person"),
                                 "Fork of dragon-pack", 1792000000)
            ref = (Path(tmp) / "dst.git" / "refs" / "heads" / "main").read_text()
        finally:
            gitstore._root = orig
    return {"files": [[p, m, base64.b64encode(d).decode()] for p, m, d in FORK_FILES], "sha": sha, "ref": ref}


def main():
    releases, change = releases_section()
    profiles, avatars, profile_db = profiles_section()
    claims, remix_claims = claims_section()
    golden = {
        "env": ENV,
        "now": NOW.isoformat(),
        "slugify": [[t, svc._slugify(t)] for t in TITLES],
        "clean_tags": [[t, svc._clean_tags(t)] for t in TAGS],
        "safe_filename": [[n, svc._safe_filename(n)] for n in FILENAMES],
        "re_escape": [[s, re.escape(s)] for s in ESCAPES],
        "author_ident": [[n, gitstore.author_ident(n).decode()] for n in IDENTS],
        "i18n": i18n_cases(),
        "clean_url": url_cases(),
        "round4": [[x, round(x, 4)] for x in (0.123456, 0.00005, 0.99995, 1 / 3, 2 / 3, 0.0, 1.0, 0.12345)],
        "sniff": sniff_cases(),
        "facets": facets_cases(),
        "projects": projects_section(),
        "releases": releases,
        "change": change,
        "profiles": profiles,
        "profile_db": profile_db,
        "avatars": avatars,
        "claims": claims,
        "remix_claims": remix_claims,
        "new_docs": new_docs(),
        "bodies": bodies(),
        "lookup_modes": lookup_modes(),
        "repo": repo_layout(),
        "fork_commit": fork_commit(),
        "fork_repo": fork_repo(),
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(golden, indent=1, ensure_ascii=False, default=str) + "\n", encoding="utf-8")
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
