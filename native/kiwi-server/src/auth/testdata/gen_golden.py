"""Regenerate golden.json from the real Python auth code.

Run from anywhere with the production-pinned venv:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/kiwi-server/src/auth/testdata/gen_golden.py
"""
import base64
import hashlib
import hmac as _hmac
import json
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path
from unittest import mock

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[4]))

import jwt  # noqa: E402
from argon2 import PasswordHasher, Type  # noqa: E402
from argon2.low_level import hash_secret  # noqa: E402
from fastapi.security.utils import get_authorization_scheme_param  # noqa: E402
from starlette.requests import Request  # noqa: E402
from starlette.responses import Response  # noqa: E402
from uvicorn.middleware.proxy_headers import _TrustedHosts  # noqa: E402

from app.core import ip_hash, ratelimit, scopes, security  # noqa: E402
from app.core.config import settings  # noqa: E402
from app.core.utils import device_label  # noqa: E402
from app.site_auth import cookies, sessions  # noqa: E402
from app.site_auth.schemas import SiteTokenResponse  # noqa: E402
from app.tokens import usage as token_usage  # noqa: E402

SECRET = "CHANGE_ME_insecure_dev_secret_do_not_use_in_production"
FIXED = datetime(2026, 10, 22, 12, 34, 56, 789123, tzinfo=timezone.utc)
settings.secret_key = SECRET
settings.api_token_prefix = "kiwi"
settings.access_token_expire_minutes = 15

out: dict = {"secret": SECRET, "fixed_now": FIXED.timestamp()}

# --- JWT encode ----------------------------------------------------------
enc = []
with mock.patch.object(security, "utcnow", return_value=FIXED), \
        mock.patch.object(sessions, "utcnow", return_value=FIXED):
    for sub, ver, sid in [("65f0a1b2c3d4e5f601234567", 0, None),
                          ("65f0a1b2c3d4e5f601234567", 7, "6718a0b1c2d3e4f5a6b7c8d9")]:
        enc.append({"kind": "access", "sub": sub, "ver": ver, "sid": sid,
                    "token": security.create_access_token(sub, ver, session_id=sid)})
        enc.append({"kind": "site", "sub": sub, "ver": ver, "sid": sid,
                    "token": sessions.create_site_access_token(sub, ver, session_id=sid)})
    enc.append({"kind": "email", "sub": "65f0a1b2c3d4e5f601234567", "purpose": "password_reset",
                "seconds": 3600, "extra": {"fp": "0123456789abcdef"},
                "token": security.create_email_token("65f0a1b2c3d4e5f601234567", "password_reset",
                                                     timedelta(hours=1), extra={"fp": "0123456789abcdef"})})
    enc.append({"kind": "email", "sub": "65f0a1b2c3d4e5f601234567", "purpose": "email_change",
                "seconds": 172800, "extra": {"new": "üser@exämple.com", "fp": "ffff"},
                "token": security.create_email_token("65f0a1b2c3d4e5f601234567", "email_change",
                                                     timedelta(hours=48),
                                                     extra={"new": "üser@exämple.com", "fp": "ffff"})})
out["jwt_encode"] = enc


# --- JWT decode accept/reject --------------------------------------------
def raw_token(header: dict | list, payload, key: str = SECRET, sig: bytes | None = None) -> str:
    h = base64.urlsafe_b64encode(json.dumps(header, separators=(",", ":")).encode()).rstrip(b"=")
    p = base64.urlsafe_b64encode(json.dumps(payload, separators=(",", ":")).encode()).rstrip(b"=")
    si = h + b"." + p
    s = sig if sig is not None else _hmac.new(key.encode(), si, hashlib.sha256).digest()
    return (si + b"." + base64.urlsafe_b64encode(s).rstrip(b"=")).decode()


