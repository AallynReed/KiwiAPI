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
