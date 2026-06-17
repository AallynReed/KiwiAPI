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


async def test_rotations_and_feeds(client):
    from app.core.utils import utcnow
    from app.trove.models import TroveNews

    h = await _signup_login(client, "trove@b.com")
    # Token with both scopes: rotations:read (1) | feeds:read (2) = 3.
    tok = (
        await client.post(
            "/tokens", headers=h, json={"name": "t", "scopes": 3, "allowed_ips": ["1.2.3.4"]}
        )
    ).json()["token"]
    th = {"Authorization": f"Bearer {tok}"}

    st = (await client.get("/v1/rotations/server-time", headers=th)).json()
    assert st["trove_day"] and st["now_unix"] and st["daily_reset_at"]

    daily = (await client.get("/v1/rotations/daily-buffs", headers=th)).json()
    assert daily["current"]["name"] and len(daily["week"]) == 7

    corr = (await client.get("/v1/rotations/corruxion", headers=th)).json()
    assert isinstance(corr["active"], bool) and len(corr["schedule"]) == 8

    flux = (await client.get("/v1/rotations/fluxion", headers=th)).json()
    assert flux["state"] in {"voting", "selling", "away"}

    biome = (await client.get("/v1/rotations/biomes", headers=th)).json()
    assert biome["current"]["biomes"] and "icon" in biome["current"]["biomes"][0]

    # Feeds: seed a cached article directly, then fetch via the API.
    await TroveNews(
        url="https://trovegame.com/news/x", title="Big Patch", published_at=utcnow()
    ).insert()
    news = (await client.get("/v1/feeds/news?limit=10", headers=th)).json()
    assert news["count"] >= 1
    assert any(i["url"] == "https://trovegame.com/news/x" for i in news["items"])


async def test_btt_releases(client):
    from datetime import timedelta

    from app.core.utils import utcnow
    from app.trove.models import BttRelease

    now = utcnow()

    def asset(name: str) -> dict:
        return {"name": name, "url": f"https://dl/{name}", "size": 1,
                "content_type": None, "download_count": 0}

    # v3 release: android + linux only (no windows -> windows must walk back).
    await BttRelease(release_id=3, tag_name="v3", name="3", html_url="https://gh/3",
                     prerelease=False, published_at=now,
                     assets=[asset("BTT-3.apk"), asset("BTT-3.deb")]).insert()
    # v2 release: windows (msi + exe).
    await BttRelease(release_id=2, tag_name="v2", name="2", html_url="https://gh/2",
                     prerelease=False, published_at=now - timedelta(days=1),
                     assets=[asset("BTT-2.msi"), asset("BTT-2.exe")]).insert()
    # v9 BETA prerelease, all platforms.
    await BttRelease(release_id=9, tag_name="v9-beta", name="beta", html_url="https://gh/9",
                     prerelease=True, published_at=now,
                     assets=[asset("BTT-9.msi"), asset("BTT-9.AppImage"), asset("BTT-9.apk")]).insert()

    # BTT endpoints are PUBLIC (no token) - the desktop app polls without auth.
    # List with channel filters.
    rel = (await client.get("/v1/btt/releases?channel=release")).json()
    assert rel["total"] == 2 and rel["items"][0]["tag_name"] == "v3"
    beta = (await client.get("/v1/btt/releases?channel=beta")).json()
    assert beta["total"] == 1 and beta["items"][0]["tag_name"] == "v9-beta"

    # Public response carries rate-limit headers (the per-IP budget).
    raw = await client.get("/v1/btt/latest?channel=release")
    assert raw.status_code == 200 and "X-RateLimit-Limit" in raw.headers
    latest = raw.json()
    plats = latest["platforms"]
    # Latest per platform on the release channel - Windows MUST walk back to v2.
    assert plats["windows"]["release"]["tag_name"] == "v2"      # walked back
    assert plats["windows"]["assets"][0]["name"] == "BTT-2.msi"  # msi prioritized over exe
    assert plats["linux"]["release"]["tag_name"] == "v3"        # newest with .deb
    assert plats["android"]["release"]["tag_name"] == "v3"

    # On beta, every platform resolves to v9-beta.
    beta_latest = (await client.get("/v1/btt/latest?channel=beta")).json()
    assert all(p and p["release"]["tag_name"] == "v9-beta"
               for p in beta_latest["platforms"].values())

    # Single-platform endpoint.
    win = (await client.get("/v1/btt/latest/windows?channel=release")).json()
    assert win["release"]["tag_name"] == "v2" and win["assets"][0]["name"] == "BTT-2.msi"

    # Errors: unknown platform (404), bad channel (400), no asset for platform.
    assert (await client.get("/v1/btt/latest/macos")).status_code == 404
    assert (await client.get("/v1/btt/latest?channel=nope")).status_code == 400

    # /check - server-side version comparison so the client just reads a bool.
    # Windows latest on release channel is v2 (walked back from v3).
    older = (await client.get(
        "/v1/btt/check?installed=v1.0.0&platform=windows&channel=release"
    )).json()
    assert older["update_available"] is True and older["comparable"] is True
    assert older["latest"]["release"]["tag_name"] == "v2"

    same = (await client.get(
        "/v1/btt/check?installed=v2&platform=windows&channel=release"
    )).json()
    assert same["update_available"] is False  # already on v2

    newer = (await client.get(
        "/v1/btt/check?installed=v9.9.9&platform=windows&channel=release"
    )).json()
    assert newer["update_available"] is False  # client is ahead of the channel

    # Unparseable installed -> comparable=false, update_available=false.
    nightly = (await client.get(
        "/v1/btt/check?installed=nightly&platform=windows&channel=release"
    )).json()
    assert nightly["comparable"] is False and nightly["update_available"] is False
    assert nightly["latest"]["release"]["tag_name"] == "v2"  # still served as info

    # Errors on /check: bad platform (404), bad channel (400), empty installed (422).
    assert (await client.get("/v1/btt/check?installed=v1&platform=macos")).status_code == 404
    assert (await client.get("/v1/btt/check?installed=v1&platform=windows&channel=nope")).status_code == 400
    assert (await client.get("/v1/btt/check?installed=&platform=windows")).status_code == 422

    # A fresh DB with no releases at all -> a platform-specific 404, not a crash.
    await BttRelease.find().delete()
    assert (await client.get("/v1/btt/latest/windows?channel=release")).status_code == 404
    # /check on empty DB: returns latest=null + update_available=false (don't 404 here:
    # the client should still get a clean answer that says "no update for you").
    empty = (await client.get(
        "/v1/btt/check?installed=v1&platform=windows&channel=release"
    )).json()
    assert empty["latest"] is None and empty["update_available"] is False


