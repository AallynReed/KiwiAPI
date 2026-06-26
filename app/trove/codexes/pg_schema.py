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

-- Tracks which parser version last (re)built each branch. When the deployed parser
-- is newer than this, the indexer force-rebuilds the branch on the next sync - so a
-- parser change reaches the data without a game update or a manual rebuild.
CREATE TABLE IF NOT EXISTS codex_meta (
    branch         TEXT PRIMARY KEY,
    parser_version INTEGER NOT NULL DEFAULT 0,
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
-- Separate version for the rig extractor so a rig-logic/coverage change forces a rig
-- rebuild WITHOUT a (heavier) full codex re-parse.
ALTER TABLE codex_meta ADD COLUMN IF NOT EXISTS rig_version INTEGER NOT NULL DEFAULT 0;

-- Rig map for the Mods Hub 3D assembler: blueprint basename -> the creature skeleton
-- it belongs to + its attach point, read structurally from EVERY skeleton-binding
-- prefab (mounts, allies' _npc, skins/costumes, npc/mobs). Separate from codex_entry
-- so rig coverage isn't limited to the collectible types the codex classifies.
-- Disposable - rebuilt from the archive by reindex_rigs.
CREATE TABLE IF NOT EXISTS rig_binding (
    branch     TEXT NOT NULL,
    blueprint  TEXT NOT NULL,   -- lowercased blueprint basename (matches a mod's .tmod)
    skeleton   TEXT NOT NULL,
    ap_key     TEXT NOT NULL,
    PRIMARY KEY (branch, blueprint)
);
"""


async def init(con) -> None:
    """Create the codex table + indexes (idempotent)."""
    await con.execute(_SCHEMA)
