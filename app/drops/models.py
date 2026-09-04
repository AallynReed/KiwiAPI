"""Beanie documents for file drops."""
from datetime import datetime

from beanie import Document, PydanticObjectId
from pydantic import Field
from pymongo import ASCENDING, DESCENDING, IndexModel

from app.core.utils import utcnow


class FileDrop(Document):
    """One upload link: a secret slug, a PIN, a deadline and a budget of uploads.

    The slug is the address (``/drop/<slug>``) and the PIN is the second factor -
    a leaked URL alone can't be uploaded to. The PIN is argon2-hashed like a
    password, so the plaintext exists only in the reply to the call that created
    the drop; it is never stored and can't be shown again."""

    slug: str                                       # URL token; the link IS this
    label: str                                      # what it's for, shown to the uploader
    pin_hash: str                                   # argon2; never reversible
    max_uploads: int = 1                            # 1 = one-time
    upload_count: int = 0                           # incremented atomically per upload
    max_file_bytes: int                             # per-file cap for this drop
    expires_at: datetime
    revoked: bool = False                           # killed by hand before it expired
    created_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "file_drops"
        indexes = [
            IndexModel([("slug", ASCENDING)], unique=True),
            IndexModel([("created_at", DESCENDING)]),
        ]


class DropUpload(Document):
    """One file that arrived through a drop.

    Nothing about the sender is kept - no IP, no account, no fingerprint. The
    file itself lives on disk under the drop's directory; this row is its name,
    size and digest."""

    drop_id: PydanticObjectId
    filename: str                                   # sanitised original name
    stored_name: str                                # what it's called on disk
    size: int
    content_type: str | None = None
    sha256: str
    note: str | None = None                         # optional message from the uploader
    uploaded_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "drop_uploads"
        indexes = [
            IndexModel([("drop_id", ASCENDING), ("uploaded_at", DESCENDING)]),
        ]
