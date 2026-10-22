"""Golden fixtures for the leaf routes (health/config/docs, supporters, design
feedback, moderation), from the repo root:
    .venv/Scripts/python.exe native/port/parity/gen_leaf_fixtures.py
Writes native/kiwi-server/testdata/leaf/golden.json.

Everything runs against the real `app.main.app` (raw ASGI) or the real service
functions; stores, auth and rate limits are swapped for in-memory fakes only
where a case would otherwise need Mongo or Redis.
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

ENV = {
    "APP_NAME": 'Kiwi <API> "é" {x}',
    "API_URL": "https://api.example.test",
    "DOCS_URL": "https://docs.example.test/?a=1&b=<2>",
    "DEV_URL": "https://dev.example.test",
    "CAPTCHA_PROVIDER": "hcaptcha",
    "CAPTCHA_SITEKEY": "10000000-ffff-ffff-ffff-000000000001",
    "REQUIRE_VERIFIED_FOR_TOKENS": "false",
    "TOKEN_CREATION_DAILY_LIMIT": "7",
    "GITHUB_CLIENT_ID": "gh-id",
    "GITHUB_CLIENT_SECRET": "gh-secret",
    "DISCORD_CLIENT_ID": "123",
    "SECRET_KEY": "leaf-fixture-secret",
}
os.environ.update(ENV)

from bson import ObjectId  # noqa: E402
from bson.tz_util import utc as bson_utc  # noqa: E402
from pydantic import TypeAdapter  # noqa: E402

from app.admin import runtime_config as rc  # noqa: E402

for _key, _spec in rc.REGISTRY.items():
    rc._CACHE[_key] = (_spec.default, float("inf"))

import app.core.dependencies as deps  # noqa: E402
import app.design_feedback.router as df_router  # noqa: E402
import app.design_feedback.service as df_service  # noqa: E402
import app.supporters.service as sup_service  # noqa: E402
import app.trove.moderation as moderation  # noqa: E402
from app.core.ratelimit import RateLimitInfo  # noqa: E402
from app.main import app  # noqa: E402
from app.supporters.models import Supporter  # noqa: E402
from app.trove.mods_hub.models import ContentReport  # noqa: E402

REAL_ADD, REAL_REPLACE = sup_service.add, sup_service.replace
OUT = Path(__file__).resolve().parents[2] / "kiwi-server" / "testdata" / "leaf" / "golden.json"
DICT = TypeAdapter(dict)
J = {"content-type": "application/json"}
KEPT_HEADERS = ("content-type", "allow", "www-authenticate", "cache-control",
                "x-ratelimit-limit", "x-ratelimit-remaining", "x-ratelimit-reset", "content-length")


def jb(v) -> bytes:
    return json.dumps(v).encode()


async def call(method: str, url: str, headers: list[tuple[bytes, bytes]], body: bytes):
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


def run(cases, prefix):
    out = []
    for i, (name, method, url, headers, body) in enumerate(cases):
        hdrs = [(k.encode(), v.encode("latin-1")) for k, v in headers.items()]
        hdrs.append((b"x-request-id", f"req_{prefix}_{i:03d}".encode()))
        status, rh, raw = asyncio.run(call(method, url, hdrs, body or b""))
        out.append({
            "name": name, "method": method, "url": url,
            "headers": [[k.decode(), v.decode("latin-1")] for k, v in hdrs],
            "body": (body or b"").decode("latin-1"),
            "status": status,
            "resp_headers": {k: v for k, v in rh if k in KEPT_HEADERS},
            "resp": raw.decode("utf-8"),
        })
    return out


BAD_TOKEN = {"authorization": "Bearer not.a.jwt"}

# Store-free: answered before any Mongo/Redis access.
HTTP = [
    ("health", "GET", "/health", {}, None),
    ("config", "GET", "/config", {}, None),
    ("config_query_ignored", "GET", "/config?x=1", {}, None),
    ("api_info", "GET", "/api-info", {}, None),
    ("docs", "GET", "/docs", {}, None),
    ("redoc", "GET", "/redoc", {}, None),
    ("oauth2_redirect", "GET", "/docs/oauth2-redirect?code=x", {}, None),
    ("health_405", "POST", "/health", {}, None),
    ("config_405", "DELETE", "/config", {}, None),
    ("api_info_405", "PUT", "/api-info", {}, None),
    ("supporters_public_405", "POST", "/v1/misc/supporters", {}, None),
    ("admin_supporters_405", "PATCH", "/admin/supporters", {}, None),
    ("admin_supporters_405_delete", "DELETE", "/admin/supporters", {}, None),
    ("admin_supporter_405", "GET", "/admin/supporters/nz", {}, None),
    ("admin_df_405", "POST", "/admin/design-feedback", {}, None),
    ("df_405", "GET", "/site/design-feedback", {}, None),
    ("report_405", "GET", "/v1/moderation/report", {}, None),
    ("admin_list_noauth", "GET", "/admin/supporters", {}, None),
    ("admin_list_badtoken", "GET", "/admin/supporters", BAD_TOKEN, None),
    ("admin_list_basic", "GET", "/admin/supporters", {"authorization": "Basic abc"}, None),
    ("admin_add_noauth", "POST", "/admin/supporters", J, jb({"name": "x"})),
    ("admin_add_noauth_invalid", "POST", "/admin/supporters", J, jb({"nope": 1})),
    ("admin_add_badjson_first", "POST", "/admin/supporters", J, b'{"name": '),
    ("admin_replace_badjson_first", "PUT", "/admin/supporters", J, b"[1 2]"),
    ("admin_replace_noauth", "PUT", "/admin/supporters", J, jb({"names": 5})),
    ("admin_delete_noauth", "DELETE", "/admin/supporters/nz", {}, None),
    ("admin_delete_badtoken", "DELETE", "/admin/supporters/n%20z", BAD_TOKEN, None),
    ("admin_delete_slash", "DELETE", "/admin/supporters/a%2Fb", {}, None),
    ("admin_df_noauth_bad_days", "GET", "/admin/design-feedback?days=0", {}, None),
    ("admin_df_badtoken", "GET", "/admin/design-feedback", BAD_TOKEN, None),
    ("df_badjson_first", "POST", "/site/design-feedback", J, b'{"design" "voxel"}'),
    ("df_bad_utf8_first", "POST", "/site/design-feedback", J, b'{"design": "\xff"}'),
    ("report_missing_body", "POST", "/v1/moderation/report", {}, None),
    ("report_null", "POST", "/v1/moderation/report", J, b"null"),
    ("report_list", "POST", "/v1/moderation/report", J, b"[]"),
    ("report_empty", "POST", "/v1/moderation/report", J, b"{}"),
    ("report_types", "POST", "/v1/moderation/report", J,
     jb({"target_type": "user", "handle": 5, "slug": None, "reason": ["x"]})),
    ("report_lengths", "POST", "/v1/moderation/report", J,
     jb({"target_type": "mod", "handle": "", "slug": "s" * 141, "reason": "ab"})),
    ("report_lengths_max", "POST", "/v1/moderation/report", J,
     jb({"target_type": "profile", "handle": "h" * 65, "reason": "r" * 2001})),
    ("report_unicode_len", "POST", "/v1/moderation/report", J,
     jb({"target_type": "modpack", "handle": "é" * 64 + "\U0001F600", "reason": "\U0001F600\U0001F600"})),
    ("report_badjson", "POST", "/v1/moderation/report", J, b'{"target_type": mod}'),
    ("report_text_ct", "POST", "/v1/moderation/report", {"content-type": "text/plain"}, b"{}"),
]


class FakeUser:
    id = ObjectId("65a1b2c3d4e5f60718293a4b")
    is_superuser = True


async def _no_limit(*_a, **_k):
    return RateLimitInfo(limit=30, remaining=29, reset=dt.datetime(2026, 1, 2, 3, 4, 5, tzinfo=dt.timezone.utc))


def validation_section():
    df_router.check_rate_limit = _no_limit
    recorded = []

    async def record_vote(design, theme, vote):
        recorded.append([design, theme, vote])

    df_service.record_vote = record_vote
    app.dependency_overrides[deps.get_current_superuser] = lambda: FakeUser()
    cases = [
        ("vote_missing", "POST", "/site/design-feedback", {}, None),
        ("vote_null", "POST", "/site/design-feedback", J, b"null"),
        ("vote_empty", "POST", "/site/design-feedback", J, b"{}"),
        ("vote_bad", "POST", "/site/design-feedback", J,
         jb({"design": "Voxel", "theme": "dim", "vote": 1})),
        ("vote_theme_null", "POST", "/site/design-feedback", J,
         jb({"design": "classic", "theme": None, "vote": "up"})),
        ("vote_ok", "POST", "/site/design-feedback", J, jb({"design": "voxel", "vote": "down", "extra": 1})),
        ("vote_ok_theme", "POST", "/site/design-feedback", J, jb({"design": "classic", "theme": "dark", "vote": "up"})),
        ("vote_no_ct", "POST", "/site/design-feedback", {}, jb({"design": "voxel", "vote": "up"})),
        ("add_missing", "POST", "/admin/supporters", {}, None),
        ("add_types", "POST", "/admin/supporters", J, jb({"name": 5})),
        ("add_list", "POST", "/admin/supporters", J, jb(["a"])),
        ("add_empty", "POST", "/admin/supporters", J, jb({})),
        ("replace_types", "PUT", "/admin/supporters", J, jb({"names": ["a", 2, None, "b"]})),
        ("replace_not_list", "PUT", "/admin/supporters", J, jb({"names": "abc"})),
        ("replace_missing", "PUT", "/admin/supporters", J, jb({"name": []})),
        ("days_low", "GET", "/admin/design-feedback?days=0", {}, None),
        ("days_high", "GET", "/admin/design-feedback?days=366", {}, None),
        ("days_text", "GET", "/admin/design-feedback?days=abc", {}, None),
        ("days_float", "GET", "/admin/design-feedback?days=1.5", {}, None),
    ]
    out = run(cases, "v")
    del app.dependency_overrides[deps.get_current_superuser]
    return out, recorded


class FakeField:
    def __init__(self, name):
        self.name = name

    def __eq__(self, other):
        return (self.name, other)

    __hash__ = None


def fake_model(fields, docs):
    calls = []
    found = []

    async def find_one(*exprs):
        calls.append([list(e) for e in exprs])
        want = dict(exprs)
        hit = next((d for d in docs if all(getattr(d, k) == v for k, v in want.items())), None)
        found.append(None if hit is None else {k: str(v) if isinstance(v, ObjectId) else v
                                                 for k, v in vars(hit).items()})
        return hit

    attrs = {f: FakeField(f) for f in fields}
    attrs["find_one"] = staticmethod(find_one)
    attrs["calls"] = calls
    attrs["found"] = found
    return type("Fake", (), attrs)


OID_A = ObjectId("65a1b2c3d4e5f60718293a41")
OID_B = ObjectId("65a1b2c3d4e5f60718293a42")
OID_C = ObjectId("65a1b2c3d4e5f60718293a43")


def resolve_section():
    ns = types.SimpleNamespace
    mods = [ns(owner_handle="aallyn", slug="cool-mod", id=OID_A, title="Cool Mod", visibility="public"),
            ns(owner_handle="aallyn", slug="draft", id=OID_A, title="D", visibility="draft"),
            ns(owner_handle="aallyn", slug="web", id=OID_A, title="W", visibility="public_website_only"),
            ns(owner_handle="ölaf", slug="ünlisted", id=OID_A, title="", visibility="unlisted")]
    packs = [ns(owner_handle="aallyn", slug="pack", id=OID_B, title="Pack", visibility="unlisted"),
             ns(owner_handle="aallyn", slug="hidden", id=OID_B, title="H", visibility="draft")]
    profiles = [ns(handle="aallyn", id=OID_C, display_name="Aallyn"),
                ns(handle="nz", id=OID_C, display_name="")]
    moderation.ModProject = fake_model(["owner_handle", "slug"], mods)
    moderation.ModpackProject = fake_model(["owner_handle", "slug"], packs)
    moderation.ModProfile = fake_model(["handle"], profiles)
    cases = [
        ("mod", " AALLYN ", "Cool-Mod\t"), ("mod", "aallyn", "draft"), ("mod", "aallyn", "web"),
        ("mod", "ÖLAF", "ÜNLISTED"), ("mod", "aallyn", "missing"), ("modpack", "Aallyn", "PACK"),
        ("modpack", "aallyn", "hidden"), ("profile", "AALLYN", "ignored"), ("profile", " NZ\x1f", ""),
        ("profile", "nobody", ""), ("mod", "İSTANBUL", "ΣΑΣ"),
    ]
    out = []
    for kind, handle, slug in cases:
        models = {"mod": moderation.ModProject, "modpack": moderation.ModpackProject,
                  "profile": moderation.ModProfile}
        model = models[kind]
        model.calls.clear()
        model.found.clear()
        got = asyncio.run(moderation._resolve_target(kind, handle, slug))
        out.append({
            "kind": kind, "handle": handle, "slug": slug,
            "filter": model.calls[0],
            "doc": model.found[0],
            "result": None if got is None else [str(got[0]), got[1], got[2]],
        })
    return out


def report_section():
    captured = []

    class FakeReport:
        def __init__(self, **kw):
            captured.append(kw)

        async def insert(self):
            return self

    async def resolve(kind, handle, slug):
        if handle == "missing":
            return None
        return OID_A, "", f"/mods/{handle}/{slug}"

    moderation._resolve_target = resolve
    moderation.ContentReport = FakeReport
    cases = [
        ("report_ok", "POST", "/v1/moderation/report", J,
         jb({"target_type": "mod", "handle": "h", "slug": "s", "reason": "  spam\n ", "extra": True})),
        ("report_not_found", "POST", "/v1/moderation/report", J,
         jb({"target_type": "profile", "handle": "missing", "reason": "why?"})),
    ]
    out = run(cases, "m")
    return out, [{k: str(v) if isinstance(v, ObjectId) else v for k, v in c.items()} for c in captured]


def responses_section():
    async def list_public():
        return ["IINikstarII", "Nao373", "Wahoo", "børyzje", "nz \"q\"", "Tues ", "\U0001F600"]

    created = dt.datetime(2025, 3, 4, 5, 6, 7, 123000, tzinfo=bson_utc)
    docs = [
        types.SimpleNamespace(name="IINikstarII", added_by=None, created_at=created),
        types.SimpleNamespace(name="ünï", added_by=ObjectId("65a1b2c3d4e5f60718293a4b"),
                              created_at=created.replace(microsecond=0)),
    ]

    async def admin_list():
        return docs

    async def add(name, *, added_by):
        return types.SimpleNamespace(name=name.strip(), added_by=added_by,
                                     created_at=dt.datetime(2025, 3, 4, 5, 6, 7, 123456, tzinfo=dt.timezone.utc))

    async def replace(names, *, added_by):
        return {"removed": 7, "added": len(names)}

    async def remove(name):
        return name.strip() == "nz"

    deps.check_rate_limit = _no_limit
    sup_service.list_public = list_public
    sup_service.admin_list = admin_list
    sup_service.add = add
    sup_service.replace = replace
    sup_service.remove = remove
    app.dependency_overrides[deps.get_current_superuser] = lambda: FakeUser()
    cases = [
        ("public_list", "GET", "/v1/misc/supporters", {}, None),
        ("admin_list", "GET", "/admin/supporters", {}, None),
        ("admin_add", "POST", "/admin/supporters", J, jb({"name": "  Néw  "})),
        ("admin_replace", "PUT", "/admin/supporters", J, jb({"names": ["a", "b"]})),
        ("admin_delete", "DELETE", "/admin/supporters/%20nz", {}, None),
        ("admin_delete_missing", "DELETE", "/admin/supporters/n%C3%A9'%22x", {}, None),
        ("df_post", "POST", "/site/design-feedback", J, jb({"design": "voxel", "vote": "up"})),
    ]
    out = run(cases, "s")
    del app.dependency_overrides[deps.get_current_superuser]
    return out


def clean_section():
    inserted = []

    class FakeDelete:
        async def delete(self):
            return types.SimpleNamespace(deleted_count=3)

    class FakeSupporter:
        def __init__(self, name, added_by):
            inserted.append(name)

        async def insert(self):
            return self

        @staticmethod
        def find_all():
            return FakeDelete()

    cases = [
        ["a", "b"], ["  Nz ", "nz", "NZ", "Wahoo"], ["", "   ", "\t"], [], [" x\x1c", "X", "x "],
        ["Straße", "STRASSE", "strasse"], ["İ", "i̇"], ["ΟΔΟΣ", "οδος", "οδοσ"], ["a", "A", "b", "a"],
    ]
    real = sup_service.Supporter
    sup_service.Supporter = FakeSupporter
    out = []
    for names in cases:
        inserted.clear()
        try:
            res = asyncio.run(REAL_REPLACE(names, added_by=None))
            out.append({"names": names, "out": list(inserted), "result": res})
        except ValueError as e:
            out.append({"names": names, "out": None, "error": str(e)})
    sup_service.Supporter = real
    return out


def add_errors_section():
    class FakeSupporter:
        name = FakeField("name")
        existing = {"nz", "Wahoo"}
        looked_up: list = []

        def __init__(self, name, added_by):
            self.value = name

        @classmethod
        async def find_one(cls, expr):
            cls.looked_up.append(expr[1])
            return object() if expr[1] in cls.existing else None

        async def insert(self):
            return self

    real = sup_service.Supporter
    sup_service.Supporter = FakeSupporter
    out = []
    for name in ["", "  \t", " nz ", "Wahoo", "wahoo", "　Ünï\x1d"]:
        FakeSupporter.looked_up.clear()
        try:
            doc = asyncio.run(REAL_ADD(name, added_by=None))
            out.append({"name": name, "lookup": list(FakeSupporter.looked_up), "stored": doc.value, "error": None})
        except ValueError as e:
            out.append({"name": name, "lookup": list(FakeSupporter.looked_up), "stored": None, "error": str(e)})
    sup_service.Supporter = real
    return out


def summary_section():
    fixed = dt.datetime(2026, 3, 1, 0, 30, tzinfo=dt.timezone.utc)
    df_service.utcnow = lambda: fixed
    rows_sets = [
        [],
        [{"_id": "x", "day": "2026-02-28", "design": "voxel", "theme": "auto", "vote": "up", "count": 3},
         {"day": "2026-02-27", "design": "classic", "theme": "dark", "vote": "down", "count": 2},
         {"day": "2026-02-28", "design": "voxel", "theme": "light", "vote": "down", "count": 1},
         {"day": "2026-02-28", "design": "neon", "theme": "sepia", "vote": "up", "count": 4},
         {"day": "2026-02-26", "design": "", "theme": None, "vote": "up"},
         {"day": "2026-02-26", "design": "voxel", "vote": "maybe", "count": 9},
         {"day": "2026-02-25", "vote": "down", "count": 2.9},
         {"day": "2026-02-25", "design": "zeta", "theme": "auto", "vote": "up", "count": "12"},
         {"day": "2026-02-25", "design": "alpha", "theme": "dark", "vote": "up", "count": True},
         {"day": "2026-02-24", "design": "voxel", "theme": "auto", "vote": "down", "count": None},
         {"day": "2026-02-24", "design": "voxel", "theme": "auto", "vote": "up", "count": 0}],
        [{"day": "2026-02-28", "design": "voxel", "theme": "auto", "vote": "up", "count": float("nan")}],
        [{"day": "2026-02-28", "design": "voxel", "theme": "auto", "vote": "nope", "count": "x"}],
    ]
    out = []
    for days in (1, 2, 30, 365):
        captured = {}

        class Cursor:
            def __init__(self, rows):
                self.rows = rows

            async def to_list(self, length=None):
                return self.rows

        rows = rows_sets[1]

        class Coll:
            def find(self, flt):
                captured["filter"] = flt
                return Cursor(rows)

        df_service.get_db = lambda: {df_service.COLLECTION: Coll()}
        asyncio.run(df_service.summary(days))
        out.append({"days": days, "since": captured["filter"]["day"]["$gte"]})
    results = []
    for rows in rows_sets:
        class Cursor2:
            async def to_list(self, length=None, rows=rows):
                return rows

        class Coll2:
            def find(self, flt):
                return Cursor2()

        df_service.get_db = lambda: {df_service.COLLECTION: Coll2()}
        tagged = [{k: NAN if isinstance(v, float) and v != v else v for k, v in r.items()} for r in rows]
        try:
            got = asyncio.run(df_service.summary(30))
            results.append({"rows": tagged, "out": DICT.dump_json(got).decode()})
        except (ValueError, TypeError, OverflowError) as e:
            results.append({"rows": tagged, "out": None, "error": type(e).__name__})
    return {"now": fixed.isoformat(), "since": out, "tallies": results}


NAN = "__nan__"


def models_section():
    skip = {"id", "revision_id"}
    return {
        "content_report": [f for f in ContentReport.model_fields if f not in skip],
        "supporter": [f for f in Supporter.model_fields if f not in skip],
    }


def main():
    http = run(HTTP, "h")
    validation, recorded = validation_section()
    responses = responses_section()
    resolve = resolve_section()
    reports, report_docs = report_section()
    data = {
        "python": sys.version.split()[0],
        "env": ENV,
        "http": http,
        "validation": validation,
        "recorded_votes": recorded,
        "responses": responses,
        "resolve": resolve,
        "reports": reports,
        "report_docs": report_docs,
        "clean": clean_section(),
        "add_errors": add_errors_section(),
        "summary": summary_section(),
        "models": models_section(),
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    text = json.dumps(data, ensure_ascii=True, indent=1, allow_nan=False)
    OUT.write_text(text + "\n", encoding="utf-8")
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes) with Python {data['python']}")


if __name__ == "__main__":
    main()
