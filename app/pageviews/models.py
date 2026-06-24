from datetime import datetime

from beanie import Document
from pydantic import Field
from pymongo import ASCENDING, DESCENDING, IndexModel

from app.core.utils import utcnow


class PageView(Document):
    """One record per showcase-site page load (a GET that returned 200 text/html).

    Powers the admin "Site Analytics" tab. Stores BOTH the matched route TEMPLATE
    and the CONCRETE URL, same as ``UsageEvent`` (route + path): ``route`` is only
    used to decide "is this a trackable page", while ``path`` is the real page URL
    the dashboard rolls up - so each individual mod (``/mods/alice/cool-mod``) and
    player (``/player/Alice``) gets its own row, not one aggregate ``{slug}`` bucket.
    Cardinality is naturally bounded: only 200s are recorded, so a path only exists
    here if a real page was actually served. ``visitor_hash`` is a salted,
    daily-rotating hash of the client IP + User-Agent: it dedupes a visitor within
    a single UTC day without storing any PII. Rows auto-expire via a TTL index on
    ``created_at`` (managed in core.database so the retention window is adjustable).
    """

    route: str              # matched route TEMPLATE, e.g. /player/{name} (page-decision only)
    path: str               # CONCRETE page URL, e.g. /player/Alice (what the dashboard rolls up)
    visitor_hash: str       # sha256(daily_salt | ip | ua)[:32] - a once-per-day id

    created_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "page_views"
        indexes = [
            # Per-page rollups over a time window (the admin aggregation groups by path).
            IndexModel([("path", ASCENDING), ("created_at", DESCENDING)]),
        ]