FAR, PAST = 4102444800, 1000000000
STD = {"alg": "HS256", "typ": "JWT"}
base = {"sub": "65f0a1b2c3d4e5f601234567", "ver": 1, "iat": 1700000000, "exp": FAR, "type": "access"}
cases = [
    ("ok", raw_token(STD, base)),
    ("ok_no_typ", raw_token({"alg": "HS256"}, base)),
    ("expired", raw_token(STD, {**base, "exp": PAST})),
    ("iat_future", raw_token(STD, {**base, "iat": FAR})),
    ("iat_float", raw_token(STD, {**base, "iat": 1700000000.75})),
    ("iat_str_int", raw_token(STD, {**base, "iat": "1700000000"})),
    ("iat_str_bad", raw_token(STD, {**base, "iat": "abc"})),
    ("iat_null", raw_token(STD, {**base, "iat": None})),
    ("exp_true", raw_token(STD, {**base, "exp": True})),
    ("exp_str_far", raw_token(STD, {**base, "exp": str(FAR)})),
    ("no_exp", raw_token(STD, {k: v for k, v in base.items() if k != "exp"})),
    ("nbf_future", raw_token(STD, {**base, "nbf": FAR})),
    ("nbf_past", raw_token(STD, {**base, "nbf": PAST})),
    ("aud_set", raw_token(STD, {**base, "aud": "x"})),
    ("aud_empty", raw_token(STD, {**base, "aud": ""})),
    ("sub_int", raw_token(STD, {**base, "sub": 5})),
    ("jti_int", raw_token(STD, {**base, "jti": 5})),
    ("jti_str", raw_token(STD, {**base, "jti": "x"})),
    ("iss_any", raw_token(STD, {**base, "iss": 5})),
    ("alg_hs512", raw_token({"alg": "HS512", "typ": "JWT"}, base)),
    ("alg_none", raw_token({"alg": "none", "typ": "JWT"}, base, sig=b"")),
    ("no_alg", raw_token({"typ": "JWT"}, base)),
    ("wrong_key", raw_token(STD, base, key="other")),
    ("kid_int", raw_token({**STD, "kid": 1}, base)),
    ("kid_str", raw_token({**STD, "kid": "a"}, base)),
    ("crit_b64_true", raw_token({**STD, "b64": True, "crit": ["b64"]}, base)),
    ("crit_other", raw_token({**STD, "crit": ["exp"]}, base)),
    ("header_list", raw_token([1], base)),
    ("payload_list", raw_token(STD, [1, 2])),
    ("sig_pad", raw_token(STD, base) + "="),
    ("two_segments", ".".join(raw_token(STD, base).split(".")[:2])),
    ("garbage", "not-a-jwt"),
    ("empty", ""),
]
dec = []
for name, tok in cases:
    try:
        payload = jwt.decode(tok, SECRET, algorithms=["HS256"])
        dec.append({"name": name, "token": tok, "ok": True, "payload": payload})
    except jwt.PyJWTError:
        dec.append({"name": name, "token": tok, "ok": False})
out["jwt_decode"] = dec

# --- argon2 --------------------------------------------------------------
salt = bytes(range(16))
fixed_hashes = []
for pw in ["hunter2hunter2", "pässwörd \U0001f600", ""]:
    fixed_hashes.append({"password": pw, "salt_b64": base64.b64encode(salt).decode(),
                         "hash": hash_secret(pw.encode(), salt, time_cost=3, memory_cost=65536,
                                             parallelism=4, hash_len=32, type=Type.ID).decode()})
out["argon2_fixed"] = fixed_hashes
ph = PasswordHasher()
verify_cases = [{"password": "correct horse", "hash": ph.hash("correct horse")}]
for t, name in [(Type.I, "i"), (Type.D, "d"), (Type.ID, "id")]:
    h = hash_secret(b"cheap-pw", b"saltsaltsalt", time_cost=1, memory_cost=1024, parallelism=2,
                    hash_len=24, type=t).decode()
    verify_cases.append({"password": "cheap-pw", "hash": h, "type": name})
verify_cases.append({"password": "cheap-pw",
                     "hash": hash_secret(b"cheap-pw", b"saltsaltsalt", time_cost=1, memory_cost=1024,
                                         parallelism=2, hash_len=24, type=Type.ID, version=0x10).decode(),
                     "type": "id-v16"})
for c in verify_cases:
    c["ok"] = ph.verify(c["hash"], c["password"])
    try:
        c["wrong_ok"] = ph.verify(c["hash"], c["password"] + "x")
    except Exception:
        c["wrong_ok"] = False
out["argon2_verify"] = verify_cases
out["argon2_default_prefix"] = ph.hash("x").rsplit("$", 2)[0]

# --- API tokens ----------------------------------------------------------
out["b62"] = [{"n": n, "s": security._b62_encode(n)} for n in [0, 1, 61, 62, 3843, 3844, 2**32 - 1]]
bodies = ["a" * 43, "Z" * 43, "0123456789abcdefghijklmnopqrstuvwxyzABCDEFG", "kQ9" * 14 + "z"]
out["checksums"] = [{"body": b, "checksum": security._token_checksum(b)} for b in bodies]
generated = [security.generate_api_token() for _ in range(5)]
out["generated_tokens"] = [{"token": t, "hash": h, "prefix": p} for t, h, p in generated]
good = generated[0][0]
body = good.split("_")[1]
shape = [
    good, good[:-1] + ("A" if good[-1] != "A" else "B"),
    "kiwi_" + body + "_" + "1234567", "kiwi_" + body[:-1] + "_abc", "kiwi_" + body + "_",
    "other_" + body + "_" + security._token_checksum(body), "legacy-token-123",
    "kiwi_" + body + "_" + security._token_checksum(body) + "\n", "",
]
out["checksum_verify"] = [{"token": t, "result": security.verify_token_checksum(t)} for t in shape]
out["sha256"] = [{"in": s, "hex": security.hash_token(s)} for s in ["", "abc", good, "été"]]
out["fingerprint"] = [{"in": s, "fp": security.password_fingerprint(s)}
                      for s in ["$argon2id$v=19$m=65536,t=3,p=4$abc$def", "user@example.com"]]

