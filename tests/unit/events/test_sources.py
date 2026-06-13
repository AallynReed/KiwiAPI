"""Live event source catalog: registry integrity, signature logic, schedule
boundaries, and consistency with the bot's event->announcement mapping.

Pure where possible - the time-based sources (merchants/rotations/daily) compute
from the clock with no DB, so their ``next_at``/``sig`` are exercised directly;
the insert-driven ``challenge``/``chaos`` data fns hit Mongo, so only their pure
``sig_fn`` is unit-tested (with synthetic payloads).
"""
import time

from app.events import sources as S

# ── registry integrity ──────────────────────────────────────────────────────

def test_source_types_are_unique():
    types = [s.type for s in S.SOURCES]
    assert len(types) == len(set(types))


def test_sources_by_type_covers_everything():
    assert set(S.SOURCES_BY_TYPE) == {s.type for s in S.SOURCES}


def test_scheduled_sources_are_exactly_the_time_driven_ones():
    assert {s.type for s in S.SCHEDULED_SOURCES} == {
        s.type for s in S.SOURCES if s.next_at_fn is not None
    }
    # challenge/chaos are insert-driven, never scheduled
    assert "challenge" not in {s.type for s in S.SCHEDULED_SOURCES}
    assert "chaos" not in {s.type for s in S.SCHEDULED_SOURCES}


# ── pure signature logic ─────────────────────────────────────────────────────

def test_challenge_sig_requires_a_captured_name():
    sig = S.SOURCES_BY_TYPE["challenge"].sig_fn
    assert sig({"starts_at": 100, "name": "Cursed Vale"}) == "100:Cursed Vale"
    assert sig({"starts_at": 100, "name": None}) is None
    assert sig({"starts_at": 100}) is None


def test_chaos_sig_requires_an_item_name():
    sig = S.SOURCES_BY_TYPE["chaos"].sig_fn
    assert sig({"starts_at": 7, "item": {"name": "Radiant"}}) == "7:Radiant"
    assert sig({"starts_at": 7, "item": None}) is None
    assert sig({"starts_at": 7, "item": {}}) is None


def test_merchant_sig_only_fires_while_active():
    sig = S.SOURCES_BY_TYPE["corruxion"].sig_fn
    assert sig({"active": True, "starts_at": 500}) == "500"
    assert sig({"active": False, "starts_at": 500}) is None


def test_rotation_sig_tracks_current_window_start():
    sig = S.SOURCES_BY_TYPE["longshade"].sig_fn
    assert sig({"current": {"starts_at": 900}}) == "900"
    assert sig({"current": {}}) is None


def test_status_sig_skips_unknown():
    sig = S.SOURCES_BY_TYPE["server_status"].sig_fn
    assert sig({"overall": "online"}) == "status:online"
    assert sig({"overall": "down"}) == "status:down"
    assert sig({"overall": "unknown"}) is None
    assert sig({}) is None


def test_news_sig_tracks_top_article_url():
    sig = S.SOURCES_BY_TYPE["trove_news"].sig_fn
    assert sig({"item": {"url": "https://trovegame.com/a"}}) == "https://trovegame.com/a"
    assert sig({"item": None}) is None


def test_giveaways_sig_tracks_newest_open():
    sig = S.SOURCES_BY_TYPE["giveaways"].sig_fn
    assert sig({"newest": {"created_at": 1700}}) == "1700"
    assert sig({"newest": None}) is None


def test_activity_is_scheduled_at_the_daily_reset():
    src = S.SOURCES_BY_TYPE["activity"]
    assert src.next_at_fn is not None      # time-driven
    assert src in S.SCHEDULED_SOURCES
    # status / news / giveaways are producer-driven, never scheduled
    for t in ("server_status", "trove_news", "giveaways"):
        assert S.SOURCES_BY_TYPE[t].next_at_fn is None
        assert S.SOURCES_BY_TYPE[t] not in S.SCHEDULED_SOURCES


# ── schedule boundaries (time-based, no DB) ─────────────────────────────────

def test_scheduled_next_at_is_always_in_the_future():
    now = int(time.time())
    for s in S.SCHEDULED_SOURCES:
        nxt = s.next_at_fn()
        assert isinstance(nxt, int)
        assert nxt > now, f"{s.type} next_at {nxt} is not after now {now}"


# ── consistency with the bot ─────────────────────────────────────────────────

def test_every_scheduled_source_is_mappable_by_the_bot():
    from app.bot.announcements import TYPES_BY_KEY
    from app.bot.runner import _EVENT_TO_ANNOUNCEMENT

    for s in S.SOURCES:
        key = _EVENT_TO_ANNOUNCEMENT.get(s.type)
        assert key is not None, f"bot has no mapping for event type {s.type!r}"
        assert key in TYPES_BY_KEY, f"{s.type}->{key} is not a real announcement type"
