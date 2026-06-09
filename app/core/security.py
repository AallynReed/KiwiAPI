import hashlib
import re
import secrets
import string
import zlib
from datetime import timedelta
from functools import lru_cache

import jwt
from argon2 import PasswordHasher
from argon2.exceptions import Argon2Error

from app.core.config import settings
from app.core.utils import utcnow

_hasher = PasswordHasher()


# --- Passwords -------------------------------------------------------------

def hash_password(password: str) -> str:
    return _hasher.hash(password)


def verify_password(password: str, hashed: str) -> bool:
    try:
        return _hasher.verify(hashed, password)
    except Argon2Error:
        return False


# --- Session JWTs ----------------------------------------------------------

def create_access_token(
    subject: str,
    token_version: int,
    session_id: str | None = None,
    expires_delta: timedelta | None = None,
) -> str:
    now = utcnow()
    expire = now + (
        expires_delta or timedelta(minutes=settings.access_token_expire_minutes)
    )
    # `ver` is checked against the user's token_version so a logout-all /
    # password change instantly invalidates every outstanding access token.
    payload = {"sub": str(subject), "ver": token_version, "iat": now, "exp": expire, "type": "access"}
    if session_id is not None:
        payload["sid"] = session_id
    return jwt.encode(payload, settings.secret_key, algorithm=settings.jwt_algorithm)


def generate_refresh_token() -> tuple[str, str]:
    """Return (refresh_token, sha256_hash). Only the hash is stored."""
    raw = secrets.token_urlsafe(48)
    return raw, hash_token(raw)


def decode_access_token(token: str) -> dict:
    """Decode and validate a session JWT. Raises jwt.PyJWTError on failure."""
    return jwt.decode(token, settings.secret_key, algorithms=[settings.jwt_algorithm])


# --- API tokens ------------------------------------------------------------
#
# Format:  <prefix>_<body>_<checksum>
#   prefix   - settings.api_token_prefix (e.g. "kiwi")
#   body     - 43 base62 chars (~256 bits of entropy), the actual secret
#   checksum - base62 CRC32 of the body (<=6 chars), a self-validating tail
#
# The checksum lets us (a) reject malformed tokens offline, before any DB hit,
# and (b) publish a precise regex for secret-scanning tools (e.g. so a leaked
# token pushed to a public repo can be detected and auto-revoked). A token that
# matches the shape but fails its checksum is junk; one that doesn't match the
# shape at all is treated as "unknown" and validated the slow way (DB lookup),
# which keeps any pre-checksum legacy tokens working.

_TOKEN_ALPHABET = string.ascii_letters + string.digits  # base62 - no "_" or "-"
_TOKEN_BODY_LEN = 43
_TOKEN_CHECKSUM_MAX = 6  # base62 of a 32-bit CRC fits in 6 chars


def _b62_encode(n: int) -> str:
    if n == 0:
        return _TOKEN_ALPHABET[0]
    base = len(_TOKEN_ALPHABET)
    out: list[str] = []
    while n:
        n, rem = divmod(n, base)
        out.append(_TOKEN_ALPHABET[rem])
    return "".join(reversed(out))


def _token_checksum(body: str) -> str:
    return _b62_encode(zlib.crc32(body.encode("ascii")))


@lru_cache(maxsize=1)
def _token_re() -> re.Pattern[str]:
    prefix = re.escape(settings.api_token_prefix)
    return re.compile(
        rf"^{prefix}_([A-Za-z0-9]{{{_TOKEN_BODY_LEN}}})_([A-Za-z0-9]{{1,{_TOKEN_CHECKSUM_MAX}}})$"
    )


def generate_api_token() -> tuple[str, str, str]:
    """Return (full_token, sha256_hash, display_prefix).

    The full token is shown to the user exactly once; only its hash is stored.
    The display prefix lets the user recognize a token in listings.
    """
    prefix = settings.api_token_prefix
    body = "".join(secrets.choice(_TOKEN_ALPHABET) for _ in range(_TOKEN_BODY_LEN))
    full = f"{prefix}_{body}_{_token_checksum(body)}"
    return full, hash_token(full), f"{prefix}_{body[:8]}"


def verify_token_checksum(token: str) -> bool | None:
    """Offline validity check for the token format.

    Returns ``True`` if the checksum matches, ``False`` if the token has our
    exact shape but a bad checksum (reject it without touching the database), or
    ``None`` when the token isn't in the new shape at all - a legacy/unknown
    token that must be validated against the database.
    """
    m = _token_re().match(token)
    if m is None:
        return None
    body, crc = m.group(1), m.group(2)
    return _token_checksum(body) == crc


def hash_token(token: str) -> str:
    return hashlib.sha256(token.encode("utf-8")).hexdigest()


# --- Purpose-scoped email links (verification, password reset) -------------

def password_fingerprint(hashed_password: str) -> str:
    """Short fingerprint of a password hash, embedded in reset tokens so they
    become single-use: once the password changes, old reset links stop working."""
    return hashlib.sha256(hashed_password.encode("utf-8")).hexdigest()[:16]


def create_email_token(
    subject: str,
    purpose: str,
    expires_delta: timedelta,
    extra: dict | None = None,
) -> str:
    now = utcnow()
    payload = {"sub": str(subject), "type": purpose, "iat": now, "exp": now + expires_delta}
    if extra:
        payload.update(extra)
    return jwt.encode(payload, settings.secret_key, algorithm=settings.jwt_algorithm)


def decode_email_token(token: str, expected_purpose: str) -> dict:
    """Decode a purpose-scoped email token. Raises jwt.PyJWTError if invalid,
    expired, or the wrong purpose."""
    payload = jwt.decode(token, settings.secret_key, algorithms=[settings.jwt_algorithm])
    if payload.get("type") != expected_purpose:
        raise jwt.InvalidTokenError("Unexpected token purpose")
    return payload
