"""Golden fixtures for the Dev Portal (app/auth, app/tokens, app/scanning), from
the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_portal_fixtures.py
Writes native/kiwi-server/testdata/portal/golden.json.

HTTP cases run the real `app.main.app` (raw ASGI) with no stores, so only
answers given before any Mongo/Redis access are recorded; the pure pieces
(EmailStr, mail rendering, view models, activity, export, IP pins, the
secret-scanning signature) are called directly with in-memory inputs. SECRET_KEY
stays the default so the Rust test process (same default) checks the same JWTs.
"""
import asyncio
import base64
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
    "APP_NAME": 'Kiwi <API> "é" & {x}',
    "API_URL": "https://api.example.com",
    "DEV_URL": "https://dev.example.com/a b?x=<1>",
    "DOCS_URL": "https://docs.example.com/?a=1&b='2'",
    "GITHUB_SECRET_SCANNING_KEYS_URL": "http://127.0.0.1:9/keys",
}
os.environ.update(ENV)
os.environ.pop("SECRET_KEY", None)

import httpx  # noqa: E402
import jwt  # noqa: E402
from bson import ObjectId  # noqa: E402
from cryptography.hazmat.primitives import hashes, serialization  # noqa: E402
from cryptography.hazmat.primitives.asymmetric import ec  # noqa: E402
from pydantic import BaseModel, EmailStr, ValidationError  # noqa: E402

from app.admin import runtime_config as rc  # noqa: E402

for _key, _spec in rc.REGISTRY.items():
    rc._CACHE[_key] = (_spec.default, float("inf"))

import app.auth.emails as emails  # noqa: E402
import app.auth.oauth as oauth  # noqa: E402
import app.scanning.github as scan_github  # noqa: E402
import app.tokens.router as tokens_router  # noqa: E402
import app.usage.service as usage_service  # noqa: E402
from app.auth import account  # noqa: E402
from app.auth.models import Session, User  # noqa: E402
from app.auth.router import _to_public as user_to_public  # noqa: E402
from app.core.config import settings  # noqa: E402
from app.main import app  # noqa: E402
from app.tokens.models import ApiToken  # noqa: E402
from app.tokens.schemas import TokenCreatedResponse  # noqa: E402
from app.usage.models import UsageEvent  # noqa: E402

OUT = Path(__file__).resolve().parents[2] / "kiwi-server" / "testdata" / "portal" / "golden.json"
J = {"content-type": "application/json"}
KEPT_HEADERS = ("content-type", "allow", "www-authenticate", "cache-control", "location", "content-disposition",
                "retry-after", "content-length", "content-security-policy")
SECRET = settings.secret_key
UTC = dt.timezone.utc


def jb(v) -> bytes:
    return json.dumps(v).encode()


async def call(method, url, headers, body):
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


def now_ts():
    return int(dt.datetime.now(UTC).timestamp())


def jwt_of(claims):
    return jwt.encode(claims, SECRET, algorithm="HS256")


SITE_JWT = jwt_of({"sub": "65a1b2c3d4e5f60718293a4b", "ver": 0, "iat": now_ts(), "exp": now_ts() + 86400 * 3650,
                   "type": "access", "kind": "site"})
EXPIRED_JWT = jwt_of({"sub": "65a1b2c3d4e5f60718293a4b", "ver": 0, "iat": 1000, "exp": 2000, "type": "access"})
NOSUB_VERIFY = jwt_of({"type": "email_verify", "iat": now_ts(), "exp": now_ts() + 86400 * 3650})
NOSUB_RESET = jwt_of({"type": "password_reset", "iat": now_ts(), "exp": now_ts() + 86400 * 3650, "fp": "x"})
WRONG_PURPOSE = jwt_of({"sub": "65a1b2c3d4e5f60718293a4b", "type": "password_reset", "iat": now_ts(),
                        "exp": now_ts() + 86400 * 3650})
BAD = {"authorization": "Bearer garbage"}
BASIC = {"authorization": "Basic abc"}
SITE = {"authorization": f"Bearer {SITE_JWT}"}
EXPIRED = {"authorization": f"Bearer {EXPIRED_JWT}"}
BADCRC = "kiwi_" + "a" * 43 + "_zz"

