"""Mongo model for outbound (Discord) webhooks."""

from datetime import datetime

from beanie import Document, PydanticObjectId
from pydantic import Field
from pymongo import ASCENDING, DESCENDING, IndexModel

from app.core.utils import utcnow
from app.embed_templates import EmbedTemplate

# The event types a webhook may subscribe to. These MUST match the bus event
# ``type`` strings emitted by ``app/events/bus.py`` (see ``delivery.py``).
WEBHOOK_EVENT_TYPES: tuple[str, ...] = ("challenge", "mod_release", "game_update")

# Auto-disable a webhook after this many consecutive failed deliveries, so a dead
# endpoint isn't retried forever (GitHub-style).
MAX_CONSECUTIVE_FAILURES = 12


class SiteWebhook(Document):
    """A Discord webhook a site user registered to receive event notifications.

    When a subscribed event fires, Kiwi renders a Discord embed and POSTs it to
    ``url``. Only Discord webhook URLs pass validation (``service.normalise_url``),
    so the stored URL is never an arbitrary host - there is no SSRF surface.
    """

    owner_id: PydanticObjectId                   # the SiteUser who owns it
    label: str = ""                              # display-only user label
    url: str                                     # validated Discord webhook URL
    events: list[str] = Field(default_factory=list)
    active: bool = True

    # Per-event custom embed templates, keyed by event type. A missing key (or a
    # template with ``enabled=False``) means "use that event's default embed".
    templates: dict[str, EmbedTemplate] = Field(default_factory=dict)

    # delivery health (updated by the delivery worker)
    consecutive_failures: int = 0
    last_status: int | None = None               # last HTTP status seen
    last_error: str | None = None
    last_delivered_at: datetime | None = None
    disabled_reason: str | None = None           # set when auto-disabled

    created_at: datetime = Field(default_factory=utcnow)
    updated_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "site_webhooks"
        indexes = [
            IndexModel([("owner_id", ASCENDING), ("created_at", DESCENDING)]),
            IndexModel([("active", ASCENDING)]),
        ]