async def test_btt_changelog(client):
    from app.core.config import settings
    from app.core.utils import utcnow
    from app.trove.models import BttChangelog

    # Seed two groups: an "Unreleased" with one conventional commit + a tagged
    # release with two. Slicing knobs trim what comes back.
    groups = [
        {"version": "Unreleased", "commits": [
            {"sha": "a"*40, "short_sha": "a"*7, "message": "feat: new thing",
             "type": "feat", "url": "https://gh/a"},
        ]},
        {"version": "v1.0.0", "commits": [
            {"sha": "b"*40, "short_sha": "b"*7, "message": "fix(x): bug",
             "type": "fix", "url": "https://gh/b"},
            {"sha": "c"*40, "short_sha": "c"*7, "message": "initial",
             "type": None, "url": "https://gh/c"},
        ]},
    ]
    await BttChangelog(repo=settings.btt_releases_repo, groups=groups,
                       rate_limited=False, fetched_at=utcnow()).insert()

    # Public - no token required.
    r = await client.get("/v1/btt/changelog")
    assert r.status_code == 200
    body = r.json()
    assert body["repo"] == settings.btt_releases_repo
    assert body["rate_limited"] is False
    assert [g["version"] for g in body["groups"]] == ["Unreleased", "v1.0.0"]
    assert body["groups"][0]["commits"][0]["type"] == "feat"
    assert body["groups"][1]["commits"][1]["type"] is None

    # Slicing knobs.
    one_group = (await client.get("/v1/btt/changelog?limit_groups=1")).json()
    assert [g["version"] for g in one_group["groups"]] == ["Unreleased"]
    one_per = (await client.get("/v1/btt/changelog?commits_per_group=1")).json()
    assert all(len(g["commits"]) == 1 for g in one_per["groups"])

    # Empty-DB case: returns an empty payload (not 404) so the client renders "loading".
    await BttChangelog.find().delete()
    empty = (await client.get("/v1/btt/changelog")).json()
    assert empty["groups"] == [] and empty["rate_limited"] is False


async def test_stats_coefficient_calculator(client):
    # Tokenless stateless calculator: the in-game Coefficient from damage + crit.
    r = await client.post("/v1/stats/coefficient", json={
        "physical_damage": 799894, "magic_damage": 14300, "critical_damage": 3438.3,
    })
    assert r.status_code == 200, r.text
    assert "X-RateLimit-Limit" in r.headers          # public -> per-IP budget headers
    body = r.json()
    assert body["coefficient"] == 28302649 and body["damage_used"] == "physical"

    # Higher of physical/magic is used (mage build).
    mage = (await client.post("/v1/stats/coefficient", json={
        "physical_damage": 1000, "magic_damage": 500000, "critical_damage": 1000,
    })).json()
    assert mage["coefficient"] == 5500000 and mage["damage_used"] == "magic"

    # Needs at least one damage stat -> 400.
    bad = await client.post("/v1/stats/coefficient", json={"critical_damage": 100})
    assert bad.status_code == 400


