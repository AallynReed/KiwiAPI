"""Mongo model for Discord DM subscriptions."""

from datetime import datetime

from beanie import Document, PydanticObjectId
from pydantic import Field
from pymongo import ASCENDING, DESCENDING, IndexModel

from app.core.utils import utcnow

# Deliverable event types. The bus-driven ones ("challenge", "corruxion",
# "fluxion", "game_update") MUST match the bus event `type` strings emitted by
# app/events/bus.py. "market_watch" is NOT a bus event - it's evaluated on each
# market ingest against the subscription's watchlist.
DM_EVENT_TYPES: tuple[str, ...] = (
    "challenge", "corruxion", "fluxion", "game_update", "market_watch",
)

# The five challenge sub-types (see app/trove/captures.classify_challenge).
CHALLENGE_TYPES: tuple[str, ...] = ("collection", "rampage", "racing", "target", "dungeon")

MAX_SUBSCRIPTIONS_PER_USER = 10
MAX_WATCH_ITEMS = 25
# Auto-disable after this many consecutive failed DMs (user closed DMs / left).
MAX_CONSECUTIVE_FAILURES = 8
# Don't re-DM the same watched item more often than this (seconds).
MARKET_NOTIFY_COOLDOWN = 12 * 3600


class DmSubscription(Document):
    """A user's opt-in for direct-message alerts from the bot.

    ``owner_discord_id`` is cached from the SiteUser at create time so delivery
    never has to re-join to the user doc. ``filters`` is event-type specific:
    ``{"challenge_types": [...]}`` for challenges, ``{"watch": [{"name",
    "max_price_each"}]}`` for the market watchlist.
    """

    owner_id: PydanticObjectId                   # the SiteUser who owns it
    owner_discord_id: int                        # cached snowflake for delivery
    label: str = ""
    events: list[str] = Field(default_factory=list)
    filters: dict = Field(default_factory=dict)
    active: bool = True

    # Per-watched-item last-notified timestamps (unix secs), so a standing deal
    # doesn't re-DM every hourly ingest. Keyed by item name.
    watch_state: dict[str, int] = Field(default_factory=dict)

    # delivery health
    consecutive_failures: int = 0
    last_status: int | None = None
    last_error: str | None = None
    last_delivered_at: datetime | None = None
    disabled_reason: str | None = None

    created_at: datetime = Field(default_factory=utcnow)
    updated_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "dm_subscriptions"
        indexes = [
            IndexModel([("owner_id", ASCENDING), ("created_at", DESCENDING)]),
            IndexModel([("active", ASCENDING)]),
        ]
