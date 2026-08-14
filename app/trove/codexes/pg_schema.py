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
-- Exact case-insensitive name lookup -> blueprint (the /market thumbnail bridge,
-- reversing a listing's display name back to its codex model).
CREATE INDEX IF NOT EXISTS codex_entry_lname_bp  ON codex_entry (branch, lower(name)) INCLUDE (blueprint);

-- Full-text search over the display name + description. A generated column so it can
-- never drift from the row it describes, and `simple` (not `english`) because item
-- names are proper nouns - stemming "Wings" to "wing" makes an exact title miss.
ALTER TABLE codex_entry ADD COLUMN IF NOT EXISTS search tsvector
    GENERATED ALWAYS AS (
        to_tsvector('simple', name || ' ' || category || ' ' || description)
    ) STORED;
CREATE INDEX IF NOT EXISTS codex_entry_search ON codex_entry USING GIN (search);

-- One row per numeric stat bonus a prefab grants. Lifted out of the `data` JSONB so a
-- stat is queryable in its own right ("every mount with Magic Find, best first") instead
-- of only readable once you already have the entry.
CREATE TABLE IF NOT EXISTS codex_stat (
    branch      TEXT    NOT NULL,
    path        TEXT    NOT NULL,
    ord         INTEGER NOT NULL,          -- source order within the prefab
    stat_key    TEXT    NOT NULL,          -- $Stat_MagicFind
    stat_id     INTEGER,
    stat_name   TEXT    NOT NULL DEFAULT '',
    operation   TEXT    NOT NULL DEFAULT '',
    amount      DOUBLE PRECISION,          -- raw decoded float
    value       DOUBLE PRECISION,          -- normalized for display
    is_percent  BOOLEAN NOT NULL DEFAULT false,
    slot_key    TEXT,                      -- $EquipmentSlot_Mount|Wings|Boat|Cart
    label       TEXT NOT NULL DEFAULT '',
    level       INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (branch, path, ord)
);
CREATE INDEX IF NOT EXISTS codex_stat_lookup ON codex_stat (branch, stat_key, value DESC);
CREATE INDEX IF NOT EXISTS codex_stat_path   ON codex_stat (branch, path);

-- One row per ability bonus reference. `hidden` rows are mechanical refs kept as
-- evidence, not displayed.
CREATE TABLE IF NOT EXISTS codex_ability (
    branch      TEXT    NOT NULL,
    path        TEXT    NOT NULL,
    ord         INTEGER NOT NULL,
    ref         TEXT    NOT NULL,
    hidden      BOOLEAN NOT NULL DEFAULT false,
    loc_key     TEXT,
    name        TEXT    NOT NULL DEFAULT '',
    description TEXT    NOT NULL DEFAULT '',
    PRIMARY KEY (branch, path, ord)
);
CREATE INDEX IF NOT EXISTS codex_ability_ref ON codex_ability (branch, ref);

-- The relationship edge table: every typed link between two codex entries.
--
-- One table rather than one per relation, because the questions are symmetric and the
-- interesting ones are REVERSE lookups: "what recipes produce this item" and "what is
-- this item used to craft" are the same query with src/dst swapped, so both directions
-- get an index and neither needs its own schema.
--
-- `rel` values in use:
--   crafts        recipe        -> the item prefab it produces        (qty = output amount)
--   ingredient    recipe        -> an item prefab it consumes         (qty = amount needed)
--   craftable_at  recipe        -> a bench/profession prefab          (data = lane, category)
--   unlocks       any prefab    -> what owning/using it grants
--   upgrade_cost  upgrade node  -> an item prefab                     (qty = amount)
--   member_of     collectible   -> its collection catalogue           (data = group label)
--   awards        badge rank    -> the collectible it grants
--
-- `dst_path` is stored in `codex_entry.path` form (prefabs/<rel>.binfab) so a link
-- joins straight to an entry; a link whose target isn't an indexed prefab is still
-- stored - the edge is real even when the far end isn't a codex row.
CREATE TABLE IF NOT EXISTS codex_link (
    branch    TEXT    NOT NULL,
    src_path  TEXT    NOT NULL,
    rel       TEXT    NOT NULL,
    dst_path  TEXT    NOT NULL,
    ord       INTEGER NOT NULL DEFAULT 0,
    qty       DOUBLE PRECISION,
    data      JSONB   NOT NULL DEFAULT '{}'::jsonb,
    PRIMARY KEY (branch, src_path, rel, dst_path, ord)
);
CREATE INDEX IF NOT EXISTS codex_link_fwd ON codex_link (branch, src_path, rel);
CREATE INDEX IF NOT EXISTS codex_link_rev ON codex_link (branch, dst_path, rel);

-- Badge requirements: one row per (badge collection, rank). Branch-scoped rather than
-- prefab-scoped - they all come from the single `prefabs/meta/badges.binfab`.
CREATE TABLE IF NOT EXISTS codex_requirement (
    branch          TEXT    NOT NULL,
    collection      TEXT    NOT NULL,      -- collections/badge/<id>
    rank            INTEGER NOT NULL,
    rank_name       TEXT    NOT NULL DEFAULT '',
    badge_id        TEXT    NOT NULL DEFAULT '',
    completion_kind TEXT    NOT NULL DEFAULT '',
    requirement_key TEXT    NOT NULL DEFAULT '',
    label           TEXT    NOT NULL DEFAULT '',
    amount          BIGINT,
    difficulty      INTEGER NOT NULL DEFAULT 0,
    status          TEXT    NOT NULL DEFAULT '',
    context         JSONB   NOT NULL DEFAULT '{}'::jsonb,
    PRIMARY KEY (branch, collection, rank)
);
CREATE INDEX IF NOT EXISTS codex_requirement_kind ON codex_requirement (branch, completion_kind);

-- Progression-tree nodes (geode modules, geode companions, class prestige). Costs stay
-- as JSONB for display AND are mirrored into codex_link as `upgrade_cost` edges, so
-- "what is bardium spent on" is answerable without opening every tree.
CREATE TABLE IF NOT EXISTS codex_upgrade (
    branch      TEXT    NOT NULL,
    system_kind TEXT    NOT NULL,          -- geode_module | geode_companion | class_prestige
    system_key  TEXT    NOT NULL,          -- barrier | gleemur_common | prestige_bard
    node_key    TEXT    NOT NULL,
    rank        INTEGER,
    source_path TEXT    NOT NULL DEFAULT '',
    costs       JSONB   NOT NULL DEFAULT '[]'::jsonb,
    requires    JSONB   NOT NULL DEFAULT '[]'::jsonb,
    PRIMARY KEY (branch, system_key, node_key)
);
CREATE INDEX IF NOT EXISTS codex_upgrade_system ON codex_upgrade (branch, system_kind, system_key);

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

-- Rig map for the Mods Hub 3D assembler: one row per (creature prefab, blueprint part)
-- - the skeleton the part belongs to + its attach point, read structurally from EVERY
-- skeleton-binding prefab (mounts, allies' _npc, skins/costumes, npc/mobs). Separate
-- from codex_entry so rig coverage isn't limited to the collectible types the codex
-- classifies. Disposable - rebuilt from the archive by reindex_rigs.
--
-- ``prefab`` is the CREATURE'S IDENTITY and belongs in the key. A skeleton is shared by
-- every creature that uses it (`mount_raptor` covers every raptor mount in the game), so
-- (branch, skeleton) alone cannot name one creature, and the blueprints all sit in the
-- same flat `blueprints/` folder - there is nothing else to tell them apart. Without
-- this column the embed's "assemble a native creature from one path" could only pick
-- parts skeleton-wide and rendered a chimera of every variant at once.
DO $$
BEGIN
    IF to_regclass('rig_binding') IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'rig_binding' AND column_name = 'prefab'
    ) THEN
        -- Pre-prefab shape: no way to add the key in place, and the table is disposable.
        -- Dropping fails CLOSED (empty map -> nothing renders) until reindex_rigs
        -- refills it, which the RIG_PARSER_VERSION bump forces on the next sync.
        DROP TABLE rig_binding;
    END IF;
END $$;

CREATE TABLE IF NOT EXISTS rig_binding (
    branch     TEXT NOT NULL,
    prefab     TEXT NOT NULL,   -- archive path of the creature prefab the binding came from
    blueprint  TEXT NOT NULL,   -- lowercased blueprint basename (matches a mod's .tmod)
    skeleton   TEXT NOT NULL,
    ap_key     TEXT NOT NULL,
    PRIMARY KEY (branch, prefab, blueprint)
);
-- "Which creature owns this part" - the embed's lookup, and the mod resolver's scan.
CREATE INDEX IF NOT EXISTS rig_binding_bp ON rig_binding (branch, blueprint);
"""


async def init(con) -> None:
    """Create the codex table + indexes (idempotent)."""
    await con.execute(_SCHEMA)
