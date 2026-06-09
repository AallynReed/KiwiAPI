"""Beanie documents for the market scope.

Two documents:

- ``MarketListing``      - one per in-game listing (UUID v1 from the game is
  the _id; re-scrapes bump ``last_seen`` so the row never duplicates).
- ``MarketInterestItem`` - the allow-list of item names the bot scans for.
  Editable via the master admin panel; seeded from ``gamedata/market_items.json``
  on first boot if the collection is empty.

Service-layer code reads the interest list via ``service._interest_items_set``
(DB-backed with a short TTL cache); the JSON file is now seed + offline fallback
only.
"""

import json
from datetime import datetime
from pathlib import Path
from uuid import UUID

from beanie import Document, PydanticObjectId
from pydantic import Field
from pymongo import ASCENDING, DESCENDING, IndexModel

from app.core.utils import utcnow


# 7 days - the in-game listing TTL.
LISTING_LIFETIME_SECONDS = 7 * 86400
# After 3h with no re-scrape we consider a listing "expired" for UI hint purposes.
LISTING_STALE_SECONDS = 3 * 3600


_GAMEDATA_DIR = Path(__file__).resolve().parents[1] / "gamedata"
INTEREST_ITEMS_SEED_FILE = _GAMEDATA_DIR / "market_items.json"


def load_interest_items_from_file() -> list[str]:
    """Seed list shipped in the repo. Used to populate the DB collection on
    first boot, and as the offline fallback if the DB is unreachable (or has
    been wiped by an admin without a replacement). Returns a list (callers
    convert to ``set`` / ``frozenset`` as needed)."""
    return json.loads(INTEREST_ITEMS_SEED_FILE.read_text(encoding="utf-8"))


class MarketInterestItem(Document):
    """One name on the bot's scan allow-list.

    Editable via the master admin panel (/admin/market/interest-items). At
    ingest time the service builds a set of these for O(1) containment checks
    - only items present here get persisted to ``MarketListing``.
    """

    name: str
    added_by: PydanticObjectId | None = None   # the admin who added it, if known
    added_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "market_interest_items"
        indexes = [
            IndexModel([("name", ASCENDING)], unique=True),
        ]


class MarketListing(Document):
    """A single in-game marketplace listing.

    The listing's UUID v1 from the game is the document _id; we bump
    ``last_seen`` on every re-scrape but never change ``created_at`` (it's
    decoded from the UUID's timestamp on first sighting and represents when
    the player actually posted the listing in-game)."""

    id: UUID = Field(default_factory=None, alias="_id")  # type: ignore[assignment]
    name: str                  # the item's display name (from the interest list)
    type: str | None = None    # the item's type/tier (free-form; can be empty)
    stack: int                 # how many of the item in this listing
    price: int                 # total flux price for the whole stack
    price_each: float          # round(price / stack, 3)
    last_seen: int             # unix seconds - last time the bot saw the listing
    created_at: int = 0        # unix seconds - when the listing was posted in-game

    class Settings:
        name = "market_listings"
        indexes = [
            IndexModel([("name", ASCENDING)]),
            IndexModel([("price_each", ASCENDING)]),
            IndexModel([("last_seen", DESCENDING)]),
            IndexModel([("created_at", DESCENDING)]),
            # Compound for the common "this item in this time window" filter.
            IndexModel([("name", ASCENDING), ("last_seen", DESCENDING)]),
        ]
