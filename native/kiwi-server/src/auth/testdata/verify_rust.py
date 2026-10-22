"""Check that artifacts minted by the Rust auth code are accepted (and, for
JWTs, re-encoded byte for byte) by the Python code. Called by the Rust test
`rust_artifacts_verify_in_python` with the path of a JSON file."""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[5]))

import jwt  # noqa: E402
from argon2 import PasswordHasher  # noqa: E402

from app.core import ip_hash, security  # noqa: E402
from app.core.config import settings  # noqa: E402

a = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
settings.secret_key = a["secret"]
settings.api_token_prefix = "kiwi"

assert PasswordHasher().verify(a["argon2"], a["password"])
assert not PasswordHasher().check_needs_rehash(a["argon2"]), "Rust hash uses non-default params"

access = security.decode_access_token(a["access"])
assert (access["sub"], access["ver"], access["type"], access["sid"]) == (
    "65f0a1b2c3d4e5f601234567", 4, "access", "6718a0b1c2d3e4f5a6b7c8d9"), access
assert access["exp"] - access["iat"] == 900
site = security.decode_access_token(a["site"])
assert (site["kind"], site["ver"], "sid" not in site) == ("site", 2, True), site
email = security.decode_email_token(a["email"], "email_change")
assert email["new"] == "üser@exämple.com", email
for key in ("access", "site", "email"):
    payload = jwt.decode(a[key], a["secret"], algorithms=["HS256"])
    assert jwt.encode(payload, a["secret"], algorithm="HS256") == a[key], key

assert security.verify_token_checksum(a["api_token"]) is True
assert security.hash_token(a["api_token"]) == a["api_hash"]
assert a["api_prefix"] == "kiwi_" + a["api_token"].split("_")[1][:8]
assert ip_hash.ip_allowed("203.0.113.7", a["ip_salt"], [a["ip_hash"]])
print("python accepted every Rust artifact")