async def test_site_routes(client):
    # The BTT showcase site lives in `site/` and is mounted at /,
    # /documentation, /commands, /leaderboards, /updates, /support, with
    # assets at /static/*. The api container serves trove.aallyn.net out
    # of this.

    # Landing page renders the BTT index.html (we just check a known string is in it).
    r = await client.get("/")
    assert r.status_code == 200
    assert "Better Trove Tools" in r.text
    assert "download-dropdown" in r.text  # the new platform dropdown is wired in
    assert "hero-platforms" in r.text     # supported-platforms row is in the hero

    # Documentation page.
    r = await client.get("/documentation")
    assert r.status_code == 200 and "Trove Tools" in r.text

    # Static files (css) - content-type and a known rule should both be served.
    r = await client.get("/static/style.css")
    assert r.status_code == 200 and "btn-primary" in r.text

    # /unlock_debug and /unlock_fps were removed 2026-06 after Trove
    # shipped anti-cheat. Both routes should now 404 - any future
    # reintroduction should be a deliberate decision, not a silent
    # ride-along. Same for the deprecated POST handler.
    assert (await client.get("/unlock_debug")).status_code == 404
    assert (await client.get("/unlock_fps")).status_code == 404
    assert (await client.post("/unlock_debug")).status_code in (404, 405)

    # The old API landing card moved to /api-info and still renders.
    r = await client.get("/api-info")
    assert r.status_code == 200 and "Programmatic API" in r.text


async def test_news_live_and_history(client):
    from datetime import timedelta

    from app.core.utils import utcnow
    from app.trove.models import TroveNews

    now = utcnow()
    for i in range(3):
        await TroveNews(url=f"https://trovegame.com/news/{i}", title=f"Patch {i}",
                        published_at=now - timedelta(days=i)).insert()

    # Live feed is public + small, newest first.
    feed = (await client.get("/v1/feeds/news?limit=2")).json()
    assert feed["count"] == 2 and feed["items"][0]["title"] == "Patch 0"

    # Full archive lives under the misc scope (token-gated), paged with a total.
    h = await _signup_login(client, "newshist@b.com")
    tok = (await client.post(
        "/tokens", headers=h, json={"name": "m", "scopes": 16, "allowed_ips": ["1.2.3.4"]}
    )).json()["token"]
    mh = {"Authorization": f"Bearer {tok}"}
    p1 = (await client.get("/v1/misc/news-history?limit=2&offset=0", headers=mh)).json()
    assert p1["total"] == 3 and p1["count"] == 2 and p1["items"][0]["title"] == "Patch 0"
    p2 = (await client.get("/v1/misc/news-history?limit=2&offset=2", headers=mh)).json()
    assert p2["count"] == 1 and p2["total"] == 3 and p2["items"][0]["title"] == "Patch 2"

    # Not public - requires the misc scope.
    assert (await client.get("/v1/misc/news-history")).status_code == 401


async def test_chaos_chest(client):
    from app.core.utils import utcnow
    from app.trove.models import FeedCache

    now = int(utcnow().timestamp())
    await FeedCache(feed="chaos_chest", items=[{
        "name": "Shadow Dragon", "identifier": "prefabs/collections/dragon/shadow",
        "blueprint": "blueprints/shadow.blueprint", "start": now - 100, "end": now + 10_000,
    }], fetched_at=utcnow()).insert()

    # Public (rotations scope) - reachable with no token.
    r = await client.get("/v1/rotations/chaos-chest")
    assert r.status_code == 200
    body = r.json()
    assert body["active"] is True
    assert body["ends_at"] == now + 10_000
    assert body["item"]["name"] == "Shadow Dragon"
    assert body["item"]["blueprint"] == "blueprints/shadow.blueprint"
    assert body["fetched_at"] is not None


async def test_delve_rotations(client):
    from app.core.utils import utcnow
    from app.trove.models import DelveRotation

    await DelveRotation(
        week=17, total=2, count=2, fetched_at=utcnow(),
        depths=[{"id": 1, "depth": 110, "biome": "Flakbeard's Hideaway"},
                {"id": 2, "depth": 111, "biome": "Pure Midnight"}],
    ).insert()

    # Public (rotations scope) - reachable with no token.
    r = await client.get("/v1/rotations/delves?week=17")
    assert r.status_code == 200
    body = r.json()
    assert body["week"] == 17 and body["count"] == 2 and len(body["depths"]) == 2
    assert body["depths"][0]["biome"] == "Flakbeard's Hideaway"

    # Week list is metadata-only (no depths) and reports the live week id.
    weeks = (await client.get("/v1/rotations/delves/weeks")).json()
    assert weeks["count"] == 1 and weeks["items"][0]["week"] == 17
    assert "depths" not in weeks["items"][0] and weeks["current_week"] >= 1

    # Unknown explicit week -> 404; current (unseeded) week -> empty rotation.
    assert (await client.get("/v1/rotations/delves?week=999")).status_code == 404
    cur = (await client.get("/v1/rotations/delves")).json()
    assert cur["is_current"] is True and cur["depths"] == []


async def test_yearly_calendar(client):
    # Public (rotations scope) - reachable with no token; pure compute, no seeding.
    r = await client.get("/v1/rotations/calendar")
    assert r.status_code == 200
    body = r.json()
    assert body["count"] == len(body["events"]) and body["count"] > 0
    assert body["starts_at"] < body["generated_at"] < body["ends_at"]
    types = {e["type"] for e in body["events"]}
    assert {"weekly_buff", "corruxion", "fluxion", "stampy", "mana"} <= types
    assert "invasion" not in types
    # sorted by start
    starts = [e["starts_at"] for e in body["events"]]
    assert starts == sorted(starts)


