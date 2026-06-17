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
    # The token IS the displayed countdown bucket (countdown_bucket), so the bot
    # re-edits exactly when the shown value changes: per-minute <1h, per-hour <1d,
    # per-day beyond. It's stable while the banner reads the same value.
    now = 1_000_000
    assert announcer._refresh_token(now + 30 * 60, now) == "m30"        # <1h -> "30m"
    assert announcer._refresh_token(now + 2 * 3600, now) == "h2"        # <1d -> "2h"
    assert announcer._refresh_token(now + 200_000, now) == "d2"         # >1d -> "2d"
    assert announcer._refresh_token(None, now) == "none0"               # no expiry
    # holds across the hour, then ticks down (one edit/hour, not per-minute)
    target = now + 2 * 3600 + 13 * 60
    assert announcer._refresh_token(target, now) == "h2"
    assert announcer._refresh_token(target, now + 12 * 60) == "h2"      # unchanged
    assert announcer._refresh_token(target, now + 14 * 60) == "h1"      # crossed 2h


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