HTTP = [
    # --- bodies validated before anything else
    ("signup_missing", "POST", "/auth/signup", {}, None),
    ("signup_null", "POST", "/auth/signup", J, b"null"),
    ("signup_list", "POST", "/auth/signup", J, b"[]"),
    ("signup_empty", "POST", "/auth/signup", J, b"{}"),
    ("signup_bad", "POST", "/auth/signup", J, jb({"email": "bad", "password": "short"})),
    ("signup_types", "POST", "/auth/signup", J,
     jb({"email": 5, "password": 5, "display_name": "x" * 81, "captcha_token": 1})),
    ("signup_badjson", "POST", "/auth/signup", J, b'{"email":'),
    ("signup_text_ct", "POST", "/auth/signup", {"content-type": "text/plain"}, b"{}"),
    ("signup_special", "POST", "/auth/signup", J, jb({"email": "a@b.test", "password": "x" * 257})),
    ("signup_pretty_short", "POST", "/auth/signup", J, jb({"email": "Ann <a@b.co>", "password": "1234567"})),
    ("signup_quoted", "POST", "/auth/signup", J, jb({"email": '"q"@b.com', "password": "12345678",
                                                      "display_name": None})),
    ("signup_unicode_pw_short", "POST", "/auth/signup", J, jb({"email": "a@b.co", "password": "\u00e9" * 7})),
    ("login_empty", "POST", "/auth/login", J, b"{}"),
    ("login_types", "POST", "/auth/login", J, jb({"email": "x@y", "password": 1, "captcha_token": []})),
    ("forgot_empty", "POST", "/auth/forgot-password", J, b"{}"),
    ("forgot_bad", "POST", "/auth/forgot-password", J, jb({"email": "nope"})),
    ("resend_bad", "POST", "/auth/resend-verification", J, jb({"email": "a@b..c"})),
    ("resend_missing", "POST", "/auth/resend-verification", {}, None),
    ("reset_empty", "POST", "/auth/reset-password", J, b"{}"),
    ("reset_types", "POST", "/auth/reset-password", J, jb({"token": 1, "new_password": "short"})),
    ("reset_bad_token", "POST", "/auth/reset-password", J, jb({"token": "nope", "new_password": "longenough"})),
    ("reset_nosub", "POST", "/auth/reset-password", J, jb({"token": NOSUB_RESET, "new_password": "longenough"})),
    ("reset_wrong_purpose", "POST", "/auth/reset-password", J,
     jb({"token": WRONG_PURPOSE.replace("a", "b", 1), "new_password": "longenough"})),
    ("refresh_empty", "POST", "/auth/refresh", J, b"{}"),
    ("refresh_types", "POST", "/auth/refresh", J, jb({"refresh_token": 5})),
    ("logout_missing", "POST", "/auth/logout", {}, None),
    ("logout_types", "POST", "/auth/logout", J, jb({"refresh_token": 5})),
    ("logout_empty", "POST", "/auth/logout", J, b"{}"),
    ("logout_blank", "POST", "/auth/logout", J, jb({"refresh_token": ""})),
    ("logout_null", "POST", "/auth/logout", J, jb({"refresh_token": None})),
    ("exchange_empty", "POST", "/auth/oauth/exchange", J, b"{}"),
    ("exchange_no_redis", "POST", "/auth/oauth/exchange", J, jb({"code": "abc"})),
    # --- portal session required
    ("me_noauth", "GET", "/auth/me", {}, None),
    ("me_bad", "GET", "/auth/me", BAD, None),
    ("me_basic", "GET", "/auth/me", BASIC, None),
    ("me_site", "GET", "/auth/me", SITE, None),
    ("me_expired", "GET", "/auth/me", EXPIRED, None),
    ("patch_me_noauth", "PATCH", "/auth/me", J, b"{}"),
    ("patch_me_badjson_first", "PATCH", "/auth/me", J, b"{"),
    ("patch_me_noauth_invalid", "PATCH", "/auth/me", J, jb({"display_name": 5})),
    ("change_password_noauth", "POST", "/auth/change-password", J, jb({})),
    ("change_password_badjson", "POST", "/auth/change-password", J, b"[1,]"),
    ("change_email_noauth", "POST", "/auth/change-email", J, jb({"new_email": "bad"})),
    ("logout_all_noauth", "POST", "/auth/logout-all", {}, None),
    ("sessions_noauth", "GET", "/auth/sessions", BAD, None),
    ("session_delete_noauth", "DELETE", "/auth/sessions/nope", {}, None),
    ("export_noauth", "GET", "/auth/me/export", {}, None),
    ("delete_account_noauth", "POST", "/auth/delete-account", J, jb({"confirm_email": 1})),
    ("tokens_create_noauth", "POST", "/tokens", J, jb({"name": ""})),
    ("tokens_create_badjson", "POST", "/tokens", J, b"{]"),
    ("tokens_list_noauth", "GET", "/tokens", {}, None),
    ("tokens_activity_noauth", "GET", "/tokens/activity?days=0", {}, None),
    ("token_activity_noauth", "GET", "/tokens/nope/activity?days=x", SITE, None),
    ("token_patch_noauth", "PATCH", "/tokens/nope", J, jb({"name": ""})),
    ("token_patch_literal_noauth", "PATCH", "/tokens/activity", J, b"{}"),
    ("token_rotate_noauth", "POST", "/tokens/nope/rotate", EXPIRED, None),
    ("token_revoke_noauth", "POST", "/tokens/nope/revoke", J, jb({})),
    # --- HTML pages and purpose tokens that fail before any lookup
    ("verify_email_no_token", "GET", "/auth/verify-email", {}, None),
    ("verify_email_bad", "GET", "/auth/verify-email?token=nope", {}, None),
    ("verify_email_nosub", "GET", f"/auth/verify-email?token={NOSUB_VERIFY}", {}, None),
    ("verify_email_wrong_purpose", "GET", f"/auth/verify-email?token={WRONG_PURPOSE}", {}, None),
    ("verify_email_html_accept", "GET", "/auth/verify-email?token=x", {"accept": "text/html"}, None),
    ("verify_change_bad", "GET", "/auth/verify-email-change?token=a.b.c", {}, None),
    ("verify_change_no_token", "GET", "/auth/verify-email-change", {}, None),
    ("reset_form", "GET", "/auth/reset-password?token=%3Cscript%3E%22%27%26x", {}, None),
    ("reset_form_plain", "GET", "/auth/reset-password?token=abc.def", {}, None),
    ("reset_form_no_token", "GET", "/auth/reset-password", {}, None),
    # --- method not allowed
    ("me_405", "DELETE", "/auth/me", {}, None),
    ("reset_405", "PUT", "/auth/reset-password", {}, None),
    ("tokens_405", "DELETE", "/tokens", {}, None),
    ("tokens_activity_405", "DELETE", "/tokens/activity", {}, None),
    ("token_405", "GET", "/tokens/abc", {}, None),
    ("token_activity_405", "POST", "/tokens/abc/activity", {}, None),
    ("session_405", "GET", "/auth/sessions/x", {}, None),
    ("sessions_405", "POST", "/auth/sessions", {}, None),
    ("scanning_405", "GET", "/secret-scanning/github", {}, None),
    ("signup_405", "GET", "/auth/signup", {}, None),
    ("exchange_405", "GET", "/auth/oauth/exchange", {}, None),
    ("start_405", "POST", "/auth/oauth/github/start", {}, None),
    # --- GitHub sign-in not configured
    ("oauth_start_off", "GET", "/auth/oauth/github/start", {}, None),
    ("oauth_callback_off", "GET", "/auth/oauth/github/callback?code=a&state=b", {}, None),
    # --- secret scanning with signature checks on (keys cannot be fetched)
    ("scan_unsigned", "POST", "/secret-scanning/github", J, b"[]"),
    ("scan_unknown_key", "POST", "/secret-scanning/github",
     {"github-public-key-identifier": "k1", "github-public-key-signature": "AAAA"}, b"[]"),
]

