"""Tracked-announcement auto-delete: registry policy + the message-delete helper.

The janitor sweep + supersede are DB/Discord integration (not unit-tested here);
this covers the pure policy and the delete-result semantics that decide whether a
tracking record is dropped."""
import asyncio

import discord

from app.bot import announcer
from app.bot.announcements import ANNOUNCEMENT_TYPES, TYPES_BY_KEY

_MANAGED = {
    "hourly_challenge", "chaos_chest", "daily_bonuses", "longshade", "wild_mana",
    "stampy", "corruxion", "fluxion", "server_status",
}


# ── registry policy ──────────────────────────────────────────────────────────

def test_auto_managed_types_are_the_time_bound_set():
    assert {t.key for t in ANNOUNCEMENT_TYPES if t.auto_manage} == _MANAGED


def test_server_status_is_supersede_only():
    # No natural end -> no expiry; it's replaced when the next status posts.
    assert TYPES_BY_KEY["server_status"].auto_manage is True
    assert TYPES_BY_KEY["server_status"].expiry is None


def test_other_managed_types_have_an_expiry():
    for key in _MANAGED - {"server_status"}:
        assert TYPES_BY_KEY[key].expiry is not None, key


def test_embed_only_types_are_not_auto_managed():
    for key in ("trove_news", "giveaways", "activity"):
        assert TYPES_BY_KEY[key].auto_manage is False
        assert TYPES_BY_KEY[key].expiry is None


# ── _delete_message result semantics (True => drop the tracking record) ───────

class _Resp:
    status = 404
    reason = "Not Found"


class _Channel:
    def __init__(self, deleter):
        self._deleter = deleter

    def get_partial_message(self, _mid):
        outer = self

        class _PM:
            async def delete(self_inner):
                await outer._deleter()
        return _PM()


def test_delete_returns_true_on_success():
    async def ok():
        return None

    class _Bot:
        def get_channel(self, _cid):
            return _Channel(ok)

    assert asyncio.run(announcer._delete_message(_Bot(), 1, 2)) is True


def test_delete_returns_true_when_already_gone():
    async def gone():
        raise discord.NotFound(_Resp(), "gone")

    class _Bot:
        def get_channel(self, _cid):
            return _Channel(gone)

    assert asyncio.run(announcer._delete_message(_Bot(), 1, 2)) is True


def test_delete_returns_false_on_transient_error():
    async def boom():
        raise discord.HTTPException(_Resp(), "later")

    class _Bot:
        def get_channel(self, _cid):
            return _Channel(boom)

    # transient -> keep the record, retry next sweep
    assert asyncio.run(announcer._delete_message(_Bot(), 1, 2)) is False


# ── image refresh cadence (edit only when the displayed countdown bucket changes) ──

def test_refresh_token_scales_to_the_countdown():
    now = 1_000_000
    assert announcer._refresh_token(now + 1800, now) == str(now // 60)      # <1h -> minute
    assert announcer._refresh_token(now + 7200, now) == str(now // 3600)    # <1d -> hour
    assert announcer._refresh_token(now + 200_000, now) == str(now // 86400)  # >1d -> day
    assert announcer._refresh_token(None, now) == str(now // 60)            # no expiry -> minute


# ── clock-aligned loop timing (edits at :55, deletes at :00) ─────────────────

def test_seconds_until_aligns_to_the_clock(monkeypatch):
    from datetime import datetime, timezone

    import app.bot.runner as runner

    monkeypatch.setattr(runner, "utcnow",
                        lambda: datetime(2026, 1, 1, 12, 30, 20, tzinfo=timezone.utc))
    assert runner._seconds_until(55) == 35      # :30:20 -> :30:55
    assert runner._seconds_until(0) == 40       # :30:20 -> :31:00

    # exactly on the target second -> skip to the next minute (never busy-loop)
    monkeypatch.setattr(runner, "utcnow",
                        lambda: datetime(2026, 1, 1, 12, 30, 0, tzinfo=timezone.utc))
    assert runner._seconds_until(0) == 60
