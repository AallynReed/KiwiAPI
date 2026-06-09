from datetime import timedelta

import jwt
import pytest

from app.core.security import (
    create_access_token,
    create_email_token,
    decode_access_token,
    decode_email_token,
    generate_api_token,
    generate_refresh_token,
    hash_password,
    hash_token,
    password_fingerprint,
    verify_password,
    verify_token_checksum,
)


def test_password_hash_roundtrip():
    h = hash_password("correct horse battery staple")
    assert h != "correct horse battery staple"
    assert verify_password("correct horse battery staple", h)
    assert not verify_password("wrong", h)


def test_api_token_generation():
    full, hashed, prefix = generate_api_token()
    assert full.startswith("kiwi_")
    assert hash_token(full) == hashed
    assert full.startswith(prefix)
    other, _, _ = generate_api_token()
    assert other != full  # unique


def test_api_token_has_three_segments():
    full, _, _ = generate_api_token()
    parts = full.split("_")
    assert len(parts) == 3  # prefix, body, checksum
    assert parts[0] == "kiwi"


def test_token_checksum_validates_generated_tokens():
    full, _, _ = generate_api_token()
    assert verify_token_checksum(full) is True


def test_token_checksum_rejects_tampered_token():
    full, _, _ = generate_api_token()
    prefix, body, crc = full.split("_")
    # Flip a character in the body - the checksum no longer matches.
    flipped = body[:-1] + ("A" if body[-1] != "A" else "B")
    assert verify_token_checksum(f"{prefix}_{flipped}_{crc}") is False


def test_token_checksum_unknown_shape_is_none():
    # Anything not in <prefix>_<body>_<crc> shape -> None, so the caller falls
    # back to a database lookup rather than rejecting it offline.
    assert verify_token_checksum("kiwi_legacytokenwithnochecksum") is None
    assert verify_token_checksum("something-else-entirely") is None


def test_access_token_roundtrip():
    payload = decode_access_token(create_access_token("user123", 3, session_id="sess1"))
    assert payload["sub"] == "user123"
    assert payload["type"] == "access"
    assert payload["ver"] == 3
    assert payload["sid"] == "sess1"


def test_access_token_expired():
    token = create_access_token("u", 0, expires_delta=timedelta(seconds=-1))
    with pytest.raises(jwt.ExpiredSignatureError):
        decode_access_token(token)


def test_refresh_token_generation():
    raw, hashed = generate_refresh_token()
    assert hash_token(raw) == hashed
    other, _ = generate_refresh_token()
    assert other != raw


def test_email_token_purpose_enforced():
    token = create_email_token("u", "email_verify", timedelta(hours=1))
    assert decode_email_token(token, "email_verify")["sub"] == "u"
    with pytest.raises(jwt.InvalidTokenError):
        decode_email_token(token, "password_reset")


def test_password_fingerprint_tracks_hash():
    h1 = hash_password("a-very-long-password")
    h2 = hash_password("a-very-long-password")  # different argon2 salt
    assert password_fingerprint(h1) != password_fingerprint(h2)
    assert password_fingerprint(h1) == password_fingerprint(h1)