SCAN_UNVERIFIED = [
    ("scan_not_json", "POST", "/secret-scanning/github", {}, b"not json"),
    ("scan_object", "POST", "/secret-scanning/github", J, b'{"a": 1}'),
    ("scan_empty", "POST", "/secret-scanning/github", J, b"[]"),
    ("scan_skipped", "POST", "/secret-scanning/github", J,
     jb(["x", 5, {"token": ""}, {"token": None}, {"type": "t"}, {"token": 0}, {"token": []}])),
    ("scan_bad_checksum", "POST", "/secret-scanning/github", J,
     jb([{"token": BADCRC, "type": "kiwi_api_token"}, {"token": BADCRC, "type": ""},
         {"token": BADCRC, "type": 7, "url": "x"}])),
    ("scan_empty_body", "POST", "/secret-scanning/github", {}, b""),
]

OAUTH_ON = [
    ("oauth_start_no_redis", "GET", "/auth/oauth/github/start", {}, None),
    ("oauth_callback_missing", "GET", "/auth/oauth/github/callback", {}, None),
    ("oauth_callback_blank", "GET", "/auth/oauth/github/callback?code=&state=s", {}, None),
    ("oauth_callback_no_redis", "GET", "/auth/oauth/github/callback?code=c&state=s", {}, None),
]


