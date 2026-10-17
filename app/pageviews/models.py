from datetime import datetime

from beanie import Document
from pydantic import BaseModel, Field
from pymongo import ASCENDING, DESCENDING, IndexModel

from app.core.utils import utcnow


class PagePerf(BaseModel):
    """How fast one page load was on the visitor's device, in ms (CLS x1000).
    A missing value was not measured."""

    ttfb: int | None = Field(default=None, ge=0, le=120_000)
    fcp: int | None = Field(default=None, ge=0, le=120_000)
    lcp: int | None = Field(default=None, ge=0, le=120_000)
    dcl: int | None = Field(default=None, ge=0, le=120_000)
    load: int | None = Field(default=None, ge=0, le=120_000)
    cls: int | None = Field(default=None, ge=0, le=10_000)


class PageView(Document):
    """One page load on the website, reported by the page's own stats script.

    Powers the admin "Site Analytics" tab and creators' mod stats. Visits are not
    stored: the report groups a visitor's views into visits at query time. Rows
    auto-expire via a TTL index on ``created_at`` (managed in core.database).

    Rows without ``pid`` predate the script (server-counted, bots included) and are
    left out of every report; they age out with the TTL.
    """

    route: str              # matched route TEMPLATE, e.g. /player/{name}
    path: str               # CONCRETE page URL - each mod/player its own row
    visitor_hash: str       # sha256(daily_salt | ip | ua)[:32] - once-per-day id, no PII

    pid: str | None = None          # random id of this one page load (joins the time beacons)
    engaged_s: int = 0              # seconds the page was in the foreground
    channel: str | None = None      # direct | search | social | ai | referral | email | campaign | internal
    source: str | None = None       # referring site's name or bare host, never a URL
    device: str | None = None       # desktop | mobile | tablet
    browser: str | None = None
    os: str | None = None
    lang: str | None = None         # the site language the page was shown in
    perf: PagePerf | None = None    # load timings, set by the perf beacons

    created_at: datetime = Field(default_factory=utcnow)
    last_at: datetime | None = None  # last time beacon heard from this page (live count)

    class Settings:
        name = "page_views"
        indexes = [
            # Per-page rollups over a time window (creator stats match on path).
            IndexModel([("path", ASCENDING), ("created_at", DESCENDING)]),
            # Time beacons update their view by pid; replays can't double-insert.
            IndexModel([("pid", ASCENDING)], unique=True,
                       partialFilterExpression={"pid": {"$type": "string"}}),
        ]
