from datetime import datetime

from beanie import Document
from pydantic import Field
from pymongo import ASCENDING, DESCENDING, IndexModel

from app.core.utils import utcnow


class TroveNews(Document):
    """A Trove news article relayed from the official RSS feed (trovegame.com).

    A background task periodically fetches the feed and upserts items here (keyed
    by ``url``); the API serves them from this collection so clients don't hit the
    upstream feed directly.
    """

    url: str  # canonical article link — unique
    title: str
    author: str = "Team Trove"
    summary: str = ""
    category: str = "News"
    categories: list[str] = Field(default_factory=list)
    image: str | None = None
    published_at: datetime | None = None

    first_seen_at: datetime = Field(default_factory=utcnow)
    updated_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "trove_news"
        indexes = [
            IndexModel([("url", ASCENDING)], unique=True),
            IndexModel([("published_at", DESCENDING)]),
        ]


class TroveEvent(Document):
    """A Trovesaurus calendar event, relayed from ``trovesaurus.com/calendar/feed``.

    A background task upserts events here (keyed by ``event_id``). Events are kept
    after they leave the upstream feed so the API can serve **history** (ended) and
    **upcoming** (not started) alongside what's **ongoing**. ``category`` is a
    free-form string — the set of categories is discovered dynamically (distinct).
    Times are unix seconds (real UTC), exactly as Trovesaurus provides them.
    """

    event_id: str  # the Trovesaurus event id — unique
    name: str
    url: str = ""
    category: str = "Event"
    image: str | None = None
    icon: str | None = None
    lookup: str | None = None
    starts_at: int  # unix seconds
    ends_at: int    # unix seconds

    first_seen_at: datetime = Field(default_factory=utcnow)
    updated_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "trove_events"
        indexes = [
            IndexModel([("event_id", ASCENDING)], unique=True),
            IndexModel([("category", ASCENDING)]),
            IndexModel([("starts_at", ASCENDING)]),
            IndexModel([("ends_at", ASCENDING)]),
        ]


class FeedCache(Document):
    """Cached payload for a relayed feed (twitch / youtube / bilibili).

    One document per feed; the background relayer replaces ``items`` on each
    refresh. We relay these from the trovesaurus bot rather than fetching the
    upstream services ourselves (it already has the credentials + scrapers)."""

    feed: str  # "twitch" | "youtube" | "bilibili" — unique
    items: list[dict] = Field(default_factory=list)
    fetched_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "feed_cache"
        indexes = [IndexModel([("feed", ASCENDING)], unique=True)]
