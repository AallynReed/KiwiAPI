"""Postgres schema for the market scope - the high-volume listings table.

One table, ``market_listing``: a single in-game listing keyed by its game UUID.
Re-scrapes bump ``last_seen`` (UPSERT) without touching the immutable fields,
exactly like the old Mongo upsert. Created idempotently on startup (IF NOT
EXISTS) alongside the leaderboards schema.

The admin interest allow-list (``MarketInterestItem``) stays in Mongo - it's a
tiny, admin-edited dimension, not the data-heavy part that motivated the move.
"""

_SCHEMA = """
CREATE TABLE IF NOT EXISTS market_listing (
    id          UUID PRIMARY KEY,
    name        TEXT   NOT NULL,
    type        TEXT,
    stack       INTEGER NOT NULL,
    price       BIGINT  NOT NULL,
    price_each  DOUBLE PRECISION NOT NULL,
    last_seen   BIGINT  NOT NULL,
    created_at  BIGINT  NOT NULL DEFAULT 0
);

-- (name, last_seen) covers the "this item, currently active" filter that the
-- summary / listings / price-history reads all share; the singles cover the
-- page's sort + the lifetime/stale cutoffs and the price-range filter.
CREATE INDEX IF NOT EXISTS market_listing_name_ls ON market_listing (name, last_seen DESC);
CREATE INDEX IF NOT EXISTS market_listing_ls      ON market_listing (last_seen DESC);
CREATE INDEX IF NOT EXISTS market_listing_created ON market_listing (created_at DESC);
CREATE INDEX IF NOT EXISTS market_listing_pe      ON market_listing (price_each);
"""


async def init(con) -> None:
    """Create the market table + indexes (idempotent)."""
    await con.execute(_SCHEMA)
