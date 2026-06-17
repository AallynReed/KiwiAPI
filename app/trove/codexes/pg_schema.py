"""Postgres schema for the codexes scope - the materialized binfab data.

One table, ``codex_entry``: one parsed prefab per ``(branch, path)`` (the two
"modes", live-us + pts, are just rows with different ``branch`` values). The
indexer UPSERTs by primary key and keeps ``content_sha256`` so a row always points
back at the exact source binfab it was parsed from; a version delta only rewrites
the rows whose source file changed. Decoded bonuses (stats / abilities / geode
levels) live in the ``data`` JSONB; the sortable scalars get real columns.

Created idempotently on startup (IF NOT EXISTS) alongside the leaderboards/market
schema. The whole table is disposable - it can be dropped and rebuilt from the
archive (UpdateState + CAS) at any time.
"""

_SCHEMA = """
CREATE TABLE IF NOT EXISTS codex_entry (
    branch          TEXT    NOT NULL,
    path            TEXT    NOT NULL,
    codex_type      TEXT    NOT NULL,
    content_sha256  TEXT    NOT NULL,
    name            TEXT    NOT NULL DEFAULT '',
    category        TEXT    NOT NULL DEFAULT '',
    description     TEXT    NOT NULL DEFAULT '',
    tradable        BOOLEAN,
    mastery         INTEGER,
    mastery_geode   INTEGER,
    power_rank      INTEGER,
    name_key        TEXT,
    desc_key        TEXT,
    blueprint       TEXT,
    data            JSONB   NOT NULL DEFAULT '{}'::jsonb,
    indexed_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (branch, path)
);

-- The per-type listing (branch + type, sorted by name) and the category dropdown.
CREATE INDEX IF NOT EXISTS codex_entry_type_name ON codex_entry (branch, codex_type, name);
CREATE INDEX IF NOT EXISTS codex_entry_category  ON codex_entry (branch, codex_type, category);
-- Source-hash lookups (invalidation / "which entries came from this blob").
CREATE INDEX IF NOT EXISTS codex_entry_sha       ON codex_entry (content_sha256);
"""


async def init(con) -> None:
    """Create the codex table + indexes (idempotent)."""
    await con.execute(_SCHEMA)