async def test_relayed_feeds(client):
    from app.core.utils import utcnow
    from app.trove.models import FeedCache

    h = await _signup_login(client, "feeds@b.com")
    tok = (
        await client.post(
            "/tokens", headers=h, json={"name": "f", "scopes": 2, "allowed_ips": ["1.2.3.4"]}
        )
    ).json()["token"]
    th = {"Authorization": f"Bearer {tok}"}

    # Seed cached feeds directly (tests never hit the upstream relay).
    await FeedCache(
        feed="twitch",
        items=[{"channel": "C", "login": "c", "url": "https://twitch.tv/c", "title": "T", "viewers": 5}],
        fetched_at=utcnow(),
    ).insert()
    await FeedCache(
        feed="youtube",
        items=[{"title": "V", "url": "https://yt/1", "channel": "Ch"}],
        fetched_at=utcnow(),
    ).insert()

    tw = (await client.get("/v1/feeds/twitch", headers=th)).json()
    assert tw["count"] == 1 and tw["items"][0]["login"] == "c" and tw["fetched_at"]

    yt = (await client.get("/v1/feeds/youtube", headers=th)).json()
    assert yt["count"] == 1 and yt["items"][0]["url"] == "https://yt/1"

    # A feed with no cached doc returns an empty list (not an error).
    bb = (await client.get("/v1/feeds/bilibili", headers=th)).json()
    assert bb["count"] == 0 and bb["items"] == []


async def test_stats_data(client):
    h = await _signup_login(client, "stats@b.com")
    # stats:read only (bit 4).
    tok = (
        await client.post(
            "/tokens", headers=h, json={"name": "s", "scopes": 4, "allowed_ips": ["1.2.3.4"]}
        )
    ).json()["token"]
    th = {"Authorization": f"Bearer {tok}"}

    pr = (await client.get("/v1/stats/power-rank", headers=th)).json()
    assert pr["stat"] == "power-rank" and pr["count"] == len(pr["sources"]) > 0
    assert any(s["name"] == "Gems" and s["value"] == 44540 for s in pr["sources"])

    classes = (await client.get("/v1/stats/classes", headers=th)).json()
    assert classes["count"] == 18 and all(c["tech_name"] for c in classes["items"])

    # Look a class up by its tech_name token.
    knight = (await client.get("/v1/stats/classes/knight", headers=th)).json()
    assert knight["name"] == "Knight" and knight["tech_name"] == "knight"

    missing = await client.get("/v1/stats/classes/not_a_class", headers=th)
    assert missing.status_code == 404 and missing.json()["error"]["code"] == "not_found"

    # A feeds-only token (bit 2) cannot read stats.
    feeds_tok = (
        await client.post(
            "/tokens", headers=h, json={"name": "f", "scopes": 2, "allowed_ips": ["1.2.3.4"]}
        )
    ).json()["token"]
    denied = await client.get(
        "/v1/stats/classes", headers={"Authorization": f"Bearer {feeds_tok}"}
    )
    assert denied.status_code == 403 and denied.json()["error"]["code"] == "insufficient_scope"


async def test_gem_tools(client):
    h = await _signup_login(client, "gems@b.com")
    tok = (
        await client.post(
            "/tokens", headers=h, json={"name": "g", "scopes": 8, "allowed_ips": ["1.2.3.4"]}
        )
    ).json()["token"]
    th = {"Authorization": f"Bearer {tok}"}

    lookups = (await client.get("/v1/gems/lookups", headers=th)).json()
    assert len(lookups["tiers"]) == 4 and len(lookups["stat_types"]) == 9

    # Simulator round-trip: generate → post the gem back → augment a stat by position.
    gem = (
        await client.post(
            "/v1/gems/generate",
            headers=th,
            json={"tier": 4, "type": 1, "element": 1, "restriction": 1, "level": 15, "augmentation": 0.0},
        )
    ).json()
    assert gem["gem_name"].endswith("Mystic Gem") and len(gem["stats"]) == 3 and gem["quality"] == 0.0

    augmented = (
        await client.post(
            "/v1/gems/augment", headers=th, json={"gem": gem, "stat_position": 0, "augment_type": 3}
        )
    ).json()
    assert augmented["applied"] is True and augmented["gem"]["quality"] > 0.0

    # Evaluator.
    ev = (
        await client.post(
            "/v1/gems/evaluate",
            headers=th,
            json={"tier": 4, "type": 1, "level": 1, "auto_guess_procs": True,
                  "stats": [{"type": 3, "value": 200, "extra_containers": 0},
                            {"type": 4, "value": 20, "extra_containers": 0},
                            {"type": 5, "value": 50000, "extra_containers": 0}]},
        )
    ).json()
    assert "quality_percent" in ev and "calculated_power_rank" in ev

    # Builds.
    opts = (await client.get("/v1/gems/builds/options", headers=th)).json()
    assert "Knight" in opts["character"]
    builds_resp = (
        await client.post(
            "/v1/gems/builds/calculate",
            headers=th,
            json={"build_type": "Light", "character": "Knight", "subclass": "Knight"},
        )
    ).json()
    assert builds_resp["count"] == 190 and builds_resp["results"][0]["rank"] == 1

    # A stats-only token (bit 4) cannot use the gem tools.
    stats_tok = (
        await client.post(
            "/tokens", headers=h, json={"name": "s", "scopes": 4, "allowed_ips": ["1.2.3.4"]}
        )
    ).json()["token"]
    denied = await client.get("/v1/gems/lookups", headers={"Authorization": f"Bearer {stats_tok}"})
    assert denied.status_code == 403 and denied.json()["error"]["code"] == "insufficient_scope"