def http_sections():
    async def no_keys():
        raise httpx.ConnectError("refused")

    scan_github._fetch_keys = no_keys
    oauth.get_redis = lambda: None
    out = {"http": run(HTTP, "h")}
    settings.github_secret_scanning_verify = False
    out["scan"] = run(SCAN_UNVERIFIED, "s")
    settings.github_secret_scanning_verify = True
    settings.github_client_id = "gh id+/é"
    settings.github_client_secret = "gh-secret"
    out["oauth"] = run(OAUTH_ON, "o")
    out["oauth_env"] = {"GITHUB_CLIENT_ID": settings.github_client_id, "GITHUB_CLIENT_SECRET": "gh-secret"}
    out["authorize_url"] = (f"{oauth._AUTHORIZE}?" + urllib.parse.urlencode({
        "client_id": settings.github_client_id, "redirect_uri": oauth._redirect_uri(),
        "scope": "read:user user:email", "state": "St4te_-x", "allow_signup": "true"}))
    out["authorize_location"] = oauth.RedirectResponse(out["authorize_url"], status_code=307).headers["location"]
    settings.github_client_id = None
    settings.github_client_secret = None
    return out


# --- EmailStr --------------------------------------------------------------------------

class EM(BaseModel):
    email: EmailStr


EMAILS = [
    "a@b.com", "  A@B.COM ", "Support@X.io", "INFO@x.io", "x@test", "x@a.local", "x@sub.localhost", "x@a.onion",
    "x@in-addr.arpa", "x@foo.invalid", "<a@b.co>", "Ann <a@b.co>", " Ann  Lee\t<a@b.co> ", '"Ann" <a@b.co>',
    '"A \\"q\\"" <a@b.co>', "N <a@b.co> d", "N <a@b.co", "<a@b.co> ", "<a@b.co>>", "a <b <c@d.co>>",
    "N. Q <a@b.co>", "x y <a@b.co> z", '"q"@b.com', 'a b@c.com', 'a@b..com', '.a@b.com', 'a.@b.com', 'a..b@c.com',
    'a@-b.com', 'a@b-.com', 'a@b.-c.com', 'a@b.c-.com', 'ab--c@ab--c.com', 'a@ab--c.com', 'a@Xn--c.com', 'a@1.2',
    'a@b.c1', 'a"b@c.com', 'a@b', 'a\\b@c.com', '@b.com', 'a@', 'abc', 'a\tb@c.com', '"a b"@c.com',
    '"a\\"b"@c.com', '"ab"x@c.com', '"ab"\x01x@c.com', 'a@B.Com', 'x' * 250 + '@b.com', 'x' * 249 + '@b.com',
    'a@' + 'b' * 64 + '.com', 'a@' + 'b' * 63 + '.com', 'a@' + ('b' * 60 + '.') * 5 + 'com',
    'a@' + ('B' * 60 + '.') * 5 + 'com', 'a@' + ('b' * 61 + '.') * 4 + 'c', 'x' * 64 + '@' + ('b' * 60 + '.') * 3 + 'ab',
    'x' * 2049, "a@b.com\n", "a@b.com\r", "a@b_c.com", "a@b c.com", "a@b.com.", "a@.b.com", "a@b@c.com",
    "a!#$%&'*+-/=?^_`{|}~@b.co", "a()@b.co", "a<@b.co", "a>b@c.co", "a,b@c.co", "a;b@c.co", "a[@b.co",
    "\x7f@b.co", "a\x00@b.co", "a @b.co", " <a b@c.co>", "<a\x01b@c.co>", "\x01 <a@b.co>", '"\x01" <a@b.co>',
    '"\x01x" <a@b.co>', "a.b <c@d.co>", "a@b.co ", "\x1ca@b.co\x1f", "a@b.co\x0b", "a@\x0bb.co", "<>",
    "<a@b.co><c@d.co>", "\"\" <a@b.co>", "<\"q\"@b.co>", "a@b.co.uk", "a@B-C.CO.UK", "a@b.c", "a@1b.c2d.ef",
    "", " ", "@", "a@b.com@", "\"a@b\"@c.com", "\"a", "'a'@b.co", "a@b.coM", "Postmaster@B.co",
]


