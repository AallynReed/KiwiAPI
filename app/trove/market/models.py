"""Mongo models + constants for the market scope.

The high-volume ``MarketListing`` moved to **Postgres**
(``app/trove/market/{pg_schema,pg_store}.py``); what stays here is Mongo:

- ``MarketInterestItem`` - the allow-list of item names the bot scans for.
  Editable via the master admin panel; seeded from ``gamedata/market_items.json``
  on first boot if the collection is empty.

plus the listing-lifetime constants (still used by the service's active-listing
cutoffs). Service-layer code reads the interest list via
``service._interest_items_set`` (DB-backed with a short TTL cache); the JSON file
is now seed + offline fallback only.
"""

import json
from datetime import datetime
from pathlib import Path

from beanie import Document, PydanticObjectId
from pydantic import Field
from pymongo import ASCENDING, IndexModel

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
