"""Supporters - the people credited on the public Supporters list.

Managed by the master via the admin panel (``/admin/supporters``), exposed
tokenless under ``misc:read`` (``/v1/misc/supporters``), and rendered on the
``/support`` page. Seeded once on first boot from ``DEFAULT_SUPPORTERS``.
"""
from datetime import datetime

from beanie import Document, PydanticObjectId
from pydantic import Field
from pymongo import ASCENDING, IndexModel

from app.core.utils import utcnow


class Supporter(Document):
    name: str                                    # display name - unique
    added_by: PydanticObjectId | None = None     # admin who added it; null = boot seed
    created_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "supporters"
        indexes = [
            IndexModel([("name", ASCENDING)], unique=True),
            IndexModel([("created_at", ASCENDING)]),
        ]