# --- scopes --------------------------------------------------------------
out["scope_catalog_json"] = json.dumps(scopes.catalog(), ensure_ascii=False, separators=(",", ":"))
masks = [0, 1, 4, 5, 65536, 131071, 131072, 131073, -1, 2**40]
out["scope_masks"] = [{"mask": m, "valid": scopes.is_valid_mask(m), "decode": scopes.decode(m) if m >= 0 else None,
                       "grants": {s.key: scopes.mask_grants(m, s.key) for s in scopes._REGISTRY} | {
                           "nope:read": scopes.mask_grants(m, "nope:read")}}
                      for m in masks]

# --- IP allowlist hashing ------------------------------------------------
ips = ["1.2.3.4", " 10.0.0.1 ", "0:0:0:0:0:0:0:1", "2001:db8:0:0:1:0:0:1",
       "2001:0db8:0000:0000:0000:ff00:0042:8329", "1:0:0:0:0:0:0:8", "::", "1:2:3:4:5:6:7:0",
       "fe80::1:0:0:0", "::1.2.3.4", "0:0:0:0:0:1:2:3", "1.2.3.4/32", "01.2.3.4", "not-an-ip", "",
       "FE80::ABCD", "0:0:0:0:0:ffff:102:304"]
norm = []
for ip in ips:
    try:
        norm.append({"in": ip, "out": ip_hash.normalize_ip(ip)})
    except ValueError:
        norm.append({"in": ip, "out": None})
out["normalize_ip"] = norm
salt_s = "c2FsdHlzYWx0eXNhbHR5cw"
out["ip_hash"] = [{"salt": salt_s, "ip": ip, "hex": ip_hash.hash_ip(salt_s, ip)} for ip in ["1.2.3.4", "::1"]]
allowed = [ip_hash.hash_ip(salt_s, "1.2.3.4")]
out["ip_allowed"] = [{"client": c, "salt": s, "allowed": allowed, "ok": ip_hash.ip_allowed(c, s, allowed)}
                     for c, s in [("1.2.3.4", salt_s), (" 1.2.3.4", salt_s), ("1.2.3.5", salt_s),
                                  ("1.2.3.4", None), ("", salt_s), ("junk", salt_s)]]

# --- device label --------------------------------------------------------
uas = [
    None, "",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Safari/537.36 Edg/130.0",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15",
    "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/120 Mobile",
    "Mozilla/5.0 (X11; Linux x86_64; rv:120.0) Gecko/20100101 Firefox/120.0",
    "Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36 Chrome/120 Mobile Safari/537.36 OPR/80",
    "curl/8.4.0", "BetterTroveTools/2.0 (Windows)",
]
out["device_label"] = [{"ua": u, "label": device_label(u)} for u in uas]

# --- bearer parsing ------------------------------------------------------
def bearer(value):
    scheme, cred = get_authorization_scheme_param(value)
    if not (value and scheme and cred):
        return None
    if scheme.lower() != "bearer":
        return None
    return cred


headers = ["Bearer abc", "bearer abc", "BEARER  abc  ", "Bearer", "Bearer ", "Basic abc", "Token abc",
           " Bearer abc", "Bearer a b", "Bearer\tabc", "Bearer \x0babc\x0c", "Bearer abc\x1f", ""]
out["bearer"] = [{"header": h, "cred": bearer(h)} for h in headers]