async def test_trovesaurus_events(client):
    from app.core.utils import utcnow
    from app.trove.models import TroveEvent

    h = await _signup_login(client, "events@b.com")
    tok = (
        await client.post(
            "/tokens", headers=h, json={"name": "e", "scopes": 2, "allowed_ips": ["1.2.3.4"]}
        )
    ).json()["token"]
    th = {"Authorization": f"Bearer {tok}"}

    now = int(utcnow().timestamp())
    # Seed one of each status, across two categories (tests never hit upstream).
    await TroveEvent(event_id="on1", name="Ongoing", category="Event",
                     starts_at=now - 100, ends_at=now + 1000).insert()
    await TroveEvent(event_id="up1", name="Upcoming", category="Giveaway",
                     starts_at=now + 1000, ends_at=now + 2000).insert()
    await TroveEvent(event_id="end1", name="Ended", category="Event",
                     starts_at=now - 2000, ends_at=now - 1000).insert()

    ongoing = (await client.get("/v1/feeds/events", headers=th)).json()
    assert ongoing["count"] == 1 and ongoing["items"][0]["event_id"] == "on1"
    assert ongoing["items"][0]["status"] == "ongoing" and ongoing["items"][0]["seconds_until"] > 0

    cats = (await client.get("/v1/feeds/events/categories", headers=th)).json()
    assert cats["categories"] == ["Event", "Giveaway"]  # distinct, sorted

    upcoming = (await client.get("/v1/feeds/events/upcoming", headers=th)).json()
    assert upcoming["count"] == 1 and upcoming["items"][0]["event_id"] == "up1"
    assert upcoming["items"][0]["status"] == "upcoming"

    history = (await client.get("/v1/feeds/events/history", headers=th)).json()
    assert history["count"] == 1 and history["items"][0]["event_id"] == "end1"
    assert history["items"][0]["status"] == "ended"

    # Category filter: the only Giveaway is upcoming, so /events (ongoing) excludes it.
    assert (await client.get("/v1/feeds/events?category=Giveaway", headers=th)).json()["count"] == 0
    up_gv = (await client.get("/v1/feeds/events/upcoming?category=Giveaway", headers=th)).json()
    assert up_gv["count"] == 1 and up_gv["items"][0]["event_id"] == "up1"


async def test_misc_tools(client):
    h = await _signup_login(client, "misc@b.com")
    tok = (
        await client.post(
            "/tokens", headers=h, json={"name": "m", "scopes": 16, "allowed_ips": ["1.2.3.4"]}
        )
    ).json()["token"]
    th = {"Authorization": f"Bearer {tok}"}

    software = (await client.get("/v1/misc/software", headers=th)).json()
    assert software["count"] >= 5
    assert any(c["key"] == "blueprints" for c in software["categories"])

    tzs = (await client.get("/v1/misc/timezones", headers=th)).json()
    assert any(z["id"] == "trove" for z in tzs["items"])

    now = (await client.get("/v1/misc/time/now", headers=th)).json()
    assert now["unix"] and len(now["zones"]) == len(tzs["items"])

    conv = (
        await client.post(
            "/v1/misc/time/convert",
            headers=th,
            json={"datetime": "2026-06-05T00:00:00", "timezone": "UTC"},
        )
    ).json()
    trove = next(z for z in conv["zones"] if z["id"] == "trove")
    assert trove["time"] == "13:00:00"  # UTC - 11h
    assert len(conv["discord"]) == 7

    # software / timezones / time/* are PUBLIC (tokenless) - work with no token.
    anon = await client.get("/v1/misc/software")
    assert anon.status_code == 200 and anon.json()["count"] >= 5

    # A scope-gated misc endpoint (news-history) still requires misc:read - a
    # gems-only token (bit 8) is rejected.
    gem_tok = (
        await client.post(
            "/tokens", headers=h, json={"name": "g", "scopes": 8, "allowed_ips": ["1.2.3.4"]}
        )
    ).json()["token"]
    denied = await client.get("/v1/misc/news-history", headers={"Authorization": f"Bearer {gem_tok}"})
    assert denied.status_code == 403 and denied.json()["error"]["code"] == "insufficient_scope"


