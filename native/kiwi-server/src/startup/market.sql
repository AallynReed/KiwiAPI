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

CREATE INDEX IF NOT EXISTS market_listing_name_ls ON market_listing (name, last_seen DESC);
CREATE INDEX IF NOT EXISTS market_listing_ls      ON market_listing (last_seen DESC);
CREATE INDEX IF NOT EXISTS market_listing_created ON market_listing (created_at DESC);
CREATE INDEX IF NOT EXISTS market_listing_pe      ON market_listing (price_each);
