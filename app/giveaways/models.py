"""Beanie documents for the giveaways feature."""
from datetime import datetime
from enum import Enum

from beanie import Document, PydanticObjectId
from pydantic import Field
from pymongo import ASCENDING, DESCENDING, IndexModel

from app.core.utils import utcnow


class CodeStatus(str, Enum):
    available = "available"   # in the vault, unassigned
    reserved = "reserved"     # attached to a giveaway, not yet awarded
    awarded = "awarded"       # emailed to a winner


class GiveawayStatus(str, Enum):
    scheduled = "scheduled"   # created; starts_at is in the future
    open = "open"             # accepting entries
    drawn = "drawn"           # a winner was picked + notified
    closed = "closed"         # ended with no entrants (no winner)
    cancelled = "cancelled"   # cancelled by an admin


class VaultItem(Document):
    """A "drawer" in the vault: a named prize holding many interchangeable codes.
    Name + description live here (written once); a giveaway inherits them."""

    name: str                                      # e.g. "Trove Radiant Mount"
    description: str | None = None
    created_at: datetime = Field(default_factory=utcnow)
    updated_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "vault_items"
        indexes = [IndexModel([("created_at", DESCENDING)])]


class PrizeCode(Document):
    """A single redeemable code, belonging to a VaultItem (drawer)."""

    vault_item_id: PydanticObjectId                # which drawer this code is in
    code: str                                      # the secret redeemable code
    status: CodeStatus = CodeStatus.available
    giveaway_id: PydanticObjectId | None = None    # set while reserved / awarded
    awarded_to: PydanticObjectId | None = None     # SiteUser id
    awarded_to_email: str | None = None
    awarded_at: datetime | None = None
    created_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "prize_codes"
        indexes = [
            IndexModel([("vault_item_id", ASCENDING), ("status", ASCENDING)]),
            IndexModel([("giveaway_id", ASCENDING)]),
        ]


class Giveaway(Document):
    """One prize draw."""

    title: str
    description: str | None = None
    prize_name: str                                # public-facing prize label
    starts_at: datetime
    ends_at: datetime
    status: GiveawayStatus = GiveawayStatus.scheduled
    vault_item_id: PydanticObjectId | None = None  # the drawer the prize comes from
    prize_code_id: PydanticObjectId | None = None  # the reserved code from that drawer

    # Winner - set when the draw runs.
    winner_user_id: PydanticObjectId | None = None
    winner_username: str | None = None
    winner_email: str | None = None
    drawn_at: datetime | None = None

    # Denormalised running count so the public list never has to fan out a
    # count() per giveaway. Kept in sync on every entry.
    entry_count: int = 0

    created_at: datetime = Field(default_factory=utcnow)
    updated_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "giveaways"
        indexes = [
            # The worker scans for (open, ended) and (scheduled, started) rows.
            IndexModel([("status", ASCENDING), ("ends_at", ASCENDING)]),
            IndexModel([("starts_at", DESCENDING)]),
        ]


class GiveawayEntry(Document):
    """One user's entry into one giveaway."""

    giveaway_id: PydanticObjectId
    site_user_id: PydanticObjectId
    username: str                                  # snapshot for winner display
    entered_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "giveaway_entries"
        indexes = [
            # One entry per user per giveaway - the DB is the source of truth
            # for "already entered" (insert raises DuplicateKeyError on a repeat).
            IndexModel(
                [("giveaway_id", ASCENDING), ("site_user_id", ASCENDING)],
                unique=True,
            ),
        ]
