"""Parser for the raw GrainusMod.cfg dump the bot POSTs.

The bot emits one line per listing in the format::

    <uuid>;<name>;<type>;<stack>;<price>

with a trailing comma (or end of buffer). UUIDs are UUID v1 - the game uses
the timestamp portion of the UUID as the listing creation time, which we
decode in ``listing_created_at``.

Listings whose ``name`` isn't in the interest-items allow-list are dropped at
the service layer (we still parse them here - keep this module pure).
"""

from __future__ import annotations

import re
from datetime import datetime, timedelta, timezone
from typing import NamedTuple
from uuid import UUID

# UUIDs: 8-4-4-4-12 hex (36 chars w/ dashes). The game's are all lowercase, but
# accept upper too. The rest of the line is semi-colon-delimited:
# name (any chars except `;`), type (may be empty), stack (1-4 digits),
# price (1-8 digits).
_LISTING_RE = re.compile(
    r"(?P<uuid>[0-9a-fA-F-]{36});"
    r"(?P<name>[^;\r\n]+);"
    r"(?P<type>[^;\r\n]*);"
    r"(?P<stack>\d{1,4});"
    r"(?P<price>\d{1,9})",
    re.MULTILINE,
)

# UUID v1 epoch: 1582-10-15 00:00:00 UTC. Its 100-ns ticks since that epoch sit
# in ``UUID.time``. We convert to "real-UTC unix seconds" matching the rest of
# the API's timestamp convention.
_UUID_EPOCH = datetime(1582, 10, 15, tzinfo=timezone.utc)

# A defensive cap: any listing claiming > 50M flux is almost certainly a
# typo / scam and isn't worth storing. (The old API used the same threshold.)
MAX_REASONABLE_PRICE = 50_000_000


class ParsedListing(NamedTuple):
    id: UUID
    name: str
    type: str | None
    stack: int
    price: int
    price_each: float
    created_at: int    # unix seconds - decoded from the UUID's timestamp


def listing_created_at(uid: UUID) -> int:
    """Decode the listing's "posted at" time from its UUID v1 timestamp.

    Returns unix seconds in real UTC. UUIDs that aren't v1 will still produce
    a number, just one that doesn't correspond to a real time - the caller
    should already have filtered those out.
    """
    posted = _UUID_EPOCH + timedelta(microseconds=uid.time / 10)
    return int(posted.replace(microsecond=0).timestamp())


def parse_dump(text: str) -> list[ParsedListing]:
    """Parse a full GrainusMod.cfg dump into ``ParsedListing`` tuples.

    De-duplicates by UUID (one dump can repeat the same id; first sighting
    wins). Prices above ``MAX_REASONABLE_PRICE`` are dropped. Malformed UUIDs
    are skipped.
    """
    out: list[ParsedListing] = []
    seen: set[UUID] = set()
    for m in _LISTING_RE.finditer(text):
        try:
            uid = UUID(m["uuid"])
        except ValueError:
            continue
        if uid in seen:
            continue
        try:
            stack = int(m["stack"])
            price = int(m["price"])
        except ValueError:
            continue
        if stack <= 0 or price <= 0 or price > MAX_REASONABLE_PRICE:
            continue
        seen.add(uid)
        out.append(ParsedListing(
            id=uid,
            name=m["name"].strip(),
            type=(m["type"].strip() or None),
            stack=stack,
            price=price,
            price_each=round(price / stack, 3),
            created_at=listing_created_at(uid),
        ))
    return out
