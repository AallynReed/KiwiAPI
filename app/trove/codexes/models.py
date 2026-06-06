"""Mongo storage for parsed codex entries (one per source prefab, per branch)."""

from datetime import datetime

from beanie import Document
from pydantic import Field
from pymongo import ASCENDING, IndexModel

from app.core.utils import utcnow


class CodexEntry(Document):
    branch: str                       # the branch it was indexed from (live-us / pts)
    codex_type: str                   # ally | mount | dragon | memento | recipe | item | fish | badge
    path: str                         # source prefab logical path (prefabs/…/x.binfab)
    content_sha256: str               # source binfab content hash (cache key / invalidation)
    name: str = ""                    # resolved display name
    category: str = ""                # in-prefab display category
    description: str = ""             # resolved description
    tradable: bool | None = None
    mastery: int | None = None        # collectible mastery (None for non-collectibles)
    name_key: str | None = None       # the $loc key (handy for debugging / re-resolve)
    desc_key: str | None = None
    blueprint: str | None = None      # 3D model path, when resolvable
    data: dict = Field(default_factory=dict)  # type-specific rich fields (stats/mastery/…), filled later
    indexed_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "codex_entries"
        indexes = [
            IndexModel([("branch", ASCENDING), ("path", ASCENDING)], unique=True),
            IndexModel([("branch", ASCENDING), ("codex_type", ASCENDING), ("name", ASCENDING)]),
            IndexModel([("content_sha256", ASCENDING)]),
        ]
