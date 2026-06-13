"""Discord slash commands + interactions webhook (signature, PING, dispatch, embeds)."""
import asyncio
import json
from datetime import datetime, timezone

import pytest
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from fastapi import FastAPI
from fastapi.testclient import TestClient


def _make_client(monkeypatch):
    """A TestClient over just the discord router, with a known signing key."""
    from app.core.config import settings
    from app.discord.router import router

    priv = Ed25519PrivateKey.generate()
    pub_hex = priv.public_key().public_bytes(
        serialization.Encoding.Raw, serialization.PublicFormat.Raw
    ).hex()
    monkeypatch.setattr(settings, "discord_public_key", pub_hex)

    app = FastAPI()
    app.include_router(router)
    return TestClient(app), priv


def _signed(priv, body: bytes, ts: str = "1700000000"):
    sig = priv.sign(ts.encode() + body).hex()
    return {"X-Signature-Ed25519": sig, "X-Signature-Timestamp": ts,
            "Content-Type": "application/json"}


# ── webhook plumbing ────────────────────────────────────────────────────────

def test_invalid_signature_returns_401(monkeypatch):
    client, _ = _make_client(monkeypatch)
    r = client.post(
        "/discord/interactions", content=b'{"type":1}',
        headers={"X-Signature-Ed25519": "ab", "X-Signature-Timestamp": "1",
                 "Content-Type": "application/json"},
    )
    assert r.status_code == 401


def test_ping_returns_pong(monkeypatch):
    client, priv = _make_client(monkeypatch)
    body = json.dumps({"type": 1}).encode()
    r = client.post("/discord/interactions", content=body, headers=_signed(priv, body))
    assert r.status_code == 200
    assert r.json() == {"type": 1}


def test_status_command_returns_embed(monkeypatch):
    client, priv = _make_client(monkeypatch)
    body = json.dumps({"type": 2, "data": {"name": "status"}}).encode()
    r = client.post("/discord/interactions", content=body, headers=_signed(priv, body))
    assert r.status_code == 200
    payload = r.json()
    assert payload["type"] == 4
    embeds = payload["data"]["embeds"]
    assert len(embeds) == 1 and embeds[0]["title"] == "Trove server status"
    names = [f["name"] for f in embeds[0]["fields"]]
    assert any("EU" in n for n in names) and any("US" in n for n in names)


def test_responses_are_never_ephemeral(monkeypatch):
    """The bot must be visible - no response carries the EPHEMERAL flag (64)."""
    client, priv = _make_client(monkeypatch)
    for payload in ({"type": 2, "data": {"name": "status"}},
                    {"type": 2, "data": {"name": "nope"}}):
        body = json.dumps(payload).encode()
        r = client.post("/discord/interactions", content=body, headers=_signed(priv, body))
        data = r.json()["data"]
        assert "flags" not in data, f"{payload} response was ephemeral"


# ── command catalog ─────────────────────────────────────────────────────────

def test_command_defs_cover_all_commands():
    from app.discord.commands import COMMAND_DEFS

    names = {c["name"] for c in COMMAND_DEFS}
    assert names == {
        "status", "activity", "chaos", "servertime", "bonuses", "hourly_challenge",
        "longshade", "giveaways", "corruxion", "fluxion", "stampy", "wild_mana",
        "trove_news", "download", "web", "change_log", "ping",
    }
    assert "challenge" not in names           # renamed to hourly_challenge
    activity = next(c for c in COMMAND_DEFS if c["name"] == "activity")
    opt = activity["options"][0]
    assert opt["name"] == "period" and not opt["required"]
    assert len(opt["choices"]) == 7
    for c in COMMAND_DEFS:               # all user-installable + guild
        assert c["integration_types"] == [0, 1]


def test_option_parsing():
    from app.discord.commands import _option
    inter = {"data": {"options": [{"name": "period", "type": 3, "value": "3m"}]}}
    assert _option(inter, "period", "7d") == "3m"
    assert _option({"data": {}}, "period", "7d") == "7d"


# ── embed builders that don't touch Mongo (run for real) ────────────────────

def test_status_embed_maps_status_to_colour_and_latency(monkeypatch):
    import app.discord.embeds as embeds_mod

    async def fake_status():
        return {
            "overall": "down",          # binary: online | down
            "auth": {"online": True},
            "environments": {
                "eu": {"status": "online", "game": {"latency_ms": 95}},
                "us": {"status": "down", "game": None},   # one live region down → partial
                "pts": {"status": "online", "game": None},
            },
            "checked_at": 1700000000,
        }
    monkeypatch.setattr(embeds_mod, "get_status_shared", fake_status)
    e = asyncio.run(embeds_mod.status_embed())
    assert e["color"] == 0xF0556A                  # red (down)
    assert "Partial outage" in e["description"]    # some Live region still up
    eu = next(f for f in e["fields"] if "EU" in f["name"])
    assert "95 ms" in eu["value"]
    us = next(f for f in e["fields"] if "US" in f["name"])
    assert "Down" in us["value"]                    # down region labelled "Down"
    assert "timestamp" in e


