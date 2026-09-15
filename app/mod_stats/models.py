"""Beanie document for the daily mod statistics snapshot."""
from datetime import datetime

from beanie import Document
from pydantic import Field
from pymongo import ASCENDING, IndexModel

from app.core.utils import utcnow


class ModStatsSnapshot(Document):
    """One UTC day's counts for every mod, as ``app.mod_stats.service.build_rows`` shapes them."""

    day: str
    taken_at: datetime = Field(default_factory=utcnow)
    mods: list[dict] = Field(default_factory=list)

    class Settings:
        name = "mod_stats_snapshots"
        indexes = [IndexModel([("day", ASCENDING)], unique=True)]