async def test_mod_tools(client):
    import base64

    h = await _signup_login(client, "mods@b.com")
    tok = (
        await client.post(
            "/tokens", headers=h, json={"name": "md", "scopes": 32, "allowed_ips": ["1.2.3.4"]}
        )
    ).json()["token"]
    th = {"Authorization": f"Bearer {tok}"}

    build_body = {
        "version": 1,
        "properties": {"title": "Test Mod", "author": "Aallyn", "modVersion": "1.0"},
        "files": [{"path": "ui/icon.png", "content_base64": base64.b64encode(b"PNGDATA").decode()}],
    }
    built = await client.post("/v1/mods/build", headers=th, json=build_body)
    assert built.status_code == 200
    assert built.headers["content-type"] == "application/octet-stream"
    assert "Test Mod.tmod" in built.headers.get("content-disposition", "")
    tmod_bytes = built.content
    assert len(tmod_bytes) > 12

    # Read the freshly built tmod back (POST the raw bytes).
    obj = {**th, "Content-Type": "application/octet-stream"}
    read = await client.post("/v1/mods/read", headers=obj, content=tmod_bytes)
    assert read.status_code == 200
    data = read.json()
    assert data["properties"]["modLoader"] == "KiwiAPI"  # not "BTT"
    assert data["properties"]["title"] == "Test Mod" and data["file_count"] == 1
    assert data["files"][0]["path"] == "ui/icon.png"
    assert base64.b64decode(data["files"][0]["content_base64"]) == b"PNGDATA"

    # Metadata-only omits content.
    meta = (await client.post("/v1/mods/read?metadata_only=true", headers=obj, content=tmod_bytes)).json()
    assert meta["metadata_only"] is True and meta["files"][0].get("content_base64") is None
    assert meta["files"][0]["size"] == len(b"PNGDATA")

    # A misc-only token (bit 16) cannot use the mod tools.
    misc_tok = (
        await client.post(
            "/tokens", headers=h, json={"name": "x", "scopes": 16, "allowed_ips": ["1.2.3.4"]}
        )
    ).json()["token"]
    denied = await client.post(
        "/v1/mods/build", headers={"Authorization": f"Bearer {misc_tok}"}, json=build_body
    )
    assert denied.status_code == 403 and denied.json()["error"]["code"] == "insufficient_scope"


async def test_updates_browse_and_download(client, monkeypatch, tmp_path):
    from app.core.config import settings
    from app.core.utils import utcnow
    from app.trove.updates.cas import ContentStore
    from app.trove.updates.models import UpdateBranch, UpdateState, UpdateVersion

    # Point the blob store at a temp dir and seed one real blob.
    monkeypatch.setattr(settings, "trove_update_store_dir", str(tmp_path))
    sha, _ = ContentStore(str(tmp_path)).put(b"WOLF-BINFAB-BYTES")

    await UpdateBranch(branch="live-us", current_version="STABLE-1", current_ordinal=1, status="idle").insert()
    await UpdateVersion(branch="live-us", ordinal=1, version_tag="STABLE-1", status="complete",
                        completed_at=utcnow(), files_added=3).insert()
    await UpdateState(branch="live-us", path="prefabs/collections/pet/wolf.binfab",
                      content_sha256=sha, size=17, archive="prefabs", archive_index=0).insert()
    await UpdateState(branch="live-us", path="prefabs/collections/pet/cat.binfab",
                      content_sha256="0" * 64, size=10, archive="prefabs", archive_index=0).insert()
    await UpdateState(branch="live-us", path="Trove_x64.exe", content_sha256="1" * 64, size=100).insert()

    h = await _signup_login(client, "updates@b.com")
    tok = (
        await client.post(
            "/tokens", headers=h, json={"name": "u", "scopes": 64, "allowed_ips": ["1.2.3.4"]}
        )
    ).json()["token"]
    th = {"Authorization": f"Bearer {tok}"}

    branches = (await client.get("/v1/updates/branches", headers=th)).json()
    assert branches["count"] == 1 and branches["items"][0]["file_count"] == 3

    versions = (await client.get("/v1/updates/live-us/versions", headers=th)).json()
    assert versions["total"] == 1 and versions["items"][0]["version_tag"] == "STABLE-1"

    root = {e["name"]: e for e in (await client.get("/v1/updates/live-us/tree", headers=th)).json()["entries"]}
    assert root["prefabs"]["is_dir"] and root["prefabs"]["file_count"] == 2
    assert not root["Trove_x64.exe"]["is_dir"]

    pet = (await client.get("/v1/updates/live-us/tree?prefix=prefabs/collections/pet/", headers=th)).json()
    assert {e["name"] for e in pet["entries"]} == {"wolf.binfab", "cat.binfab"}

    meta = (
        await client.get("/v1/updates/live-us/file/meta?path=prefabs/collections/pet/wolf.binfab", headers=th)
    ).json()
    assert meta["content_sha256"] == sha and meta["size"] == 17

    dl = await client.get("/v1/updates/live-us/file?path=prefabs/collections/pet/wolf.binfab", headers=th)
    assert dl.status_code == 200 and dl.content == b"WOLF-BINFAB-BYTES"

    assert (await client.get("/v1/updates/nope/tree", headers=th)).status_code == 404      # bad branch
    assert (await client.get("/v1/updates/live-us/file?path=x.bin", headers=th)).status_code == 404

    # A mods-only token (bit 32) cannot browse the archive.
    mtok = (
        await client.post(
            "/tokens", headers=h, json={"name": "m", "scopes": 32, "allowed_ips": ["1.2.3.4"]}
        )
    ).json()["token"]
    denied = await client.get("/v1/updates/branches", headers={"Authorization": f"Bearer {mtok}"})
    assert denied.status_code == 403 and denied.json()["error"]["code"] == "insufficient_scope"