def test_status_embed_legacy_maintenance_renders_as_down(monkeypatch):
    """A stale snapshot still carrying the old 'maintenance' value renders red/Down."""
    import app.discord.embeds as embeds_mod

    async def fake_status():
        return {
            "overall": "maintenance",
            "auth": {"online": True},
            "environments": {"eu": {"status": "maintenance", "game": None},
                             "us": {"status": "maintenance", "game": None},
                             "pts": {"status": "online", "game": None}},
            "checked_at": 1700000000,
        }
    monkeypatch.setattr(embeds_mod, "get_status_shared", fake_status)
    e = asyncio.run(embeds_mod.status_embed())
    assert e["color"] == 0xF0556A
    eu = next(f for f in e["fields"] if "EU" in f["name"])
    assert "Down" in eu["value"]


def test_servertime_embed_uses_discord_timestamps():
    from app.discord.embeds import servertime_embed
    e = servertime_embed()
    assert e["title"].startswith("🕙")
    blob = e["description"] + " ".join(f["value"] for f in e["fields"])
    assert "<t:" in blob                                  # discord timestamps
    assert any("reset" in f["name"].lower() for f in e["fields"])


def test_bonuses_embed_has_daily_and_weekly():
    from app.discord.embeds import bonuses_embed
    e = bonuses_embed()
    names = [f["name"] for f in e["fields"]]
    assert any("Daily" in n for n in names) and any("Weekly" in n for n in names)
    assert isinstance(e["color"], int)


def test_longshade_embed_shows_biomes():
    from app.discord.embeds import longshade_embed
    e = longshade_embed()
    assert "Depth 15" in e["title"]
    assert any("biome" in f["name"].lower() for f in e["fields"])
    assert any("<t:" in f["value"] for f in e["fields"])  # rotation time


# ── async builder (Mongo mocked) ────────────────────────────────────────────

def test_giveaways_embed_sections(monkeypatch):
    import app.discord.embeds as em
    import app.giveaways.service as svc

    class G:
        def __init__(self, **k):
            self.__dict__.update(k)

    now = datetime(2026, 6, 11, tzinfo=timezone.utc)

    async def _ongoing():
        return [G(title="Mount drop", prize_name="Mount Code", starts_at=now,
                  ends_at=now, entry_count=42, winner_username=None)]

    async def _upcoming():
        return [G(title="Next week", prize_name="Mystery", starts_at=now,
                  ends_at=now, entry_count=0, winner_username=None)]

    async def _ended(days=7):
        return [G(title="Last draw", prize_name="Glim", starts_at=now,
                  ends_at=now, entry_count=10, winner_username="Aallyn")]

    monkeypatch.setattr(svc, "list_ongoing", _ongoing)
    monkeypatch.setattr(svc, "list_upcoming", _upcoming)
    monkeypatch.setattr(svc, "list_ended", _ended)

    e = asyncio.run(em.giveaways_embed())
    assert e["title"] == "🎉 Giveaways"
    body = " ".join(f["value"] for f in e["fields"])
    assert "Mount drop" in body and "won by Aallyn" in body
    assert "<t:" in body


# ── registration ────────────────────────────────────────────────────────────

def test_register_commands_requires_config(monkeypatch):
    from app.core.config import settings
    from app.discord.registration import DiscordRegistrationError, register_commands

    monkeypatch.setattr(settings, "discord_client_id", None)
    monkeypatch.setattr(settings, "discord_bot_token", None)
    with pytest.raises(DiscordRegistrationError):
        asyncio.run(register_commands())


# ── more no-DB builders + the one ephemeral command ─────────────────────────

def test_merchant_and_static_embeds_build():
    from app.discord.embeds import (
        corruxion_embed,
        fluxion_embed,
        stampy_embed,
        web_embed,
        wild_mana_embed,
    )
    for build in (corruxion_embed, fluxion_embed, stampy_embed, wild_mana_embed, web_embed):
        e = build()
        assert e.get("title") and (e.get("fields") or e.get("description")), build.__name__


def test_change_log_is_ephemeral_others_are_not(monkeypatch):
    import app.discord.commands as cmds

    async def fake_changelog():
        return {"title": "changelog"}

    monkeypatch.setattr(cmds, "changelog_embed", fake_changelog)
    r = asyncio.run(cmds.handle({"type": 2, "data": {"name": "change_log"}}))
    assert r["data"]["flags"] == 64                  # the one private command
    r2 = asyncio.run(cmds.handle({"type": 2, "data": {"name": "web"}}))
    assert "flags" not in r2["data"]                  # everything else is visible
