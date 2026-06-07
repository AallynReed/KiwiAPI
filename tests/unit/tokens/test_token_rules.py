import pytest
from fastapi import BackgroundTasks

from app.core.errors import APIError
from app.core.ip_hash import hash_ip, ip_allowed, make_ip_salt, normalize_ip
from app.tokens.router import _hash_pinned_ips, create_token
from app.tokens.schemas import CreateTokenRequest


# --- _hash_pinned_ips ------------------------------------------------------


def test_hash_pinned_ips_round_trip():
    salt = make_ip_salt()
    hashes = _hash_pinned_ips(["1.2.3.4", "10.0.0.1"], salt)
    assert len(hashes) == 2
    # Hashes are stable + per-IP (re-running with the same salt is deterministic).
    assert _hash_pinned_ips(["1.2.3.4"], salt) == [hashes[0]]


def test_hash_pinned_ips_uses_salt_so_same_ip_different_salt_different_hash():
    salt_a, salt_b = make_ip_salt(), make_ip_salt()
    assert _hash_pinned_ips(["1.2.3.4"], salt_a) != _hash_pinned_ips(["1.2.3.4"], salt_b)


def test_hash_pinned_ips_dedupes_canonical_form():
    salt = make_ip_salt()
    # IPv6 has multiple textual forms — canonicalization happens before hashing,
    # so these are stored as one entry.
    hashes = _hash_pinned_ips(["::1", "0:0:0:0:0:0:0:1"], salt)
    assert len(hashes) == 1


def test_hash_pinned_ips_rejects_invalid():
    salt = make_ip_salt()
    with pytest.raises(APIError):
        _hash_pinned_ips(["nope"], salt)


def test_hash_pinned_ips_rejects_cidrs():
    # CIDRs were supported before — explicitly removed because hashes can't
    # range-match a single IP. The error message points the user there.
    salt = make_ip_salt()
    with pytest.raises(APIError) as e:
        _hash_pinned_ips(["10.0.0.0/8"], salt)
    assert "CIDR" in e.value.message


def test_hash_pinned_ips_accepts_empty():
    # Empty list = opt-out of IP restriction. Not an error.
    salt = make_ip_salt()
    assert _hash_pinned_ips([], salt) == []
    assert _hash_pinned_ips(["", "  "], salt) == []


# --- ip_hash helpers -------------------------------------------------------


def test_normalize_ip_canonicalizes():
    assert normalize_ip("::1") == "::1"
    assert normalize_ip("0:0:0:0:0:0:0:1") == "::1"  # same address, different text
    assert normalize_ip(" 192.168.1.1 ") == "192.168.1.1"


def test_normalize_ip_rejects_cidr_with_helpful_message():
    with pytest.raises(ValueError) as e:
        normalize_ip("10.0.0.0/8")
    assert "CIDR" in str(e.value)


def test_ip_allowed_matches_only_with_correct_salt():
    salt = make_ip_salt()
    hashes = [hash_ip(salt, "1.2.3.4"), hash_ip(salt, "5.6.7.8")]
    assert ip_allowed("1.2.3.4", salt, hashes) is True
    assert ip_allowed("5.6.7.8", salt, hashes) is True
    assert ip_allowed("9.9.9.9", salt, hashes) is False
    # Wrong salt → never matches.
    assert ip_allowed("1.2.3.4", make_ip_salt(), hashes) is False


def test_ip_allowed_falsy_inputs_short_circuit():
    salt = make_ip_salt()
    hashes = [hash_ip(salt, "1.2.3.4")]
    assert ip_allowed("", salt, hashes) is False
    assert ip_allowed("1.2.3.4", None, hashes) is False
    assert ip_allowed("not an ip", salt, hashes) is False


# --- Schema-side ----------------------------------------------------------


def test_create_token_schema_accepts_no_ips():
    # Schema default is an empty list; payloads without allowed_ips must validate.
    payload = CreateTokenRequest(name="t", scopes=0)
    assert payload.allowed_ips == []


# --- create_token gate ----------------------------------------------------


class _UnverifiedUser:
    is_verified = False
    id = "x"


async def test_create_token_blocks_unverified_before_db():
    # The verified-email gate must raise before any DB access.
    payload = CreateTokenRequest(name="t", scopes=0, allowed_ips=["1.2.3.4"])
    with pytest.raises(APIError) as e:
        await create_token(payload, BackgroundTasks(), _UnverifiedUser())
    assert e.value.status_code == 403 and e.value.code == "email_unverified"
