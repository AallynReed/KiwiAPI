import pytest

pytestmark = pytest.mark.integration


async def _signup_login(client, email):
    r = await client.post("/auth/signup", json={"email": email, "password": "longpassword1"})
    assert r.status_code == 201, r.text
    r = await client.post("/auth/login", json={"email": email, "password": "longpassword1"})
    assert r.status_code == 200, r.text
    return {"Authorization": f"Bearer {r.json()['access_token']}"}


async def test_signup_login_me(client):
    h = await _signup_login(client, "a@b.com")
    r = await client.get("/auth/me", headers=h)
    assert r.status_code == 200
    assert r.json()["email"] == "a@b.com"


async def test_duplicate_email_conflict(client):
    await _signup_login(client, "dup@b.com")
    r = await client.post("/auth/signup", json={"email": "dup@b.com", "password": "longpassword1"})
    assert r.status_code == 409
    assert r.json()["error"]["code"] == "email_taken"


async def test_token_lifecycle(client):
    h = await _signup_login(client, "c@d.com")

    # The base ships no scopes, so tokens use the "all" mask (0).
    r = await client.post("/tokens", headers=h, json={"name": "laptop", "scopes": 0, "allowed_ips": ["1.2.3.4"]})
    assert r.status_code == 201, r.text
    body = r.json()
    assert body["token"].startswith("kiwi_")
    assert body["scopes"] == 0 and body["scope_names"] == []
    tid = body["id"]

    r = await client.get("/tokens", headers=h)
    assert r.status_code == 200 and len(r.json()) == 1

    r = await client.patch(f"/tokens/{tid}", headers=h, json={"name": "desktop", "allowed_ips": ["5.6.7.8"]})
    assert r.status_code == 200 and r.json()["name"] == "desktop"

    # Revoke requires a reason.
    r = await client.post(f"/tokens/{tid}/revoke", headers=h, json={"reason": "No longer using it"})
    assert r.status_code == 200 and r.json()["revoked"] is True
    assert r.json()["revoke_reason"] == "No longer using it"


async def test_session_refresh_and_logout(client):
    await client.post("/auth/signup", json={"email": "s@s.com", "password": "longpassword1"})
    tokens = (
        await client.post("/auth/login", json={"email": "s@s.com", "password": "longpassword1"})
    ).json()
    assert "access_token" in tokens and "refresh_token" in tokens
    h = {"Authorization": f"Bearer {tokens['access_token']}"}

    sessions = (await client.get("/auth/sessions", headers=h)).json()
    assert len(sessions) == 1 and sessions[0]["current"] is True

    # Refresh rotates the token.
    new = (await client.post("/auth/refresh", json={"refresh_token": tokens["refresh_token"]})).json()
    assert new["refresh_token"] != tokens["refresh_token"]

    # Old refresh token is dead (single-use).
    assert (
        await client.post("/auth/refresh", json={"refresh_token": tokens["refresh_token"]})
    ).status_code == 401

    # Logout ends the session.
    assert (
        await client.post("/auth/logout", json={"refresh_token": new["refresh_token"]})
    ).status_code == 204
    assert (
        await client.post("/auth/refresh", json={"refresh_token": new["refresh_token"]})
    ).status_code == 401


async def test_logout_all_invalidates_access_tokens(client):
    await client.post("/auth/signup", json={"email": "la@s.com", "password": "longpassword1"})
    tokens = (
        await client.post("/auth/login", json={"email": "la@s.com", "password": "longpassword1"})
    ).json()
    h = {"Authorization": f"Bearer {tokens['access_token']}"}
    assert (await client.get("/auth/me", headers=h)).status_code == 200

    # logout-all bumps token_version, so the existing access token is now rejected.
    assert (await client.post("/auth/logout-all", headers=h)).status_code == 204
    assert (await client.get("/auth/me", headers=h)).status_code == 401


async def test_export_and_delete_account(client):
    h = await _signup_login(client, "del@b.com")
    await client.post("/tokens", headers=h, json={"name": "t", "scopes": 0, "allowed_ips": ["1.2.3.4"]})

    r = await client.get("/auth/me/export", headers=h)
    assert r.status_code == 200
    data = r.json()
    assert data["user"]["email"] == "del@b.com"
    assert len(data["api_tokens"]) == 1

    # Wrong confirmation email is refused.
    r = await client.post("/auth/delete-account", headers=h, json={"confirm_email": "no@b.com"})
    assert r.status_code == 400

    # Correct confirmation deletes the account.
    r = await client.post("/auth/delete-account", headers=h, json={"confirm_email": "del@b.com"})
    assert r.status_code == 204
    # The user is gone, so the access token no longer resolves.
    assert (await client.get("/auth/me", headers=h)).status_code == 401


async def test_token_rotation(client):
    h = await _signup_login(client, "rot@b.com")
    created = (
        await client.post(
            "/tokens", headers=h, json={"name": "k", "scopes": 0, "allowed_ips": ["1.2.3.4"]}
        )
    ).json()
    old_secret = created["token"]
    tid = created["id"]

    r = await client.post(f"/tokens/{tid}/rotate", headers=h)
    assert r.status_code == 200, r.text
    body = r.json()
    assert body["token"] != old_secret          # a brand-new secret
    assert body["token"].startswith("kiwi_")
    assert body["rotated_at"] is not None
    assert body["scopes"] == 0                   # scopes preserved across rotation

    # The single token record now reflects the rotated prefix + timestamp.
    listed = (await client.get("/tokens", headers=h)).json()
    assert len(listed) == 1
    assert listed[0]["prefix"] == body["prefix"] and listed[0]["rotated_at"] is not None


