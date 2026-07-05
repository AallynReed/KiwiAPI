from datetime import datetime

from beanie import Document
from pydantic import Field
from pymongo import ASCENDING, DESCENDING, IndexModel

from app.core.utils import utcnow


class PageView(Document):
    """One record per showcase-site page load (a GET that returned 200 text/html).

    Powers the admin "Site Analytics" tab. Cardinality is naturally bounded: only
    200s are recorded, so a path exists here only if a real page was served. Rows
    auto-expire via a TTL index on ``created_at`` (managed in core.database so the
    retention window is adjustable).
    """

    route: str              # matched route TEMPLATE, e.g. /player/{name} (page-decision only)
    path: str               # CONCRETE page URL rolled up by the dashboard - each mod/player its own row
    visitor_hash: str       # sha256(daily_salt | ip | ua)[:32] - once-per-day id, no PII

    created_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "page_views"
        indexes = [
            # Per-page rollups over a time window (the admin aggregation groups by path).
            IndexModel([("path", ASCENDING), ("created_at", DESCENDING)]),
        ]