def email_section():
    out = []
    for v in EMAILS:
        try:
            out.append({"input": v, "ok": EM.model_validate({"email": v}).email})
        except ValidationError as e:
            err = e.errors(include_url=False)[0]
            out.append({"input": v, "err": {"type": err["type"], "msg": err["msg"], "ctx": err.get("ctx")}})
    for v in [5, None, True, ["a@b.co"]]:
        try:
            EM.model_validate({"email": v})
        except ValidationError as e:
            err = e.errors(include_url=False)[0]
            out.append({"input": v, "err": {"type": err["type"], "msg": err["msg"], "ctx": err.get("ctx")}})
    return out


# --- mail ------------------------------------------------------------------------------

def fake_user(**kw):
    base = dict(id=ObjectId("65a1b2c3d4e5f60718293a4b"), email="user@example.com",
                hashed_password="$argon2id$v=19$m=65536,t=3,p=4$c2FsdA$aGFzaA", display_name="Ann")
    base.update(kw)
    return types.SimpleNamespace(**base)


def mail_section():
    captured = []

    async def queue(to, subject, text, html=None):
        captured.append({"to": to, "subject": subject, "text": text, "html": html})

    emails.queue_email = queue
    emails.create_email_token = lambda *a, **k: "TOKEN"
    user = fake_user()
    name, prefix = 'My "token" <&>', "kiwi_AbCd1234"
    calls = [
        ("verification", emails.send_verification_email(user)),
        ("reset", emails.send_password_reset_email(user)),
        ("email_change", emails.send_email_change_verification(user, "new@example.com")),
        ("new_login", emails.send_new_login_email(user, "203.0.113.7", "Chrome on Windows")),
        ("new_login_unknown", emails.send_new_login_email(user, "", None)),
        ("password_changed", emails.send_password_changed_email(user)),
        ("token_created", emails.send_token_created_email(user, name, prefix)),
        ("token_expiring", emails.send_token_expiring_email(user, name, prefix, 3)),
        ("token_expiring_today", emails.send_token_expiring_email(user, name, prefix, 0)),
        ("github_linked", emails.send_github_linked_email(user)),
        ("token_compromised", emails.send_token_compromised_email(user, name, prefix)),
        ("email_changed_old", emails.send_email_changed_notice_to_old("old@example.com", "new@example.com")),
    ]
    out = []
    for label, coro in calls:
        captured.clear()
        asyncio.run(coro)
        out.append({"name": label, **captured[0]})
    return out


# --- view models -----------------------------------------------------------------------