async def test_admin_events_pagination(client):
    from beanie import PydanticObjectId

    from app.auth.models import User
    from app.usage.models import UsageEvent

    h = await _signup_login(client, "adm@b.com")
    u = await User.find_one(User.email == "adm@b.com")
    u.is_superuser = True
    await u.save()

    # Seed raw events directly so the test doesn't race the async recorder.
    tid = PydanticObjectId()
    for i in range(5):
        await UsageEvent(
            user_id=u.id,
            token_id=tid,
            method="GET",
            route="/v1/example",
            path="/v1/example",
            status_code=429 if i == 0 else 200,
            duration_ms=1.0,
        ).insert()

    page = (await client.get("/admin/events?limit=2", headers=h)).json()
    assert len(page["items"]) == 2 and page["has_more"] is True and page["next_cursor"]

    page2 = (
        await client.get(f"/admin/events?limit=2&cursor={page['next_cursor']}", headers=h)
    ).json()
    assert len(page2["items"]) == 2
    # Pages don't overlap.
    assert {e["id"] for e in page["items"]}.isdisjoint({e["id"] for e in page2["items"]})

    filtered = (await client.get("/admin/events?status_code=429", headers=h)).json()
    assert filtered["items"] and all(e["status_code"] == 429 for e in filtered["items"])


async def test_trove_calendar_and_news(client):
    from app.core.utils import utcnow
    from app.trove.models import TroveNews

    h = await _signup_login(client, "trove@b.com")
    # A token with the trove:read scope (bit 1).
    tok = (
        await client.post(
            "/tokens", headers=h, json={"name": "t", "scopes": 1, "allowed_ips": ["1.2.3.4"]}
        )
    ).json()["token"]
    th = {"Authorization": f"Bearer {tok}"}

    # Calendar: server time + buffs + the three merchant timers.
    r = await client.get("/v1/trove/calendar", headers=th)
    assert r.status_code == 200, r.text
    cal = r.json()
    assert set(cal) == {"server_time", "daily", "weekly", "merchants"}
    assert set(cal["merchants"]) == {"corruxion", "fluxion"}
    assert isinstance(cal["merchants"]["corruxion"]["active"], bool)
    assert cal["server_time"]["trove_day"]  # a weekday name

    # News: seed a cached article directly, then fetch via the API.
    await TroveNews(
        url="https://trovegame.com/news/x", title="Big Patch", published_at=utcnow()
    ).insert()
    r2 = await client.get("/v1/trove/news?limit=10", headers=th)
    assert r2.status_code == 200
    body = r2.json()
    assert body["count"] >= 1
    assert any(i["url"] == "https://trovegame.com/news/x" for i in body["items"])


async def test_secret_scanning_auto_revokes(client, monkeypatch):
    from app.core.config import settings

    monkeypatch.setattr(settings, "github_secret_scanning_verify", False)  # skip sig in test

    h = await _signup_login(client, "leak@b.com")
    created = (
        await client.post(
            "/tokens", headers=h, json={"name": "k", "scopes": 0, "allowed_ips": ["1.2.3.4"]}
        )
    ).json()
    secret, tid = created["token"], created["id"]

    # Report the real token as leaked -> true_positive + auto-revoked.
    r = await client.post(
        "/secret-scanning/github", json=[{"token": secret, "type": "kiwi_api_token"}]
    )
    assert r.status_code == 200, r.text
    assert r.json()[0]["label"] == "true_positive"

    tokens = (await client.get("/tokens", headers=h)).json()
    revoked = next(t for t in tokens if t["id"] == tid)
    assert revoked["revoked"] is True
    assert revoked["revoke_reason"]  # records why it was revoked

    # An unknown token is a false_positive (and not an error).
    r2 = await client.post(
        "/secret-scanning/github", json=[{"token": "kiwi_nope", "type": "x"}]
    )
    assert r2.json()[0]["label"] == "false_positive"


async def test_disposable_email_rejected(client):
    r = await client.post("/auth/signup", json={"email": "x@mailinator.com", "password": "longpassword1"})
    assert r.status_code == 400
    assert r.json()["error"]["code"] == "disposable_email"


async def test_verified_login_gate(client):
    from app.core.config import settings

    await client.post("/auth/signup", json={"email": "v@b.com", "password": "longpassword1"})
    settings.require_verified_for_login = True
    try:
        r = await client.post("/auth/login", json={"email": "v@b.com", "password": "longpassword1"})
        assert r.status_code == 403
        assert r.json()["error"]["code"] == "email_unverified"
    finally:
        settings.require_verified_for_login = False


async def test_account_lockout(client):
    await client.post("/auth/signup", json={"email": "lock@b.com", "password": "longpassword1"})
    for _ in range(5):
        r = await client.post("/auth/login", json={"email": "lock@b.com", "password": "wrong-pass"})
        assert r.status_code == 401
    # Locked now — even the correct password is refused.
    r = await client.post("/auth/login", json={"email": "lock@b.com", "password": "longpassword1"})
    assert r.status_code == 429
    assert r.json()["error"]["code"] == "account_locked"


async def test_token_creation_capped_per_day(client):
    h = await _signup_login(client, "g@h.com")
    body = {"name": "t", "scopes": 0, "allowed_ips": ["1.2.3.4"]}
    for _ in range(3):
        assert (await client.post("/tokens", headers=h, json=body)).status_code == 201
    # 4th in the same day -> 429
    r = await client.post("/tokens", headers=h, json=body)
    assert r.status_code == 429
    assert r.json()["error"]["code"] == "rate_limited"
