"""Wiki storage.

A page is its current text plus a numbered, append-only revision log. Authors are
stored by account id only and named at read time, so a deleted account's history
reads as the anonymised tombstone without rewriting any revision.
"""
from datetime import datetime
from typing import Literal

from beanie import Document, PydanticObjectId
from pydantic import Field
from pymongo import ASCENDING, DESCENDING, TEXT, IndexModel

from app.core.utils import utcnow

RevisionAction = Literal["edit", "revert", "delete"]
SuggestionStatus = Literal["pending", "accepted", "rejected", "withdrawn"]


class WikiPage(Document):
    slug: str
    title: str
    body: str = ""
    rev: int = 0
    deleted: bool = False
    created_at: datetime = Field(default_factory=utcnow)
    updated_at: datetime = Field(default_factory=utcnow)
    updated_by: PydanticObjectId | None = None

    class Settings:
        name = "wiki_pages"
        indexes = [
            IndexModel([("slug", ASCENDING)], unique=True),
            IndexModel([("title", TEXT), ("body", TEXT)],
                       weights={"title": 10, "body": 1}, name="wiki_text"),
        ]


class WikiRevision(Document):
    slug: str
    rev: int
    title: str
    body: str
    summary: str = ""
    action: RevisionAction = "edit"
    author_id: PydanticObjectId | None = None
    # The editor who accepted it, when the revision came from a suggestion.
    approved_by: PydanticObjectId | None = None
    created_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "wiki_revisions"
        indexes = [
            IndexModel([("slug", ASCENDING), ("rev", DESCENDING)], unique=True),
            IndexModel([("created_at", DESCENDING)]),
        ]


class WikiSuggestion(Document):
    slug: str
    base_rev: int                     # 0 = proposes a new page
    title: str
    body: str
    summary: str = ""
    author_id: PydanticObjectId
    status: SuggestionStatus = "pending"
    note: str = ""                    # the reviewer's reason on reject
    created_at: datetime = Field(default_factory=utcnow)
    resolved_at: datetime | None = None
    resolved_by: PydanticObjectId | None = None

    class Settings:
        name = "wiki_suggestions"
        indexes = [
            IndexModel([("status", ASCENDING), ("created_at", DESCENDING)]),
            IndexModel([("author_id", ASCENDING), ("created_at", DESCENDING)]),
        ]
