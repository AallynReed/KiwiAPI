import pytest

import app.core.lockout as lockout
import app.core.passwords as passwords
from app.core.config import settings
from app.core.errors import APIError

# --- HIBP gate (mocked count, no network) ---

async def test_password_check_disabled(monkeypatch):
    monkeypatch.setattr(settings, "password_breach_check", False)
    await passwords.ensure_password_not_breached("anything")  # no-op


async def test_password_breached_raises(monkeypatch):
    monkeypatch.setattr(settings, "password_breach_check", True)

    async def fake_count(_):
        return 42

    monkeypatch.setattr(passwords, "password_breach_count", fake_count)
    with pytest.raises(APIError) as e:
        await passwords.ensure_password_not_breached("password")
    assert e.value.code == "password_breached"
    assert e.value.details["breach_count"] == 42


async def test_password_ok(monkeypatch):
    monkeypatch.setattr(settings, "password_breach_check", True)

    async def fake_count(_):
        return 0

    monkeypatch.setattr(passwords, "password_breach_count", fake_count)
    await passwords.ensure_password_not_breached("good-and-unique")  # no raise


# --- Lockout gracefully no-ops without Redis ---

async def test_lockout_noop_without_redis(monkeypatch):
    monkeypatch.setattr(lockout, "get_redis", lambda: None)
    assert await lockout.lock_ttl("a@b.com") == 0
    await lockout.record_failure("a@b.com")
    await lockout.clear("a@b.com")
