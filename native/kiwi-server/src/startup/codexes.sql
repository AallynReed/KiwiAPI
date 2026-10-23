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
CREATE INDEX IF NOT EXISTS codex_entry_type_name ON codex_entry (branch, codex_type, name);
CREATE INDEX IF NOT EXISTS codex_entry_category  ON codex_entry (branch, codex_type, category);
CREATE INDEX IF NOT EXISTS codex_entry_sha       ON codex_entry (content_sha256);
CREATE INDEX IF NOT EXISTS codex_entry_lname_bp  ON codex_entry (branch, lower(name)) INCLUDE (blueprint);

ALTER TABLE codex_entry ADD COLUMN IF NOT EXISTS search tsvector
    GENERATED ALWAYS AS (
        to_tsvector('simple', name || ' ' || category || ' ' || description)
    ) STORED;
CREATE INDEX IF NOT EXISTS codex_entry_search ON codex_entry USING GIN (search);

CREATE TABLE IF NOT EXISTS codex_stat (
    branch      TEXT    NOT NULL,
    path        TEXT    NOT NULL,
    ord         INTEGER NOT NULL,
    stat_key    TEXT    NOT NULL,
    stat_id     INTEGER,
    stat_name   TEXT    NOT NULL DEFAULT '',
    operation   TEXT    NOT NULL DEFAULT '',
    amount      DOUBLE PRECISION,
    value       DOUBLE PRECISION,
    is_percent  BOOLEAN NOT NULL DEFAULT false,
    slot_key    TEXT,
    label       TEXT NOT NULL DEFAULT '',
    level       INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (branch, path, ord)
);
CREATE INDEX IF NOT EXISTS codex_stat_lookup ON codex_stat (branch, stat_key, value DESC);
CREATE INDEX IF NOT EXISTS codex_stat_path   ON codex_stat (branch, path);

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

CREATE TABLE IF NOT EXISTS codex_requirement (
    branch          TEXT    NOT NULL,
    collection      TEXT    NOT NULL,
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

CREATE TABLE IF NOT EXISTS codex_upgrade (
    branch      TEXT    NOT NULL,
    system_kind TEXT    NOT NULL,
    system_key  TEXT    NOT NULL,
    node_key    TEXT    NOT NULL,
    rank        INTEGER,
    source_path TEXT    NOT NULL DEFAULT '',
    costs       JSONB   NOT NULL DEFAULT '[]'::jsonb,
    requires    JSONB   NOT NULL DEFAULT '[]'::jsonb,
    PRIMARY KEY (branch, system_key, node_key)
);
CREATE INDEX IF NOT EXISTS codex_upgrade_system ON codex_upgrade (branch, system_kind, system_key);
ALTER TABLE codex_upgrade ADD COLUMN IF NOT EXISTS effects JSONB NOT NULL DEFAULT '{}'::jsonb;

CREATE TABLE IF NOT EXISTS codex_meta (
    branch         TEXT PRIMARY KEY,
    parser_version INTEGER NOT NULL DEFAULT 0,
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
ALTER TABLE codex_meta ADD COLUMN IF NOT EXISTS rig_version INTEGER NOT NULL DEFAULT 0;

DO $$
BEGIN
    IF to_regclass('rig_binding') IS NOT NULL AND NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'rig_binding' AND column_name = 'prefab'
    ) THEN
        DROP TABLE rig_binding;
    END IF;
END $$;

CREATE TABLE IF NOT EXISTS rig_binding (
    branch     TEXT NOT NULL,
    prefab     TEXT NOT NULL,
    blueprint  TEXT NOT NULL,
    skeleton   TEXT NOT NULL,
    ap_key     TEXT NOT NULL,
    PRIMARY KEY (branch, prefab, blueprint)
);
ALTER TABLE rig_binding ADD COLUMN IF NOT EXISTS mesh_scale REAL NOT NULL DEFAULT 1;
ALTER TABLE rig_binding ADD COLUMN IF NOT EXISTS head_scale REAL NOT NULL DEFAULT 1;
CREATE INDEX IF NOT EXISTS rig_binding_bp ON rig_binding (branch, blueprint);
