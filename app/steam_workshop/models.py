from datetime import datetime

from beanie import Document
from pydantic import Field
from pymongo import ASCENDING, DESCENDING, IndexModel

from app.core.utils import utcnow


class SteamWorkshopItem(Document):
    published_file_id: str
    title: str = ""
    description: str = ""
    creator: str = ""                            # SteamID64
    creator_name: str = ""
    preview_url: str = ""
    file_size: int = 0
    created: int = 0                             # unix seconds, as Steam reports them
    updated: int = 0
    subscriptions: int = 0
    favorites: int = 0
    tags: list[str] = Field(default_factory=list)
    seen_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "steam_workshop_items"
        indexes = [
            IndexModel([("published_file_id", ASCENDING)], unique=True),
            IndexModel([("subscriptions", DESCENDING)]),
        ]