T0 = dt.datetime(2026, 3, 4, 5, 6, 7, 123000, tzinfo=UTC)
T1 = dt.datetime(2026, 3, 5, 0, 0, 0, tzinfo=UTC)
T2 = dt.datetime(2026, 4, 1, 12, 30, 45, 999000, tzinfo=UTC)
FRESH = dt.datetime(2026, 3, 4, 5, 6, 7, 123456, tzinfo=UTC)


def iso(d):
    return None if d is None else d.isoformat()


def token_docs():
    return [
        dict(id=ObjectId("65a1b2c3d4e5f60718293a41"), user_id=ObjectId("65a1b2c3d4e5f60718293a4b"), name="Bot ✓",
             prefix="kiwi_AbCd1234", hashed_token="h1", scopes=0, ip_salt="s", allowed_ip_hashes=["a", "b"],
             revoked=False, revoked_at=None, revoke_reason=None, expires_at=T2, expiry_warned=False, created_at=T0,
             last_used_at=T1, rotated_at=None, request_count=12),
        dict(id=ObjectId("65a1b2c3d4e5f60718293a42"), user_id=ObjectId("65a1b2c3d4e5f60718293a4b"), name="x",
             prefix="kiwi_zz", hashed_token="h2", scopes=5 | 512, ip_salt=None, allowed_ip_hashes=[],
             revoked=True, revoked_at=T1, revoke_reason='Leaked "oops"', expires_at=None, expiry_warned=True,
             created_at=T1, last_used_at=None, rotated_at=T2, request_count=2 ** 33),
    ]


def views_section():
    tokens = []
    for d in token_docs():
        t = ApiToken(**d)
        public = tokens_router._to_public(t)
        created = TokenCreatedResponse(**public.model_dump(), token="kiwi_secret")
        tokens.append({"doc": {k: (str(v) if isinstance(v, ObjectId) else iso(v) if isinstance(v, dt.datetime) else v)
                               for k, v in d.items()},
                       "public": public.model_dump_json(), "created": created.model_dump_json()})
    users = []
    for kw in [dict(created_at=T0, last_login_at=None, roles=[], display_name=None),
               dict(created_at=FRESH, last_login_at=T2, roles=["mod", "ü"], display_name='Ann "<x>"',
                    is_superuser=True, is_verified=True, is_active=False)]:
        u = User(id=ObjectId("65a1b2c3d4e5f60718293a4b"), email="ann@example.com", hashed_password="h", **kw)
        users.append({"doc": {k: (iso(v) if isinstance(v, dt.datetime) else v) for k, v in kw.items()},
                      "public": user_to_public(u).model_dump_json()})
    return {"tokens": tokens, "users": users}


def export_section():
    user = User(id=ObjectId("65a1b2c3d4e5f60718293a4b"), email="ann@example.com", hashed_password="h",
                display_name="Ann ✓", roles=["r"], github_id=12345, created_at=T0, last_login_at=None)
    tokens = [ApiToken(**d) for d in token_docs()]
    sessions = [Session(id=ObjectId("65a1b2c3d4e5f60718293a51"), user_id=user.id, refresh_token_hash="x",
                        device="Chrome on Windows", created_at=T0, last_used_at=T1, expires_at=T2, revoked=False),
                Session(id=ObjectId("65a1b2c3d4e5f60718293a52"), user_id=user.id, refresh_token_hash="y",
                        device=None, created_at=T1, last_used_at=T1, expires_at=T2, revoked=True)]
    raw_usage = [
        {"method": "GET", "route": "/v1/x/{id}", "path": "/v1/x/1", "status_code": 200, "duration_ms": 12.5,
         "created_at": T0},
        {"method": "POST", "route": "/v1/y", "path": "/v1/y", "status_code": 429, "duration_ms": 3,
         "created_at": T1},
        {"method": "GET", "route": "/v1/z", "path": "/v1/z?q=é", "status_code": 500, "duration_ms": 0.1 + 0.2,
         "created_at": T2},
    ]
    usage = [UsageEvent(user_id=user.id, token_id=tokens[0].id, **e) for e in raw_usage]

    class Q:
        def __init__(self, rows):
            self.rows = rows

        async def to_list(self):
            return self.rows

    fixed = dt.datetime(2026, 5, 6, 7, 8, 9, 654321, tzinfo=UTC)
    account.utcnow = lambda: fixed
    account.ApiToken = types.SimpleNamespace(find=lambda *_: Q(tokens), user_id=None)
    account.Session = types.SimpleNamespace(find=lambda *_: Q(sessions), user_id=None)
    account.UsageEvent = types.SimpleNamespace(find=lambda *_: Q(usage), user_id=None)
    resp = asyncio.run(account.export_account(user))
    return {
        "exported_at": fixed.isoformat(),
        "user": {"email": user.email, "display_name": user.display_name, "roles": user.roles,
                 "github_id": user.github_id, "created_at": iso(user.created_at)},
        "tokens": [{k: (str(v) if isinstance(v, ObjectId) else iso(v) if isinstance(v, dt.datetime) else v)
                    for k, v in d.items()} for d in token_docs()],
        "sessions": [{"id": str(s.id), "device": s.device, "revoked": s.revoked, "created_at": iso(s.created_at),
                      "last_used_at": iso(s.last_used_at), "expires_at": iso(s.expires_at)} for s in sessions],
        "usage": [{**e, "created_at": iso(e["created_at"])} for e in raw_usage],
        "body": resp.body.decode(),
        "disposition": resp.headers["content-disposition"],
    }


