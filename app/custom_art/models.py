"""Beanie document for custom art requests."""
from datetime import datetime
from typing import Literal

from beanie import Document
from pydantic import Field
from pymongo import ASCENDING, DESCENDING, IndexModel

from app.core.utils import utcnow

ArtKind = Literal["pfp", "club", "banner"]
ArtStatus = Literal["pending", "approved", "queued", "building", "released", "failed", "denied"]


class ArtRequest(Document):
    """One picture someone asked to have added.

    ``pending`` waits for a decision; ``approved`` waits for the master's release;
    ``queued`` and ``building`` belong to the worker; ``versions`` records each mod
    version the picture shipped in, so a retried batch never re-releases a mod
    that already carries it."""

    kind: ArtKind
    name: str
    email: str
    note: str | None = None
    image_sha: str
    content_type: str
    width: int | None = None
    height: int | None = None

    status: ArtStatus = "pending"
    reason: str | None = None
    batch: str | None = None
    log: str | None = None
    versions: dict[str, str] = Field(default_factory=dict)
    steam_pending: bool = False

    created_at: datetime = Field(default_factory=utcnow)
    decided_at: datetime | None = None
    released_at: datetime | None = None

    class Settings:
        name = "art_requests"
        indexes = [
            IndexModel([("status", ASCENDING), ("created_at", DESCENDING)]),
            IndexModel([("batch", ASCENDING)]),
        ]
