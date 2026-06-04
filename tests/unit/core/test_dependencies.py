import pytest

from app.core.dependencies import _ip_allowed, get_current_superuser, require_scope
from app.core.errors import APIError


def test_ip_allowed_exact_and_cidr():
    assert _ip_allowed("1.2.3.4", ["1.2.3.4"])
    assert _ip_allowed("1.2.3.4", ["1.2.3.0/24"])
    assert not _ip_allowed("9.9.9.9", ["1.2.3.0/24"])
    assert not _ip_allowed("1.2.3.4", ["bogus"])
    assert not _ip_allowed("not-an-ip", ["1.2.3.4"])


class _Token:
    def __init__(self, mask):
        self.scopes = mask


class _Ctx:
    def __init__(self, mask):
        self.token = _Token(mask)
        self.user = None


async def test_require_scope_all_mask_grants_anything():
    # The "all" mask (0) grants every scope, present or future.
    dep = require_scope("widgets:read")
    assert (await dep(_Ctx(0))).token.scopes == 0


async def test_require_scope_denies_when_not_granted():
    # A non-zero mask that doesn't carry the scope is rejected with 403.
    dep = require_scope("widgets:read")
    with pytest.raises(APIError) as e:
        await dep(_Ctx(1))
    assert e.value.code == "insufficient_scope"
    assert e.value.status_code == 403


class _User:
    def __init__(self, su):
        self.is_superuser = su


async def test_require_superuser():
    assert (await get_current_superuser(_User(True))).is_superuser
    with pytest.raises(APIError) as e:
        await get_current_superuser(_User(False))
    assert e.value.status_code == 403 and e.value.code == "forbidden"
