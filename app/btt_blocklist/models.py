from datetime import datetime
from typing import Literal

from beanie import Document
from pydantic import Field
from pymongo import ASCENDING, IndexModel

from app.core.utils import utcnow

BlockKind = Literal["author", "mod", "hash"]


class BttModBlock(Document):
    kind: BlockKind
    value: str                                   # catalog id, or lowercase md5
    note: str = ""
    created_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "btt_mod_blocklist"
        indexes = [IndexModel([("kind", ASCENDING), ("value", ASCENDING)], unique=True)]
