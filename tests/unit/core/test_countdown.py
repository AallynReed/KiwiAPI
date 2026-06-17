"""countdown_bucket: the shared coarsening used by the announcement image text
and the bot's image refresh token (so a banner shows '16h', not '16h13m', and the
bot re-edits at most once an hour until the last hour)."""
from app.bot.announcer import _refresh_token
from app.core.utils import countdown_bucket


def test_bucket_tiers():
    now = 1_000_000
    assert countdown_bucket(None, now) == ("none", 0)
    assert countdown_bucket(now - 5, now) == ("now", 0)           # already passed
    assert countdown_bucket(now, now) == ("now", 0)
    assert countdown_bucket(now + 45 * 60, now) == ("m", 45)      # under 1h -> minutes
    assert countdown_bucket(now + 3599, now) == ("m", 59)         # 59m59s -> still minutes
    assert countdown_bucket(now + 3600, now) == ("h", 1)          # exactly 1h -> hours
    assert countdown_bucket(now + 16 * 3600 + 13 * 60, now) == ("h", 16)  # 16h13m -> "16h"
    assert countdown_bucket(now + 86399, now) == ("h", 23)        # <1d -> hours
    assert countdown_bucket(now + 86400, now) == ("d", 1)         # exactly 1d -> days
    assert countdown_bucket(now + 2 * 86400, now) == ("d", 2)


def test_bucket_is_stable_within_an_hour_then_ticks():
    target = 1000 + 16 * 3600 + 13 * 60      # 16h13m out
    # "16h" holds while remaining stays in the 16h bucket...
    assert countdown_bucket(target, 1000) == ("h", 16)
    assert countdown_bucket(target, 1000 + 12 * 60) == ("h", 16)   # 12 min later, unchanged
    # ...and drops to 15h once remaining crosses 16h (one change per hour).
    assert countdown_bucket(target, 1000 + 14 * 60) == ("h", 15)


def test_refresh_token_matches_bucket():
    # The bot's ?v token IS the displayed bucket, so an edit fires iff it changes.
    assert _refresh_token(1000 + 16 * 3600, 1000) == "h16"
    assert _refresh_token(1000 + 45 * 60, 1000) == "m45"
    assert _refresh_token(1000 + 2 * 86400, 1000) == "d2"
    assert _refresh_token(None, 1000) == "none0"
    # stable across the hour (no needless re-edit), then changes
    tok_a = _refresh_token(1000 + 16 * 3600 + 13 * 60, 1000)
    tok_b = _refresh_token(1000 + 16 * 3600 + 13 * 60, 1000 + 10 * 60)
    tok_c = _refresh_token(1000 + 16 * 3600 + 13 * 60, 1000 + 20 * 60)
    assert tok_a == tok_b == "h16"
    assert tok_c == "h15"