def activity_section():
    sets = [
        ([], [], []),
        ([{"_id": None, "count": 10, "error_count": 3, "rate_limited": 1, "avg_duration_ms": 12.345}],
         [{"_id": "2026-03-01", "count": 4, "error_count": 0, "avg_duration_ms": 0.125},
          {"_id": "2026-03-02", "count": 6, "error_count": 3, "avg_duration_ms": 2.675}],
         [{"_id": {"route": "/v1/a", "method": "GET"}, "count": 7, "error_count": 1, "avg_duration_ms": 1.005},
          {"_id": {"route": "/v1/b/{x}", "method": "POST"}, "count": 3, "error_count": 2, "avg_duration_ms": None}]),
        ([{"_id": None, "count": 1, "error_count": 0, "rate_limited": 0, "avg_duration_ms": None}],
         [{"_id": "2026-03-01", "count": 1, "error_count": 0, "avg_duration_ms": 1e16}],
         [{"_id": {"route": "/v1/a", "method": "GET"}, "count": 1, "error_count": 0, "avg_duration_ms": 0.0}]),
    ]
    out = []
    for days, (totals, by_day, by_endpoint) in zip((7, 1, 365), sets):
        queue = [totals, by_day, by_endpoint]
        pipelines = []

        class Agg:
            def __init__(self, rows):
                self.rows = rows

            async def to_list(self):
                return self.rows

        def aggregate(pipeline, queue=queue, pipelines=pipelines):
            pipelines.append(pipeline)
            return Agg(queue.pop(0))

        usage_service.UsageEvent = types.SimpleNamespace(aggregate=aggregate)
        got = asyncio.run(usage_service.aggregate_activity({"user_id": ObjectId("65a1b2c3d4e5f60718293a4b")}, days))
        out.append({"days": days, "totals": totals, "by_day": by_day, "by_endpoint": by_endpoint,
                    "out": got.model_dump_json(),
                    "pipelines": json.loads(json.dumps(pipelines, default=str))})
    return out


# --- token IP pins ---------------------------------------------------------------------

def ips_section():
    cases = [
        ["1.2.3.4"], [" 1.2.3.4 ", "1.2.3.4", "\t"], ["::1", "0:0::1", "0:0:0:0:0:0:0:1"], ["fe80::1%eth0"],
        ["FE80::0001%Eth0"], ["fe80::1%"], ["1.2.3.4%x"], ["1.2.3.4/24"], ["CIDR"], [" CIDR?/"], ["abc"], [""],
        ["::ffff:1.2.3.4"], ["1.2.3.04"], ["2001:DB8::1"], ["1.2.3.4", "bad", "5.6.7.8/8"], ["256.1.1.1"],
        ["::ffff:0102:0304"], ["2001:db8:0:0:1:0:0:1"], ["a%b%c"], [], ["\u00a01.2.3.4\u3000"],
    ]
    out = []
    for ips in cases:
        try:
            out.append({"ips": ips, "hashes": tokens_router._hash_pinned_ips(ips, "S4lt_-")})
        except Exception as e:  # APIError
            out.append({"ips": ips, "error": e.message})
    return out