async def test_codexes_index_and_browse(client, monkeypatch, tmp_path):
    from app.core.config import settings
    from app.trove.codexes.indexer import reindex
    from app.trove.updates.cas import ContentStore
    from app.trove.updates.models import UpdateState

    # Minimal .binfab builders (the format is self-describing - see test_codexes.py).
    def uleb(n: int) -> bytes:
        out = bytearray()
        while True:
            b = n & 0x7F
            n >>= 7
            out.append(b | 0x80 if n else b)
            if not n:
                return bytes(out)

    def strf(field: int, text: str) -> bytes:
        raw = text.encode()
        return uleb((field << 4) | 8) + uleb(len(raw)) + raw

    def prefab(name_key, category, desc_key, tradable) -> bytes:
        stream = (uleb(0xE) + strf(1, name_key) + strf(2, category) + strf(5, desc_key)
                  + uleb(0xE0) + uleb(1) + uleb(2 if tradable else 1) + uleb(0xE))
        return bytes([0x05, 0x00]) + uleb(len(stream)) + stream

    def locale(key, value) -> bytes:
        return key.encode() + bytes([0x18]) + uleb(len(value.encode())) + value.encode()

    def mult_group(identifier: str) -> bytes:  # one multipliers.binfab group
        return (b"\xBE\x01\xAE" + uleb(1) + uleb(4) + b"\x00" + b"\x00\x00"
                + uleb(len(identifier)) + identifier.encode())

    monkeypatch.setattr(settings, "trove_update_store_dir", str(tmp_path))
    store = ContentStore(str(tmp_path))
    # multipliers: wolf in the ×2 group (pet base 10 → 20), ember in ×5 (mount 50 → 250).
    mult_sha, _ = store.put(b"\x00" * 9 + mult_group("zero") + mult_group("collections/pet/wolf")
                            + mult_group("three") + mult_group("collections/mount/ember"))
    wolf_sha, _ = store.put(prefab("$wolf_name", "Pets", "$wolf_desc", True))
    sword_sha, _ = store.put(prefab("$sword_name", "Weapons", "$sword_desc", False))
    # A mount prefab + the collection table that marks it a dragon (category "Dragons").
    ember_sha, _ = store.put(prefab("$ember_name", "Mounts", "$ember_desc", True))
    table_sha, _ = store.put(
        strf(1, "Dragons") + strf(1, "$CollectionName_Dragons") + strf(1, "collections/mount/ember")
    )
    loc_sha, _ = store.put(locale("$wolf_name", "Dire Wolf") + locale("$wolf_desc", "A loyal companion.")
                           + locale("$sword_name", "Iron Sword") + locale("$sword_desc", "A basic blade.")
                           + locale("$ember_name", "Ember Drake") + locale("$ember_desc", "A fiery dragon."))

    await UpdateState(branch="live-us", path="prefabs/collections/pet/wolf.binfab",
                      content_sha256=wolf_sha, size=1).insert()
    await UpdateState(branch="live-us", path="prefabs/item/sword.binfab",
                      content_sha256=sword_sha, size=1).insert()
    await UpdateState(branch="live-us", path="prefabs/collections/mount/ember.binfab",
                      content_sha256=ember_sha, size=1).insert()
    await UpdateState(branch="live-us", path="prefabs/collections/collection_mount.binfab",
                      content_sha256=table_sha, size=1).insert()
    await UpdateState(branch="live-us", path="prefabs/meta/multipliers.binfab",
                      content_sha256=mult_sha, size=1).insert()
    await UpdateState(branch="live-us", path="languages/en/strings.binfab",
                      content_sha256=loc_sha, size=1).insert()
    await UpdateState(branch="live-us", path="Trove_x64.exe", content_sha256="1" * 64, size=1).insert()

    counts = await reindex("live-us", store)
    assert counts["indexed"] == 3  # wolf, sword, ember; locale + collection table + exe ignored

    h = await _signup_login(client, "codex@b.com")
    tok = (await client.post(
        "/tokens", headers=h, json={"name": "c", "scopes": 128, "allowed_ips": ["1.2.3.4"]}
    )).json()["token"]
    th = {"Authorization": f"Bearer {tok}"}

    types_resp = await client.get("/v1/codexes/types", headers=th)
    # Authenticated codexes get 5× the base per-token cap (120 → 600).
    assert types_resp.headers.get("X-RateLimit-Limit") == "600"
    types = types_resp.json()
    by_type = {t["type"]: t["count"] for t in types["items"]}
    # ember is a mount split into the dragon codex by its collection category.
    assert by_type == {"ally": 1, "item": 1, "dragon": 1}
    dragons = (await client.get("/v1/codexes/dragon", headers=th)).json()
    assert dragons["total"] == 1 and dragons["items"][0]["name"] == "Ember Drake"
    assert dragons["items"][0]["category"] == "Dragons"
    assert dragons["items"][0]["mastery"] == 250  # mount base 50 × ×5 group

    allies = (await client.get("/v1/codexes/ally", headers=th)).json()
    assert allies["total"] == 1
    wolf = allies["items"][0]
    assert wolf["name"] == "Dire Wolf" and wolf["category"] == "Pets" and wolf["tradable"] is True
    assert wolf["description"] == "A loyal companion."
    assert wolf["mastery"] == 20  # pet base 10 × ×2 group
    # Items carry no mastery.
    assert (await client.get("/v1/codexes/item", headers=th)).json()["items"][0]["mastery"] is None

    # Search by name (case-insensitive substring) and lookup by path.
    assert (await client.get("/v1/codexes/item?search=iron", headers=th)).json()["total"] == 1
    assert (await client.get("/v1/codexes/item?search=zzz", headers=th)).json()["total"] == 0

    # Filters: category (exact), tradable, and search now matches description too.
    assert (await client.get("/v1/codexes/item?category=Weapons", headers=th)).json()["total"] == 1
    assert (await client.get("/v1/codexes/ally?tradable=true", headers=th)).json()["total"] == 1
    assert (await client.get("/v1/codexes/ally?tradable=false", headers=th)).json()["total"] == 0
    # "dragon" hits the ember entry via its description ("A fiery dragon.").
    assert (await client.get("/v1/codexes/dragon?search=fiery", headers=th)).json()["total"] == 1

    # Category listing (for filter dropdowns).
    cats = (await client.get("/v1/codexes/ally/categories", headers=th)).json()
    assert cats["items"] == [{"category": "Pets", "count": 1}]

    # Cross-type search surface.
    s_wolf = (await client.get("/v1/codexes/search?q=wolf", headers=th)).json()
    assert s_wolf["total"] == 1 and s_wolf["items"][0]["type"] == "ally"
    s_trade = (await client.get("/v1/codexes/search?tradable=true", headers=th)).json()
    assert s_trade["total"] == 2  # wolf + ember, across types
    assert (await client.get("/v1/codexes/search?type=item&q=blade", headers=th)).json()["total"] == 1
    assert (await client.get("/v1/codexes/search?type=bogus", headers=th)).status_code == 404
    assert (await client.get("/v1/codexes/item?sort=nope", headers=th)).status_code == 400
    one = (await client.get(
        "/v1/codexes/ally/entry?path=prefabs/collections/pet/wolf.binfab", headers=th
    )).json()
    assert one["name"] == "Dire Wolf"

    assert (await client.get("/v1/codexes/dragon/entry?path=nope.binfab", headers=th)).status_code == 404
    assert (await client.get("/v1/codexes/bogus", headers=th)).status_code == 404      # bad type
    assert (await client.get("/v1/codexes/ally?branch=nope", headers=th)).status_code == 404  # bad branch

    # Codexes are PUBLIC: reachable with no token at all, and a token lacking
    # codexes:read (bit 128) is served via the anonymous path (not 403).
    anon_resp = await client.get("/v1/codexes/types")
    # Anonymous codexes get 5× the base per-IP cap (30 → 150).
    assert anon_resp.headers.get("X-RateLimit-Limit") == "150"
    anon = anon_resp.json()
    assert {t["type"] for t in anon["items"]} == {"ally", "item", "dragon"}
    other = (await client.post(
        "/tokens", headers=h, json={"name": "o", "scopes": 64, "allowed_ips": ["1.2.3.4"]}
    )).json()["token"]
    ok = await client.get("/v1/codexes/types", headers={"Authorization": f"Bearer {other}"})
    assert ok.status_code == 200


