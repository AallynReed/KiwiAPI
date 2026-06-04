from datetime import datetime

from beanie import Document, PydanticObjectId
from pydantic import Field
from pymongo import ASCENDING, DESCENDING, IndexModel

from app.core.utils import utcnow


class UsageEvent(Document):
    """One record per authenticated API request.

    Powers per-user activity metrics. Rows auto-expire via a TTL index on
    `created_at` (managed in core.database so the retention window is adjustable).
    """

    user_id: PydanticObjectId
    token_id: PydanticObjectId
    method: str
    route: str        # route template, e.g. /v1/widgets/{widget_id}
    path: str         # concrete request path
    status_code: int
    duration_ms: float

    created_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "usage_events"
        indexes = [
            # Per-user / per-token activity feeds and aggregations (scoped + time).
            IndexModel([("user_id", ASCENDING), ("created_at", DESCENDING)]),
            IndexModel([("token_id", ASCENDING), ("created_at", DESCENDING)]),
            # Rate-limit digest + admin 429 rollups: match {status_code, created_at}.
            IndexModel([("status_code", ASCENDING), ("created_at", DESCENDING)]),
        ]