# --- cookies -------------------------------------------------------------
tokens = SiteTokenResponse(access_token="aaa.bbb.ccc", refresh_token="r-_efresh", expires_in=900)
cookie_cases = []
for app_url, api_url, debug in [("https://trove.aallyn.net", "https://api.aallyn.net", False),
                                ("http://localhost:8913", "http://localhost:8000", True),
                                ("https://aallyn.net", "https://api.aallyn.net", False)]:
    settings.app_url, settings.api_url, settings.debug = app_url, api_url, debug
    r = Response()
    cookies.set_session_cookies(r, tokens)
    set_headers = [v.decode("latin-1") for k, v in r.raw_headers if k == b"set-cookie"]
    r = Response()
    with mock.patch("time.time", return_value=1761136496.25):
        cookies.clear_session_cookies(r)
    clear_headers = [v.decode("latin-1") for k, v in r.raw_headers if k == b"set-cookie"]
    allowed_cases = []
    for origin, sfs in [(None, None), (None, "same-origin"), (None, "same-site"), ("https://trove.aallyn.net", None),
                        ("https://api.aallyn.net", None), ("https://docs.aallyn.net", "same-origin"),
                        ("http://localhost:8913", None), ("http://127.0.0.1:8913", None),
                        ("http://localhost:8000", None), ("", "same-origin")]:
        hdrs = []
        if origin is not None:
            hdrs.append((b"origin", origin.encode()))
        if sfs is not None:
            hdrs.append((b"sec-fetch-site", sfs.encode()))
        req = Request({"type": "http", "headers": hdrs, "method": "GET", "path": "/"})
        allowed_cases.append({"origin": origin, "sec_fetch_site": sfs, "ok": cookies.cookie_auth_allowed(req)})
    cookie_cases.append({"app_url": app_url, "api_url": api_url, "debug": debug, "set": set_headers,
                         "clear_now": 1761136496.25, "clear": clear_headers, "allowed": allowed_cases})
out["cookies"] = cookie_cases

# --- uvicorn proxy headers -----------------------------------------------
proxy_cases = []
for trusted, peer, xff in [("127.0.0.1", "127.0.0.1", "1.2.3.4"), ("127.0.0.1", "10.0.0.5", "1.2.3.4"),
                           ("127.0.0.1", "127.0.0.1", "9.9.9.9, 1.2.3.4"),
                           ("127.0.0.1,10.0.0.0/8", "10.1.1.1", "5.5.5.5, 10.2.2.2"),
                           ("127.0.0.1", "127.0.0.1", "127.0.0.1"), ("*", "8.8.8.8", "7.7.7.7, 6.6.6.6"),
                           ("127.0.0.1", "127.0.0.1", "1.2.3.4:5678"), ("127.0.0.1", "127.0.0.1", "[::1]:80"),
                           ("127.0.0.1", "127.0.0.1", ""), ("127.0.0.1", "127.0.0.1", None),
                           ("172.16.0.0/12", "172.18.0.3", "203.0.113.9")]:
    th = _TrustedHosts(trusted)
    host = peer
    if peer in th and xff is not None:
        h, _ = th.get_trusted_client_address(xff)
        if h:
            host = h
    proxy_cases.append({"trusted": trusted, "peer": peer, "xff": xff, "client": host})
out["client_ip"] = proxy_cases

# --- rate-limit header math ----------------------------------------------
rl = []
for now, window, limit, count, later in [(1761136496.123456, 60, 120, 5, 0.0021),
                                         (1761136496.999999, 60, 30, 31, 0.0005),
                                         (1761136400.0, 3600, 5, 6, 0.25),
                                         (1761136496.5, 1, 1, 2, 0.9)]:
    reset = datetime.fromtimestamp(now + window, tz=timezone.utc)
    info = ratelimit.RateLimitInfo(limit=limit, remaining=max(0, limit - count), reset=reset)
    now2 = datetime.fromtimestamp(now + later, tz=timezone.utc)
    retry = max(1, int((info.reset - now2).total_seconds()))
    rl.append({"now": now, "window": window, "limit": limit, "count": count, "later": later,
               "headers": ratelimit.rate_limit_headers(info), "retry_after": str(retry),
               "member": f"{now:.6f}:{7}", "now_str": str(now)})
out["ratelimit_redis"] = rl
mongo_rl = []
for now_ts, window in [(1761136496.123456, 60), (1761136499.999, 3600), (1761136440.0, 300)]:
    now = datetime.fromtimestamp(now_ts, tz=timezone.utc)
    window_start = int(now.timestamp() // window) * window
    reset = datetime.fromtimestamp(window_start + window, tz=timezone.utc)
    mongo_rl.append({"now": now_ts, "window": window, "bucket_suffix": str(window_start),
                     "reset": int(reset.timestamp()),
                     "retry_after": max(1, int((reset - now).total_seconds()))})
out["ratelimit_mongo"] = mongo_rl
out["lua_sliding_window"] = ratelimit._SLIDING_WINDOW_LUA
out["lua_token_flush"] = token_usage._FLUSH_LUA

# --- usage ---------------------------------------------------------------
out["round2"] = [{"in": x, "out": round(x, 2)} for x in
                 [0.125, 0.375, 2.675, 1.005, 12.3456, 0.0, 1e-7, 99999.995, 3.14159, 0.005, 0.015]]

with open(HERE / "golden.json", "w", encoding="utf-8") as fh:
    json.dump(out, fh, indent=1, ensure_ascii=True)
    fh.write("\n")
print("wrote", HERE / "golden.json")