async def test_scope_separation(client):
    h = await _signup_login(client, "scopesep@b.com")
    # stats:read only (bit 4) - can read stats, not gems. (rotations/feeds are
    # public now, so a still-token-gated pair is used to test scope isolation.)
    stats_only = (
        await client.post(
            "/tokens", headers=h, json={"name": "s", "scopes": 4, "allowed_ips": ["1.2.3.4"]}
        )
    ).json()["token"]
    th = {"Authorization": f"Bearer {stats_only}"}

    assert (await client.get("/v1/stats/power-rank", headers=th)).status_code == 200
    denied = await client.get("/v1/gems/lookups", headers=th)
    assert denied.status_code == 403
    assert denied.json()["error"]["code"] == "insufficient_scope"


async def test_public_scopes_anonymous_access(client):
    from app.core.utils import utcnow
    from app.trove.models import TroveNews

    # rotations + feeds are reachable with NO Authorization header (anonymous),
    # and the response carries rate-limit headers (the per-IP budget).
    r = await client.get("/v1/rotations/server-time")
    assert r.status_code == 200 and "X-RateLimit-Limit" in r.headers

    await TroveNews(url="https://trovegame.com/news/pub", title="Pub", published_at=utcnow()).insert()
    f = await client.get("/v1/feeds/news")
    assert f.status_code == 200 and f.json()["count"] >= 1

    # A still-gated scope is NOT public - anonymous access is rejected.
    assert (await client.get("/v1/stats/power-rank")).status_code == 401

    # A token WITHOUT the (now-public) scope still gets in via the anonymous path.
    h = await _signup_login(client, "pubtok@b.com")
    misc_only = (
        await client.post(
            "/tokens", headers=h, json={"name": "m", "scopes": 16, "allowed_ips": ["1.2.3.4"]}
        )
    ).json()["token"]
    th = {"Authorization": f"Bearer {misc_only}"}
    assert (await client.get("/v1/rotations/server-time", headers=th)).status_code == 200


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
    # Locked now - even the correct password is refused.
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
