import pytest
from fastapi import BackgroundTasks

from app.core.errors import APIError
from app.tokens.router import _normalize_ips, create_token
from app.tokens.schemas import CreateTokenRequest


def test_normalize_ips_ok():
    assert _normalize_ips(["1.2.3.4", "10.0.0.0/8"]) == ["1.2.3.4", "10.0.0.0/8"]


def test_normalize_ips_rejects_invalid():
    with pytest.raises(APIError):
        _normalize_ips(["nope"])


def test_normalize_ips_requires_at_least_one():
    with pytest.raises(APIError):
        _normalize_ips([])


class _UnverifiedUser:
    is_verified = False
    id = "x"


async def test_create_token_blocks_unverified_before_db():
    # The verified-email gate must raise before any DB access.
    payload = CreateTokenRequest(name="t", scopes=0, allowed_ips=["1.2.3.4"])
    with pytest.raises(APIError) as e:
        await create_token(payload, BackgroundTasks(), _UnverifiedUser())
    assert e.value.status_code == 403 and e.value.code == "email_unverified"
