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

    # A gems-only token (bit 8) cannot use misc.
    gem_tok = (
        await client.post(
            "/tokens", headers=h, json={"name": "g", "scopes": 8, "allowed_ips": ["1.2.3.4"]}
        )
    ).json()["token"]
    denied = await client.get("/v1/misc/software", headers={"Authorization": f"Bearer {gem_tok}"})
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

    # Minimal .binfab builders (the format is self-describing — see test_codexes.py).
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

    monkeypatch.setattr(settings, "trove_update_store_dir", str(tmp_path))
    store = ContentStore(str(tmp_path))
    wolf_sha, _ = store.put(prefab("$wolf_name", "Pets", "$wolf_desc", True))
    sword_sha, _ = store.put(prefab("$sword_name", "Weapons", "$sword_desc", False))
    loc_sha, _ = store.put(locale("$wolf_name", "Dire Wolf") + locale("$wolf_desc", "A loyal companion.")
                           + locale("$sword_name", "Iron Sword") + locale("$sword_desc", "A basic blade."))

    await UpdateState(branch="live-us", path="prefabs/collections/pet/wolf.binfab",
                      content_sha256=wolf_sha, size=1).insert()
    await UpdateState(branch="live-us", path="prefabs/item/sword.binfab",
                      content_sha256=sword_sha, size=1).insert()
    await UpdateState(branch="live-us", path="languages/en/strings.binfab",
                      content_sha256=loc_sha, size=1).insert()
    await UpdateState(branch="live-us", path="Trove_x64.exe", content_sha256="1" * 64, size=1).insert()

    counts = await reindex("live-us", store)
    assert counts["indexed"] == 2  # the two prefabs; locale table + exe ignored

    h = await _signup_login(client, "codex@b.com")
    tok = (await client.post(
        "/tokens", headers=h, json={"name": "c", "scopes": 128, "allowed_ips": ["1.2.3.4"]}
    )).json()["token"]
    th = {"Authorization": f"Bearer {tok}"}

    types = (await client.get("/v1/codexes/types", headers=th)).json()
    by_type = {t["type"]: t["count"] for t in types["items"]}
    assert by_type == {"ally": 1, "item": 1}

    allies = (await client.get("/v1/codexes/ally", headers=th)).json()
    assert allies["total"] == 1
    wolf = allies["items"][0]
    assert wolf["name"] == "Dire Wolf" and wolf["category"] == "Pets" and wolf["tradable"] is True
    assert wolf["description"] == "A loyal companion."

    # Search by name (case-insensitive substring) and lookup by path.
    assert (await client.get("/v1/codexes/item?search=iron", headers=th)).json()["total"] == 1
    assert (await client.get("/v1/codexes/item?search=zzz", headers=th)).json()["total"] == 0
    one = (await client.get(
        "/v1/codexes/ally/entry?path=prefabs/collections/pet/wolf.binfab", headers=th
    )).json()
    assert one["name"] == "Dire Wolf"

    assert (await client.get("/v1/codexes/dragon/entry?path=nope.binfab", headers=th)).status_code == 404
    assert (await client.get("/v1/codexes/bogus", headers=th)).status_code == 404      # bad type
    assert (await client.get("/v1/codexes/ally?branch=nope", headers=th)).status_code == 404  # bad branch

    # A token without codexes:read (bit 128) is denied.
    other = (await client.post(
        "/tokens", headers=h, json={"name": "o", "scopes": 64, "allowed_ips": ["1.2.3.4"]}
    )).json()["token"]
    denied = await client.get("/v1/codexes/types", headers={"Authorization": f"Bearer {other}"})
    assert denied.status_code == 403 and denied.json()["error"]["code"] == "insufficient_scope"


async def test_scope_separation(client):
    h = await _signup_login(client, "scopesep@b.com")
    # rotations:read only (bit 1) — can read rotations, not feeds.
    rot_only = (
        await client.post(
            "/tokens", headers=h, json={"name": "r", "scopes": 1, "allowed_ips": ["1.2.3.4"]}
        )
    ).json()["token"]
    th = {"Authorization": f"Bearer {rot_only}"}

    assert (await client.get("/v1/rotations/server-time", headers=th)).status_code == 200
    denied = await client.get("/v1/feeds/news", headers=th)
    assert denied.status_code == 403
    assert denied.json()["error"]["code"] == "insufficient_scope"


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