# --- secret-scanning signatures --------------------------------------------------------

def signature_section():
    body = b'[{"token":"kiwi_x","type":"kiwi_api_token","url":"https://github.com/x"}]'
    out = {"body": body.decode(), "keys": []}
    for name, curve in (("p256", ec.SECP256R1()), ("p384", ec.SECP384R1())):
        key = ec.generate_private_key(curve)
        pem = key.public_key().public_bytes(serialization.Encoding.PEM,
                                            serialization.PublicFormat.SubjectPublicKeyInfo).decode()
        sig = base64.b64encode(key.sign(body, ec.ECDSA(hashes.SHA256()))).decode()
        other = ec.generate_private_key(curve).public_key().public_bytes(
            serialization.Encoding.PEM, serialization.PublicFormat.SubjectPublicKeyInfo).decode()
        cases = []
        for label, b, s, p in [("ok", body, sig, pem), ("tampered", body + b" ", sig, pem),
                               ("wrong_key", body, sig, other), ("bad_b64", body, sig[:-2], pem),
                               ("spaced_b64", body, " ".join([sig[:10], sig[10:]]), pem)]:
            cases.append({"label": label, "body": b.decode(), "sig": s, "pem": p,
                          "ok": asyncio.run(_verify_with(p, b, s))})
        out["keys"].append({"name": name, "cases": cases})
    b64 = []
    for s in ["", "QQ==", "QQ", "QUI=", "QUJD", "QUJDRA", "Q", "QQ==QUJD", "Q Q = =", "QQ=", "Q=Q=", "QUJD\n",
              "=QUJD", "QU*JD", "QUJ=D", "QUI==extra", "QQ=QUJD", "Q=Q==", "QUI=QQ==", "QUI=Q", "QUJD====",
              "QQ===", "QQ=x=", "QQ= =", "QQ==QQ==", "QQ==Q", "=", "Q===", "QUJD=QQ==", "QQ=*=", "Q\x80Q==",
              "QUJDRA==\n\n", "QUJDRA="]:
        try:
            b64.append({"in": s, "out": base64.b64decode(s).hex()})
        except ValueError:
            b64.append({"in": s, "out": None})
    out["b64"] = b64
    return out


async def _verify_with(pem, body, sig):
    scan_github._key_cache.clear()
    scan_github._key_cache["k"] = pem
    return await scan_github.verify_signature(body, "k", sig)


def models_section():
    skip = {"id", "revision_id"}
    from app.core.email_outbox import OutboxEmail
    return {name: [f for f in m.model_fields if f not in skip]
            for name, m in (("user", User), ("session", Session), ("api_token", ApiToken),
                            ("outbox", OutboxEmail), ("usage_event", UsageEvent))}


async def init_models():
    import beanie.odm.utils.init as beanie_init
    from beanie import init_beanie
    from pymongo import AsyncMongoClient

    from app.core.email_outbox import OutboxEmail

    async def no_server(self):
        self._database_major_version = 7
        self._existing_collections = []

    beanie_init.Initializer._load_cached_info = no_server
    client = AsyncMongoClient("mongodb://127.0.0.1:1/", tz_aware=True, connect=False)
    await init_beanie(database=client["fixtures"], document_models=[User, Session, ApiToken, UsageEvent, OutboxEmail],
                      skip_indexes=True)


def main():
    http = http_sections()
    asyncio.run(init_models())
    data = {
        "python": sys.version.split()[0],
        "env": ENV,
        **http,
        "emails": email_section(),
        "mail": mail_section(),
        "views": views_section(),
        "export": export_section(),
        "activity": activity_section(),
        "ips": ips_section(),
        "signatures": signature_section(),
        "models": models_section(),
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    text = json.dumps(data, ensure_ascii=True, indent=1, allow_nan=False)
    OUT.write_text(text + "\n", encoding="utf-8")
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
